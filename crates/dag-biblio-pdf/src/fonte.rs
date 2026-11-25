//! De onde vem o arquivo: um caminho (lido só para leitura pelas ferramentas) ou bytes em memória
//! (Membros de Contêiner, ADR 0006). Bytes nunca vão para o disco: viram um `memfd`.

use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::proc::{Entrada, criar_memfd};

/// Origem do arquivo a extrair.
#[derive(Debug, Clone, Copy)]
pub enum Fonte<'a> {
    /// Caminho de um arquivo catalogado (só leitura).
    Caminho(&'a Path),
    /// Bytes já em memória (p. ex. Membro de Contêiner lido em fluxo).
    Bytes(&'a [u8]),
}

/// Onde as ferramentas leem o arquivo.
#[derive(Debug)]
pub(crate) enum Local {
    Caminho(PathBuf),
    Memoria(File),
}

impl Local {
    /// Argumento de "arquivo de entrada" para as ferramentas.
    pub(crate) fn arg(&self) -> OsString {
        match self {
            Self::Caminho(p) => p.as_os_str().to_os_string(),
            Self::Memoria(_) => OsString::from("/dev/stdin"),
        }
    }

    pub(crate) fn entrada(&self) -> Entrada<'_> {
        match self {
            Self::Caminho(_) => Entrada::Nenhuma,
            Self::Memoria(f) => Entrada::Memoria(f),
        }
    }
}

/// Início e fim do arquivo (o arquivo todo, se pequeno), para checar cabeçalho e marcas de DRM sem
/// ler gigabytes.
#[derive(Debug)]
pub(crate) struct Trechos {
    pub tamanho: u64,
    pub inicio: Vec<u8>,
    pub fim: Vec<u8>,
}

const ARQUIVO_PEQUENO: u64 = 64 * 1024 * 1024;
const TRECHO_INICIO: u64 = 1024 * 1024;
const TRECHO_FIM: u64 = 4 * 1024 * 1024;

pub(crate) fn ler_trechos(fonte: &Fonte<'_>) -> std::io::Result<Trechos> {
    match fonte {
        Fonte::Bytes(b) => {
            let tamanho = b.len() as u64;
            Ok(dividir(tamanho, b))
        }
        Fonte::Caminho(p) => {
            let meta = std::fs::metadata(p)?;
            if !meta.is_file() {
                return Err(std::io::Error::other("não é um arquivo regular"));
            }
            let tamanho = meta.len();
            let mut f = File::open(p)?;
            if tamanho <= ARQUIVO_PEQUENO {
                let mut tudo = Vec::new();
                f.read_to_end(&mut tudo)?;
                return Ok(Trechos {
                    tamanho,
                    inicio: tudo,
                    fim: Vec::new(),
                });
            }
            let mut inicio = vec![0u8; TRECHO_INICIO as usize];
            f.read_exact(&mut inicio)?;
            let mut fim = vec![0u8; TRECHO_FIM as usize];
            f.seek(SeekFrom::End(-(TRECHO_FIM as i64)))?;
            f.read_exact(&mut fim)?;
            Ok(Trechos {
                tamanho,
                inicio,
                fim,
            })
        }
    }
}

fn dividir(tamanho: u64, b: &[u8]) -> Trechos {
    if tamanho <= ARQUIVO_PEQUENO {
        return Trechos {
            tamanho,
            inicio: b.to_vec(),
            fim: Vec::new(),
        };
    }
    Trechos {
        tamanho,
        inicio: b[..TRECHO_INICIO as usize].to_vec(),
        fim: b[b.len() - TRECHO_FIM as usize..].to_vec(),
    }
}

pub(crate) fn abrir(fonte: &Fonte<'_>) -> std::io::Result<Local> {
    match fonte {
        Fonte::Bytes(b) => Ok(Local::Memoria(criar_memfd("dag-documento", b)?)),
        Fonte::Caminho(p) => {
            File::open(p)?;
            Ok(Local::Caminho(std::path::absolute(p)?))
        }
    }
}

pub(crate) fn contem(haystack: &[u8], agulha: &[u8]) -> bool {
    !agulha.is_empty() && haystack.windows(agulha.len()).any(|w| w == agulha)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn contem_acha_subsequencia() {
        assert!(contem(b"abc /EBX_HANDLER def", b"/EBX_HANDLER"));
        assert!(!contem(b"abc", b"abcd"));
        assert!(!contem(b"abc", b""));
    }

    #[test]
    fn bytes_viram_memfd_legivel_por_dev_stdin() {
        let l = abrir(&Fonte::Bytes(b"conteudo")).unwrap();
        assert_eq!(l.arg(), OsString::from("/dev/stdin"));
        let s = crate::proc::executar(
            Path::new("/bin/cat"),
            &[l.arg()],
            l.entrada(),
            &[],
            std::time::Duration::from_secs(5),
            1024,
        )
        .unwrap();
        assert_eq!(s.stdout, b"conteudo");
        // Reutilizável: uma segunda chamada lê tudo de novo.
        let s2 = crate::proc::executar(
            Path::new("/bin/cat"),
            &[l.arg()],
            l.entrada(),
            &[],
            std::time::Duration::from_secs(5),
            1024,
        )
        .unwrap();
        assert_eq!(s2.stdout, b"conteudo");
    }
}
