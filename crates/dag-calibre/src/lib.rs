//! Integração com o Calibre (Fase B, frente A).
//!
//! - [`leitura`]: `metadata.db` em `mode=ro`, livros paginados como `LivroCalibre`.
//! - [`perfil`]: inventário, vocabulário, candidatos a estante, compatibilidade de clone.
//! - [`instantaneo`]: BLAKE3 antes/depois (CA-B01).
//! - [`calibredb`] + [`executor`]: wrapper do `calibredb`, só para a A Classificar.
//! - [`trava`]: detecção de Calibre/servidor abertos.
//! - [`homologacao`]: único caminho que escreve na Homologada (com aprovação), via `calibredb`.
//!
//! Regra de ouro: a Biblioteca Homologada nunca é aberta pelo `calibredb` fora da homologação
//! e nunca é escrita por SQLite.

#![allow(clippy::collapsible_if)]

pub mod calibredb;
pub mod erro;
pub mod executor;
pub mod homologacao;
pub mod instantaneo;
pub mod leitura;
pub mod perfil;
pub mod trava;

#[cfg(test)]
mod testes;

pub use erro::{ErroCalibre, Resultado};
