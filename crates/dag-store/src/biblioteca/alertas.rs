//! Alertas (WOW-01 "Você já tem este livro", resumo diário WOW-10).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::{Pagina, enum_de_texto, limite_de, texto_enum};
use crate::Resultado;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoAlerta {
    JaTemEsteLivro,
    DuplicataExata,
    MesmaEdicaoOutroFormato,
    PossivelMesmaObra,
    ExtracaoFalhou,
    BibliotecaAlterada,
    ResumoDiario,
    Outro,
}

/// Alerta a emitir. Com `chave`, emitir de novo o mesmo fato não cria outro alerta.
#[derive(Debug, Clone)]
pub struct NovoAlerta {
    pub chave: Option<String>,
    pub tipo: TipoAlerta,
    pub file_id: Option<i64>,
    pub dados_json: String,
}

/// Alerta gravado.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Alerta {
    pub id: i64,
    pub chave: Option<String>,
    pub tipo: TipoAlerta,
    pub file_id: Option<i64>,
    pub dados_json: String,
    pub criado_em: i64,
    pub visto_em: Option<i64>,
    pub silenciado: bool,
}

const COLUNAS: &str = "id, chave, tipo, file_id, dados_json, criado_em, visto_em, silenciado";

fn de_linha(l: &Row<'_>) -> rusqlite::Result<Alerta> {
    Ok(Alerta {
        id: l.get(0)?,
        chave: l.get(1)?,
        tipo: enum_de_texto(&l.get::<_, String>(2)?)?,
        file_id: l.get(3)?,
        dados_json: l.get(4)?,
        criado_em: l.get(5)?,
        visto_em: l.get(6)?,
        silenciado: l.get(7)?,
    })
}

/// Emite o alerta. Devolve `(id, novo)`: `novo = false` se a `chave` já existia.
///
/// # Errors
/// Falha do SQLite.
pub fn emitir_alerta(conn: &Connection, a: &NovoAlerta, agora: i64) -> Resultado<(i64, bool)> {
    let n = conn.execute(
        "INSERT INTO alertas_biblio (chave, tipo, file_id, dados_json, criado_em) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(chave) DO NOTHING",
        params![a.chave, texto_enum(&a.tipo), a.file_id, a.dados_json, agora],
    )?;
    if n == 1 {
        return Ok((conn.last_insert_rowid(), true));
    }
    let id = conn.query_row(
        "SELECT id FROM alertas_biblio WHERE chave = ?1",
        [&a.chave],
        |l| l.get(0),
    )?;
    Ok((id, false))
}

/// Um alerta pelo id.
///
/// # Errors
/// Falha do SQLite.
pub fn obter_alerta(conn: &Connection, id: i64) -> Resultado<Option<Alerta>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUNAS} FROM alertas_biblio WHERE id = ?1"),
            [id],
            de_linha,
        )
        .optional()?)
}

/// Alertas não vistos e não silenciados, do mais novo ao mais antigo, por cursor (`id`).
///
/// # Errors
/// Falha do SQLite.
pub fn listar_nao_vistos(
    conn: &Connection,
    antes_de_id: Option<i64>,
    limite: usize,
) -> Resultado<Pagina<Alerta, i64>> {
    let limite = limite_de(limite);
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS} FROM alertas_biblio WHERE visto_em IS NULL AND silenciado = 0 AND id < ?1
         ORDER BY id DESC LIMIT ?2"
    ))?;
    let linhas = st
        .query_map(
            params![antes_de_id.unwrap_or(i64::MAX), (limite + 1) as i64],
            de_linha,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Pagina::de_linhas(linhas, limite, |a| a.id))
}

/// Marca o alerta como visto (não sobrescreve um "visto" anterior). Devolve se mudou.
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_visto(conn: &Connection, id: i64, agora: i64) -> Resultado<bool> {
    Ok(conn.execute(
        "UPDATE alertas_biblio SET visto_em = ?2 WHERE id = ?1 AND visto_em IS NULL",
        params![id, agora],
    )? == 1)
}

/// Marca como vistos todos os alertas até `id` (inclusive). Devolve quantos.
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_vistos_ate(conn: &Connection, id: i64, agora: i64) -> Resultado<u64> {
    Ok(conn.execute(
        "UPDATE alertas_biblio SET visto_em = ?2 WHERE id <= ?1 AND visto_em IS NULL",
        params![id, agora],
    )? as u64)
}

/// Silencia (ou reativa) um alerta.
///
/// # Errors
/// Falha do SQLite.
pub fn silenciar_alerta(conn: &Connection, id: i64, silenciar: bool) -> Resultado<()> {
    conn.execute(
        "UPDATE alertas_biblio SET silenciado = ?2 WHERE id = ?1",
        params![id, silenciar],
    )?;
    Ok(())
}

/// Alertas não vistos (contador mantido).
///
/// # Errors
/// Falha do SQLite.
pub fn contar_nao_vistos(conn: &Connection) -> Resultado<i64> {
    super::contador(conn, "biblio_alertas_nao_vistos")
}

/// Alertas de um tipo no período.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResumoAlertas {
    pub tipo: TipoAlerta,
    pub total: i64,
    pub nao_vistos: i64,
}

/// Resumo por tipo dos alertas criados em `[de, ate)` (resumo diário, WOW-10).
///
/// # Errors
/// Falha do SQLite.
pub fn resumo_periodo(conn: &Connection, de: i64, ate: i64) -> Resultado<Vec<ResumoAlertas>> {
    let mut st = conn.prepare(
        "SELECT tipo, COUNT(*), SUM(visto_em IS NULL AND silenciado = 0) FROM alertas_biblio
         WHERE criado_em >= ?1 AND criado_em < ?2 GROUP BY tipo ORDER BY tipo",
    )?;
    let v = st
        .query_map(params![de, ate], |l| {
            Ok(ResumoAlertas {
                tipo: enum_de_texto(&l.get::<_, String>(0)?)?,
                total: l.get(1)?,
                nao_vistos: l.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}
