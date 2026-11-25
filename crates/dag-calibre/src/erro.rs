//! Erros do crate. Mensagens em português; nenhum erro carrega senha.

use std::path::PathBuf;

/// Erro de qualquer operação com o Calibre.
#[derive(Debug, thiserror::Error)]
pub enum ErroCalibre {
    #[error("E/S em {contexto}: {fonte}")]
    Io {
        contexto: String,
        #[source]
        fonte: std::io::Error,
    },
    #[error("SQLite em {contexto}: {fonte}")]
    Sqlite {
        contexto: String,
        #[source]
        fonte: rusqlite::Error,
    },
    #[error("JSON inválido em {contexto}: {fonte}")]
    Json {
        contexto: String,
        #[source]
        fonte: serde_json::Error,
    },
    #[error("não há biblioteca Calibre em {0:?} (falta metadata.db)")]
    BibliotecaInexistente(PathBuf),
    #[error("esquema do Calibre inesperado: {0}")]
    EsquemaInesperado(String),
    #[error("executável do calibredb indisponível: {0}")]
    CalibredbAusente(String),
    #[error("calibredb falhou (status {status:?}) em `{comando}`: {stderr}")]
    CalibredbFalhou {
        comando: String,
        status: Option<i32>,
        stderr: String,
    },
    #[error("calibredb excedeu {segundos}s em `{comando}` e foi encerrado")]
    Timeout { comando: String, segundos: u64 },
    #[error("saída do calibredb não compreendida: {0}")]
    SaidaInvalida(String),
    #[error(
        "o Calibre está ocupado ({detalhe}). O lock `db` do calibredb é por usuário, não por \
         biblioteca: com o GUI ou o calibre-server de pé, o calibredb local recusa até outra \
         biblioteca. Escrita local recusada para não arriscar corrupção. Opções: feche o \
         Calibre/servidor, ou use o modo servidor (--with-library http://host:porta/#id com \
         usuário e senha; a A Classificar precisa estar entre as bibliotecas servidas). \
         Nunca contorne com CALIBRE_NO_SI_DANGER_DANGER"
    )]
    CalibreOcupado { detalhe: String },
    #[error(
        "calibre-server (pid {pid}) está servindo {biblioteca:?}. Escrita local recusada; \
         use o modo servidor com usuário e senha"
    )]
    ServidorServindo { pid: u32, biblioteca: PathBuf },
    #[error("caminho proibido para a A Classificar: {0}")]
    CaminhoProibido(String),
    #[error("o diretório {0:?} não está vazio e não é uma biblioteca já criada")]
    DiretorioNaoVazio(PathBuf),
    #[error("valor inválido: {0}")]
    ValorInvalido(String),
    #[error("aprovação de homologação inválida: {0}")]
    AprovacaoInvalida(String),
    #[error("o livro já existe na Biblioteca Homologada: {0}")]
    JaExisteNaHomologada(String),
    #[error("livro inexistente: {0}")]
    LivroInexistente(String),
    #[error("verificação falhou: {0}")]
    VerificacaoFalhou(String),
    #[error("homologação parcial: livro {id_criado} criado e {situacao}; detalhe: {detalhe}")]
    HomologacaoParcial {
        id_criado: i64,
        situacao: String,
        detalhe: String,
    },
}

impl ErroCalibre {
    pub(crate) fn io(contexto: impl Into<String>, fonte: std::io::Error) -> Self {
        Self::Io {
            contexto: contexto.into(),
            fonte,
        }
    }

    pub(crate) fn sqlite(contexto: impl Into<String>, fonte: rusqlite::Error) -> Self {
        Self::Sqlite {
            contexto: contexto.into(),
            fonte,
        }
    }

    pub(crate) fn json(contexto: impl Into<String>, fonte: serde_json::Error) -> Self {
        Self::Json {
            contexto: contexto.into(),
            fonte,
        }
    }
}

/// Resultado padrão do crate.
pub type Resultado<T> = Result<T, ErroCalibre>;
