//! Serviço opcional em segundo plano (A-14): unidade **systemd de usuário** `dag-agente.service`.
//!
//! * Três modos: não instalado → instalado sem autostart → instalado com autostart.
//! * Serviço de usuário só vive enquanto há sessão do usuário; para subir no boot e sobreviver ao
//!   último logout é preciso *linger* (`persistir_sem_sessao`), ação **separada e explícita**.
//! * Nunca instala nem habilita nada por conta própria.
//! * `systemctl`/`loginctl` são chamados por [`ExecutorSistema`] com argumentos fixos.

mod estado;
mod executor;
mod gerenciador;
mod unidade;

pub use estado::{Atividade, EstadoServico, ModoServico, de_show, parse_linger, parse_show};
pub use executor::{ExecutorReal, ExecutorSistema, MAX_SAIDA, SaidaProcesso};
pub use gerenciador::{GerenciadorServico, diretorio_unidades};
pub use unidade::{ConfigUnidade, NOME_UNIDADE, gerar as gerar_unidade};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ErroServico {
    #[error("{0}")]
    Invalido(String),
    #[error("{contexto}: {fonte}")]
    Es {
        contexto: String,
        #[source]
        fonte: std::io::Error,
    },
    #[error("{0}")]
    SistemaAusente(String),
    #[error("o serviço não está instalado; rode `dag servico instalar`")]
    NaoInstalado,
    #[error("`{comando}` falhou (código {codigo:?}): {stderr}")]
    Comando {
        comando: String,
        codigo: Option<i32>,
        stderr: String,
    },
    #[error("{0}")]
    Polkit(String),
}

impl ErroServico {
    pub(crate) fn es(contexto: impl Into<String>, fonte: std::io::Error) -> Self {
        Self::Es {
            contexto: contexto.into(),
            fonte,
        }
    }

    /// Código estável para máquinas.
    #[must_use]
    pub fn codigo(&self) -> &'static str {
        match self {
            Self::Invalido(_) => "invalido",
            Self::Es { .. } => "es",
            Self::SistemaAusente(_) => "systemd_indisponivel",
            Self::NaoInstalado => "servico_nao_instalado",
            Self::Comando { .. } => "systemctl",
            Self::Polkit(_) => "polkit",
        }
    }
}
