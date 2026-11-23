//! Árvore de pastas preguiçosa e tabela de arquivos, por keyset (nunca OFFSET).

use rusqlite::{Connection, OptionalExtension, params};

use super::n;
use crate::Resultado;

/// Teto de linhas por página (a GUI nunca pede mais que isto).
pub const LIMITE_MAXIMO: usize = 500;

fn limite(l: usize) -> i64 {
    l.clamp(1, LIMITE_MAXIMO) as i64
}

/// Uma pasta da árvore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoPasta {
    pub id: i64,
    pub volume_id: i64,
    pub nome: String,
    pub caminho: String,
    /// Soma recursiva (0 enquanto sem agregado).
    pub bytes_logicos: u64,
    pub bytes_alocados: Option<u64>,
    pub arquivos: u64,
    pub pastas: u64,
    pub estado_agregado: Option<String>,
    pub tem_subpastas: bool,
    pub mtime_ns: Option<i64>,
}

/// Posição keyset da árvore: `(bytes, id)` da última pasta devolvida.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPasta {
    pub bytes: i64,
    pub id: i64,
}

/// Página de pastas; `proximo` é `Some` se há mais.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaginaPastas {
    pub itens: Vec<NoPasta>,
    pub proximo: Option<CursorPasta>,
}

const COLUNAS_PASTA: &str = "f.id, f.volume_id, f.name_display, f.path_display,
    COALESCE(s.logical_bytes, 0), s.allocated_entry_bytes, COALESCE(s.recursive_file_count, 0),
    COALESCE(s.recursive_folder_count, 0), s.aggregate_state,
    EXISTS(SELECT 1 FROM folders c WHERE c.parent_folder_id = f.id), f.mtime_ns";

fn ler_pasta(l: &rusqlite::Row<'_>) -> rusqlite::Result<NoPasta> {
    Ok(NoPasta {
        id: l.get(0)?,
        volume_id: l.get(1)?,
        nome: l.get(2)?,
        caminho: l.get(3)?,
        bytes_logicos: n(l.get(4)?),
        bytes_alocados: l.get::<_, Option<i64>>(5)?.map(n),
        arquivos: n(l.get(6)?),
        pastas: n(l.get(7)?),
        estado_agregado: l.get(8)?,
        tem_subpastas: l.get::<_, i64>(9)? != 0,
        mtime_ns: l.get(10)?,
    })
}

/// Pastas-raiz catalogadas (sem pai) de todos os Volumes, por tamanho; no máximo
/// [`LIMITE_MAXIMO`].
///
/// # Errors
/// Falha do SQLite.
pub fn raizes(conn: &Connection) -> Resultado<Vec<NoPasta>> {
    let sql = format!(
        "SELECT {COLUNAS_PASTA} FROM folders f LEFT JOIN folder_stats s ON s.folder_id = f.id
         WHERE f.parent_folder_id IS NULL
         ORDER BY COALESCE(s.logical_bytes, 0) DESC, f.id LIMIT ?1"
    );
    let mut st = conn.prepare_cached(&sql)?;
    Ok(st
        .query_map([LIMITE_MAXIMO as i64], ler_pasta)?
        .collect::<Result<_, _>>()?)
}

/// Subpastas diretas de `pai`, da maior para a menor, em páginas de até `lim` (≤ 500).
///
/// # Errors
/// Falha do SQLite.
pub fn subpastas(
    conn: &Connection,
    pai: i64,
    cursor: Option<CursorPasta>,
    lim: usize,
) -> Resultado<PaginaPastas> {
    let (cb, ci) = cursor.map_or((None, None), |c| (Some(c.bytes), Some(c.id)));
    let sql = format!(
        "SELECT {COLUNAS_PASTA} FROM folders f LEFT JOIN folder_stats s ON s.folder_id = f.id
         WHERE f.parent_folder_id = ?1
           AND (?2 IS NULL OR COALESCE(s.logical_bytes, 0) < ?2
                OR (COALESCE(s.logical_bytes, 0) = ?2 AND f.id > ?3))
         ORDER BY COALESCE(s.logical_bytes, 0) DESC, f.id LIMIT ?4"
    );
    let lim = limite(lim);
    let mut st = conn.prepare_cached(&sql)?;
    let mut itens: Vec<NoPasta> = st
        .query_map(params![pai, cb, ci, lim + 1], ler_pasta)?
        .collect::<Result<_, _>>()?;
    let mais = itens.len() as i64 > lim;
    itens.truncate(lim as usize);
    let proximo = if mais {
        itens.last().map(|p| CursorPasta {
            bytes: i64::try_from(p.bytes_logicos).unwrap_or(i64::MAX),
            id: p.id,
        })
    } else {
        None
    };
    Ok(PaginaPastas { itens, proximo })
}

/// Uma linha da tabela de arquivos.
#[derive(Debug, Clone, PartialEq)]
pub struct LinhaArquivo {
    pub id: i64,
    pub nome: String,
    /// `arquivo`, `symlink`, `fifo`...
    pub tipo: String,
    pub bytes_logicos: u64,
    pub bytes_alocados: Option<u64>,
    pub categoria: Option<String>,
    pub mime: Option<String>,
    pub finalidade: Option<String>,
    /// Confiança da classificação de finalidade (0..1); `None` = sem classificação.
    pub confianca: Option<f64>,
    pub mtime_ns: Option<i64>,
    pub birthtime_ns: Option<i64>,
    /// Já tem Hash Completo.
    pub tem_hash: bool,
    /// É cópia em algum Grupo de Duplicados.
    pub duplicado: bool,
    /// `backup` ou `delecao` se há Marcação manual neste arquivo.
    pub marcacao: Option<String>,
}

