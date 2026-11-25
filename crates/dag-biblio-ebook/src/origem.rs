//! De onde vêm os bytes: memória ou arquivo (aberto só para leitura, quantas vezes preciso).

use std::fs::File;
use std::io::{self, BufReader, Cursor, Read};
use std::path::PathBuf;
use std::sync::Arc;

use dag_biblio::EstadoLeitura;
use dag_conteineres::FonteBox;

use crate::pacote::Falha;

#[derive(Debug, Clone)]
pub(crate) enum Origem {
    Memoria(Arc<[u8]>),
    Arquivo(PathBuf, u64),
}

impl Origem {
    pub(crate) fn de_vec(v: Vec<u8>) -> Self {
        Self::Memoria(Arc::from(v))
    }

    pub(crate) fn tamanho(&self) -> u64 {
        match self {
            Self::Memoria(b) => b.len() as u64,
            Self::Arquivo(_, t) => *t,
        }
    }

    pub(crate) fn abrir(&self) -> io::Result<FonteBox> {
        Ok(match self {
            Self::Memoria(b) => Box::new(Cursor::new(Arc::clone(b))),
            Self::Arquivo(p, _) => Box::new(BufReader::new(File::open(p)?)),
        })
    }

    /// Até `n` primeiros bytes.
    pub(crate) fn prefixo(&self, n: usize) -> io::Result<Vec<u8>> {
        match self {
            Self::Memoria(b) => Ok(b.iter().copied().take(n).collect()),
            Self::Arquivo(p, _) => {
                let mut v = Vec::new();
                File::open(p)?.take(n as u64).read_to_end(&mut v)?;
                Ok(v)
            }
        }
    }

    /// O conteúdo inteiro, se não passar de `teto` bytes.
    pub(crate) fn tudo(&self, teto: u64) -> Result<Arc<[u8]>, Falha> {
        if self.tamanho() > teto {
            return Err(Falha::Limite(format!(
                "tamanho do arquivo ({} > {teto} bytes)",
                self.tamanho()
            )));
        }
        match self {
            Self::Memoria(b) => Ok(Arc::clone(b)),
            Self::Arquivo(p, _) => {
                let mut v = Vec::new();
                File::open(p)
                    .and_then(|f| f.take(teto.saturating_add(1)).read_to_end(&mut v))
                    .map_err(|e| {
                        Falha::Estado(EstadoLeitura::Corrompido {
                            motivo: format!("falha de leitura: {e}"),
                        })
                    })?;
                if v.len() as u64 > teto {
                    return Err(Falha::Limite(format!(
                        "tamanho do arquivo (> {teto} bytes)"
                    )));
                }
                Ok(Arc::from(v))
            }
        }
    }
}
