//! Ações sobre o serviço de usuário `dag-agente.service`. Nada é instalado ou habilitado sem
//! pedido explícito, e cada ação é uma chamada com argumentos fixos (sem shell).

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::ErroServico;
use super::estado::{EstadoServico, de_show, parse_linger};
use super::executor::{ExecutorSistema, SaidaProcesso};
use super::unidade::{ConfigUnidade, NOME_UNIDADE, gerar};

const PROPRIEDADES: [&str; 6] = [
    "LoadState",
    "ActiveState",
    "UnitFileState",
    "MainPID",
    "ActiveEnterTimestamp",
    "SubState",
];

/// Onde ficam as unidades de usuário: `$XDG_CONFIG_HOME/systemd/user` (ou `~/.config/...`).
/// `XDG_CONFIG_HOME` relativo é ignorado, como manda a especificação XDG.
#[must_use]
pub fn diretorio_unidades(xdg_config: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = xdg_config
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|h| h.join(".config"))
        })?;
    Some(base.join("systemd").join("user"))
}

/// Gerenciador do serviço, com o executor injetável.
#[derive(Clone)]
pub struct GerenciadorServico {
    exec: Arc<dyn ExecutorSistema>,
    unidade: ConfigUnidade,
    dir_unidades: PathBuf,
    uid: u32,
}

impl std::fmt::Debug for GerenciadorServico {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GerenciadorServico")
            .field("dir_unidades", &self.dir_unidades)
            .finish_non_exhaustive()
    }
}

impl GerenciadorServico {
    #[must_use]
    pub fn novo(
        exec: Arc<dyn ExecutorSistema>,
        unidade: ConfigUnidade,
        dir_unidades: PathBuf,
        uid: u32,
    ) -> Self {
        Self {
            exec,
            unidade,
            dir_unidades,
            uid,
        }
    }

    /// Caminho do arquivo da unidade.
    #[must_use]
    pub fn caminho_unidade(&self) -> PathBuf {
        self.dir_unidades.join(NOME_UNIDADE)
    }

    fn rodar(&self, programa: &str, args: &[&str]) -> Result<SaidaProcesso, ErroServico> {
        match self.exec.executar(programa, args, None) {
            Ok(s) => Ok(s),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(ErroServico::SistemaAusente(
                format!("`{programa}` não encontrado: este sistema parece não usar systemd"),
            )),
            Err(e) => Err(ErroServico::es(format!("executar {programa}"), e)),
        }
    }

    /// `systemctl --user ...`; saída diferente de zero vira [`ErroServico::Comando`].
    fn systemctl(&self, args: &[&str]) -> Result<SaidaProcesso, ErroServico> {
        let mut a = vec!["--user"];
        a.extend_from_slice(args);
        let s = self.rodar("systemctl", &a)?;
        if s.sucesso() {
            return Ok(s);
        }
        let err = s.stderr_texto();
        if err.contains("Failed to connect to bus")
            || err.contains("No medium found")
            || err.contains("not been booted with systemd")
        {
            return Err(ErroServico::SistemaAusente(format!(
                "o systemd de usuário não está acessível ({err}). Rode numa sessão de login normal (XDG_RUNTIME_DIR definido) ou em um sistema com systemd."
            )));
        }
        Err(ErroServico::Comando {
            comando: format!("systemctl --user {}", args.join(" ")),
            codigo: s.codigo,
            stderr: err,
        })
    }

    fn linger(&self) -> Option<bool> {
        let uid = self.uid.to_string();
        let s = self
            .rodar("loginctl", &["show-user", &uid, "-p", "Linger"])
            .ok()?;
        s.sucesso().then(|| parse_linger(&s.stdout_texto()))?
    }

    fn instalado(&self) -> bool {
        self.caminho_unidade().is_file()
    }

    fn exigir_instalado(&self) -> Result<(), ErroServico> {
        if self.instalado() {
            Ok(())
        } else {
            Err(ErroServico::NaoInstalado)
        }
    }

    /// Estado atual (somente leitura).
    ///
    /// # Errors
    /// Falha ao consultar o systemd de uma unidade que existe.
    pub fn estado(&self) -> Result<EstadoServico, ErroServico> {
        let caminho = self.caminho_unidade().display().to_string();
        if !self.instalado() {
            return Ok(EstadoServico::nao_instalado(caminho, self.linger()));
        }
        let mut args = vec!["show", NOME_UNIDADE];
        for p in PROPRIEDADES {
            args.push("-p");
            args.push(p);
        }
        let s = self.systemctl(&args)?;
        Ok(de_show(&s.stdout_texto(), caminho, self.linger()))
    }

    /// Escreve a unidade (sem habilitar) e recarrega o systemd de usuário.
    ///
    /// # Errors
    /// Falhas de E/S ou do `daemon-reload`.
    pub fn instalar(&self) -> Result<String, ErroServico> {
        let conteudo = gerar(&self.unidade)?;
        let mut db = fs::DirBuilder::new();
        db.recursive(true).mode(0o700);
        for d in &self.unidade.dirs_gravaveis {
            db.create(d)
                .map_err(|e| ErroServico::es(format!("criar {}", d.display()), e))?;
        }
        fs::create_dir_all(&self.dir_unidades)
            .map_err(|e| ErroServico::es(format!("criar {}", self.dir_unidades.display()), e))?;
        let destino = self.caminho_unidade();
        escrever_atomico(&destino, conteudo.as_bytes())
            .map_err(|e| ErroServico::es(format!("gravar {}", destino.display()), e))?;
        self.systemctl(&["daemon-reload"])?;
        Ok(format!(
            "serviço instalado em {} (NÃO habilitado e NÃO iniciado). Para iniciar agora: `dag servico iniciar`; para subir no login: `dag servico autostart on`.",
            destino.display()
        ))
    }