/// Posição keyset da tabela de arquivos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorArquivo {
    pub bytes: i64,
    pub id: i64,
}

/// Página de arquivos.
#[derive(Debug, Clone, PartialEq)]
pub struct PaginaArquivos {
    pub itens: Vec<LinhaArquivo>,
    pub proximo: Option<CursorArquivo>,
}

/// Arquivos diretos de `pasta`, do maior para o menor, em páginas de até `lim` (≤ 500).
///
/// # Errors
/// Falha do SQLite.
pub fn arquivos_da_pasta(
    conn: &Connection,
    pasta: i64,
    cursor: Option<CursorArquivo>,
    lim: usize,
) -> Resultado<PaginaArquivos> {
    let (cb, ci) = cursor.map_or((None, None), |c| (Some(c.bytes), Some(c.id)));
    let lim = limite(lim);
    let mut st = conn.prepare_cached(
        "SELECT a.id, a.name_display, a.kind, a.logical_bytes, a.allocated_bytes,
                t.broad_category, t.mime, c.category, c.confidence, a.mtime_ns, a.birthtime_ns,
                EXISTS(SELECT 1 FROM hashes h WHERE h.file_id = a.id),
                EXISTS(SELECT 1 FROM grupo_membros g WHERE g.file_id = a.id),
                (SELECT m.tipo FROM marcacoes m WHERE m.file_id = a.id LIMIT 1)
         FROM files a
         LEFT JOIN file_types t ON t.id = a.mime_id
         LEFT JOIN classifications c ON c.id = a.class_id
         WHERE a.folder_id = ?1
           AND (?2 IS NULL OR a.logical_bytes < ?2 OR (a.logical_bytes = ?2 AND a.id > ?3))
         ORDER BY a.logical_bytes DESC, a.id LIMIT ?4",
    )?;
    let mut itens: Vec<LinhaArquivo> = st
        .query_map(params![pasta, cb, ci, lim + 1], |l| {
            Ok(LinhaArquivo {
                id: l.get(0)?,
                nome: l.get(1)?,
                tipo: l.get(2)?,
                bytes_logicos: n(l.get(3)?),
                bytes_alocados: l.get::<_, Option<i64>>(4)?.map(n),
                categoria: l.get(5)?,
                mime: l.get(6)?,
                finalidade: l.get(7)?,
                confianca: l.get(8)?,
                mtime_ns: l.get(9)?,
                birthtime_ns: l.get(10)?,
                tem_hash: l.get::<_, i64>(11)? != 0,
                duplicado: l.get::<_, i64>(12)? != 0,
                marcacao: l.get(13)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mais = itens.len() as i64 > lim;
    itens.truncate(lim as usize);
    let proximo = if mais {
        itens.last().map(|a| CursorArquivo {
            bytes: i64::try_from(a.bytes_logicos).unwrap_or(i64::MAX),
            id: a.id,
        })
    } else {
        None
    };
    Ok(PaginaArquivos { itens, proximo })
}

/// Dados básicos de uma pasta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoPasta {
    pub id: i64,
    pub volume_id: i64,
    pub pai: Option<i64>,
    pub nome: String,
    pub caminho: String,
}

/// Lê uma pasta.
///
/// # Errors
/// Falha do SQLite.
pub fn info_da_pasta(conn: &Connection, id: i64) -> Resultado<Option<InfoPasta>> {
    Ok(conn
        .query_row(
            "SELECT id, volume_id, parent_folder_id, name_display, path_display
             FROM folders WHERE id = ?1",
            [id],
            |l| {
                Ok(InfoPasta {
                    id: l.get(0)?,
                    volume_id: l.get(1)?,
                    pai: l.get(2)?,
                    nome: l.get(3)?,
                    caminho: l.get(4)?,
                })
            },
        )
        .optional()?)
}

/// Ancestrais da pasta, da raiz até ela (inclusive), para breadcrumbs. Limitada a 256 níveis.
///
/// # Errors
/// Falha do SQLite.
pub fn trilha_da_pasta(conn: &Connection, id: i64) -> Resultado<Vec<InfoPasta>> {
    let mut st = conn.prepare_cached(
        "WITH RECURSIVE anc(id, volume_id, pai, nome, caminho, nivel) AS (
             SELECT id, volume_id, parent_folder_id, name_display, path_display, 0
             FROM folders WHERE id = ?1
             UNION ALL
             SELECT f.id, f.volume_id, f.parent_folder_id, f.name_display, f.path_display, a.nivel + 1
             FROM folders f JOIN anc a ON f.id = a.pai WHERE a.nivel < 256)
         SELECT id, volume_id, pai, nome, caminho FROM anc ORDER BY nivel DESC",
    )?;
    Ok(st
        .query_map([id], |l| {
            Ok(InfoPasta {
                id: l.get(0)?,
                volume_id: l.get(1)?,
                pai: l.get(2)?,
                nome: l.get(3)?,
                caminho: l.get(4)?,
            })
        })?
        .collect::<Result<_, _>>()?)
}
