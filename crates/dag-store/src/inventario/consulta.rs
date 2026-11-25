//! Consultas de leitura paginadas sobre o inventário (A-10: carregamento incremental, nunca o
//! catálogo inteiro).

use rusqlite::{Connection, OptionalExtension, params};

use crate::Resultado;

/// Pasta ou arquivo, para ordenar juntos numa listagem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipoFilho {
    Pasta,
    Arquivo,
}

/// Filho direto de uma pasta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filho {
    pub tipo: TipoFilho,
    pub id: i64,
    pub nome_raw: Vec<u8>,
    pub nome_display: String,
    /// Pasta: soma recursiva (0 se ainda sem agregado); arquivo: tamanho lógico.
    pub bytes_logicos: i64,
    /// `None` = indisponível (ou pasta sem agregado).
    pub bytes_alocados: Option<i64>,
    pub mtime_ns: Option<i64>,
    /// Só para arquivos: `arquivo`, `symlink`, `fifo`...
    pub kind: Option<String>,
    /// Só para pastas: `pendente`/`parcial`/`completo`/`desatualizado`; `None` sem agregado.
    pub estado_agregado: Option<String>,
}

/// Cursor keyset: a posição do último item devolvido.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorFilhos {
    pub bytes: i64,
    /// 0 = pasta, 1 = arquivo (desempate estável).
    pub ordem: u8,
    pub id: i64,
}

/// Uma página de resultados; `proximo` é `Some` se há mais.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pagina<T> {
    pub itens: Vec<T>,
    pub proximo: Option<CursorFilhos>,
}

/// Filhos diretos de `pasta_id` do maior para o menor tamanho (pastas pelo total recursivo),
/// em páginas de `limite` por keyset `(bytes DESC, ordem, id)`. Custo por página independe de
/// quantas páginas já foram lidas.
///
/// # Errors
/// Falha do SQLite.
pub fn filhos_por_tamanho(
    conn: &Connection,
    pasta_id: i64,
    limite: usize,
    cursor: Option<CursorFilhos>,
) -> Resultado<Pagina<Filho>> {
    let (cb, co, ci) = cursor.map_or((None, None, None), |c| {
        (Some(c.bytes), Some(i64::from(c.ordem)), Some(c.id))
    });
    let mut st = conn.prepare_cached(
        "SELECT ord, id, name_raw, name_display, bytes, alloc, mtime_ns, kind, estado FROM (
             SELECT * FROM (
                 SELECT 0 AS ord, f.id AS id, f.name_raw AS name_raw, f.name_display AS name_display,
                        COALESCE(s.logical_bytes, 0) AS bytes, s.allocated_entry_bytes AS alloc,
                        f.mtime_ns AS mtime_ns, NULL AS kind, s.aggregate_state AS estado
                 FROM folders f LEFT JOIN folder_stats s ON s.folder_id = f.id
                 WHERE f.parent_folder_id = ?1
                   AND (?2 IS NULL OR COALESCE(s.logical_bytes, 0) < ?2
                        OR (COALESCE(s.logical_bytes, 0) = ?2 AND (0 > ?3 OR (0 = ?3 AND f.id > ?4))))
                 ORDER BY bytes DESC, f.id LIMIT ?5)
             UNION ALL
             SELECT * FROM (
                 SELECT 1 AS ord, a.id AS id, a.name_raw AS name_raw, a.name_display AS name_display,
                        a.logical_bytes AS bytes, a.allocated_bytes AS alloc, a.mtime_ns AS mtime_ns,
                        a.kind AS kind, NULL AS estado
                 FROM files a
                 WHERE a.folder_id = ?1
                   AND (?2 IS NULL OR a.logical_bytes < ?2
                        OR (a.logical_bytes = ?2 AND (1 > ?3 OR (1 = ?3 AND a.id > ?4))))
                 ORDER BY a.logical_bytes DESC, a.id LIMIT ?5)
         ) ORDER BY bytes DESC, ord, id LIMIT ?5",
    )?;
    let mut itens: Vec<(u8, Filho)> = st
        .query_map(params![pasta_id, cb, co, ci, limite as i64 + 1], |l| {
            let ord: i64 = l.get(0)?;
            Ok((
                ord as u8,
                Filho {
                    tipo: if ord == 0 {
                        TipoFilho::Pasta
                    } else {
                        TipoFilho::Arquivo
                    },
                    id: l.get(1)?,
                    nome_raw: l.get(2)?,
                    nome_display: l.get(3)?,
                    bytes_logicos: l.get(4)?,
                    bytes_alocados: l.get(5)?,
                    mtime_ns: l.get(6)?,
                    kind: l.get(7)?,
                    estado_agregado: l.get(8)?,
                },
            ))
        })?
        .collect::<Result<_, _>>()?;
    let mais = itens.len() > limite;
    itens.truncate(limite);
    let proximo = if mais {
        itens.last().map(|(o, f)| CursorFilhos {
            bytes: f.bytes_logicos,
            ordem: *o,
            id: f.id,
        })
    } else {
        None
    };
    Ok(Pagina {
        itens: itens.into_iter().map(|(_, f)| f).collect(),
        proximo,
    })
}

/// `folder_stats` de uma pasta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstatisticaPasta {
    pub pasta_id: i64,
    pub filhos_diretos: i64,
    pub arquivos: i64,
    pub pastas: i64,
    pub especiais: i64,
    pub bytes_logicos: i64,
    pub bytes_alocados_por_entrada: Option<i64>,
    pub bytes_fisicos_unicos: Option<i64>,
    pub estado: String,
}