    /// Para, desabilita, apaga a unidade e recarrega. Idempotente. Não mexe no linger.
    ///
    /// # Errors
    /// Falha ao apagar o arquivo.
    pub fn remover(&self) -> Result<String, ErroServico> {
        if !self.instalado() {
            return Ok("o serviço já não está instalado.".into());
        }
        // Melhor esforço: o systemd pode estar indisponível, e ainda assim queremos remover.
        let _ = self.systemctl(&["stop", NOME_UNIDADE]);
        let _ = self.systemctl(&["disable", NOME_UNIDADE]);
        let destino = self.caminho_unidade();
        fs::remove_file(&destino)
            .map_err(|e| ErroServico::es(format!("apagar {}", destino.display()), e))?;
        let recarga = self.systemctl(&["daemon-reload"]);
        let mut m = format!("serviço removido ({}).", destino.display());
        if let Err(e) = recarga {
            m.push_str(&format!(" Aviso: daemon-reload falhou: {e}"));
        }
        m.push_str(
            " O linger do usuário, se estava ligado, não foi alterado (`dag servico linger off`).",
        );
        Ok(m)
    }

    /// `start`.
    ///
    /// # Errors
    /// [`ErroServico::NaoInstalado`] ou falha do systemctl.
    pub fn iniciar(&self) -> Result<String, ErroServico> {
        self.exigir_instalado()?;
        self.systemctl(&["start", NOME_UNIDADE])?;
        Ok("serviço iniciado.".into())
    }

    /// `stop`.
    ///
    /// # Errors
    /// [`ErroServico::NaoInstalado`] ou falha do systemctl.
    pub fn parar(&self) -> Result<String, ErroServico> {
        self.exigir_instalado()?;
        self.systemctl(&["stop", NOME_UNIDADE])?;
        Ok("serviço parado.".into())
    }

    /// `restart`.
    ///
    /// # Errors
    /// [`ErroServico::NaoInstalado`] ou falha do systemctl.
    pub fn reiniciar(&self) -> Result<String, ErroServico> {
        self.exigir_instalado()?;
        self.systemctl(&["restart", NOME_UNIDADE])?;
        Ok("serviço reiniciado.".into())
    }

    /// `enable`/`disable`.
    ///
    /// # Errors
    /// [`ErroServico::NaoInstalado`] ou falha do systemctl.
    pub fn autostart(&self, ativo: bool) -> Result<String, ErroServico> {
        self.exigir_instalado()?;
        if ativo {
            self.systemctl(&["enable", NOME_UNIDADE])?;
            let mut m =
                String::from("inicialização automática ligada (o serviço sobe a cada login).");
            if self.linger() != Some(true) {
                m.push_str(" Para subir já no boot e sobreviver ao último logout é preciso o linger: `dag servico linger on`.");
            }
            Ok(m)
        } else {
            self.systemctl(&["disable", NOME_UNIDADE])?;
            Ok(
                "inicialização automática desligada (o serviço, se já ativo, continua até parar)."
                    .into(),
            )
        }
    }

    /// `loginctl enable-linger`/`disable-linger`, só quando pedido.
    ///
    /// # Errors
    /// [`ErroServico::Polkit`] se o loginctl precisar de autorização que não veio.
    pub fn persistir_sem_sessao(&self, ativo: bool) -> Result<String, ErroServico> {
        let acao = if ativo {
            "enable-linger"
        } else {
            "disable-linger"
        };
        let s = self.rodar("loginctl", &[acao])?;
        if !s.sucesso() {
            let err = s.stderr_texto();
            let precisa_autorizar = err.contains("Interactive authentication required")
                || err.contains("Access denied")
                || err.contains("not authorized")
                || err.contains("polkit");
            return Err(if precisa_autorizar {
                ErroServico::Polkit(format!(
                    "o loginctl pediu autorização e ela não veio ({err}). Sem um agente Polkit ativo na sessão, rode você mesmo: `sudo loginctl {acao} \"$USER\"`."
                ))
            } else {
                ErroServico::Comando {
                    comando: format!("loginctl {acao}"),
                    codigo: s.codigo,
                    stderr: err,
                }
            });
        }
        Ok(if ativo {
            "linger ligado: o serviço de usuário sobe no boot e continua após o último logout."
                .into()
        } else {
            "linger desligado: o serviço de usuário para quando a última sessão terminar.".into()
        })
    }
}

fn escrever_atomico(destino: &Path, dados: &[u8]) -> io::Result<()> {
    let pai = destino
        .parent()
        .ok_or_else(|| io::Error::other("destino sem diretório pai"))?;
    let tmp = pai.join(format!(".dag-agente.service.{}.tmp", std::process::id()));
    let r = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&tmp)?;
        f.write_all(dados)?;
        f.sync_all()?;
        fs::rename(&tmp, destino)
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}
