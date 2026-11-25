//! Perfis de modelo e configuração em JSON. Nada aqui depende de um modelo específico.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::erro::{ErroIa, Resultado};

/// Protocolo falado com o servidor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// `POST /v1/chat/completions` (llama-server do llama.cpp, Ollama `/v1`, LM Studio...).
    #[default]
    OpenAi,
    /// API nativa do Ollama: `POST /api/chat` com `format` = JSON Schema.
    Ollama,
}

/// Como pedir saída estruturada no backend OpenAI-compatível.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModoSaida {
    /// `response_format: {type:"json_schema", json_schema:{name, strict, schema}}` (padrão OpenAI;
    /// llama-server e Ollama `/v1` convertem o schema em gramática).
    #[default]
    JsonSchema,
    /// Forma legada do llama.cpp: `response_format: {type:"json_object", schema:{...}}`.
    JsonObjectComSchema,
    /// Campo de topo `json_schema` (extensão do llama-server em `/completion` e `/v1/chat/completions`).
    JsonSchemaTopo,
    /// Gramática GBNF explícita no campo `grammar` (legado do llama.cpp).
    Gramatica(String),
    /// Sem restrição: o modelo só é instruído por texto (o validador continua valendo).
    SemRestricao,
}

/// Papel do modelo no pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PapelModelo {
    #[default]
    Primario,
    Secundario,
}

fn contexto_padrao() -> u32 {
    8192
}
fn saida_padrao() -> u32 {
    2048
}
fn seed_padrao() -> i64 {
    42
}

/// Um modelo candidato.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerfilModelo {
    /// Nome que o servidor conhece (`gemma3:12b` no Ollama; alias/`id` no llama-server).
    pub nome: String,
    /// Arquivo GGUF (llama-server); vazio para Ollama.
    #[serde(default)]
    pub arquivo_gguf: Option<PathBuf>,
    /// `Q4_K_M`, `Q8_0`... (informativo; confirmado pelo servidor quando possível).
    #[serde(default)]
    pub quantizacao: Option<String>,
    /// Janela de contexto pedida (`-c` / `num_ctx`).
    #[serde(default = "contexto_padrao")]
    pub contexto: u32,
    #[serde(default)]
    pub multimodal: bool,
    /// URL base: `http://127.0.0.1:8080` (llama-server) ou `http://127.0.0.1:11434` (Ollama).
    pub endpoint: String,
    #[serde(default)]
    pub backend: Backend,
    #[serde(default)]
    pub modo_saida: ModoSaida,
    #[serde(default)]
    pub papel: PapelModelo,
    /// Máximo de tokens gerados.
    #[serde(default = "saida_padrao")]
    pub max_tokens_saida: u32,
    #[serde(default = "seed_padrao")]
    pub seed: i64,
    /// Camadas na GPU (`-ngl`); `None` = o gerenciador decide.
    #[serde(default)]
    pub ngl: Option<i32>,
    /// Campos extras mesclados no corpo do pedido (ex.: `chat_template_kwargs` para desligar o
    /// "thinking" do Qwen3).
    #[serde(default)]
    pub extras: serde_json::Map<String, serde_json::Value>,
    /// Anotação livre (ex.: "a instalar pelo usuário").
    #[serde(default)]
    pub nota: Option<String>,
}

impl PerfilModelo {
    /// Perfil Ollama mínimo.
    #[must_use]
    pub fn ollama(nome: &str, multimodal: bool, papel: PapelModelo) -> Self {
        Self {
            nome: nome.to_owned(),
            arquivo_gguf: None,
            quantizacao: None,
            contexto: contexto_padrao(),
            multimodal,
            endpoint: "http://127.0.0.1:11434".to_owned(),
            backend: Backend::Ollama,
            modo_saida: ModoSaida::JsonSchema,
            papel,
            max_tokens_saida: saida_padrao(),
            seed: seed_padrao(),
            ngl: None,
            extras: serde_json::Map::new(),
            nota: None,
        }
    }

    /// Versão legível para a proveniência (B-23).
    #[must_use]
    pub fn versao(&self) -> String {
        match &self.quantizacao {
            Some(q) => format!("{} [{q}]", self.nome),
            None => self.nome.clone(),
        }
    }
}

/// Configuração do módulo de IA (arquivo JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigIa {
    pub modelos: Vec<PerfilModelo>,
    /// Abaixo disso um campo crítico aciona o segundo parecer.
    #[serde(default = "limiar_padrao")]
    pub limiar_confianca: f32,
    /// Orçamento de caracteres por token na estimativa (sem tokenizador).
    #[serde(default = "cpt_padrao")]
    pub caracteres_por_token: f32,
}

fn limiar_padrao() -> f32 {
    0.75
}
fn cpt_padrao() -> f32 {
    3.5
}

