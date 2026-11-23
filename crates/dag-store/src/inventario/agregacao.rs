//! Agregação do inventário: grupos de hard link, `folder_stats` bottom-up e `volume_stats`.
//!
//! Semântica dos números (A-09, CA-A05):
//!
//! * `logical_bytes`: soma dos tamanhos lógicos (`st_size`) de todas as entradas de `files` da
//!   subárvore (cada caminho conta; hard links contam uma vez por caminho).
//! * `allocated_entry_bytes`: soma de `allocated_bytes` por entrada (também conta cada caminho).
//!   **NULL** quando ao menos uma entrada da subárvore não informou alocação; 0 só quando a
//!   subárvore não tem entradas.
//! * `physical_unique_bytes`: bytes alocados contando cada Mesmo Arquivo Físico (hard links no
//!   mesmo filesystem, mesmo `(dev, inode)`) uma única vez **dentro da subárvore**. **NULL** pela
//!   mesma regra. Não exclui reflinks/COW nem snapshots (limitação conhecida).
//! * `recursive_file_count` conta arquivos regulares; symlinks e especiais ficam em
//!   `recursive_special_count`. `child_count` conta os filhos diretos (pastas e entradas).
//! * `aggregate_state`: `parcial` se algum problema de severidade `aviso` ou maior ocorreu na
//!   subárvore durante a execução; `completo` caso contrário. Itens ignorados de propósito
//!   (severidade `info`) não tornam o agregado parcial.
//!
//! Memória limitada: a árvore nunca é carregada; o cálculo usa uma tabela temporária e passes
//! SQL por altura (folhas primeiro).

use std::collections::HashSet;

use bancada_armazem::transacao_imediata;
use rusqlite::{Connection, OptionalExtension, params};

use crate::Resultado;

/// O que a agregação fez.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResumoAgregacao {
    pub pastas: u64,
    pub volumes: u64,
    pub grupos_hardlink: u64,
    /// Pastas que não chegaram a ser agregadas (só ocorre se a hierarquia tiver ciclo).
    pub pastas_sem_agregado: u64,
}

/// Atribui `hardlink_group` aos arquivos regulares com `nlink > 1` desta execução: todos os
/// caminhos do mesmo `(volume, dev, inode)` recebem o menor `files.id` do grupo. Devolve quantos
/// grupos distintos. Idempotente.
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_grupos_hardlink(conn: &Connection, run_id: i64) -> Resultado<u64> {
    conn.execute(
        "UPDATE files SET hardlink_group = NULL WHERE seen_run_id = ?1 AND hardlink_group IS NOT NULL",
        [run_id],
    )?;
    conn.execute(
        "UPDATE files SET hardlink_group = (
             SELECT MIN(g.id) FROM files g
             WHERE g.volume_id = files.volume_id AND g.native_dev = files.native_dev
               AND g.native_file_id = files.native_file_id AND g.kind = 'arquivo'
               AND g.nlink > 1 AND g.seen_run_id = ?1)
         WHERE seen_run_id = ?1 AND kind = 'arquivo' AND nlink > 1",
        [run_id],
    )?;
    let n: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT hardlink_group) FROM files WHERE seen_run_id = ?1 AND hardlink_group IS NOT NULL",
        [run_id],
        |l| l.get(0),
    )?;
    Ok(n as u64)
}

