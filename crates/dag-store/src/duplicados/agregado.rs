//! Agregados dos Grupos de Duplicados calculados **em SQL**, sem carregar grupos na memória.
//!
//! A listagem paginada, o resumo e o espaço recuperável por Volume/pasta/grupo usam estas
//! consultas: a memória fica limitada à página (ou ao número de Volumes/pastas do resultado) e
//! não cresce com o número de grupos. A semântica é a mesma de [`super::marca_efetiva`] e de
//! `dag_core::regras::espaco_recuperavel`; os testes comparam com as versões de referência
//! (`*_referencia`), que carregam cada grupo na memória.
//!
//! Os `CROSS JOIN` e o `INDEXED BY` fixam a ordem de junção: sem estatísticas (`ANALYZE`) o
//! planejador do SQLite escolhia varrer `files` inteiro por grupo (quadrático).
//!
//! Todo membro de `grupo_membros` nasce de um hash válido (`reconstruir_grupos`), então a
//! consulta não revisita `hashes` (custava ~35% do tempo a 1 milhão de entradas).
//!
//! Cada cópia recebe a Marcação que vale para ela (arquivo > regra da pasta mais específica >
//! regra do Volume). Por grupo: `n_del` = cópias com Deleção ativa; as demais sem marca, exceto a
//! Cópia Principal, são "estimadas"; Backup ativo e qualquer Marcação Pendente sobrevivem; se
//! nada sobrevive, poupa-se a estimada de menor id.

use dag_core::{ArquivoId, GrupoDuplicados, GrupoId, HashCompleto, ModoPrincipal, OrdemDuplicados};
use rusqlite::{Connection, named_params};

use super::{Resultado, hash_de, sob_sql, u};

/// Filtros de grupos (aplicados antes de agregar).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Filtro {
    pub volume: Option<i64>,
    pub tamanho_minimo: u64,
}

/// Condição sobre `grupos_duplicados g` (parâmetros nomeados `:vol` e `:min`).
const FILTRO_GRUPO: &str = "g.logical_bytes >= :min AND (:vol IS NULL OR EXISTS (
        SELECT 1 FROM grupo_membros x INDEXED BY idx_grupo_membros_grupo
        CROSS JOIN files fx ON fx.id = x.file_id
        WHERE x.grupo_id = g.id AND fx.volume_id = :vol))";

/// CTEs `m` (uma linha por cópia, com a Marcação efetiva), `a` (agregado por grupo) e `r`
/// (`a` com a contagem final de estimadas). `extra` restringe os grupos (ex.: `g.id IN (...)`).
fn cte(extra: &str) -> String {
    let sob = sob_sql("f.path_raw", "rp.path_raw");
    format!(
        "WITH rp AS MATERIALIZED (
             SELECT m.tipo || '/' || m.estado AS marca, pf.volume_id AS vid, pf.path_raw AS path_raw
             FROM marcacoes m JOIN folders pf ON pf.id = m.folder_id
             WHERE m.escopo = 'regra_pasta'),
         rv AS MATERIALIZED (
             SELECT m.tipo || '/' || m.estado AS marca, m.volume_id AS vid
             FROM marcacoes m WHERE m.escopo = 'regra_volume'),
         m AS MATERIALIZED (
             SELECT gm.grupo_id AS gid, gm.file_id AS fid, f.volume_id AS vid, f.folder_id AS pid,
                    g.copia_principal_file_id AS princ, g.logical_bytes AS tam, v.mounted AS conectado,
                    COALESCE(
                        mf.tipo || '/' || mf.estado,
                        (SELECT rp.marca FROM rp
                          WHERE rp.vid = f.volume_id AND {sob}
                          ORDER BY length(rp.path_raw) DESC LIMIT 1),
                        (SELECT rv.marca FROM rv WHERE rv.vid = f.volume_id)) AS marca
             FROM grupos_duplicados g
             CROSS JOIN grupo_membros gm INDEXED BY idx_grupo_membros_grupo ON gm.grupo_id = g.id
             CROSS JOIN files f ON f.id = gm.file_id
             CROSS JOIN volumes v ON v.id = f.volume_id
             LEFT JOIN marcacoes mf ON mf.file_id = gm.file_id
             WHERE {extra}),
         a AS (
             SELECT gid, COUNT(*) AS copias,
                    COALESCE(SUM(conectado = 0), 0) AS offline,
                    COALESCE(SUM(marca = 'delecao/ativa'), 0) AS n_del,
                    COALESCE(SUM(marca = 'backup/ativa' OR marca LIKE '%/pendente'), 0) AS n_surv,
                    COALESCE(SUM(marca IS NULL AND fid IS princ), 0) AS principal_sem,
                    COALESCE(SUM(marca IS NULL AND fid IS NOT princ), 0) AS n_est0,
                    MIN(CASE WHEN marca IS NULL AND fid IS NOT princ THEN fid END) AS min_est
             FROM m GROUP BY gid),
         r AS (
             SELECT a.*, a.n_est0 - (CASE WHEN a.n_surv + a.principal_sem = 0 AND a.n_est0 > 0
                                          THEN 1 ELSE 0 END) AS n_est
             FROM a)"
    )
}

