//! Trava de escrita: detecta Calibre (GUI) e `calibre-server` abertos.
//!
//! Política (documentada porque é fonte de corrupção):
//! - O `calibredb` local toma o lock de instância única global `db` (por usuário, via
//!   `fcntl.lockf` em `/var/lock/calibre-singleinstance-<uid>-db.lock`). Se o GUI ou o
//!   `calibre-server` estiverem de pé, o `calibredb` aborta com "Another calibre program such
//!   as calibre-server or the main calibre program is running". Nas versões 7.x ele NÃO
//!   encaminha sozinho ao servidor: só fala com um servidor se receber
//!   `--with-library http://host:porta/#id` (+ `--username/--password`). Comandos como
//!   `add_custom_column` não aceitam modo remoto.
//! - Antes de escrever localmente, o DAG procura processos `calibre`/`calibre-server`.
//!   Servidor servindo a biblioteca alvo: recusa sempre (`ServidorServindo`).
//!   GUI ou servidor de pé (qualquer biblioteca): recusa na política `Estrita` (padrão),
//!   porque o lock `db` é por usuário e não por biblioteca.
//!   Em qualquer caso, a mensagem de lock do `calibredb` vira `CalibreOcupado`.
//! - Leitura (`mode=ro`) nunca é bloqueada por esta trava.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::erro::{ErroCalibre, Resultado};

/// Tipo de programa Calibre em execução.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoProcessoCalibre {
    Gui,
    Servidor,
}

/// Processo Calibre detectado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessoCalibre {
    pub pid: u32,
    pub tipo: TipoProcessoCalibre,
    /// Bibliotecas servidas (argumentos posicionais do `calibre-server`), canonicalizadas
    /// quando existem.
    pub bibliotecas: Vec<PathBuf>,
    pub escuta: Option<String>,
    pub porta: Option<u16>,
    /// `--disable-local-write` presente.
    pub desabilita_escrita_local: bool,
}

/// Quão estrita é a trava.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoliticaTrava {
    /// Recusa com qualquer GUI/servidor de pé (o lock `db` é por usuário).
    #[default]
    Estrita,
    /// Só recusa servidor servindo a biblioteca; o resto fica por conta do lock do calibredb.
    /// Use com o prefixo `unshare -r -n` (lock isolado) ou quando o servidor não serve o alvo.
    ApenasServidor,
}

fn nome_base(a: &str) -> &str {
    a.rsplit('/').next().unwrap_or(a)
}

/// Interpreta a linha de comando de um processo (pura, testável).
#[must_use]
pub fn analisar_cmdline(pid: u32, args: &[String]) -> Option<ProcessoCalibre> {
    // O programa pode ser o primeiro argumento ou o segundo (`python /usr/bin/calibre-server`).
    let pos = args
        .iter()
        .take(3)
        .position(|a| matches!(nome_base(a), "calibre" | "calibre-server"))?;
    let tipo = if nome_base(&args[pos]) == "calibre-server" {
        TipoProcessoCalibre::Servidor
    } else {
        TipoProcessoCalibre::Gui
    };
    let mut bibliotecas = Vec::new();
    let mut escuta = None;
    let mut porta = None;
    let mut sem_escrita = false;
    let mut i = pos + 1;
    while i < args.len() {
        let a = &args[i];
        let (chave, valor) = match a.split_once('=') {
            Some((k, v)) if a.starts_with("--") => (k, Some(v.to_owned())),
            _ => (a.as_str(), None),
        };
        match chave {
            "--disable-local-write" => sem_escrita = true,
            "--port"
            | "--listen-on"
            | "--userdb"
            | "--log"
            | "--access-log"
            | "--auth-mode"
            | "--max-log-size"
            | "--url-prefix"
            | "--manage-users"
            | "--ban-after"
            | "--ban-for"
            | "--max-header-line-size"
            | "--max-request-body-size"
            | "--timeout"
            | "--num-per-page"
            | "--max-jobs"
            | "--worker-count"
            | "--ssl-certfile"
            | "--ssl-keyfile"
            | "--custom-list-template"
            | "--search-the-net-urls"
            | "--book-list-mode"
            | "-p" => {
                let v = valor.or_else(|| {
                    i += 1;
                    args.get(i).cloned()
                });
                if chave == "--port" || chave == "-p" {
                    porta = v.as_deref().and_then(|x| x.parse().ok());
                } else if chave == "--listen-on" {
                    escuta = v;
                }
            }
            c if c.starts_with('-') => {}
            c => {
                // Posicional: biblioteca. O GUI também aceita um caminho posicional.
                bibliotecas.push(
                    Path::new(c)
                        .canonicalize()
                        .unwrap_or_else(|_| PathBuf::from(c)),
                );
            }
        }
        i += 1;
    }
    Some(ProcessoCalibre {
        pid,
        tipo,
        bibliotecas,
        escuta,
        porta,
        desabilita_escrita_local: sem_escrita,
    })
}

/// Varre `/proc` atrás de processos Calibre (GUI e servidor).
#[must_use]
pub fn processos_calibre() -> Vec<ProcessoCalibre> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return out;
    };
    for ent in rd.flatten() {
        let Some(pid) = ent.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(bruto) = std::fs::read(ent.path().join("cmdline")) else {
            continue;
        };
        let args: Vec<String> = bruto
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        if let Some(p) = analisar_cmdline(pid, &args) {
            out.push(p);
        }
    }
    out.sort_by_key(|p| p.pid);
    out
}

/// Decide se é seguro escrever localmente em `biblioteca` dados os processos detectados.
///
/// # Errors
/// `ServidorServindo` ou `CalibreOcupado`.
pub fn verificar_escrita_local(
    biblioteca: &Path,
    politica: PoliticaTrava,
    processos: &[ProcessoCalibre],
) -> Resultado<()> {
    let alvo = biblioteca
        .canonicalize()
        .unwrap_or_else(|_| biblioteca.to_path_buf());
    for p in processos {
        if p.tipo == TipoProcessoCalibre::Servidor && p.bibliotecas.contains(&alvo) {
            return Err(ErroCalibre::ServidorServindo {
                pid: p.pid,
                biblioteca: alvo,
            });
        }
    }
    if politica == PoliticaTrava::Estrita {
        // O lock `db` é por usuário (socket abstrato `calibre-singleinstance-<uid>-db` no
        // 9.x, `fcntl.lockf` no 7.x): qualquer GUI/servidor faz o calibredb local recusar,
        // mesmo para outra biblioteca. Melhor falhar cedo e com a orientação certa.
        if let Some(p) = processos.first() {
            let quem = match p.tipo {
                TipoProcessoCalibre::Gui => "GUI do Calibre",
                TipoProcessoCalibre::Servidor => "calibre-server",
            };
            return Err(ErroCalibre::CalibreOcupado {
                detalhe: format!("{quem}, pid {}", p.pid),
            });
        }
    }
    Ok(())
}

/// Atalho: varre `/proc` e decide.
///
/// # Errors
/// Ver [`verificar_escrita_local`].
pub fn verificar_agora(biblioteca: &Path, politica: PoliticaTrava) -> Resultado<()> {
    verificar_escrita_local(biblioteca, politica, &processos_calibre())
}
