//! Tipo de erro do crate.

use std::io;

/// Erro de leitura de uma Imagem de Disco.
#[derive(Debug, thiserror::Error)]
pub enum Erro {
    /// Falha de E/S no fluxo subjacente.
    #[error("erro de E/S: {0}")]
    Io(#[from] io::Error),
    /// Formato não reconhecido como Imagem de Disco.
    #[error("formato de imagem não reconhecido")]
    Desconhecido,
    /// Estrutura inconsistente ou truncada.
    #[error("imagem corrompida: {0}")]
    Corrompido(String),
    /// Recurso válido, mas fora do escopo desta versão.
    #[error("sem suporte: {0}")]
    SemSuporte(String),
    /// Erro devolvido pela função visitante do chamador.
    #[error("erro do visitante: {0}")]
    Visita(Box<dyn std::error::Error + Send + Sync>),
}

/// Resultado do crate.
pub type Result<T, E = Erro> = std::result::Result<T, E>;

pub(crate) fn corrompido<T>(msg: impl Into<String>) -> Result<T> {
    Err(Erro::Corrompido(msg.into()))
}

pub(crate) fn sem_suporte<T>(msg: impl Into<String>) -> Result<T> {
    Err(Erro::SemSuporte(msg.into()))
}

impl From<Erro> for io::Error {
    fn from(e: Erro) -> Self {
        match e {
            Erro::Io(e) => e,
            outro => io::Error::new(io::ErrorKind::InvalidData, outro.to_string()),
        }
    }
}
