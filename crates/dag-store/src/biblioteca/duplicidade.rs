//! Candidatos a duplicidade bibliográfica. Só `exata` (mesmo Hash Completo e tamanho) pode gerar
//! espaço recuperável; as demais vão para revisão e nunca autorizam descarte.

use dag_biblio::TipoDuplicidade;
use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};

use super::{Pagina, enum_de_texto, limite_de, texto_enum};
use crate::Resultado;

/// Com o que o arquivo candidato se parece.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", content = "id", rename_all = "snake_case")]
pub enum AlvoDuplicidade {
    /// `formatos_homologados.id`.
    FormatoHomologado(i64),
    /// Outro arquivo do Catálogo (`files.id`).
    Arquivo(i64),
    /// `edicoes.id`.
    Edicao(i64),
}

impl AlvoDuplicidade {
    fn partes(self) -> (&'static str, i64) {
        match self {
            Self::FormatoHomologado(i) => ("formato_homologado", i),
            Self::Arquivo(i) => ("arquivo", i),
            Self::Edicao(i) => ("edicao", i),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiscoDuplicidade {
    Baixo,
    Medio,
    Alto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoCandidato {
    Aberto,
    Confirmado,
    DescartadoComoFalso,
}

/// Candidato a registrar.
#[derive(Debug, Clone)]
pub struct NovoCandidato {
    pub file_id: i64,
    pub alvo: AlvoDuplicidade,
    pub tipo: TipoDuplicidade,
    pub evidencias_json: String,
    pub risco: RiscoDuplicidade,
    /// Tamanho do arquivo candidato (conta como recuperável só se `tipo` for `Exata`).
    pub bytes: u64,
}

/// Candidato gravado.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CandidatoDuplicidade {
    pub id: i64,
    pub file_id: i64,
    pub alvo: AlvoDuplicidade,
    pub tipo: TipoDuplicidade,
    pub evidencias_json: String,
    pub risco: RiscoDuplicidade,
    pub estado: EstadoCandidato,
    pub bytes: u64,
    pub criado_em: i64,
    pub decidido_em: Option<i64>,
}

const COLUNAS: &str = "id, file_id, alvo_tipo, alvo_id, tipo, evidencias_json, risco, estado, bytes, criado_em, decidido_em";

fn de_linha(l: &Row<'_>) -> rusqlite::Result<CandidatoDuplicidade> {
    let alvo_tipo: String = l.get(2)?;
    let alvo_id: i64 = l.get(3)?;
    let alvo = match alvo_tipo.as_str() {
        "formato_homologado" => AlvoDuplicidade::FormatoHomologado(alvo_id),
        "arquivo" => AlvoDuplicidade::Arquivo(alvo_id),
        _ => AlvoDuplicidade::Edicao(alvo_id),
    };
    Ok(CandidatoDuplicidade {
        id: l.get(0)?,
        file_id: l.get(1)?,
        alvo,
        tipo: enum_de_texto(&l.get::<_, String>(4)?)?,
        evidencias_json: l.get(5)?,
        risco: enum_de_texto(&l.get::<_, String>(6)?)?,
        estado: enum_de_texto(&l.get::<_, String>(7)?)?,
        bytes: l.get::<_, i64>(8)? as u64,
        criado_em: l.get(9)?,
        decidido_em: l.get(10)?,
    })
}

/// Registra o candidato (idempotente por arquivo, alvo e tipo). Reprocessar atualiza evidências,
/// risco e bytes, mas **não** reabre um candidato já confirmado ou descartado como falso.
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_candidato(conn: &Connection, c: &NovoCandidato, agora: i64) -> Resultado<i64> {
    let (alvo_tipo, alvo_id) = c.alvo.partes();
    Ok(conn.query_row(
        "INSERT INTO candidatos_duplicidade (file_id, alvo_tipo, alvo_id, tipo, evidencias_json, risco, bytes, criado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(file_id, alvo_tipo, alvo_id, tipo) DO UPDATE SET
            evidencias_json = excluded.evidencias_json, risco = excluded.risco, bytes = excluded.bytes
         RETURNING id",
        params![
            c.file_id,
            alvo_tipo,
            alvo_id,
            texto_enum(&c.tipo),
            c.evidencias_json,
            texto_enum(&c.risco),
            c.bytes as i64,
            agora
        ],
        |l| l.get(0),
    )?)
}

/// Confirma ou descarta (como falso) um candidato; também serve para reabri-lo.
///
/// # Errors
/// Falha do SQLite.
pub fn decidir_candidato(
    conn: &Connection,
    id: i64,
    estado: EstadoCandidato,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "UPDATE candidatos_duplicidade SET estado = ?2, decidido_em = CASE WHEN ?2 = 'aberto' THEN NULL ELSE ?3 END
         WHERE id = ?1",
        params![id, texto_enum(&estado), agora],
    )?;
    Ok(())
}

/// Candidatos do arquivo.
///
/// # Errors
/// Falha do SQLite.
pub fn candidatos_do_arquivo(
    conn: &Connection,
    file_id: i64,
) -> Resultado<Vec<CandidatoDuplicidade>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS} FROM candidatos_duplicidade WHERE file_id = ?1 ORDER BY id"
    ))?;
    let v = st
        .query_map([file_id], de_linha)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Cursor da lista por bytes: `(bytes, id)` do último item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorCandidato {
    pub bytes: i64,
    pub id: i64,
}

/// Candidatos abertos de um tipo, do maior para o menor (`bytes`, `id`), por cursor keyset.
/// Para `Exata`, é a lista "duplicatas por bytes recuperáveis" da GUI/CLI/MCP.
///
/// # Errors
/// Falha do SQLite.
pub fn listar_candidatos_por_bytes(
    conn: &Connection,
    estado: EstadoCandidato,
    tipo: TipoDuplicidade,
    apos: Option<CursorCandidato>,
    limite: usize,
) -> Resultado<Pagina<CandidatoDuplicidade, CursorCandidato>> {
    let limite = limite_de(limite);
    let (bytes, id) = apos.map_or((i64::MAX, i64::MAX), |c| (c.bytes, c.id));
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS} FROM candidatos_duplicidade
         WHERE estado = ?1 AND tipo = ?2 AND (bytes, id) < (?3, ?4)
         ORDER BY bytes DESC, id DESC LIMIT ?5"
    ))?;
    let linhas = st
        .query_map(
            params![
                texto_enum(&estado),
                texto_enum(&tipo),
                bytes,
                id,
                (limite + 1) as i64
            ],
            de_linha,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Pagina::de_linhas(linhas, limite, |c| CursorCandidato {
        bytes: c.bytes as i64,
        id: c.id,
    }))
}

/// Soma dos bytes dos candidatos **exatos abertos** (varredura só do índice
/// `(estado, tipo, bytes)`; nunca soma `mesma_edicao`, `mesma_obra` nem `incerta`).
///
/// # Errors
/// Falha do SQLite.
pub fn bytes_recuperaveis_exatos(conn: &Connection) -> Resultado<u64> {
    let t: i64 = conn.query_row(
        "SELECT COALESCE(SUM(bytes), 0) FROM candidatos_duplicidade WHERE estado = 'aberto' AND tipo = 'exata'",
        [],
        |l| l.get(0),
    )?;
    Ok(t as u64)
}

/// Candidatos abertos (contador mantido).
///
/// # Errors
/// Falha do SQLite.
pub fn contar_candidatos_abertos(conn: &Connection) -> Resultado<i64> {
    super::contador(conn, "biblio_candidatos_abertos")
}
