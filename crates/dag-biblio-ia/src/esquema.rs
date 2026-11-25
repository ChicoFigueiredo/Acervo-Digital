//! Esquema JSON da saída do modelo e leitura tolerante dela.

use std::collections::BTreeMap;

use dag_biblio::Campo;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::erro::{ErroIa, Resultado};

/// Campos que a IA pode sugerir, na ordem do esquema.
pub const CAMPOS_IA: [Campo; 14] = [
    Campo::Titulo,
    Campo::Subtitulo,
    Campo::Autores,
    Campo::Editora,
    Campo::Edicao,
    Campo::Data,
    Campo::Idioma,
    Campo::Isbn,
    Campo::Doi,
    Campo::Serie,
    Campo::Volume,
    Campo::Resumo,
    Campo::Tags,
    Campo::Estante,
];

/// Campos críticos: baixa confiança neles aciona o segundo parecer (B-19).
pub const CAMPOS_CRITICOS: [Campo; 4] =
    [Campo::Titulo, Campo::Autores, Campo::Isbn, Campo::Estante];

/// Campos que são listas.
#[must_use]
pub fn e_lista(c: Campo) -> bool {
    matches!(c, Campo::Autores | Campo::Tags)
}

/// Nome JSON (snake_case) do campo.
#[must_use]
pub fn nome_campo(c: Campo) -> String {
    serde_json::to_value(c)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Campo a partir do nome JSON, restrito aos campos da IA.
#[must_use]
pub fn campo_do_nome(nome: &str) -> Option<Campo> {
    CAMPOS_IA.iter().copied().find(|c| nome_campo(*c) == nome)
}

fn esquema_evidencias(max: u32) -> Value {
    json!({
        "type": "array",
        "maxItems": max,
        "items": {
            "type": "object",
            "properties": {
                "pagina": {"anyOf": [{"type": "integer"}, {"type": "null"}]},
                "trecho": {"type": "string", "maxLength": 200}
            },
            "required": ["pagina", "trecho"],
            "additionalProperties": false
        }
    })
}

fn esquema_campo(valor: Value, max_evidencias: u32) -> Value {
    // A confiança autodeclarada pelo modelo não é pedida nem usada (medição: sempre 1,0).
    json!({
        "type": "object",
        "properties": {
            "valor": valor,
            "evidencias": esquema_evidencias(max_evidencias)
        },
        "required": ["valor", "evidencias"],
        "additionalProperties": false
    })
}

fn texto_ou_nulo() -> Value {
    json!({"anyOf": [{"type": "string"}, {"type": "null"}]})
}

/// Papéis aceitos para pessoas ligadas ao livro.
pub const PAPEIS_PESSOA: [&str; 5] = ["autor", "organizador", "tradutor", "ilustrador", "outro"];

fn esquema_pessoas() -> Value {
    json!({
        "type": "array",
        "maxItems": 20,
        "items": {
            "type": "object",
            "properties": {
                "nome": {"type": "string", "maxLength": 120},
                "papel": {"enum": PAPEIS_PESSOA}
            },
            "required": ["nome", "papel"],
            "additionalProperties": false
        }
    })
}

/// Esquema da saída. `tags_candidatas` (já pré-filtradas, nunca as ~1800 do acervo) e `estantes`
/// entram como `enum`: o servidor gera a gramática e o modelo não consegue sair do vocabulário.
/// Tags novas só em `tags_novas_propostas` (texto livre, com evidência).
#[must_use]
pub fn esquema_saida(tags_candidatas: &[String], estantes: &[String]) -> Value {
    let tags_itens = if tags_candidatas.is_empty() {
        json!({"type": "string"})
    } else {
        json!({"enum": tags_candidatas})
    };
    let estante = if estantes.is_empty() {
        texto_ou_nulo()
    } else {
        let mut e: Vec<Value> = estantes.iter().map(|s| json!(s)).collect();
        e.push(Value::Null);
        json!({"enum": e})
    };
    let mut props = serde_json::Map::new();
    for c in CAMPOS_IA {
        let valor = match c {
            Campo::Autores => esquema_pessoas(),
            Campo::Tags => json!({"type": "array", "items": tags_itens, "maxItems": 12}),
            Campo::Estante => estante.clone(),
            // Resumo curto: a geração longa é o que mais custa na IA local (medição de 25/11).
            Campo::Resumo => {
                json!({"anyOf": [{"type": "string", "maxLength": 400}, {"type": "null"}]})
            }
            _ => texto_ou_nulo(),
        };
        // Até 2 evidências em todos os campos: com só 1 nos não críticos (prompt/4) o validador
        // aceitou ~20% menos campos por item, quase sem ganho de tempo (medição de 25/11).
        props.insert(nome_campo(c), esquema_campo(valor, 2));
    }
    // Só título e autores são obrigatórios: campo sem evidência é omitido (cada `null` com a
    // lista de evidências vazia custa tokens de saída, e a saída é o que pesa na IA local).
    let nomes: Vec<String> = [Campo::Titulo, Campo::Autores]
        .iter()
        .map(|c| nome_campo(*c))
        .collect();
    json!({
        "type": "object",
        "properties": {
            "campos": {
                "type": "object",
                "properties": props,
                "required": nomes,
                "additionalProperties": false
            },
            "tags_novas_propostas": {
                "type": "array",
                "maxItems": 10,
                "items": {
                    "type": "object",
                    "properties": {
                        "tag": {"type": "string", "maxLength": 80},
                        "evidencias": esquema_evidencias(1)
                    },
                    "required": ["tag", "evidencias"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["campos", "tags_novas_propostas"],
        "additionalProperties": false
    })
}

/// Campos que a leitura da imagem da página 1 pode confirmar.
pub const CAMPOS_VISAO: [Campo; 7] = [
    Campo::Titulo,
    Campo::Subtitulo,
    Campo::Autores,
    Campo::Editora,
    Campo::Edicao,
    Campo::Isbn,
    Campo::Data,
];

/// Esquema da passada de visão (sem evidências em texto: a evidência é a imagem).
#[must_use]
pub fn esquema_visao() -> Value {
    let mut props = serde_json::Map::new();
    for c in CAMPOS_VISAO {
        let valor = if c == Campo::Autores {
            esquema_pessoas()
        } else {
            texto_ou_nulo()
        };
        props.insert(
            nome_campo(c),
            json!({"type": "object", "properties": {"valor": valor},
                   "required": ["valor"], "additionalProperties": false}),
        );
    }
    let nomes: Vec<String> = CAMPOS_VISAO.iter().map(|c| nome_campo(*c)).collect();
    json!({
        "type": "object",
        "properties": {"campos": {"type": "object", "properties": props,
                                   "required": nomes, "additionalProperties": false}},
        "required": ["campos"],
        "additionalProperties": false
    })
}

/// Papel de uma pessoa ligada ao livro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PapelPessoa {
    Autor,
    Organizador,
    Tradutor,
    Ilustrador,
    Outro,
}

impl PapelPessoa {
    #[must_use]
    pub fn do_texto(t: &str) -> Self {
        match crate::normaliza::normalizar(t).as_str() {
            "autor" | "autora" | "author" => Self::Autor,
            "organizador" | "organizadora" | "editor" | "coordenador" => Self::Organizador,
            "tradutor" | "tradutora" | "translator" => Self::Tradutor,
            "ilustrador" | "ilustradora" | "illustrator" => Self::Ilustrador,
            _ => Self::Outro,
        }
    }
}

/// Lê o valor de `autores`: objetos `{nome, papel}` ou (tolerância) strings, que são `autor`.
#[must_use]
pub fn pessoas_do_valor(v: &Value) -> Vec<(String, PapelPessoa)> {
    let item = |x: &Value| -> Option<(String, PapelPessoa)> {
        match x {
            Value::String(s) => Some((s.trim().to_owned(), PapelPessoa::Autor)),
            Value::Object(o) => {
                let nome = o.get("nome")?.as_str()?.trim().to_owned();
                let papel = o
                    .get("papel")
                    .and_then(Value::as_str)
                    .map_or(PapelPessoa::Autor, PapelPessoa::do_texto);
                Some((nome, papel))
            }
            _ => None,
        }
    };
    match v {
        Value::Array(a) => a.iter().filter_map(item).collect(),
        Value::String(s) => s
            .split(['&', ';'])
            .map(|p| (p.trim().to_owned(), PapelPessoa::Autor))
            .collect(),
        other => item(other).into_iter().collect(),
    }
}

/// Evidência como o modelo a escreveu.
#[derive(Debug, Clone, Deserialize)]
pub struct EvidenciaSaida {
    #[serde(default)]
    pub pagina: Option<i64>,
    #[serde(default)]
    pub trecho: String,
}

/// Um campo como o modelo o escreveu (tolerante: o validador decide).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CampoSaida {
    #[serde(default)]
    pub valor: Option<Value>,
    #[serde(default)]
    pub confianca: Option<f64>,
    #[serde(default)]
    pub evidencias: Vec<EvidenciaSaida>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TagNovaSaida {
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub evidencias: Vec<EvidenciaSaida>,
}

/// Saída do modelo já parseada, antes da validação.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SaidaModelo {
    #[serde(default)]
    pub campos: BTreeMap<String, CampoSaida>,
    #[serde(default)]
    pub tags_novas_propostas: Vec<TagNovaSaida>,
    /// Qualquer outra chave de topo (ex.: um "acao":"deletar" induzido por injeção) é ignorada
    /// e só registrada.
    #[serde(flatten)]
    pub extras: BTreeMap<String, Value>,
}

/// Lê o JSON do modelo, tolerando cercas de código e texto ao redor.
///
/// # Errors
/// `SaidaInvalida` se não houver um objeto JSON válido.
pub fn interpretar_saida(conteudo: &str) -> Resultado<SaidaModelo> {
    let t = conteudo.trim();
    let candidato = match (t.find('{'), t.rfind('}')) {
        (Some(i), Some(f)) if f > i => &t[i..=f],
        _ => return Err(ErroIa::SaidaInvalida("sem objeto JSON".into())),
    };
    serde_json::from_str(candidato).map_err(|e| ErroIa::SaidaInvalida(e.to_string()))
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn esquema_restringe_tags_e_estante_sem_confianca() {
        let e = esquema_saida(&["Cálculo".into()], &["Matemática".into()]);
        let campos = &e["properties"]["campos"]["properties"];
        assert_eq!(
            campos["tags"]["properties"]["valor"]["items"]["enum"][0],
            "Cálculo"
        );
        assert_eq!(
            campos["estante"]["properties"]["valor"]["enum"][0],
            "Matemática"
        );
        assert!(campos["titulo"]["properties"].get("confianca").is_none());
        assert_eq!(
            campos["autores"]["properties"]["valor"]["items"]["properties"]["papel"]["enum"][2],
            "tradutor"
        );
        assert_eq!(
            e["properties"]["campos"]["required"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            esquema_visao()["properties"]["campos"]["required"]
                .as_array()
                .unwrap()
                .len(),
            7
        );
    }

    #[test]
    fn pessoas_com_papel_e_tolerancia_a_string() {
        let v = json!([{"nome": "A B", "papel": "tradutor"}, "C D", {"nome": "E", "papel": "xyz"}]);
        let p = pessoas_do_valor(&v);
        assert_eq!(p[0].1, PapelPessoa::Tradutor);
        assert_eq!(p[1].1, PapelPessoa::Autor);
        assert_eq!(p[2].1, PapelPessoa::Outro);
    }

    #[test]
    fn interpreta_com_cerca_e_chave_extra() {
        let s = interpretar_saida("```json\n{\"campos\":{},\"acao\":\"deletar\"}\n```").unwrap();
        assert!(s.extras.contains_key("acao"));
        assert!(interpretar_saida("não é json").is_err());
        assert!(interpretar_saida("{quebrado").is_err());
    }

    #[test]
    fn nomes_de_campo() {
        assert_eq!(nome_campo(Campo::Titulo), "titulo");
        assert_eq!(campo_do_nome("estante"), Some(Campo::Estante));
        assert_eq!(campo_do_nome("capa"), None);
    }
}
