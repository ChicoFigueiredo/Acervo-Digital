//! DAG — Acervo Digital: aplicativo desktop (Bancada).
//!
//! A GUI nunca escreve no SQLite: hospeda um worker de sessão ([`worker`], spec §7 Modo 1) que
//! é o único escritor, e lê o Catálogo por uma conexão somente-leitura em outra thread
//! ([`leitor`]), sempre em páginas pequenas (CA-A08). A lógica pura (treemap, cores, bases de
//! percentual, formatação, progresso, confirmação de marcações) fica em módulos sem egui para
//! ser testada sem display.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod app;
pub mod bases;
pub mod confirmacao;
pub mod cores;
pub mod formato;
pub mod leitor;
pub mod paginas;
pub mod progresso;
pub mod treemap;
pub mod worker;

/// Título da janela.
pub const TITULO: &str = "DAG — Acervo Digital";
