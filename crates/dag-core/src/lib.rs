//! Domínio do DAG, independente de plataforma e sem I/O.
//!
//! Termos seguem o glossário em `CONTEXT.md`. O módulo [`comando`] é o contrato
//! compartilhado entre GUI, CLI e MCP (clientes) e o worker (único escritor).

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod comando;
pub mod hash;
pub mod ids;
pub mod regras;
pub mod tipos;

pub use comando::{
    AcaoServico, Alvo, Comando, GrupoDuplicados, Pedido, Relatorio, Resposta, StatusExecucao,
    VolumeInfo,
};
pub use hash::{HashCompleto, HashInvalido};
pub use ids::{ArquivoId, DispositivoId, ExecucaoId, GrupoId, MarcacaoId, PastaId, VolumeId};
pub use tipos::{
    Autor, EstadoExecucao, EstadoMarcacao, EtapaVarredura, Finalidade, ModoVarredura,
    OrigemMarcacao, TipoMarcacao,
};
