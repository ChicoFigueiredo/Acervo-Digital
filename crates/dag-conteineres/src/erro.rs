use std::io;

/// Falhas de leitura de um Contêiner.
#[derive(Debug, thiserror::Error)]
pub enum ErroConteiner {
    /// O Contêiner é cifrado e nenhuma senha foi dada.
    #[error("o contêiner é cifrado e precisa de senha")]
    SenhaNecessaria,
    /// A senha dada não abre o Contêiner.
    #[error("senha incorreta")]
    SenhaIncorreta,
    /// Truncado, corrompido ou ilegível, com a mensagem do leitor.
    #[error("contêiner corrompido ou ilegível: {0}")]
    Corrompido(String),
    /// Formato ou recurso do formato sem suporte.
    #[error("sem suporte: {0}")]
    NaoSuportado(String),
    /// Falha de E/S da fonte.
    #[error("falha de leitura: {0}")]
    Io(#[from] io::Error),
    /// Cancelado pelo chamador (pausa ou cancelamento da etapa).
    #[error("cancelado")]
    Cancelado,
}

impl ErroConteiner {
    /// Classifica um erro de E/S ocorrido ao ler o conteúdo de um Membro.
    pub(crate) fn de_leitura(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::PermissionDenied => Self::SenhaIncorreta,
            io::ErrorKind::Unsupported => Self::NaoSuportado(e.to_string()),
            io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => {
                Self::Corrompido(e.to_string())
            }
            _ => Self::Io(e),
        }
    }
}
