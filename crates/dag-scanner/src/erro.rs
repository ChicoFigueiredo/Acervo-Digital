//! Erros do scanner.

use thiserror::Error;

/// Falha que interrompe o scan (problemas de acesso a itens não são erros: ficam registrados).
#[derive(Debug, Error)]
pub enum ErroScan {
    #[error("catálogo: {0}")]
    Catalogo(#[from] dag_store::Erro),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("execução {0} não existe")]
    ExecucaoInexistente(i64),
    #[error("execução {id} não pode ser retomada (estado {estado})")]
    NaoRetomavel { id: i64, estado: String },
    #[error("nenhuma raiz válida informada")]
    SemRaizes,
    #[error("falha interna: {0}")]
    Interno(String),
}
