//! Protocolo do helper: uma requisição JSON em uma linha no stdin; linhas JSON no stdout.
//!
//! ```text
//! -> {"raizes":["/mnt/dados"],"caminho":"/mnt/dados/privado"}
//! <- {"tipo":"dir","dev":..,"ino":..,...}            (a própria pasta)
//! <- {"tipo":"entrada","nome":"a.txt","tipo_arquivo":"file","dev":..,...}   (uma por filho)
//! <- {"tipo":"fim","entradas":N}
//!  ou {"tipo":"erro","codigo":"caminho_invalido","mensagem":".."}
//! ```
//!
//! Nomes e alvos de symlink são `nome`/`alvo` (UTF-8) ou `nome_hex`/`alvo_hex` (bytes brutos).

use serde::{Deserialize, Serialize};

/// Versão do protocolo (só informativa; a requisição tem formato fixo).
pub const VERSAO_PROTOCOLO: u32 = 1;

/// A única requisição aceita. Qualquer outro campo é recusado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requisicao {
    /// Raízes autorizadas (absolutas, canônicas). `caminho` deve estar dentro de uma delas.
    pub raizes: Vec<String>,
    /// Pasta cujos metadados (e os de seus filhos diretos) serão devolvidos.
    pub caminho: String,
}

/// Tipo do objeto, como `lstat` o vê.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoArquivo {
    File,
    Dir,
    Symlink,
    Fifo,
    Socket,
    Block,
    Char,
    Other,
}

/// Metadados `lstat` (os mesmos campos que o scanner coleta sem privilégio).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadados {
    pub tipo_arquivo: TipoArquivo,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    /// Blocos de 512 bytes alocados.
    pub blocks: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
}

/// Uma linha da resposta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", rename_all = "snake_case")]
pub enum Linha {
    /// A própria pasta pedida.
    Dir {
        #[serde(flatten)]
        meta: Metadados,
    },
    Entrada {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nome: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nome_hex: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alvo: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alvo_hex: Option<String>,
        #[serde(flatten)]
        meta: Metadados,
    },
    Fim {
        entradas: u64,
    },
    Erro {
        codigo: String,
        mensagem: String,
    },
}

/// Codifica bytes: `(Some(utf8), None)` ou `(None, Some(hex))`.
#[must_use]
pub fn codificar_bytes(b: &[u8]) -> (Option<String>, Option<String>) {
    match std::str::from_utf8(b) {
        Ok(s) => (Some(s.to_string()), None),
        Err(_) => {
            use std::fmt::Write;
            let mut h = String::with_capacity(b.len() * 2);
            for x in b {
                let _ = write!(h, "{x:02x}");
            }
            (None, Some(h))
        }
    }
}

/// Inverso de [`codificar_bytes`]; `None` se o hex for inválido ou faltarem ambos.
#[must_use]
pub fn decodificar_bytes(texto: Option<&str>, hex: Option<&str>) -> Option<Vec<u8>> {
    if let Some(t) = texto {
        return Some(t.as_bytes().to_vec());
    }
    let h = hex?;
    if h.len() % 2 != 0 {
        return None;
    }
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(h.get(i..i + 2)?, 16).ok())
        .collect()
}
