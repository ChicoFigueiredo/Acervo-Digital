//! Ponto de Confiança por Raiz (ADR 0003): marca que só o monitoramento mantém a Raiz em dia.
//!
//! Gravado quando uma passada de verificação não encontra nenhuma mudança; apagado por qualquer
//! lacuna do monitor (overflow, fila saturada, limite de watches, raiz desmontada, watches
//! refeitos) e por encerramento não limpo. Raiz sem linha ou com `confiavel_em` nulo não é
//! confiável.

use rusqlite::{Connection, params};

use crate::Resultado;
use crate::inventario::exibir;

/// Situação da confiança de uma Raiz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaizConfianca {
    pub volume_id: i64,
    pub path_raw: Vec<u8>,
    pub path_display: String,
    /// Segundos Unix em que a Raiz virou confiável; `None` = fora do Ponto de Confiança.
    pub confiavel_em: Option<i64>,
    pub perdido_em: Option<i64>,
    pub motivo_perda: Option<String>,
}

impl RaizConfianca {
    #[must_use]
    pub fn confiavel(&self) -> bool {
        self.confiavel_em.is_some()
    }
}

/// Marca a Raiz como confiável agora (limpa a perda anterior).
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_confiavel(
    conn: &Connection,
    volume_id: i64,
    path_raw: &[u8],
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "INSERT INTO ponto_confianca (volume_id, path_raw, path_display, confiavel_em)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(volume_id, path_raw) DO UPDATE SET confiavel_em = ?4, perdido_em = NULL,
             motivo_perda = NULL",
        params![volume_id, path_raw, exibir(path_raw), agora],
    )?;
    Ok(())
}

/// Tira uma Raiz do Ponto de Confiança, com o motivo. Sem efeito se já não era confiável (o
/// primeiro motivo fica). Raiz desconhecida vira linha sem confiança, para o status mostrar.
///
/// # Errors
/// Falha do SQLite.
pub fn perder(
    conn: &Connection,
    volume_id: i64,
    path_raw: &[u8],
    motivo: &str,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "INSERT INTO ponto_confianca (volume_id, path_raw, path_display, perdido_em, motivo_perda)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(volume_id, path_raw) DO UPDATE SET confiavel_em = NULL,
             perdido_em = ?4, motivo_perda = ?5 WHERE confiavel_em IS NOT NULL",
        params![volume_id, path_raw, exibir(path_raw), agora, motivo],
    )?;
    Ok(())
}

/// Tira todas as Raízes do Ponto de Confiança (encerramento não limpo). Devolve quantas eram
/// confiáveis.
///
/// # Errors
/// Falha do SQLite.
pub fn perder_todas(conn: &Connection, motivo: &str, agora: i64) -> Resultado<usize> {
    Ok(conn.execute(
        "UPDATE ponto_confianca SET confiavel_em = NULL, perdido_em = ?1, motivo_perda = ?2
         WHERE confiavel_em IS NOT NULL",
        params![agora, motivo],
    )?)
}

/// Todas as Raízes com situação conhecida.
///
/// # Errors
/// Falha do SQLite.
pub fn listar(conn: &Connection) -> Resultado<Vec<RaizConfianca>> {
    let mut st = conn.prepare(
        "SELECT volume_id, path_raw, path_display, confiavel_em, perdido_em, motivo_perda
         FROM ponto_confianca ORDER BY volume_id, path_raw",
    )?;
    let v = st
        .query_map([], |l| {
            Ok(RaizConfianca {
                volume_id: l.get(0)?,
                path_raw: l.get(1)?,
                path_display: l.get(2)?,
                confiavel_em: l.get(3)?,
                perdido_em: l.get(4)?,
                motivo_perda: l.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// A Raiz está no Ponto de Confiança?
///
/// # Errors
/// Falha do SQLite.
pub fn eh_confiavel(conn: &Connection, volume_id: i64, path_raw: &[u8]) -> Resultado<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ponto_confianca
         WHERE volume_id = ?1 AND path_raw = ?2 AND confiavel_em IS NOT NULL",
        params![volume_id, path_raw],
        |l| l.get(0),
    )?;
    Ok(n > 0)
}
