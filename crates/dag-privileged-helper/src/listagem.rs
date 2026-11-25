//! Percurso e leitura de metadados sem seguir symlinks.
//!
//! Cada componente é aberto com `openat(O_PATH | O_NOFOLLOW)` a partir do anterior e conferido
//! com `fstat`; symlink ou não-diretório no caminho (inclusive no último) encerra o pedido.
//! A pasta final é reaberta para listagem (`O_RDONLY | O_DIRECTORY | O_NOFOLLOW`) e confirmada
//! pelo par `(dev, ino)`. Os filhos são vistos com `fstatat(AT_SYMLINK_NOFOLLOW)` e
//! `readlinkat`. **Nenhum arquivo é aberto**: só nomes e metadados saem daqui.

use std::ffi::OsStr;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;

use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, Stat, fstat, openat, readlinkat, statat};
use thiserror::Error;

use crate::protocolo::{Linha, Metadados, TipoArquivo, codificar_bytes};
use crate::validacao::Validado;

#[derive(Debug, Error)]
pub enum ErroListagem {
    #[error("symlink ou objeto que não é pasta no caminho: {0}")]
    Suspeito(String),
    #[error("o caminho mudou durante a verificação: {0}")]
    Corrida(String),
    #[error("{contexto}: {fonte}")]
    Es {
        contexto: String,
        #[source]
        fonte: io::Error,
    },
}

impl ErroListagem {
    #[must_use]
    pub fn codigo(&self) -> &'static str {
        match self {
            Self::Suspeito(_) => "caminho_suspeito",
            Self::Corrida(_) => "corrida",
            Self::Es { fonte, .. } if fonte.kind() == io::ErrorKind::NotFound => "nao_encontrado",
            Self::Es { .. } => "erro_es",
        }
    }
}

fn es(contexto: impl Into<String>, e: rustix::io::Errno) -> ErroListagem {
    ErroListagem::Es {
        contexto: contexto.into(),
        fonte: e.into(),
    }
}

fn tipo_de(st: &Stat) -> TipoArquivo {
    match FileType::from_raw_mode(st.st_mode) {
        FileType::RegularFile => TipoArquivo::File,
        FileType::Directory => TipoArquivo::Dir,
        FileType::Symlink => TipoArquivo::Symlink,
        FileType::Fifo => TipoArquivo::Fifo,
        FileType::Socket => TipoArquivo::Socket,
        FileType::BlockDevice => TipoArquivo::Block,
        FileType::CharacterDevice => TipoArquivo::Char,
        FileType::Unknown => TipoArquivo::Other,
    }
}

fn ns(s: i64, n: i64) -> i64 {
    s.saturating_mul(1_000_000_000).saturating_add(n)
}

#[allow(clippy::useless_conversion, clippy::unnecessary_cast)]
fn metadados(st: &Stat) -> Metadados {
    Metadados {
        tipo_arquivo: tipo_de(st),
        dev: st.st_dev as u64,
        ino: st.st_ino as u64,
        nlink: st.st_nlink as u64,
        mode: st.st_mode as u32,
        uid: st.st_uid,
        gid: st.st_gid,
        size: u64::try_from(st.st_size).unwrap_or(0),
        blocks: u64::try_from(st.st_blocks).unwrap_or(0),
        mtime_ns: ns(st.st_mtime as i64, st.st_mtime_nsec as i64),
        ctime_ns: ns(st.st_ctime as i64, st.st_ctime_nsec as i64),
    }
}

/// Abre a pasta pedida de forma segura, devolvendo o fd de listagem.
fn abrir_pasta(v: &Validado) -> Result<OwnedFd, ErroListagem> {
    let o_path = OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut atual = openat(
        rustix::fs::CWD,
        "/",
        o_path | OFlags::DIRECTORY,
        Mode::empty(),
    )
    .map_err(|e| es("abrir /", e))?;
    let n = v.componentes.len();
    for (i, comp) in v.componentes.iter().enumerate() {
        let ultimo = i + 1 == n;
        let proximo = openat(&atual, comp.as_str(), o_path, Mode::empty()).map_err(|e| {
            es(
                format!("abrir componente {comp:?} de {}", v.caminho.display()),
                e,
            )
        })?;
        let st = fstat(&proximo).map_err(|e| es(format!("fstat de {comp:?}"), e))?;
        if tipo_de(&st) != TipoArquivo::Dir {
            return Err(ErroListagem::Suspeito(v.caminho.display().to_string()));
        }
        if ultimo {
            // Reabre para listar e confirma que é o mesmo objeto.
            let leitura = openat(
                &atual,
                comp.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| es(format!("abrir pasta {comp:?} para listar"), e))?;
            let st2 = fstat(&leitura).map_err(|e| es("fstat da pasta", e))?;
            if (st2.st_dev, st2.st_ino) != (st.st_dev, st.st_ino) {
                return Err(ErroListagem::Corrida(v.caminho.display().to_string()));
            }
            return Ok(leitura);
        }
        atual = proximo;
    }
    // Caminho `/`: lista a raiz.
    openat(
        rustix::fs::CWD,
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| es("abrir /", e))
}

/// Lista a pasta e entrega cada [`Linha`] a `saida`; devolve o número de filhos.
///
/// # Errors
/// [`ErroListagem`] ao abrir/percorrer; erros de `saida` (ex.: pipe fechado) viram `Es`.
pub fn listar(
    v: &Validado,
    saida: &mut dyn FnMut(Linha) -> io::Result<()>,
) -> Result<u64, ErroListagem> {
    let fd = abrir_pasta(v)?;
    let propria = fstat(&fd).map_err(|e| es("fstat da pasta", e))?;
    let enviar = |saida: &mut dyn FnMut(Linha) -> io::Result<()>, l| {
        saida(l).map_err(|fonte| ErroListagem::Es {
            contexto: "escrever resposta".into(),
            fonte,
        })
    };
    enviar(
        saida,
        Linha::Dir {
            meta: metadados(&propria),
        },
    )?;
    let dir = Dir::read_from(&fd).map_err(|e| es("listar pasta", e))?;
    let mut total = 0u64;
    for item in dir {
        let item = item.map_err(|e| es("ler entrada da pasta", e))?;
        let nome = item.file_name().to_bytes();
        if nome == b"." || nome == b".." {
            continue;
        }
        let nome_os = OsStr::from_bytes(nome);
        let Ok(st) = statat(&fd, nome_os, AtFlags::SYMLINK_NOFOLLOW) else {
            // Sumiu entre a listagem e o stat (ou sem permissão): omitido, não inventado.
            continue;
        };
        let meta = metadados(&st);
        let alvo = if meta.tipo_arquivo == TipoArquivo::Symlink {
            readlinkat(&fd, nome_os, Vec::new())
                .ok()
                .map(|c| c.to_bytes().to_vec())
        } else {
            None
        };
        let (nome_s, nome_hex) = codificar_bytes(nome);
        let (alvo_s, alvo_hex) = alvo.as_deref().map_or((None, None), codificar_bytes);
        enviar(
            saida,
            Linha::Entrada {
                nome: nome_s,
                nome_hex,
                alvo: alvo_s,
                alvo_hex,
                meta,
            },
        )?;
        total += 1;
    }
    Ok(total)
}
