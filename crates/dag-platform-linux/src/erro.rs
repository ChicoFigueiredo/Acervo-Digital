use std::path::PathBuf;

/// Falhas da descoberta. Falhas parciais (um statvfs, um atributo de sysfs) não são
/// erro: viram `None` no resultado.
#[derive(Debug, thiserror::Error)]
pub enum Erro {
    /// Não foi possível ler uma fonte indispensável (ex.: `/proc/self/mountinfo`).
    #[error("falha ao ler {caminho}: {fonte}")]
    Io {
        caminho: PathBuf,
        #[source]
        fonte: std::io::Error,
    },
    /// Linha de mountinfo fora do formato documentado em `proc(5)`.
    #[error("linha de mountinfo inválida: {0}")]
    MountinfoInvalido(String),
}
