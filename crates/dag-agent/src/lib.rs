//! `dag-agent`: o escritor único do Catálogo (spec §6).
//!
//! [`Agente`] abre o Catálogo, garante por trava exclusiva que nenhum outro processo escreve nele
//! ([`GuardaEscritor`]) e executa [`Comando`](dag_core::Comando)s em processo, devolvendo
//! [`Resposta`](dag_core::Resposta) (modo 1, agente de sessão). [`Servidor`] é o mesmo agente
//! como processo de longa duração (`dag agente`), servindo um socket Unix protegido
//! ([`protocolo`]); [`Cliente`] e [`Conexao`] são o lado de GUI, CLI e MCP, que usam o agente em
//! segundo plano se houver e, senão, uma sessão em processo.
//!
//! Somente leitura sobre o acervo: o agente só escreve no Catálogo, no arquivo de trava ao lado
//! dele e nos destinos de backup pedidos pelo usuário.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod abertura;
mod agente;
mod caminho;
mod classificador;
pub mod cliente;
pub mod cofre;
mod conteineres;
mod erro;
mod exibir;
mod explorar;
mod finalidade;
pub mod lancar;
pub mod lixeira;
pub mod observador;
pub mod protocolo;
mod registro;
pub mod servico;
pub mod servidor;
mod trava;
pub mod versao;

pub use abertura::{PassadaAbertura, PoliticaAbertura, RelatorioAbertura, VerificarAoAbrir};
pub use agente::{Agente, executar_comando, incremento_pendente, somente_leitura};
pub use caminho::{
    VAR_CATALOGO, caminho_catalogo_do_ambiente, caminho_do_lock, resolver_caminho_catalogo,
};
pub use classificador::ClassificadorPorExtensao;
pub use cliente::{
    Assinatura, Cancelador, Cliente, Conexao, OpcoesConexao, VAR_SOCKET, caminho_socket_padrao,
    resolver_caminho_socket,
};
pub use erro::ErroAgente;
pub use exibir::ExibidorStderr;
pub use lancar::{OpcoesLancamento, garantir_agente, localizar_dag, sem_agente_do_ambiente};
pub use observador::{EventoObservado, Observador, OpcoesObservador};
pub use protocolo::InfoServidor;
pub use servidor::{ConfigServidor, ReinicioPedido, Servidor, uid_autorizado};
pub use trava::GuardaEscritor;
pub use versao::{DetectorTroca, IdentidadeBinario, PoliticaAtualizacao, SituacaoVersao, comparar};
