//! Gancho de classificação por nome (extensão → MIME). A implementação real vem de
//! `dag_core::classificacao` e é ligada depois; aqui só o contrato e a implementação nula.
//! Nunca abre o conteúdo do arquivo.

/// De onde veio a evidência do tipo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FonteEvidencia {
    Extensao,
    Magic,
    Mime,
    Usuario,
}

impl FonteEvidencia {
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::Extensao => "extensao",
            Self::Magic => "magic",
            Self::Mime => "mime",
            Self::Usuario => "usuario",
        }
    }
}

/// Tipo reconhecido para um arquivo.
#[derive(Debug, Clone, PartialEq)]
pub struct TipoDetectado {
    pub extensao: Option<String>,
    pub mime: Option<String>,
    pub descricao: Option<String>,
    pub categoria: Option<String>,
    pub fonte: FonteEvidencia,
    pub confianca: Option<f64>,
}

/// Classifica um arquivo regular só pelo nome (bytes brutos do último componente).
pub trait Classificador: Send + Sync {
    fn tipo_do_arquivo(&self, nome_raw: &[u8]) -> Option<TipoDetectado>;
}

/// Não classifica nada (padrão).
#[derive(Debug, Clone, Copy, Default)]
pub struct SemClassificacao;

impl Classificador for SemClassificacao {
    fn tipo_do_arquivo(&self, _nome_raw: &[u8]) -> Option<TipoDetectado> {
        None
    }
}
