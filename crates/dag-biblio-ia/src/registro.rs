//! Proveniência de cada inferência (B-23): o que foi perguntado, a quem, o que voltou e o que o
//! validador decidiu.

use dag_biblio::Campo;
use serde::{Deserialize, Serialize};

use crate::cliente::{InfoServidor, Metricas};
use crate::modelo::PapelModelo;
use crate::validador::Rejeicao;

/// O que o validador fez com a resposta.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DecisaoValidador {
    pub campos_aceitos: Vec<Campo>,
    pub campos_desconhecidos: Vec<Campo>,
    pub rejeicoes: Vec<Rejeicao>,
    pub tags_novas: Vec<String>,
    pub avisos: Vec<String>,
}

/// Registro completo de uma chamada ao modelo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistroInferencia {
    pub papel: PapelModelo,
    pub modelo: String,
    /// Nome + quantização, como o perfil declara.
    pub versao_modelo: String,
    /// O que o servidor informou (quantização real, n_ctx, build).
    #[serde(default)]
    pub info_servidor: Option<InfoServidor>,
    /// `texto` ou `visao`.
    #[serde(default = "etapa_texto")]
    pub etapa: String,
    pub versao_prompt: String,
    pub hash_prompt: String,
    pub seed: i64,
    pub temperatura: f32,
    pub metricas: Metricas,
    /// Tempo de parede da chamada (ms); 0 se nem chegou ao servidor.
    pub tempo_ms: u64,
    /// Resposta bruta do modelo (limitada).
    #[serde(default)]
    pub resposta_bruta: Option<String>,
    /// Falha de transporte, de formato ou de memória.
    #[serde(default)]
    pub erro: Option<String>,
    #[serde(default)]
    pub decisao: Option<DecisaoValidador>,
    pub material_truncado: bool,
    /// Trechos do livro que pareciam ordens ao modelo (tratados como dado).
    #[serde(default)]
    pub suspeitas_injecao: Vec<String>,
    /// Milissegundos desde a época Unix no início da chamada.
    pub inicio_unix_ms: u128,
}

fn etapa_texto() -> String {
    "texto".to_owned()
}
