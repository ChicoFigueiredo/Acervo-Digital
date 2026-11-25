//! Servidor MCP (Model Context Protocol) do DAG: JSON-RPC 2.0 por stdio, sem SDK externo.
//!
//! Cada ferramenta vira um [`Pedido`](dag_core::Pedido) com `autor = Mcp` repassado ao worker
//! por um [`Encaminhador`]; o MCP nunca escreve no Catálogo nem toca no acervo. O stdout carrega
//! somente quadros do protocolo (um JSON por linha); logs vão para o stderr.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod ferramentas;
mod resumo;
mod servidor;

pub use ferramentas::{Ferramenta, montar, todas};
pub use servidor::{
    Encaminhador, Servidor, VERSAO_PROTOCOLO, servir, servir_stdio, servir_stdio_com,
};
