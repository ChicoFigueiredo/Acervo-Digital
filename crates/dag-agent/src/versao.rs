//! Identidade do binário do agente e atualização no lugar (`cargo install` com o agente rodando).
//!
//! O agente informa de qual binário veio ([`IdentidadeBinario`]); os clientes comparam com a
//! própria ([`comparar`]) e avisam "agente desatualizado". O agente também vigia o próprio
//! executável ([`DetectorTroca`]): quando o arquivo é trocado (o `cargo install` grava um arquivo
//! novo e o renomeia sobre o antigo), valida o novo e, conforme a [`PoliticaAtualizacao`],
//! reexecuta-se no lugar ([`reexecutar_no_lugar`]): mesmo PID, então systemd, `systemd-run` e
//! `setsid` seguem satisfeitos.
//!
//! Tudo aqui é puro ou só lê metadados, exceto [`validar_executavel`] e [`reexecutar_no_lugar`].

use std::ffi::OsString;
use std::fmt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::protocolo::InfoServidor;

/// Versão do protocolo do socket; sobe quando o formato dos quadros muda de forma incompatível.
pub const PROTOCOLO: u32 = 1;

/// Intervalo padrão entre as leituras do executável do agente.
pub const INTERVALO_VIGIA_EXE: Duration = Duration::from_secs(30);
/// Quanto tempo o arquivo novo precisa ficar parado (mesmo mtime, tamanho e inode) antes de ser
/// considerado completo.
pub const ESTABILIDADE_EXE: Duration = Duration::from_secs(5);

/// O que identifica um arquivo executável no disco.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assinatura {
    pub mtime_ns: i64,
    pub tamanho: u64,
    pub inode: u64,
    /// Arquivo regular com algum bit de execução.
    pub executavel: bool,
}

impl Assinatura {
    /// Lê o arquivo (seguindo links simbólicos); `None` se não existe.
    #[must_use]
    pub fn ler(caminho: &Path) -> Option<Self> {
        let m = std::fs::metadata(caminho).ok()?;
        Some(Self {
            mtime_ns: m.mtime().saturating_mul(1_000_000_000) + m.mtime_nsec(),
            tamanho: m.size(),
            inode: m.ino(),
            executavel: m.is_file() && m.mode() & 0o111 != 0,
        })
    }

    /// Existe, é executável e não está vazio: pode ser o binário novo.
    #[must_use]
    pub fn valida(&self) -> bool {
        self.executavel && self.tamanho > 0
    }
}

/// Quem o agente é: versão, commit, binário e protocolo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentidadeBinario {
    pub versao: String,
    /// Commit curto do git na compilação, quando havia git.
    #[serde(default)]
    pub commit: Option<String>,
    /// Caminho do executável no início do processo.
    pub caminho: String,
    #[serde(default)]
    pub mtime_ns: i64,
    #[serde(default)]
    pub tamanho: u64,
    #[serde(default)]
    pub inode: u64,
    #[serde(default)]
    pub protocolo: u32,
}

impl IdentidadeBinario {
    /// A identidade deste processo (o executável é lido agora).
    #[must_use]
    pub fn propria() -> Self {
        let caminho = std::env::current_exe().unwrap_or_default();
        let a = Assinatura::ler(&caminho);
        Self {
            versao: env!("CARGO_PKG_VERSION").to_string(),
            commit: option_env!("DAG_GIT_COMMIT").map(str::to_string),
            caminho: caminho.display().to_string(),
            mtime_ns: a.map_or(0, |a| a.mtime_ns),
            tamanho: a.map_or(0, |a| a.tamanho),
            inode: a.map_or(0, |a| a.inode),
            protocolo: PROTOCOLO,
        }
    }

    /// `0.1.0 (abc1234)` ou só `0.1.0`.
    #[must_use]
    pub fn rotulo(&self) -> String {
        rotulo(&self.versao, self.commit.as_deref())
    }

    /// Assinatura do arquivo no início do processo.
    #[must_use]
    pub fn assinatura(&self) -> Assinatura {
        Assinatura {
            mtime_ns: self.mtime_ns,
            tamanho: self.tamanho,
            inode: self.inode,
            executavel: true,
        }
    }
}

fn rotulo(versao: &str, commit: Option<&str>) -> String {
    commit.map_or_else(|| versao.to_string(), |c| format!("{versao} ({c})"))
}

