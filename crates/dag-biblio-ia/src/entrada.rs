//! Tipos de entrada da classificação (tudo serializável, para casos de benchmark em JSON).

use dag_biblio::{Campo, ResultadoExtracao, ValorSugerido};
use serde::{Deserialize, Serialize};

/// Vocabulário da Biblioteca Homologada (confiável: veio do usuário).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabularioHomologado {
    /// Tags existentes.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Estantes candidatas (coluna `#pasta` do Chico).
    #[serde(default)]
    pub estantes: Vec<String>,
    /// Autores já existentes parecidos com os do arquivo.
    #[serde(default)]
    pub autores_parecidos: Vec<String>,
}

/// Uma decisão humana aprovada (campo + valores).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisaoExemplo {
    pub campo: Campo,
    pub valores: Vec<String>,
}

/// Exemplo few-shot: um arquivo (ou título) e o que o humano aprovou para ele (B-10, WOW-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExemploAprovado {
    pub nome_arquivo: String,
    pub decisoes: Vec<DecisaoExemplo>,
}

/// Tudo o que a IA recebe sobre um arquivo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntradaClassificacao {
    /// Identificador do caso (benchmark) ou do arquivo.
    #[serde(default)]
    pub id: String,
    pub nome_arquivo: String,
    pub extracao: ResultadoExtracao,
    #[serde(default)]
    pub vocabulario: VocabularioHomologado,
    #[serde(default)]
    pub exemplos: Vec<ExemploAprovado>,
    /// Valores já conhecidos da Homologada para este arquivo (origem `Homologada`), usados só
    /// para detectar divergência.
    #[serde(default)]
    pub referencias: Vec<ValorSugerido>,
    /// Imagem da página 1 em memória (JPEG/PNG), para a passada de visão. Se ausente, usa
    /// `extracao.capa`. Nunca é serializada nem gravada.
    #[serde(default, skip_serializing)]
    pub imagem_pagina1: Option<Vec<u8>>,
}
