use dag_scanner::ErroScan;
use thiserror::Error;

/// Falhas do monitor.
#[derive(Debug, Error)]
pub enum ErroMonitor {
    #[error("catálogo: {0}")]
    Catalogo(#[from] dag_store::Erro),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("varredura: {0}")]
    Scan(#[from] ErroScan),
    #[error("nenhuma raiz válida para monitorar")]
    SemRaizes,
    #[error("inotify: {0}")]
    Observador(String),
}
