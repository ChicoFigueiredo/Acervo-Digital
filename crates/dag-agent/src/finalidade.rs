//! Classificação de finalidade (A-08) de pastas e arquivos de uma execução, só pelo caminho
//! (`classificar_finalidade_caminho`). Grava em `classifications` (escopo `finalidade`) e liga
//! por `class_id`. Não consolida pastas pelo conteúdo (ver TODO no relatório do incremento).

use std::collections::HashMap;

use dag_core::{Finalidade, classificar_finalidade_caminho};
use rusqlite::{Connection, OptionalExtension, params};

use crate::erro::ErroAgente;
use dag_store::Catalogo;

const LOTE: i64 = 5000;
pub(crate) const ESCOPO: &str = "finalidade";

/// Nome estável (snake_case, o mesmo do serde) gravado em `classifications.category`.
pub(crate) fn nome(f: Finalidade) -> String {
    serde_json::to_value(f)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "desconhecido".into())
}

pub(crate) fn de_nome(s: &str) -> Option<Finalidade> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

type Chave = (String, &'static str, u32);

fn classe_id(
    tx: &Connection,
    cache: &mut HashMap<Chave, i64>,
    f: Finalidade,
    regra: &'static str,
    confianca: f32,
    agora: i64,
) -> Result<i64, ErroAgente> {
    let chave = (nome(f), regra, confianca.to_bits());
    if let Some(id) = cache.get(&chave) {
        return Ok(*id);
    }
    let conf = f64::from(confianca);
    let existente: Option<i64> = tx
        .query_row(
            "SELECT id FROM classifications
             WHERE category = ?1 AND scope = ?2 AND rule_id IS ?3 AND confidence IS ?4",
            params![chave.0, ESCOPO, regra, conf],
            |l| l.get(0),
        )
        .optional()?;
    let id = match existente {
        Some(id) => id,
        None => {
            tx.execute(
                "INSERT INTO classifications (category, scope, rule_id, confidence, classified_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![chave.0, ESCOPO, regra, conf, agora],
            )?;
            tx.last_insert_rowid()
        }
    };
    cache.insert(chave, id);
    Ok(id)
}

/// Classifica pastas e arquivos vistos em `run_id` que ainda não têm `class_id`.
/// Devolve `(pastas, arquivos)` classificados.
pub(crate) fn classificar_execucao(
    catalogo: &mut Catalogo,
    run_id: i64,
    agora: i64,
) -> Result<(u64, u64), ErroAgente> {
    let mut cache = HashMap::new();
    let pastas = passe(catalogo, &mut cache, run_id, agora, true)?;
    let arquivos = passe(catalogo, &mut cache, run_id, agora, false)?;
    Ok((pastas, arquivos))
}

fn passe(
    catalogo: &mut Catalogo,
    cache: &mut HashMap<Chave, i64>,
    run_id: i64,
    agora: i64,
    pastas: bool,
) -> Result<u64, ErroAgente> {
    let (select, update) = if pastas {
        (
            "SELECT id, path_raw FROM folders
             WHERE id > ?2 AND seen_run_id = ?1 AND class_id IS NULL ORDER BY id LIMIT ?3",
            "UPDATE folders SET class_id = ?1, classification_confidence = ?2 WHERE id = ?3",
        )
    } else {
        (
            "SELECT id, path_raw FROM files
             WHERE id > ?2 AND seen_run_id = ?1 AND class_id IS NULL ORDER BY id LIMIT ?3",
            "UPDATE files SET class_id = ?1 WHERE id = ?3",
        )
    };
    let mut ultimo = 0_i64;
    let mut total = 0_u64;
    loop {
        let lote: Vec<(i64, Vec<u8>)> = {
            let conn = catalogo.armazem().conexao();
            let mut st = conn.prepare_cached(select)?;
            st.query_map(params![run_id, ultimo, LOTE], |l| {
                Ok((l.get(0)?, l.get(1)?))
            })?
            .collect::<Result<_, _>>()?
        };
        let Some((fim, _)) = lote.last() else { break };
        ultimo = *fim;
        total += lote.len() as u64;
        catalogo
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroAgente> {
                let mut up = tx.prepare_cached(update)?;
                for (id, caminho) in &lote {
                    let c = classificar_finalidade_caminho(caminho);
                    let classe =
                        classe_id(tx, cache, c.finalidade, c.regra_id, c.confianca, agora)?;
                    up.execute(params![classe, f64::from(c.confianca), id])?;
                }
                Ok(())
            })?;
    }
    Ok(total)
}
