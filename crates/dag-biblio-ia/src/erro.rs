//! Erros do crate. Nenhum caminho de IA entra em pânico: tudo volta como `ErroIa` (CA-B15).

use std::time::Duration;

/// Falhas ao falar com o servidor de IA, validar ou gerenciar o processo.
#[derive(Debug, thiserror::Error)]
pub enum ErroIa {
    #[error("endpoint inválido: {0}")]
    Endpoint(String),
    #[error("falha de transporte com o servidor de IA: {0}")]
    Transporte(String),
    #[error("tempo esgotado ({0:?}) esperando o servidor de IA")]
    Timeout(Duration),
    #[error("o servidor de IA respondeu HTTP {status}: {corpo}")]
    Http { status: u16, corpo: String },
    #[error("memória insuficiente no servidor de IA: {0}")]
    MemoriaInsuficiente(String),
    #[error("resposta do servidor maior que o limite de {0} bytes")]
    RespostaGrande(usize),
    #[error("resposta do servidor não é um chat válido: {0}")]
    RespostaInvalida(String),
    #[error("saída truncada pelo modelo (limite de tokens atingido); aumente max_tokens")]
    SaidaTruncada,
    #[error("saída do modelo não é o JSON esperado: {0}")]
    SaidaInvalida(String),
    #[error("configuração inválida: {0}")]
    Configuracao(String),
    #[error("modelo ausente no servidor: {0}")]
    ModeloAusente(String),
    #[error("processo do servidor de IA: {0}")]
    Servidor(String),
    #[error("E/S: {0}")]
    Io(#[from] std::io::Error),
}

impl ErroIa {
    /// Vale tentar de novo (falha momentânea de rede/servidor)?
    #[must_use]
    pub fn e_transitorio(&self) -> bool {
        match self {
            Self::Transporte(_) | Self::Timeout(_) => true,
            Self::Http { status, .. } => *status == 429 || *status >= 500,
            _ => false,
        }
    }
}

/// Resultado padrão do crate.
pub type Resultado<T> = Result<T, ErroIa>;

/// Heurística: o texto de erro do servidor indica falta de memória?
#[must_use]
pub fn indica_falta_de_memoria(texto: &str) -> bool {
    let t = texto.to_lowercase();
    [
        "out of memory",
        "cudamalloc failed",
        "failed to allocate",
        "unable to allocate",
        "buffer_type_alloc_buffer",
        "requires more system memory",
        "model requires more memory",
    ]
    .iter()
    .any(|p| t.contains(p))
}