/// Resultado de comparar o agente com quem pergunta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SituacaoVersao {
    Igual,
    /// O agente roda um binário mais antigo (ou há um binário novo no disco ainda não aplicado).
    AgenteDesatualizado {
        agente: String,
        propria: String,
    },
    /// O agente é mais novo que quem pergunta (o cliente é que está velho).
    ClienteDesatualizado {
        agente: String,
        propria: String,
    },
}

impl SituacaoVersao {
    /// Texto do aviso, se houver o que avisar.
    #[must_use]
    pub fn aviso(&self) -> Option<String> {
        match self {
            Self::Igual => None,
            Self::AgenteDesatualizado { agente, propria } => Some(format!(
                "agente desatualizado (versão {agente}, você tem {propria}) — reinicie com `dag servico reiniciar`"
            )),
            Self::ClienteDesatualizado { agente, propria } => Some(format!(
                "este programa é mais antigo que o agente (agente {agente}, você tem {propria}) — atualize e reabra"
            )),
        }
    }
}

fn numeros(v: &str) -> Vec<u64> {
    v.split(['.', '-', '+'])
        .map_while(|p| p.parse::<u64>().ok())
        .collect()
}

/// Compara o agente (`info`) com a identidade de quem pergunta. Puro.
#[must_use]
pub fn comparar(info: &InfoServidor, propria: &IdentidadeBinario) -> SituacaoVersao {
    let (versao, commit, protocolo) = info.binario.as_ref().map_or_else(
        || (info.versao.as_str(), None, 0),
        |b| (b.versao.as_str(), b.commit.as_deref(), b.protocolo),
    );
    let agente = rotulo(versao, commit);
    let propria_txt = propria.rotulo();
    let desatualizado = SituacaoVersao::AgenteDesatualizado {
        agente: agente.clone(),
        propria: propria_txt.clone(),
    };
    let cliente_velho = SituacaoVersao::ClienteDesatualizado {
        agente: agente.clone(),
        propria: propria_txt,
    };
    match numeros(versao).cmp(&numeros(&propria.versao)) {
        std::cmp::Ordering::Less => return desatualizado,
        std::cmp::Ordering::Greater => return cliente_velho,
        std::cmp::Ordering::Equal => {}
    }
    if protocolo != 0 && protocolo != propria.protocolo {
        return if protocolo < propria.protocolo {
            desatualizado
        } else {
            cliente_velho
        };
    }
    // Mesma versão do pacote: o commit decide (um `cargo install` do mesmo número de versão).
    if let (Some(a), Some(p)) = (commit, propria.commit.as_deref())
        && a != p
    {
        return desatualizado;
    }
    // Há um binário novo no disco que o agente ainda não aplicou (política `avisar`/`esperar`).
    if let Some(novo) = &info.atualizacao_pendente {
        return SituacaoVersao::AgenteDesatualizado {
            agente,
            propria: novo.clone(),
        };
    }
    SituacaoVersao::Igual
}

/// O que fazer quando o executável do agente é trocado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PoliticaAtualizacao {
    /// Reinicia já. Padrão: o trabalho em curso é interrompido num ponto seguro e retomado sozinho
    /// pelo agente novo, sem perda.
    #[default]
    ReiniciarAgora,
    /// Espera não haver trabalho (varredura, hash...) para reiniciar.
    EsperarTrabalho,
    /// Nunca reinicia sozinho: só avisa os clientes (o botão/`dag servico reiniciar` aplica).
    Avisar,
}

impl PoliticaAtualizacao {
    pub const VALORES: [&'static str; 3] = ["reiniciar-agora", "esperar-trabalho", "avisar"];

    #[must_use]
    pub fn como_texto(self) -> &'static str {
        match self {
            Self::ReiniciarAgora => "reiniciar-agora",
            Self::EsperarTrabalho => "esperar-trabalho",
            Self::Avisar => "avisar",
        }
    }
}

impl fmt::Display for PoliticaAtualizacao {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.como_texto())
    }
}

impl FromStr for PoliticaAtualizacao {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "reiniciar-agora" => Ok(Self::ReiniciarAgora),
            "esperar-trabalho" => Ok(Self::EsperarTrabalho),
            "avisar" => Ok(Self::Avisar),
            outro => Err(format!(
                "política desconhecida `{outro}` (use {})",
                Self::VALORES.join(", ")
            )),
        }
    }
}

/// Decisão para um binário novo, estável e válido.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acao {
    Reiniciar,
    /// Tenta de novo no próximo ciclo (há trabalho).
    Aguardar,
    /// Só publica que há atualização pendente.
    SoAvisar,
}

