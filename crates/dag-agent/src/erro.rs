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
    #[error("monitor: {0}")]
    Monitor(#[from] dag_monitor::ErroMonitor),
    #[error("{0}")]
    Duplicados(#[from] dag_store::duplicados::ErroDuplicados),
    #[error("descoberta de volumes: {0}")]
    Plataforma(#[from] dag_core::ErroPlataforma),
    #[error("{0}")]
    Invalido(String),
    #[error("{0}")]
    NaoEncontrado(String),
    #[error("ainda não implementado (incremento {0})")]
    NaoImplementado(u8),
    #[error(
        "nenhum agente do DAG está em execução: pausar, retomar e cancelar só atuam sobre o agente em segundo plano (`dag agente` ou o serviço); uma varredura em primeiro plano se cancela com Ctrl-C"
    )]
    SemAgente,
    #[error("nenhuma varredura em andamento neste agente")]
    NadaEmExecucao,
    #[error("não foi possível falar com o agente em {}: {motivo}", .socket.display())]
    AgenteIndisponivel { socket: PathBuf, motivo: String },
    /// Erro da curadoria bibliográfica, com código estável (`calibre_ocupado`,
    /// `aprovacao_humana_exigida`...).
    #[error("{mensagem}")]
    Biblioteca {
        codigo: &'static str,
        mensagem: String,
    },
    /// Violação do protocolo do socket (quadro malformado, conexão cortada...).
    #[error("protocolo do agente: {0}")]
    Ipc(String),
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
            Self::Monitor(_) => "monitor",
            Self::Duplicados(e) => e.codigo(),
            Self::Plataforma(_) => "plataforma",
            Self::Invalido(_) => "invalido",
            Self::NaoEncontrado(_) => "nao_encontrado",
            Self::NaoImplementado(_) => "nao_implementado",
            Self::SemAgente => "sem_agente",
            Self::NadaEmExecucao => "nada_em_execucao",
            Self::AgenteIndisponivel { .. } => "agente_indisponivel",
            Self::Biblioteca { codigo, .. } => codigo,
            Self::Ipc(_) => "ipc",
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

impl ErroAgente {
    /// Erro da curadoria com código estável.
    pub(crate) fn biblio(codigo: &'static str, mensagem: impl Into<String>) -> Self {
        Self::Biblioteca {
            codigo,
            mensagem: mensagem.into(),
        }
    }
}

impl From<dag_calibre::ErroCalibre> for ErroAgente {
    fn from(e: dag_calibre::ErroCalibre) -> Self {
        use dag_calibre::ErroCalibre as E;
        let codigo = match &e {
            E::CalibreOcupado { .. } | E::ServidorServindo { .. } => "calibre_ocupado",
            E::CalibredbAusente(_) => "calibredb_ausente",
            E::JaExisteNaHomologada(_) => "ja_existe_na_homologada",
            E::AprovacaoInvalida(_) => "aprovacao_invalida",
            E::CaminhoProibido(_) | E::DiretorioNaoVazio(_) => "caminho_proibido",
            E::BibliotecaInexistente(_) => "biblioteca_inexistente",
            _ => "calibre",
        };
        Self::biblio(codigo, e.to_string())
    }
}

impl From<ErroAgente> for Resposta {
    fn from(e: ErroAgente) -> Self {
        e.para_resposta()
    }
}