/// Lê `folder_stats` de uma pasta.
///
/// # Errors
/// Falha do SQLite.
pub fn estatisticas_da_pasta(
    conn: &Connection,
    pasta_id: i64,
) -> Resultado<Option<EstatisticaPasta>> {
    Ok(conn
        .query_row(
            "SELECT folder_id, child_count, recursive_file_count, recursive_folder_count,
                    recursive_special_count, logical_bytes, allocated_entry_bytes,
                    physical_unique_bytes, aggregate_state
             FROM folder_stats WHERE folder_id = ?1",
            [pasta_id],
            |l| {
                Ok(EstatisticaPasta {
                    pasta_id: l.get(0)?,
                    filhos_diretos: l.get(1)?,
                    arquivos: l.get(2)?,
                    pastas: l.get(3)?,
                    especiais: l.get(4)?,
                    bytes_logicos: l.get(5)?,
                    bytes_alocados_por_entrada: l.get(6)?,
                    bytes_fisicos_unicos: l.get(7)?,
                    estado: l.get(8)?,
                })
            },
        )
        .optional()?)
}

/// `volume_stats` de um volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstatisticaVolume {
    pub volume_id: i64,
    pub execucao_id: Option<i64>,
    pub pastas: i64,
    pub arquivos: i64,
    pub especiais: i64,
    pub bytes_logicos: i64,
    pub bytes_alocados_por_entrada: Option<i64>,
    pub bytes_fisicos_unicos: Option<i64>,
    pub problemas: i64,
    pub estado: String,
}

/// Lê `volume_stats`.
///
/// # Errors
/// Falha do SQLite.
pub fn estatisticas_do_volume(
    conn: &Connection,
    volume_id: i64,
) -> Resultado<Option<EstatisticaVolume>> {
    Ok(conn
        .query_row(
            "SELECT volume_id, run_id, folder_count, file_count, special_count, logical_bytes,
                    allocated_entry_bytes, physical_unique_bytes, issue_count, aggregate_state
             FROM volume_stats WHERE volume_id = ?1",
            [volume_id],
            |l| {
                Ok(EstatisticaVolume {
                    volume_id: l.get(0)?,
                    execucao_id: l.get(1)?,
                    pastas: l.get(2)?,
                    arquivos: l.get(3)?,
                    especiais: l.get(4)?,
                    bytes_logicos: l.get(5)?,
                    bytes_alocados_por_entrada: l.get(6)?,
                    bytes_fisicos_unicos: l.get(7)?,
                    problemas: l.get(8)?,
                    estado: l.get(9)?,
                })
            },
        )
        .optional()?)
}

/// Id da pasta com esse caminho bruto no volume.
///
/// # Errors
/// Falha do SQLite.
pub fn buscar_pasta(conn: &Connection, volume_id: i64, path_raw: &[u8]) -> Resultado<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM folders WHERE volume_id = ?1 AND path_raw = ?2",
            params![volume_id, path_raw],
            |l| l.get(0),
        )
        .optional()?)
}

/// Pastas-raiz (sem pai) de um volume, por tamanho decrescente.
///
/// # Errors
/// Falha do SQLite.
pub fn pastas_raiz(conn: &Connection, volume_id: i64) -> Resultado<Vec<(i64, Vec<u8>)>> {
    let mut st = conn.prepare_cached(
        "SELECT f.id, f.path_raw FROM folders f LEFT JOIN folder_stats s ON s.folder_id = f.id
         WHERE f.volume_id = ?1 AND f.parent_folder_id IS NULL
         ORDER BY COALESCE(s.logical_bytes, 0) DESC, f.id",
    )?;
    Ok(st
        .query_map([volume_id], |l| Ok((l.get(0)?, l.get(1)?)))?
        .collect::<Result<_, _>>()?)
}

/// Linha de `scan_errors`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problema {
    pub id: i64,
    pub pasta_id: Option<i64>,
    pub path_raw: Option<Vec<u8>>,
    pub path_display: Option<String>,
    pub fase: String,
    pub codigo: String,
    pub motivo: Option<String>,
    pub severidade: String,
}

/// Problemas e itens ignorados de uma execução, paginados por `id` (keyset: passe o último `id`
/// visto em `apos_id`, 0 para começar). `somente_ignorados`: só severidade `info`.
///
/// # Errors
/// Falha do SQLite.
pub fn listar_problemas(
    conn: &Connection,
    run_id: i64,
    apos_id: i64,
    limite: usize,
    somente_ignorados: Option<bool>,
) -> Resultado<Vec<Problema>> {
    let filtro = match somente_ignorados {
        None => "",
        Some(true) => "AND severity = 'info'",
        Some(false) => "AND severity <> 'info'",
    };
    let mut st = conn.prepare(&format!(
        "SELECT id, folder_id, path_raw, path_display, phase, error_code, reason, severity
         FROM scan_errors WHERE run_id = ?1 AND id > ?2 {filtro} ORDER BY id LIMIT ?3"
    ))?;
    Ok(st
        .query_map(params![run_id, apos_id, limite as i64], |l| {
            Ok(Problema {
                id: l.get(0)?,
                pasta_id: l.get(1)?,
                path_raw: l.get(2)?,
                path_display: l.get(3)?,
                fase: l.get(4)?,
                codigo: l.get(5)?,
                motivo: l.get(6)?,
                severidade: l.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?)
}