/// Calcula `folder_stats` (todas as pastas vistas na execução) e `volume_stats`.
///
/// # Errors
/// Falha do SQLite.
pub fn agregar_execucao(
    conn: &mut Connection,
    run_id: i64,
    agora: i64,
) -> Resultado<ResumoAgregacao> {
    let mut resumo = ResumoAgregacao::default();
    transacao_imediata(conn, |tx| -> Resultado<()> {
        resumo.grupos_hardlink = marcar_grupos_hardlink(tx, run_id)?;
        Ok(())
    })?;
    transacao_imediata(conn, |tx| -> Resultado<()> {
        preparar_tabela_temporaria(tx, run_id)?;
        somar_grupos_hardlink(tx, run_id)?;
        Ok(())
    })?;
    // Um passe por altura da árvore; cada passe é uma transação curta.
    loop {
        let feito = transacao_imediata(conn, |tx| -> Resultado<bool> {
            let prontas = tx.execute("UPDATE temp.agg SET st = 1 WHERE st = 0 AND pend = 0", [])?;
            if prontas == 0 {
                return Ok(false);
            }
            resumo.pastas += prontas as u64;
            tx.execute(
                "INSERT INTO folder_stats (folder_id, child_count, recursive_file_count,
                     recursive_folder_count, recursive_special_count, logical_bytes,
                     allocated_entry_bytes, physical_unique_bytes, aggregate_state, updated_at)
                 SELECT folder_id, nchild, nf, nd, ns, lb,
                        CASE WHEN abnull > 0 THEN NULL ELSE ab END,
                        CASE WHEN ppnull > 0 OR pgnull > 0 THEN NULL ELSE pp + pg END,
                        CASE WHEN err > 0 THEN 'parcial' ELSE 'completo' END, ?1
                 FROM temp.agg WHERE st = 1
                 ON CONFLICT(folder_id) DO UPDATE SET
                     child_count = excluded.child_count,
                     recursive_file_count = excluded.recursive_file_count,
                     recursive_folder_count = excluded.recursive_folder_count,
                     recursive_special_count = excluded.recursive_special_count,
                     logical_bytes = excluded.logical_bytes,
                     allocated_entry_bytes = excluded.allocated_entry_bytes,
                     physical_unique_bytes = excluded.physical_unique_bytes,
                     aggregate_state = excluded.aggregate_state, updated_at = excluded.updated_at",
                [agora],
            )?;
            tx.execute(
                "UPDATE temp.agg SET
                     pend = agg.pend - s.c, nf = agg.nf + s.nf, ns = agg.ns + s.ns,
                     nd = agg.nd + s.nd + s.c, lb = agg.lb + s.lb, ab = agg.ab + s.ab,
                     abnull = agg.abnull + s.abnull, pp = agg.pp + s.pp,
                     ppnull = agg.ppnull + s.ppnull, err = agg.err + s.err
                 FROM (SELECT parent AS p, COUNT(*) AS c, SUM(nf) AS nf, SUM(ns) AS ns, SUM(nd) AS nd,
                              SUM(lb) AS lb, SUM(ab) AS ab, SUM(abnull) AS abnull, SUM(pp) AS pp,
                              SUM(ppnull) AS ppnull, SUM(err) AS err
                       FROM temp.agg WHERE st = 1 AND parent IS NOT NULL GROUP BY parent) AS s
                 WHERE agg.folder_id = s.p",
                [],
            )?;
            tx.execute("UPDATE temp.agg SET st = 2 WHERE st = 1", [])?;
            Ok(true)
        })?;
        if !feito {
            break;
        }
    }
    transacao_imediata(conn, |tx| -> Resultado<()> {
        resumo.pastas_sem_agregado =
            tx.query_row("SELECT COUNT(*) FROM temp.agg WHERE st = 0", [], |l| {
                l.get::<_, i64>(0)
            })? as u64;
        tx.execute("DROP TABLE IF EXISTS temp.agg", [])?;
        resumo.volumes = agregar_volumes(tx, run_id, agora)?;
        Ok(())
    })?;
    Ok(resumo)
}

fn preparar_tabela_temporaria(tx: &Connection, run_id: i64) -> Resultado<()> {
    tx.execute_batch(
        "DROP TABLE IF EXISTS temp.agg;
         CREATE TEMP TABLE agg (
             folder_id INTEGER PRIMARY KEY, parent INTEGER, pend INTEGER NOT NULL, st INTEGER NOT NULL,
             nchild INTEGER NOT NULL, nf INTEGER NOT NULL, ns INTEGER NOT NULL, nd INTEGER NOT NULL,
             lb INTEGER NOT NULL, ab INTEGER NOT NULL, abnull INTEGER NOT NULL,
             pp INTEGER NOT NULL, ppnull INTEGER NOT NULL, pg INTEGER NOT NULL DEFAULT 0,
             pgnull INTEGER NOT NULL DEFAULT 0, err INTEGER NOT NULL
         );
         CREATE INDEX temp.agg_pronta ON agg(st, pend);",
    )?;
    tx.execute(
        "INSERT INTO temp.agg (folder_id, parent, pend, st, nchild, nf, ns, nd, lb, ab, abnull, pp, ppnull, err)
         SELECT f.id, f.parent_folder_id,
                (SELECT COUNT(*) FROM folders c WHERE c.parent_folder_id = f.id AND c.seen_run_id = ?1),
                0,
                (SELECT COUNT(*) FROM folders c WHERE c.parent_folder_id = f.id AND c.seen_run_id = ?1)
                    + COALESCE(d.n, 0),
                COALESCE(d.nf, 0), COALESCE(d.ns, 0), 0, COALESCE(d.lb, 0), COALESCE(d.ab, 0),
                COALESCE(d.abnull, 0), COALESCE(d.pp, 0), COALESCE(d.ppnull, 0), COALESCE(e.n, 0)
         FROM folders f
         LEFT JOIN (SELECT folder_id, COUNT(*) AS n, SUM(kind = 'arquivo') AS nf,
                           SUM(kind <> 'arquivo') AS ns, SUM(logical_bytes) AS lb,
                           SUM(allocated_bytes) AS ab, SUM(allocated_bytes IS NULL) AS abnull,
                           SUM(CASE WHEN hardlink_group IS NULL THEN allocated_bytes END) AS pp,
                           SUM(hardlink_group IS NULL AND allocated_bytes IS NULL) AS ppnull
                    FROM files WHERE seen_run_id = ?1 GROUP BY folder_id) d ON d.folder_id = f.id
         LEFT JOIN (SELECT folder_id, COUNT(*) AS n FROM scan_errors
                    WHERE run_id = ?1 AND severity <> 'info' AND folder_id IS NOT NULL
                    GROUP BY folder_id) e ON e.folder_id = f.id
         WHERE f.seen_run_id = ?1",
        [run_id],
    )?;
    Ok(())
}

