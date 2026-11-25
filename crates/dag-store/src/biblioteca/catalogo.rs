//! Ligação do espelho homologado com o Catálogo da Fase A (escala: dezenas de milhões de
//! `files`). Tudo aqui entra pelos índices existentes: nenhuma consulta varre `files` ou `hashes`.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::limite_de;
use crate::Resultado;
use crate::duplicados::HASH_VALIDO;

/// Liga formatos ainda sem `file_id` ao arquivo do Catálogo pelo caminho, uma fatia por vez
/// (cursor `apos_id`). O caminho do Calibre perde `prefixo_a_remover` (p. ex. a montagem do
/// Volume) e vira `files.path_raw` no `volume_id` dado, pelo índice único `(volume, path_raw)`:
/// uma busca em índice por formato, nunca varredura de `files`. Devolve `(ligados, ultimo_id)`;
/// `ultimo_id` é `None` quando a fatia foi a última.
///
/// # Errors
/// Falha do SQLite.
pub fn ligar_formatos_por_caminho(
    conn: &Connection,
    biblioteca_id: i64,
    volume_id: i64,
    prefixo_a_remover: &str,
    apos_id: i64,
    limite: usize,
) -> Resultado<(u64, Option<i64>)> {
    let limite = limite_de(limite);
    let mut st = conn.prepare(
        "SELECT id, caminho FROM formatos_homologados
         WHERE biblioteca_id = ?1 AND file_id IS NULL AND id > ?2 ORDER BY id LIMIT ?3",
    )?;
    let linhas = st
        .query_map(params![biblioteca_id, apos_id, limite as i64], |l| {
            Ok((l.get::<_, i64>(0)?, l.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut busca =
        conn.prepare_cached("SELECT id FROM files WHERE volume_id = ?1 AND path_raw = ?2")?;
    let mut ligar =
        conn.prepare_cached("UPDATE formatos_homologados SET file_id = ?2 WHERE id = ?1")?;
    let mut ligados = 0;
    for (id, caminho) in &linhas {
        let rel = caminho.strip_prefix(prefixo_a_remover).unwrap_or(caminho);
        let achado: Option<i64> = busca
            .query_row(params![volume_id, rel.as_bytes()], |l| l.get(0))
            .optional()?;
        if let Some(f) = achado {
            ligar.execute(params![id, f])?;
            ligados += 1;
        }
    }
    let ultimo = if linhas.len() == limite {
        linhas.last().map(|(i, _)| *i)
    } else {
        None
    };
    Ok((ligados, ultimo))
}

/// Reaproveita o Hash Completo que o Catálogo já tem (`hashes`) para os formatos ligados a
/// `files` e ainda sem hash, desde que o tamanho do hash no Catálogo seja igual ao do espelho
/// (nunca recalcula à toa). Uma fatia por vez; devolve `(com_hash, ultimo_id)`.
///
/// # Errors
/// Falha do SQLite.
pub fn importar_hashes_do_catalogo(
    conn: &Connection,
    biblioteca_id: i64,
    apos_id: i64,
    limite: usize,
    agora: i64,
) -> Resultado<(u64, Option<i64>)> {
    let ultimo: Option<i64> = conn.query_row(
        "SELECT max(id) FROM (SELECT id FROM formatos_homologados
         WHERE biblioteca_id = ?1 AND hash IS NULL AND file_id IS NOT NULL AND id > ?2
         ORDER BY id LIMIT ?3)",
        params![biblioteca_id, apos_id, limite_de(limite) as i64],
        |l| l.get(0),
    )?;
    let Some(ultimo) = ultimo else {
        return Ok((0, None));
    };
    // Só vale o hash do Catálogo cuja evidência (tamanho, mtime, ctime, inode) ainda confere com
    // a linha de `files`: um hash velho de arquivo que mudou faria uma "duplicata exata" falsa.
    let n = conn.execute(
        &"UPDATE formatos_homologados SET
            hash = (SELECT h.hash FROM hashes h JOIN files f ON f.id = h.file_id
                    WHERE h.file_id = formatos_homologados.file_id
                    AND h.size_bytes = formatos_homologados.tamanho AND HASH_VALIDO),
            hash_em = ?4
         WHERE biblioteca_id = ?1 AND hash IS NULL AND file_id IS NOT NULL AND id > ?2 AND id <= ?3
           AND EXISTS (SELECT 1 FROM hashes h JOIN files f ON f.id = h.file_id
                       WHERE h.file_id = formatos_homologados.file_id
                       AND h.size_bytes = formatos_homologados.tamanho AND HASH_VALIDO)"
            .replace("HASH_VALIDO", HASH_VALIDO),
        params![biblioteca_id, apos_id, ultimo, agora],
    )?;
    Ok((n as u64, Some(ultimo)))
}

/// Arquivo do Catálogo com Hash Completo e tamanho iguais aos de um formato homologado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArquivoIgualAHomologado {
    pub formato_id: i64,
    pub livro_id: i64,
    pub file_id: i64,
    pub tamanho: u64,
}

/// Para uma fatia de formatos homologados com hash, do maior para o menor (cursor `apos` =
/// tamanho e id do último formato da página anterior; `None` começa do maior), acha os arquivos
/// do Catálogo com o mesmo hash e tamanho (exceto o próprio arquivo ligado), pelo índice
/// `hashes(size_bytes, hash)`: ~3 mil buscas em índice para uma biblioteca de 3 mil livros, mesmo
/// com 20 milhões de arquivos. É a fonte dos candidatos `exata`.
///
/// # Errors
/// Falha do SQLite.
pub fn arquivos_iguais_aos_homologados(
    conn: &Connection,
    biblioteca_id: i64,
    apos: Option<(u64, i64)>,
    limite: usize,
) -> Resultado<Vec<ArquivoIgualAHomologado>> {
    let (apos_tam, apos_id) = apos.map_or((i64::MAX, i64::MIN), |(t, i)| {
        (i64::try_from(t).unwrap_or(i64::MAX), i)
    });
    let mut st = conn.prepare(
        "WITH fatia AS (
            SELECT id, livro_id, hash, tamanho, file_id FROM formatos_homologados
            WHERE biblioteca_id = ?1 AND hash IS NOT NULL
              AND (tamanho < ?2 OR (tamanho = ?2 AND id > ?3))
            ORDER BY tamanho DESC, id LIMIT ?4)
         SELECT fatia.id, fatia.livro_id, h.file_id, fatia.tamanho FROM fatia
         CROSS JOIN hashes h INDEXED BY idx_hashes_dup
         WHERE h.size_bytes = fatia.tamanho AND h.hash = fatia.hash
           AND (fatia.file_id IS NULL OR h.file_id <> fatia.file_id)
         ORDER BY fatia.tamanho DESC, fatia.id, h.file_id",
    )?;
    let v = st
        .query_map(
            params![biblioteca_id, apos_tam, apos_id, limite_de(limite) as i64],
            |l| {
                Ok(ArquivoIgualAHomologado {
                    formato_id: l.get(0)?,
                    livro_id: l.get(1)?,
                    file_id: l.get(2)?,
                    tamanho: l.get::<_, i64>(3)? as u64,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}
