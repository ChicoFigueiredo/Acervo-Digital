//! Descoberta de Volumes e Dispositivos Físicos no Linux (requisitos A-01, A-06, A-18).
//!
//! Estritamente somente leitura: lê `/proc/self/mountinfo`, `/sys` e `/dev/disk/*`
//! e consulta `statvfs`. Nada é escrito, montado ou aberto para varredura.
//!
//! Todo acesso ao sistema passa por [`FonteSistema`], o que permite testar o
//! parsing e a resolução de dispositivos com árvores de fixtures.
//!
//! Ponto de entrada: [`descobrir_volumes`].

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod dispositivo;
pub mod elevacao;
mod erro;
mod fonte;
mod mountinfo;
pub mod servico;
mod volumes;

pub use erro::Erro;
pub use fonte::{EstatisticasFs, FonteSistema, FonteSistemaReal};
pub use mountinfo::{EntradaMontagem, parse_linha_mountinfo, parse_mountinfo};
pub use volumes::{descobrir_volumes, descobrir_volumes_com};