/// Cada grupo de hard link conta uma vez em toda pasta que contém ao menos um de seus caminhos
/// (a própria pasta e todos os ancestrais). Percorre os ancestrais de cada caminho do grupo.
fn somar_grupos_hardlink(tx: &Connection, run_id: i64) -> Resultado<()> {
    let mut membros = tx.prepare(
        "SELECT hardlink_group, folder_id, allocated_bytes FROM files
         WHERE seen_run_id = ?1 AND hardlink_group IS NOT NULL ORDER BY hardlink_group",
    )?;
    let mut pai = tx.prepare_cached("SELECT parent FROM temp.agg WHERE folder_id = ?1")?;
    let mut soma = tx.prepare_cached("UPDATE temp.agg SET pg = pg + ?2 WHERE folder_id = ?1")?;
    let mut nula =
        tx.prepare_cached("UPDATE temp.agg SET pgnull = pgnull + 1 WHERE folder_id = ?1")?;

    let mut grupo_atual: Option<i64> = None;
    let mut visitadas: HashSet<i64> = HashSet::new();
    let mut linhas = membros.query([run_id])?;
    while let Some(l) = linhas.next()? {
        let grupo: i64 = l.get(0)?;
        let pasta: i64 = l.get(1)?;
        let alocado: Option<i64> = l.get(2)?;
        if grupo_atual != Some(grupo) {
            grupo_atual = Some(grupo);
            visitadas.clear();
        }
        let mut atual = Some(pasta);
        while let Some(p) = atual {
            if !visitadas.insert(p) {
                break;
            }
            match alocado {
                Some(b) => soma.execute(params![p, b])?,
                None => nula.execute([p])?,
            };
            atual = pai
                .query_row([p], |r| r.get::<_, Option<i64>>(0))
                .optional()?
                .flatten();
        }
    }
    Ok(())
}

fn agregar_volumes(tx: &Connection, run_id: i64, agora: i64) -> Resultado<u64> {
    let volumes: Vec<i64> = {
        let mut st = tx.prepare("SELECT DISTINCT volume_id FROM folders WHERE seen_run_id = ?1")?;
        st.query_map([run_id], |l| l.get(0))?
            .collect::<Result<_, _>>()?
    };
    for v in &volumes {
        let pastas: i64 = tx.query_row(
            "SELECT COUNT(*) FROM folders WHERE volume_id = ?1 AND seen_run_id = ?2",
            params![v, run_id],
            |l| l.get(0),
        )?;
        let (arquivos, especiais, logicos, alocado, alocado_nulo): (i64, i64, i64, i64, i64) = tx
            .query_row(
            "SELECT COALESCE(SUM(kind = 'arquivo'), 0), COALESCE(SUM(kind <> 'arquivo'), 0),
                        COALESCE(SUM(logical_bytes), 0), COALESCE(SUM(allocated_bytes), 0),
                        COALESCE(SUM(allocated_bytes IS NULL), 0)
                 FROM files WHERE volume_id = ?1 AND seen_run_id = ?2",
            params![v, run_id],
            |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?, l.get(3)?, l.get(4)?)),
        )?;
        let (fisico, fisico_nulo): (i64, i64) = tx.query_row(
            "SELECT COALESCE(SUM(a), 0), COALESCE(SUM(a IS NULL), 0) FROM (
                 SELECT allocated_bytes AS a FROM files
                   WHERE volume_id = ?1 AND seen_run_id = ?2 AND hardlink_group IS NULL
                 UNION ALL
                 SELECT MAX(allocated_bytes) AS a FROM files
                   WHERE volume_id = ?1 AND seen_run_id = ?2 AND hardlink_group IS NOT NULL
                   GROUP BY hardlink_group)",
            params![v, run_id],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )?;
        let problemas: i64 = tx.query_row(
            "SELECT COUNT(*) FROM scan_errors e JOIN folders f ON f.id = e.folder_id
             WHERE e.run_id = ?1 AND e.severity <> 'info' AND f.volume_id = ?2",
            params![run_id, v],
            |l| l.get(0),
        )?;
        tx.execute(
            "INSERT INTO volume_stats (volume_id, run_id, folder_count, file_count, special_count,
                 logical_bytes, allocated_entry_bytes, physical_unique_bytes, issue_count,
                 aggregate_state, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(volume_id) DO UPDATE SET
                 run_id = excluded.run_id, folder_count = excluded.folder_count,
                 file_count = excluded.file_count, special_count = excluded.special_count,
                 logical_bytes = excluded.logical_bytes,
                 allocated_entry_bytes = excluded.allocated_entry_bytes,
                 physical_unique_bytes = excluded.physical_unique_bytes,
                 issue_count = excluded.issue_count, aggregate_state = excluded.aggregate_state,
                 updated_at = excluded.updated_at",
            params![
                v,
                run_id,
                pastas,
                arquivos,
                especiais,
                logicos,
                (alocado_nulo == 0).then_some(alocado),
                (fisico_nulo == 0).then_some(fisico),
                problemas,
                if problemas > 0 { "parcial" } else { "completo" },
                agora
            ],
        )?;
    }
    Ok(volumes.len() as u64)
}