/// Classe de espaço da cópia (`m`) dado o agregado do seu grupo (`r`): `'marcada'`, `'estimada'`
/// ou NULL.
const CLASSE: &str = "CASE
        WHEN m.marca = 'delecao/ativa' THEN 'marcada'
        WHEN m.marca IS NULL AND m.fid IS NOT m.princ
             AND NOT (r.n_surv + r.principal_sem = 0 AND m.fid = r.min_est) THEN 'estimada'
    END";

/// Preenche `copias_em_conteineres` dos grupos de uma página (uma consulta pelo índice do grupo).
pub(crate) fn preencher_membros(
    conn: &Connection,
    grupos: &mut [GrupoDuplicados],
) -> Resultado<()> {
    let mut st = conn.prepare_cached(
        "SELECT COUNT(*) FROM grupo_membros_conteiner gc JOIN membros m ON m.id = gc.membro_id
         JOIN files c ON c.id = m.conteiner_file_id AND c.na_lixeira = 0 WHERE gc.grupo_id = ?1",
    )?;
    for g in grupos {
        g.copias_em_conteineres =
            u32::try_from(st.query_row([g.id.0], |l| l.get::<_, i64>(0))?).unwrap_or(u32::MAX);
    }
    Ok(())
}

fn i(x: u64) -> i64 {
    i64::try_from(x).unwrap_or(i64::MAX)
}

/// Total de grupos que casam com o filtro.
pub(crate) fn total(conn: &Connection, f: &Filtro) -> Resultado<u64> {
    Ok(u(conn.query_row(
        &format!("SELECT COUNT(*) FROM grupos_duplicados g WHERE {FILTRO_GRUPO}"),
        named_params! { ":min": i(f.tamanho_minimo), ":vol": f.volume },
        |l| l.get(0),
    )?))
}

fn grupo_da_linha(l: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, GrupoDuplicados)> {
    let hash: Vec<u8> = l.get(2)?;
    let tam = u(l.get(1)?);
    let n_del = u(l.get(7)?);
    let n_est = u(l.get(8)?);
    let principal: Option<i64> = l.get(3)?;
    let marcados = n_del * tam;
    let estimados = n_est * tam;
    Ok((
        u(l.get(9)?),
        GrupoDuplicados {
            id: GrupoId(l.get(0)?),
            hash: hash_de(&hash).unwrap_or(HashCompleto([0; 32])),
            tamanho: tam,
            copias: u32::try_from(l.get::<_, i64>(5)?).unwrap_or(u32::MAX),
            copias_offline: u32::try_from(l.get::<_, i64>(6)?).unwrap_or(u32::MAX),
            copias_em_conteineres: 0,
            copia_principal: principal.map(ArquivoId),
            modo_principal: if l.get::<_, String>(4)? == "manual" {
                ModoPrincipal::Manual
            } else {
                ModoPrincipal::Auto
            },
            bytes_marcados_delecao: marcados,
            bytes_redundantes_estimados: estimados,
            espaco_recuperavel_bytes: marcados + estimados,
        },
    ))
}

const COLUNAS: &str = "g.id, g.logical_bytes, g.hash, g.copia_principal_file_id,
        g.copia_principal_modo, r.copias, r.offline, r.n_del, r.n_est,
        (r.n_del + r.n_est) * g.logical_bytes";

/// Até `limite` grupos depois do cursor `(chave, id)`, na ordem pedida, com a chave de ordenação
/// de cada um (espaço recuperável ou tamanho). Memória: só a página.
pub(crate) fn pagina(
    conn: &Connection,
    f: &Filtro,
    ordem: OrdemDuplicados,
    depois: Option<(u64, i64)>,
    limite: usize,
) -> Resultado<Vec<(u64, GrupoDuplicados)>> {
    let (ck, ci) = depois.map_or((None, None), |(k, id)| (Some(i(k)), Some(id)));
    let n = i64::try_from(limite).unwrap_or(i64::MAX);
    match ordem {
        OrdemDuplicados::Recuperavel => {
            // Precisa agregar todos os grupos para ordenar pela chave calculada (uma varredura
            // em SQL, sem materializar grupos no processo).
            let sql = format!(
                "{cte} SELECT {COLUNAS}, ((r.n_del + r.n_est) * g.logical_bytes) AS chave
                 FROM r JOIN grupos_duplicados g ON g.id = r.gid
                 WHERE :ck IS NULL OR chave < :ck OR (chave = :ck AND g.id > :ci)
                 ORDER BY chave DESC, g.id ASC LIMIT :n",
                cte = cte(FILTRO_GRUPO)
            );
            let mut st = conn.prepare(&sql)?;
            let linhas = st.query_map(
                named_params! {
                    ":min": i(f.tamanho_minimo), ":vol": f.volume,
                    ":ck": ck, ":ci": ci, ":n": n
                },
                grupo_da_linha,
            )?;
            Ok(linhas.collect::<Result<Vec<_>, _>>()?)
        }
        OrdemDuplicados::Tamanho => {
            // A chave já é uma coluna: escolhe a página pelo índice/ordenação simples e agrega só
            // esses grupos.
            let mut st = conn.prepare(&format!(
                "SELECT g.id FROM grupos_duplicados g
                 WHERE {FILTRO_GRUPO}
                   AND (:ck IS NULL OR g.logical_bytes < :ck OR (g.logical_bytes = :ck AND g.id > :ci))
                 ORDER BY g.logical_bytes DESC, g.id ASC LIMIT :n"
            ))?;
            let ids: Vec<i64> = st
                .query_map(
                    named_params! {
                        ":min": i(f.tamanho_minimo), ":vol": f.volume,
                        ":ck": ck, ":ci": ci, ":n": n
                    },
                    |l| l.get(0),
                )?
                .collect::<Result<_, _>>()?;
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let lista = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
            let sql = format!(
                "{cte} SELECT {COLUNAS}
                 FROM r JOIN grupos_duplicados g ON g.id = r.gid
                 ORDER BY g.logical_bytes DESC, g.id ASC",
                cte = cte(&format!("g.id IN ({lista})"))
            );
            let mut st = conn.prepare(&sql)?;
            let linhas = st.query_map([], grupo_da_linha)?;
            let mut v = Vec::new();
            for l in linhas {
                let (_, g) = l?;
                v.push((g.tamanho, g));
            }
            Ok(v)
        }
    }
}