/// Aplica a política. `ocupado`: há trabalho do usuário rodando ou na fila. `forcado`: o usuário
/// pediu o reinício (botão da GUI), que vale para qualquer política.
#[must_use]
pub fn decidir(politica: PoliticaAtualizacao, ocupado: bool, forcado: bool) -> Acao {
    if forcado {
        return Acao::Reiniciar;
    }
    match politica {
        PoliticaAtualizacao::ReiniciarAgora => Acao::Reiniciar,
        PoliticaAtualizacao::EsperarTrabalho if ocupado => Acao::Aguardar,
        PoliticaAtualizacao::EsperarTrabalho => Acao::Reiniciar,
        PoliticaAtualizacao::Avisar => Acao::SoAvisar,
    }
}

/// Resultado de uma observação do executável.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Troca {
    /// Nada mudou (ou o que apareceu é inválido).
    Nenhuma,
    /// Mudou, mas ainda não ficou parado pelo tempo de estabilidade.
    Aguardando,
    /// Mudou, é válido e ficou estável: pode ser aplicado.
    Estavel(Assinatura),
}

/// Detecta a troca do executável com debounce: o arquivo novo só vale depois de ficar igual por
/// `estabilidade`. Puro (o relógio entra por parâmetro).
#[derive(Debug, Clone)]
pub struct DetectorTroca {
    base: Assinatura,
    candidato: Option<(Assinatura, Instant)>,
    estabilidade: Duration,
}

impl DetectorTroca {
    #[must_use]
    pub fn novo(base: Assinatura, estabilidade: Duration) -> Self {
        Self {
            base,
            candidato: None,
            estabilidade,
        }
    }

    /// Dá o arquivo como o corrente (após aplicar ou rejeitar), para não repetir.
    pub fn aceitar(&mut self, a: Assinatura) {
        self.base = a;
        self.candidato = None;
    }

    pub fn observar(&mut self, agora: Instant, atual: Option<Assinatura>) -> Troca {
        let Some(a) = atual.filter(Assinatura::valida) else {
            // Sumiu (rename em andamento) ou ainda inválido: zera a contagem.
            self.candidato = None;
            return Troca::Nenhuma;
        };
        if a == self.base {
            self.candidato = None;
            return Troca::Nenhuma;
        }
        match self.candidato {
            Some((c, desde)) if c == a => {
                if agora.saturating_duration_since(desde) >= self.estabilidade {
                    Troca::Estavel(a)
                } else {
                    Troca::Aguardando
                }
            }
            _ => {
                self.candidato = Some((a, agora));
                if self.estabilidade.is_zero() {
                    Troca::Estavel(a)
                } else {
                    Troca::Aguardando
                }
            }
        }
    }
}