impl Default for ConfigIa {
    /// Padrão para o computador do Chico: `gemma3:12b` (Ollama, multimodal) como primário e um
    /// `qwen3:14b` (Q4_K_M, sem "thinking") como segundo parecer, marcado "a instalar pelo usuário".
    fn default() -> Self {
        let mut primario = PerfilModelo::ollama("gemma3:12b", true, PapelModelo::Primario);
        primario.nota = Some("instalado no computador do Chico".into());
        // Escolhido pelo Chico em 25/11/2026. O Qwen3 "pensa" antes de responder; `think: false`
        // desliga isso no Ollama, para a saída sair direto no JSON Schema.
        let mut segundo = PerfilModelo::ollama("qwen3:14b", false, PapelModelo::Secundario);
        segundo.quantizacao = Some("Q4_K_M".into());
        // Medido no cerebro (25/11): 9,2 GB de VRAM com 4096; com 8192 sobra pouca margem para
        // os outros programas da GPU de 16 GB.
        segundo.contexto = 4096;
        segundo
            .extras
            .insert("think".into(), serde_json::Value::Bool(false));
        segundo.nota = Some("a instalar pelo usuário: `ollama pull qwen3:14b`".into());
        Self {
            modelos: vec![primario, segundo],
            limiar_confianca: limiar_padrao(),
            caracteres_por_token: cpt_padrao(),
        }
    }
}

impl ConfigIa {
    /// Lê JSON de texto.
    ///
    /// # Errors
    /// `Configuracao` se o JSON for inválido ou sem modelos.
    pub fn de_json(texto: &str) -> Resultado<Self> {
        let c: Self =
            serde_json::from_str(texto).map_err(|e| ErroIa::Configuracao(e.to_string()))?;
        if c.modelos.is_empty() {
            return Err(ErroIa::Configuracao("nenhum modelo configurado".into()));
        }
        Ok(c)
    }

    /// Lê JSON de arquivo.
    ///
    /// # Errors
    /// `Io` ou `Configuracao`.
    pub fn de_arquivo(caminho: &Path) -> Resultado<Self> {
        Self::de_json(&std::fs::read_to_string(caminho)?)
    }

    /// Primeiro modelo com o papel dado.
    #[must_use]
    pub fn com_papel(&self, papel: PapelModelo) -> Option<&PerfilModelo> {
        self.modelos.iter().find(|m| m.papel == papel)
    }
}

/// Extrai a quantização (`Q4_K_M`, `IQ4_XS`, `Q8_0`, `F16`, `BF16`...) de um nome de arquivo/modelo.
#[must_use]
pub fn quantizacao_do_nome(nome: &str) -> Option<String> {
    let maiusculo = nome.to_uppercase();
    for parte in maiusculo.split(['-', '.', ':', '/', ' ']) {
        let mut cs = parte.chars();
        let eh_q = match (cs.next(), cs.next()) {
            (Some('Q'), Some(d)) => d.is_ascii_digit(),
            (Some('I'), Some('Q')) => cs.next().is_some_and(|d| d.is_ascii_digit()),
            _ => matches!(parte, "F16" | "BF16" | "F32" | "MXFP4"),
        };
        if eh_q {
            return Some(parte.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn detecta_quantizacao() {
        assert_eq!(
            quantizacao_do_nome("/m/Qwen2.5-7B-Instruct-Q4_K_M.gguf").as_deref(),
            Some("Q4_K_M")
        );
        assert_eq!(
            quantizacao_do_nome("gemma-3-12b-it-IQ4_XS.gguf").as_deref(),
            Some("IQ4_XS")
        );
        assert_eq!(
            quantizacao_do_nome("qwen2.5:14b-instruct-q4_K_M").as_deref(),
            Some("Q4_K_M")
        );
        assert_eq!(quantizacao_do_nome("Qwen2.5-7B.gguf"), None);
    }

    #[test]
    fn config_padrao_tem_gemma_e_segundo() {
        let c = ConfigIa::default();
        assert_eq!(c.modelos[0].nome, "gemma3:12b");
        assert!(c.modelos[0].multimodal);
        let seg = c.com_papel(PapelModelo::Secundario).unwrap();
        assert_eq!(seg.nome, "qwen3:14b");
        assert_eq!(
            seg.extras.get("think"),
            Some(&serde_json::Value::Bool(false))
        );
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(ConfigIa::de_json(&json).unwrap(), c);
    }

    #[test]
    fn config_sem_modelos_falha() {
        assert!(ConfigIa::de_json(r#"{"modelos":[]}"#).is_err());
        assert!(ConfigIa::de_json("{").is_err());
    }

    #[test]
    fn exemplo_de_configuracao_do_repositorio_e_valido() {
        let c = ConfigIa::de_json(include_str!("../exemplos/modelos.json")).unwrap();
        assert_eq!(c.modelos.len(), 3);
        assert_eq!(c.modelos[0].backend, Backend::Ollama);
        assert_eq!(c.modelos[2].backend, Backend::OpenAi);
        assert_eq!(
            c.com_papel(PapelModelo::Primario).unwrap().nome,
            "gemma3:12b"
        );
    }
}
