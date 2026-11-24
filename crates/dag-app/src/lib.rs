//! DAG — Acervo Digital: aplicativo desktop (Bancada).
//!
//! A GUI nunca escreve no SQLite: por padrão é cliente do agente (ADR 0004), que ela lança se
//! não existir, e assina seus eventos; o worker ([`worker`]) só vira escritor em processo com
//! `--sem-agente`. Lê o Catálogo por uma conexão somente-leitura em outra thread
//! ([`leitor`]), sempre em páginas pequenas (CA-A08). A lógica pura (treemap, cores, bases de
//! percentual, formatação, progresso, confirmação de marcações) fica em módulos sem egui para
//! ser testada sem display.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod app;
pub mod bases;
pub mod confirmacao;
pub mod cores;
pub mod formato;
pub mod janela;
pub mod leitor;
pub mod paginas;
pub mod progresso;
pub mod selecao;
pub mod treemap;
pub mod worker;

/// Título da janela.
pub const TITULO: &str = "DAG — Acervo Digital";
