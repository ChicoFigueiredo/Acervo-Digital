//! Domínio da Curadoria Bibliográfica (Fase B), sem I/O.
//!
//! Termos seguem `CONTEXT.md` (seção Biblioteca) e `docs/fase-b/`. Este crate é o contrato
//! compartilhado entre os adaptadores (`dag-calibre`, `dag-biblio-extracao`, `dag-biblio-ia`),
//! o Catálogo (`dag-store`) e o agente. O módulo [`tipos`] só cresce de forma aditiva.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod autores;
pub mod descarte;
pub mod duplicidade;
pub mod espaco;
pub mod identidade;
pub mod isbn;
pub mod normalizacao;
pub mod prioridade;
pub mod reconciliacao;
pub mod taxonomia;
pub mod tipos;
pub mod vocabulario;

pub use tipos::*;
