//! Classificação bibliográfica com IA local (Fase B, frente C).
//!
//! Fluxo: [`prompt`] monta o pedido (texto do livro como DADO não confiável) → [`cliente`] fala com
//! `llama-server`/Ollama com saída estruturada → [`validador`] só aceita o que tem evidência
//! literal no material → [`classificador`] decide sobre segundo parecer e combina por regras →
//! [`registro`] guarda a proveniência. [`benchmark`] mede modelos; [`servidor`] gerencia o processo.
//!
//! A IA nunca decide sozinha: o resultado são `ValorSugerido` com origem `Ia` e evidências, mais
//! uma lista de revisões humanas. Nada aqui escreve em disco, na Homologada ou no Catálogo.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod benchmark;
pub mod classificador;
pub mod cliente;
pub mod entrada;
pub mod erro;
pub mod esquema;
pub mod modelo;
pub mod normaliza;
pub mod prompt;
pub mod registro;
pub mod servidor;
pub mod validador;

#[cfg(test)]
mod testutil;

pub use classificador::{Classificador, MotivoRevisao, ResultadoClassificacao, RevisaoHumana};
pub use cliente::{ClienteLlm, ConfigCliente, InfoServidor, Mensagem, Metricas, RespostaChat};
pub use entrada::{DecisaoExemplo, EntradaClassificacao, ExemploAprovado, VocabularioHomologado};
pub use erro::{ErroIa, Resultado};
pub use esquema::PapelPessoa;
pub use modelo::{Backend, ConfigIa, ModoSaida, PapelModelo, PerfilModelo};
pub use registro::{DecisaoValidador, RegistroInferencia};
pub use validador::{
    Colaborador, ConfigValidador, FatosConfianca, MotivoRejeicao, Rejeicao, ResultadoValidacao,
    TagProposta, confianca_deterministica,
};