/// Confere que o executável novo roda (`<exe> --version` sai com sucesso em até 10 s) e devolve a
/// versão que ele diz ter (último token da primeira linha, ex.: `dag 0.2.0` → `0.2.0`).
#[must_use]
pub fn validar_executavel(caminho: &Path) -> Option<String> {
    let mut filho = Command::new(caminho)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let fim = Instant::now() + Duration::from_secs(10);
    loop {
        match filho.try_wait() {
            Ok(Some(st)) if st.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if Instant::now() >= fim => {
                let _ = filho.kill();
                let _ = filho.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut saida = String::new();
    std::io::Read::read_to_string(&mut filho.stdout.take()?, &mut saida).ok()?;
    versao_da_saida(&saida)
}

fn versao_da_saida(saida: &str) -> Option<String> {
    saida
        .lines()
        .next()?
        .split_whitespace()
        .next_back()
        .map(str::to_string)
}

/// Substitui o processo pelo executável em `caminho` com os mesmos argumentos (`exec`: mesmo
/// PID, mesmo ambiente e stdio). Só retorna se falhar; descritores com CLOEXEC (trava, socket,
/// Catálogo) fecham no `exec`, mas quem chama deve soltá-los antes (ver `Servidor::reexecutar`).
#[must_use]
pub fn reexecutar_no_lugar(
    caminho: &Path,
    args: impl IntoIterator<Item = OsString>,
) -> std::io::Error {
    Command::new(caminho).args(args).exec()
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn sig(inode: u64, tamanho: u64) -> Assinatura {
        Assinatura {
            mtime_ns: 1,
            tamanho,
            inode,
            executavel: true,
        }
    }

    fn info(versao: &str, commit: Option<&str>) -> InfoServidor {
        InfoServidor {
            catalogo: String::new(),
            pid: 1,
            versao: versao.into(),
            monitor: true,
            binario: Some(IdentidadeBinario {
                versao: versao.into(),
                commit: commit.map(str::to_string),
                caminho: "/x/dag".into(),
                mtime_ns: 0,
                tamanho: 0,
                inode: 0,
                protocolo: PROTOCOLO,
            }),
            atualizacao_pendente: None,
            politica_atualizacao: None,
            iniciado_em: 0,
        }
    }

    fn propria(versao: &str, commit: Option<&str>) -> IdentidadeBinario {
        info(versao, commit).binario.unwrap()
    }

    #[test]
    fn versoes_iguais_nao_avisam() {
        let s = comparar(&info("0.1.0", Some("abc")), &propria("0.1.0", Some("abc")));
        assert_eq!(s, SituacaoVersao::Igual);
        assert!(s.aviso().is_none());
        assert_eq!(
            comparar(&info("0.1.0", None), &propria("0.1.0", Some("abc"))),
            SituacaoVersao::Igual
        );
    }

    #[test]
    fn agente_mais_antigo_avisa_com_a_instrucao() {
        let s = comparar(&info("0.1.0", None), &propria("0.2.0", None));
        let aviso = s.aviso().unwrap();
        assert!(aviso.contains("agente desatualizado (versão 0.1.0, você tem 0.2.0)"));
        assert!(aviso.contains("dag servico reiniciar"));
        // Comparação numérica, não de texto: 0.10 > 0.9.
        assert!(matches!(
            comparar(&info("0.9.0", None), &propria("0.10.0", None)),
            SituacaoVersao::AgenteDesatualizado { .. }
        ));
    }

    #[test]
    fn mesma_versao_com_commit_diferente_e_desatualizado() {
        let s = comparar(&info("0.1.0", Some("aaa")), &propria("0.1.0", Some("bbb")));
        assert_eq!(
            s,
            SituacaoVersao::AgenteDesatualizado {
                agente: "0.1.0 (aaa)".into(),
                propria: "0.1.0 (bbb)".into()
            }
        );
    }

    #[test]
    fn agente_mais_novo_e_o_cliente_que_esta_velho() {
        let s = comparar(&info("0.3.0", None), &propria("0.2.0", None));
        assert!(matches!(s, SituacaoVersao::ClienteDesatualizado { .. }));
        assert!(s.aviso().unwrap().contains("mais antigo que o agente"));
    }

    #[test]
    fn protocolo_antigo_e_atualizacao_pendente_avisam() {
        let mut i = info("0.1.0", None);
        i.binario.as_mut().unwrap().protocolo = 0;
        // Protocolo 0 = agente anterior ao campo: não acusa.
        assert_eq!(comparar(&i, &propria("0.1.0", None)), SituacaoVersao::Igual);
        let mut p = propria("0.1.0", None);
        p.protocolo = 2;
        i.binario.as_mut().unwrap().protocolo = 1;
        assert!(matches!(
            comparar(&i, &p),
            SituacaoVersao::AgenteDesatualizado { .. }
        ));
        let mut i = info("0.1.0", None);
        i.atualizacao_pendente = Some("0.2.0".into());
        assert!(matches!(
            comparar(&i, &propria("0.1.0", None)),
            SituacaoVersao::AgenteDesatualizado { .. }
        ));
    }

    #[test]
    fn agente_antigo_sem_binario_usa_o_campo_versao() {
        let mut i = info("0.1.0", None);
        i.binario = None;
        assert!(matches!(
            comparar(&i, &propria("0.2.0", None)),
            SituacaoVersao::AgenteDesatualizado { .. }
        ));
    }

    #[test]
    fn politica_le_e_escreve_os_tres_nomes() {
        for v in PoliticaAtualizacao::VALORES {
            assert_eq!(v.parse::<PoliticaAtualizacao>().unwrap().to_string(), v);
        }
        assert!("depois".parse::<PoliticaAtualizacao>().is_err());
        assert_eq!(
            PoliticaAtualizacao::default(),
            PoliticaAtualizacao::ReiniciarAgora
        );
    }

    #[test]
    fn decisao_por_politica() {
        use Acao::{Aguardar, Reiniciar, SoAvisar};
        use PoliticaAtualizacao::{Avisar, EsperarTrabalho, ReiniciarAgora};
        assert_eq!(decidir(ReiniciarAgora, true, false), Reiniciar);
        assert_eq!(decidir(ReiniciarAgora, false, false), Reiniciar);
        assert_eq!(decidir(EsperarTrabalho, true, false), Aguardar);
        assert_eq!(decidir(EsperarTrabalho, false, false), Reiniciar);
        assert_eq!(decidir(Avisar, true, false), SoAvisar);
        assert_eq!(decidir(Avisar, false, false), SoAvisar);
        // O pedido explícito do usuário vale para qualquer política.
        assert_eq!(decidir(Avisar, true, true), Reiniciar);
        assert_eq!(decidir(EsperarTrabalho, true, true), Reiniciar);
    }

    #[test]
    fn detector_so_aceita_depois_de_estavel() {
        let t0 = Instant::now();
        let mut d = DetectorTroca::novo(sig(1, 100), Duration::from_secs(5));
        assert_eq!(d.observar(t0, Some(sig(1, 100))), Troca::Nenhuma);
        // Troca por rename: inode novo.
        assert_eq!(d.observar(t0, Some(sig(2, 100))), Troca::Aguardando);
        assert_eq!(
            d.observar(t0 + Duration::from_secs(4), Some(sig(2, 100))),
            Troca::Aguardando
        );
        assert_eq!(
            d.observar(t0 + Duration::from_secs(5), Some(sig(2, 100))),
            Troca::Estavel(sig(2, 100))
        );
    }

    #[test]
    fn detector_reinicia_a_contagem_se_o_arquivo_ainda_muda_ou_some() {
        let t0 = Instant::now();
        let s = |n| t0 + Duration::from_secs(n);
        let mut d = DetectorTroca::novo(sig(1, 100), Duration::from_secs(5));
        assert_eq!(d.observar(s(0), Some(sig(2, 10))), Troca::Aguardando);
        // Ainda sendo escrito: o tamanho cresce, a contagem recomeça.
        assert_eq!(d.observar(s(4), Some(sig(2, 20))), Troca::Aguardando);
        assert_eq!(d.observar(s(8), Some(sig(2, 20))), Troca::Aguardando);
        // Sumiu um instante (rename em andamento).
        assert_eq!(d.observar(s(9), None), Troca::Nenhuma);
        assert_eq!(d.observar(s(10), Some(sig(2, 20))), Troca::Aguardando);
        assert_eq!(
            d.observar(s(15), Some(sig(2, 20))),
            Troca::Estavel(sig(2, 20))
        );
    }

    #[test]
    fn detector_ignora_arquivo_vazio_ou_sem_permissao_de_execucao() {
        let t0 = Instant::now();
        let mut d = DetectorTroca::novo(sig(1, 100), Duration::ZERO);
        assert_eq!(d.observar(t0, Some(sig(2, 0))), Troca::Nenhuma);
        let mut sem_x = sig(2, 50);
        sem_x.executavel = false;
        assert_eq!(d.observar(t0, Some(sem_x)), Troca::Nenhuma);
        assert_eq!(d.observar(t0, Some(sig(1, 100))), Troca::Nenhuma);
        assert_eq!(d.observar(t0, Some(sig(3, 50))), Troca::Estavel(sig(3, 50)));
        d.aceitar(sig(3, 50));
        assert_eq!(d.observar(t0, Some(sig(3, 50))), Troca::Nenhuma);
    }

    #[test]
    fn detector_ve_arquivo_trocado_por_rename_no_disco() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("dag");
        std::fs::write(&exe, b"#!/bin/sh\necho v1\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let base = Assinatura::ler(&exe).unwrap();
        assert!(base.valida());
        let mut d = DetectorTroca::novo(base, Duration::ZERO);
        let t0 = Instant::now();
        assert_eq!(d.observar(t0, Assinatura::ler(&exe)), Troca::Nenhuma);
        // Como o `cargo install`: grava ao lado e renomeia sobre o antigo.
        let novo = tmp.path().join(".dag.tmp");
        std::fs::write(&novo, b"#!/bin/sh\necho 'dag 9.9.9'\n").unwrap();
        std::fs::set_permissions(&novo, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(&novo, &exe).unwrap();
        let atual = Assinatura::ler(&exe).unwrap();
        assert_ne!(atual.inode, base.inode);
        assert_eq!(d.observar(t0, Some(atual)), Troca::Estavel(atual));
        assert_eq!(validar_executavel(&exe).as_deref(), Some("9.9.9"));
    }

    #[test]
    fn validar_recusa_arquivo_que_nao_roda() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("dag");
        std::fs::write(&exe, b"lixo").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(validar_executavel(&exe), None);
        assert_eq!(validar_executavel(&tmp.path().join("nao-existe")), None);
    }

    #[test]
    fn versao_da_saida_pega_o_ultimo_token() {
        assert_eq!(versao_da_saida("dag 0.2.0\n").as_deref(), Some("0.2.0"));
        assert_eq!(versao_da_saida(""), None);
    }
}