/// Totais de todos os grupos: `(grupos, cópias, espaço recuperável em bytes)`.
pub(crate) fn totais(conn: &Connection) -> Resultado<(u64, u64, u64)> {
    let sql = format!(
        "{cte} SELECT COUNT(*), COALESCE(SUM(r.copias), 0),
                COALESCE(SUM((r.n_del + r.n_est) * g.logical_bytes), 0)
         FROM r JOIN grupos_duplicados g ON g.id = r.gid",
        cte = cte("1")
    );
    Ok(conn.query_row(&sql, [], |l| Ok((u(l.get(0)?), u(l.get(1)?), u(l.get(2)?))))?)
}

/// Bytes por (Volume ou pasta direta): `(id, bytes_marcados, bytes_estimados, copias)`.
pub(crate) fn por_destino(
    conn: &Connection,
    por_pasta: bool,
) -> Resultado<Vec<(i64, u64, u64, u64)>> {
    let col = if por_pasta { "m.pid" } else { "m.vid" };
    let sql = format!(
        "{cte} SELECT {col},
                COALESCE(SUM(CASE WHEN {CLASSE} = 'marcada' THEN m.tam END), 0),
                COALESCE(SUM(CASE WHEN {CLASSE} = 'estimada' THEN m.tam END), 0),
                COUNT({CLASSE})
         FROM m JOIN r ON r.gid = m.gid
         GROUP BY {col} HAVING COUNT({CLASSE}) > 0",
        cte = cte("1")
    );
    let mut st = conn.prepare(&sql)?;
    let linhas = st.query_map([], |l| {
        Ok((l.get(0)?, u(l.get(1)?), u(l.get(2)?), u(l.get(3)?)))
    })?;
    Ok(linhas.collect::<Result<Vec<_>, _>>()?)
}

/// Um grupo no relatório de espaço: `(grupo, nome, bytes_marcados, bytes_estimados, copias)`.
pub(crate) type LinhaEspacoGrupo = (i64, String, u64, u64, u64);

/// Grupos com espaço recuperável (> 0), do maior para o menor, depois do cursor `(chave, id)`.
pub(crate) fn grupos_com_espaco(
    conn: &Connection,
    depois: Option<(u64, i64)>,
    limite: usize,
) -> Resultado<Vec<(u64, LinhaEspacoGrupo)>> {
    let (ck, ci) = depois.map_or((None, None), |(k, id)| (Some(i(k)), Some(id)));
    let sql = format!(
        "{cte} SELECT g.id, ((r.n_del + r.n_est) * g.logical_bytes) AS chave,
                COALESCE((SELECT path_display FROM files WHERE id = g.copia_principal_file_id),
                         (SELECT f.path_display FROM grupo_membros gm JOIN files f ON f.id = gm.file_id
                           WHERE gm.grupo_id = g.id ORDER BY f.id LIMIT 1), ''),
                r.n_del * g.logical_bytes, r.n_est * g.logical_bytes, r.n_del + r.n_est
         FROM r JOIN grupos_duplicados g ON g.id = r.gid
         WHERE chave > 0 AND (:ck IS NULL OR chave < :ck OR (chave = :ck AND g.id > :ci))
         ORDER BY chave DESC, g.id ASC LIMIT :n",
        cte = cte("1")
    );
    let mut st = conn.prepare(&sql)?;
    let linhas = st.query_map(
        named_params! { ":ck": ck, ":ci": ci, ":n": i64::try_from(limite).unwrap_or(i64::MAX) },
        |l| {
            Ok((
                u(l.get(1)?),
                (
                    l.get(0)?,
                    l.get(2)?,
                    u(l.get(3)?),
                    u(l.get(4)?),
                    u(l.get(5)?),
                ),
            ))
        },
    )?;
    Ok(linhas.collect::<Result<Vec<_>, _>>()?)
}
