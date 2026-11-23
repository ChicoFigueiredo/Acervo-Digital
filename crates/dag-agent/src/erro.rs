//! Erros do agente e sua tradução para [`Resposta::Erro`].

use std::path::PathBuf;

use dag_core::Resposta;
use dag_scanner::ErroScan;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ErroAgente {
    #[error("outro processo do DAG já é o escritor deste catálogo ({})", .0.display())]
    EscritorOcupado(PathBuf),
    #[error("catálogo não encontrado em {}; rode `dag volumes` ou `dag scan` para criá-lo", .0.display())]
    SemCatalogo(PathBuf),
    #[error(
        "catálogo em {} está em versão {encontrada} (esperada {esperada}); rode um comando de escrita (ex.: `dag volumes`) para migrá-lo",
        caminho.display()
    )]
    CatalogoDesatualizado {
        caminho: PathBuf,
        encontrada: u32,
        esperada: u32,
    },
    #[error("catálogo: {0}")]
    Catalogo(#[from] dag_store::Erro),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{contexto}: {fonte}")]
    Es {
        contexto: String,
        #[source]
        fonte: std::io::Error,
    },
    #[error("varredura: {0}")]
    Scan(#[from] ErroScan),
    #[error("descoberta de volumes: {0}")]
    Plataforma(#[from] dag_platform_linux::Erro),
    #[error("{0}")]
    Invalido(String),
    #[error("{0}")]
    NaoEncontrado(String),
    #[error("ainda não implementado (incremento {0})")]
    NaoImplementado(u8),
}

impl ErroAgente {
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
            Self::EscritorOcupado(_) => "escritor_ocupado",
            Self::SemCatalogo(_) => "sem_catalogo",
            Self::CatalogoDesatualizado { .. } => "catalogo_desatualizado",
            Self::Catalogo(dag_store::Erro::DestinoJaExiste(_)) => "destino_existe",
            Self::Catalogo(_) | Self::Sqlite(_) => "catalogo",
            Self::Es { .. } => "es",
            Self::Scan(ErroScan::SemRaizes) => "sem_raizes",
            Self::Scan(ErroScan::ExecucaoInexistente(_)) => "execucao_inexistente",
            Self::Scan(ErroScan::NaoRetomavel { .. }) => "nao_retomavel",
            Self::Scan(_) => "varredura",
            Self::Plataforma(_) => "plataforma",
            Self::Invalido(_) => "invalido",
            Self::NaoEncontrado(_) => "nao_encontrado",
            Self::NaoImplementado(_) => "nao_implementado",
        }
    }

    #[must_use]
    pub fn para_resposta(&self) -> Resposta {
        Resposta::Erro {
            codigo: self.codigo().to_string(),
            mensagem: self.to_string(),
        }
    }
}

impl From<ErroAgente> for Resposta {
    fn from(e: ErroAgente) -> Self {
        e.para_resposta()
    }
}
