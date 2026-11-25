//! Curadoria Bibliográfica (Fase B): persistência de bibliotecas Calibre, espelho da Biblioteca
//! Homologada, Obra > Edição > Formato, fila de jobs, extrações, sugestões, decisões humanas,
//! duplicidade, alertas, A Classificar, homologações e backups. Esquema: migração 0011.
//!
//! Padrões herdados dos demais módulos do Catálogo: as funções recebem uma `&Connection` (o
//! chamador decide a transação; o agente é o escritor único, ADR 0004), o instante `agora` (segundos
//! Unix) vem do chamador, listas grandes saem por **cursor keyset** (nunca `OFFSET`) e as contagens
//! vêm de **contadores mantidos por gatilhos** (nunca `COUNT(*)` sobre tabelas grandes).
//!
//! Nada aqui toca em arquivos de biblioteca Calibre nem em `files`: o DAG só registra o que viu
//! (a Biblioteca Homologada é somente leitura).

pub mod alertas;
pub mod apoio;
pub mod bibliotecas;
pub mod cadastros;
pub mod catalogo;
pub mod curadoria;
pub mod dispositivos;
pub mod duplicidade;
pub mod espelho;
pub mod itens;
pub mod jobs;
pub mod obras;
pub mod vocabulario;

use rusqlite::Connection;
use rusqlite::types::Type;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use alertas::*;
pub use apoio::*;
pub use bibliotecas::*;
pub use cadastros::*;
pub use catalogo::*;
pub use curadoria::*;
pub use dispositivos::*;
pub use duplicidade::*;
pub use espelho::*;
pub use itens::*;
pub use jobs::*;
pub use obras::*;
pub use vocabulario::*;

use crate::{Erro, Resultado};

/// Uma página de resultado por cursor keyset: `proximo` é o cursor da página seguinte (ou `None`
/// no fim).
#[derive(Debug, Clone, PartialEq)]
pub struct Pagina<T, C> {
    pub itens: Vec<T>,
    pub proximo: Option<C>,
}

impl<T, C> Pagina<T, C> {
    /// Monta a página a partir de `limite + 1` linhas lidas: a sobra só indica que há mais.
    pub(crate) fn de_linhas(mut linhas: Vec<T>, limite: usize, cursor: impl Fn(&T) -> C) -> Self {
        let proximo = if linhas.len() > limite {
            linhas.truncate(limite);
            linhas.last().map(cursor)
        } else {
            None
        };
        Self {
            itens: linhas,
            proximo,
        }
    }

    /// Converte os itens mantendo o cursor.
    pub(crate) fn mapear<U>(self, f: impl FnMut(T) -> U) -> Pagina<U, C> {
        Pagina {
            itens: self.itens.into_iter().map(f).collect(),
            proximo: self.proximo,
        }
    }
}

/// Quem tomou uma decisão (mesma lista do `autor` das Marcações).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutorAcao {
    Gui,
    Cli,
    Mcp,
}

/// Nome de um enum serde de unidade (`snake_case`), como gravado nas colunas `TEXT`.
pub(crate) fn texto_enum<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

/// Inverso de [`texto_enum`], como erro de conversão do SQLite (serve dentro de `query_map`).
pub(crate) fn enum_de_texto<T: DeserializeOwned>(s: &str) -> rusqlite::Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned()))
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(e)))
}

/// Erro de regra de negócio (precondição violada) no vocabulário do armazém.
pub(crate) fn invalido(msg: impl Into<String>) -> Erro {
    Erro::Sqlite(rusqlite::Error::ToSqlConversionFailure(msg.into().into()))
}

pub(crate) fn hash_de_blob(b: Option<Vec<u8>>) -> Option<[u8; 32]> {
    b.and_then(|v| <[u8; 32]>::try_from(v).ok())
}

pub(crate) fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

/// Limite de página seguro (1..=1000).
pub(crate) fn limite_de(l: usize) -> usize {
    l.clamp(1, 1000)
}

/// Valor de um contador mantido (0 se ainda não existe).
///
/// # Errors
/// Falha do SQLite.
pub fn contador(conn: &Connection, chave: &str) -> Resultado<i64> {
    Ok(conn
        .query_row(
            "SELECT valor FROM contadores WHERE chave = ?1",
            [chave],
            |l| l.get(0),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(0),
            e => Err(e),
        })?)
}

/// Título principal normalizado (sem artigo inicial, subtítulo nem volume): chave do pré-filtro
/// de identidade bibliográfica (coluna `livros_homologados.titulo_principal`).
#[must_use]
pub fn titulo_principal(titulo: &str) -> String {
    dag_biblio::normalizacao::normalizar_titulo(titulo).principal
}

/// Normaliza texto para comparação: minúsculas, sem acentos latinos, só letras/dígitos separados
/// por um espaço. Serve a `titulo_norm`, `nome_norm` e chaves de obra.
#[must_use]
pub fn normalizar_texto(s: &str) -> String {
    let mut saida = String::with_capacity(s.len());
    let mut espaco = true;
    for c in s.chars() {
        let c = match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' | 'Á' | 'À' | 'Â' | 'Ã' | 'Ä' | 'Å' => {
                'a'
            }
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'u',
            'ç' | 'Ç' => 'c',
            'ñ' | 'Ñ' => 'n',
            c => c,
        };
        if c.is_alphanumeric() {
            saida.extend(c.to_lowercase());
            espaco = false;
        } else if !espaco {
            saida.push(' ');
            espaco = true;
        }
    }
    saida.truncate(saida.trim_end().len());
    saida
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn normaliza_acentos_e_pontuacao() {
        assert_eq!(
            normalizar_texto("  A Dança, dos Ônibus! "),
            "a danca dos onibus"
        );
        assert_eq!(normalizar_texto(""), "");
    }

    #[test]
    fn hex_e_pagina() {
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
        let p = Pagina::de_linhas(vec![1, 2, 3], 2, |x| *x);
        assert_eq!(p.itens, vec![1, 2]);
        assert_eq!(p.proximo, Some(2));
        let p = Pagina::de_linhas(vec![1, 2], 2, |x| *x);
        assert_eq!(p.proximo, None);
    }
}
