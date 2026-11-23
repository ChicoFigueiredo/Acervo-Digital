//! `dag-agent`: o escritor único do Catálogo (spec §6).
//!
//! Neste incremento é uma biblioteca: [`Agente`] abre o Catálogo, garante por trava
//! exclusiva que nenhum outro processo escreve nele ([`GuardaEscritor`]) e executa
//! [`Comando`](dag_core::Comando)s em processo, devolvendo [`Resposta`](dag_core::Resposta).
//! O worker de longa duração com IPC virá em incremento posterior; ele reaproveita esta lógica.
//!
//! Somente leitura sobre o acervo: o agente só escreve no Catálogo, no arquivo de trava ao lado
//! dele e nos destinos de backup pedidos pelo usuário.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod agente;
mod caminho;
mod classificador;
mod erro;
mod explorar;
mod finalidade;
mod trava;

pub use agente::{Agente, executar_comando, incremento_pendente, somente_leitura};
pub use caminho::{
    VAR_CATALOGO, caminho_catalogo_do_ambiente, caminho_do_lock, resolver_caminho_catalogo,
};
pub use classificador::ClassificadorPorExtensao;
pub use erro::ErroAgente;
pub use trava::GuardaEscritor;
