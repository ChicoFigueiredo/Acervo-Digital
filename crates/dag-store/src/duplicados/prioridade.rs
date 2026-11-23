//! Prioridade de Volume/pasta (A-26): ordem definida pelo usuário que decide a Cópia Principal.

use std::collections::HashSet;

use dag_core::Autor;
use rusqlite::Connection;

use super::grupos::recalcular_principais;
use super::resolver::{resolver_pasta, resolver_volume};
use super::{ErroDuplicados, Resultado};
use crate::Catalogo;

/// Uma entrada da lista de prioridade (menor `ordem` = maior prioridade).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemPrioridade {
    pub ordem: i64,
    pub volume: i64,
    /// `None` = o Volume inteiro.
    pub pasta: Option<i64>,
    pub nome: String,
}

/// Resolve um item: Volume (id, rótulo, `platform_id`, ponto de montagem) ou pasta catalogada.
fn resolver_item(conn: &Connection, r: &str) -> Resultado<(i64, Option<i64>)> {
    match resolver_volume(conn, r) {
        Ok(v) => Ok((v, None)),
        Err(ErroDuplicados::NaoEncontrado(m)) => {
            if r.contains('/') {
                let (pasta, volume) = resolver_pasta(conn, r).map_err(|e| match e {
                    ErroDuplicados::NaoEncontrado(_) => ErroDuplicados::NaoEncontrado(format!(
                        "`{r}` não é Volume nem pasta catalogada"
                    )),
                    outro => outro,
                })?;
                Ok((volume, Some(pasta)))
            } else {
                Err(ErroDuplicados::NaoEncontrado(m))
            }
        }
        Err(e) => Err(e),
    }
}

/// Substitui a Prioridade de Volume pela `ordem` dada (do mais ao menos prioritário) e
/// reescolhe a Cópia Principal dos grupos em modo automático; escolhas manuais ficam.
///
/// # Errors
/// [`ErroDuplicados::NaoEncontrado`]/[`ErroDuplicados::Ambiguo`] para item inválido;
/// [`ErroDuplicados::Invalido`] para item repetido ou lista vazia.
pub fn definir_prioridade(
    cat: &mut Catalogo,
    ordem: &[String],
    _autor: Autor,
    agora: i64,
) -> Resultado<Vec<ItemPrioridade>> {
    if ordem.is_empty() {
        return Err(ErroDuplicados::Invalido(
            "informe ao menos um Volume ou pasta".into(),
        ));
    }
    cat.armazem_mut().transacao_imediata(|tx| {
        let mut vistos = HashSet::new();
        let mut itens = Vec::new();
        for r in ordem {
            let item = resolver_item(tx, r)?;
            if !vistos.insert(item) {
                return Err(ErroDuplicados::Invalido(format!(
                    "`{r}` aparece mais de uma vez na prioridade"
                )));
            }
            itens.push(item);
        }
        tx.execute("DELETE FROM prioridades", [])?;
        for (i, (volume, pasta)) in itens.iter().enumerate() {
            tx.execute(
                "INSERT INTO prioridades (volume_id, folder_id, ordem) VALUES (?1, ?2, ?3)",
                rusqlite::params![volume, pasta, i as i64 + 1],
            )?;
        }
        recalcular_principais(tx, None, agora)?;
        listar_prioridades(tx)
    })
}

/// A lista de prioridade atual.
///
/// # Errors
/// Falha do SQLite.
pub fn listar_prioridades(conn: &Connection) -> Resultado<Vec<ItemPrioridade>> {
    let mut st = conn.prepare(
        "SELECT p.ordem, p.volume_id, p.folder_id,
                COALESCE(f.path_display, v.rotulo, v.platform_id)
         FROM prioridades p JOIN volumes v ON v.id = p.volume_id
         LEFT JOIN folders f ON f.id = p.folder_id ORDER BY p.ordem, p.id",
    )?;
    Ok(st
        .query_map([], |l| {
            Ok(ItemPrioridade {
                ordem: l.get(0)?,
                volume: l.get(1)?,
                pasta: l.get(2)?,
                nome: l.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?)
}
