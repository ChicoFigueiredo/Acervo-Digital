//! Domínio da Curadoria Bibliográfica (Fase B), sem I/O.
//!
//! Termos seguem `CONTEXT.md` (seção Biblioteca) e `docs/fase-b/`. Este crate é o contrato
//! compartilhado entre os adaptadores (`dag-calibre`, `dag-biblio-extracao`, `dag-biblio-ia`),
//! o Catálogo (`dag-store`) e o agente. O módulo [`tipos`] só cresce de forma aditiva.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod isbn;
pub mod tipos;

pub use tipos::*;
