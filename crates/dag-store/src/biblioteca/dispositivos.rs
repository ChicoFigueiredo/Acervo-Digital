//! Fila por Dispositivo Físico: a extração paralela usa um arquivo por disco (até N discos).

use rusqlite::{Connection, OptionalExtension, params};

use super::jobs::{COLUNAS, Job, de_linha};
use crate::Resultado;

/// Chave do Dispositivo Físico do arquivo: o `device_id` do Volume, ou `-volume_id` quando o
/// Volume não tem dispositivo conhecido (cada Volume sem dispositivo conta como um disco).
///
/// # Errors
/// Falha do SQLite.
pub fn dispositivo_do_arquivo(conn: &Connection, file_id: i64) -> Resultado<i64> {
    Ok(conn
        .prepare_cached(
            "SELECT COALESCE(v.device_id, -v.id) FROM files f JOIN volumes v ON v.id = f.volume_id
             WHERE f.id = ?1",
        )?
        .query_row([file_id], |l| l.get(0))
        .optional()?
        .unwrap_or(0))
}

/// Como `pegar_proximo_job`, mas pula os jobs cujo arquivo está num dos Dispositivos Físicos
/// em `ocupados` (extração paralela: um arquivo por disco). Devolve também a chave do
/// dispositivo do job escolhido. Atômico: dois trabalhadores nunca recebem o mesmo job.
///
/// # Errors
/// Falha do SQLite.
pub fn pegar_proximo_job_fora(
    conn: &Connection,
    agora: i64,
    ocupados: &[i64],
) -> Resultado<Option<(Job, i64)>> {
    let json = serde_json::to_string(ocupados)?;
    let job = conn
        .prepare_cached(&format!(
            "UPDATE jobs_bibliograficos SET estado = 'em_andamento', iniciado_em = ?1, atualizado_em = ?1,
                    tentativas = tentativas + 1
             WHERE id = (SELECT j.id FROM jobs_bibliograficos j
                         JOIN files f ON f.id = j.file_id JOIN volumes v ON v.id = f.volume_id
                         WHERE j.estado = 'pendente'
                           AND COALESCE(v.device_id, -v.id) NOT IN (SELECT value FROM json_each(?2))
                         ORDER BY j.ordem DESC, j.id DESC LIMIT 1)
             RETURNING {COLUNAS}"
        ))?
        .query_row(params![agora, json], de_linha)
        .optional()?;
    match job {
        Some(j) => {
            let d = dispositivo_do_arquivo(conn, j.file_id)?;
            Ok(Some((j, d)))
        }
        None => Ok(None),
    }
}

/// Devolve à fila um job `em_andamento` (pausa/cancelamento antes de processar). A tentativa
/// consumida ao pegá-lo é desfeita.
///
/// # Errors
/// Falha do SQLite.
pub fn devolver_job(conn: &Connection, job_id: i64, agora: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE jobs_bibliograficos SET estado = 'pendente', tentativas = max(tentativas - 1, 0),
                atualizado_em = ?2 WHERE id = ?1 AND estado = 'em_andamento'",
        params![job_id, agora],
    )?;
    Ok(())
}
