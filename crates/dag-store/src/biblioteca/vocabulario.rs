//! Consultas do aprendizado de vocabulário (tags e estantes) e dos itens da Homologada: decisões
//! humanas que treinam o classificador, assinaturas para invalidar o cache, livros da Homologada
//! sem tag ou com `!Catalogar`, pré-filtro de identidade bibliográfica e contagens do resumo.
//! Só leitura do Catálogo.

use dag_biblio::Campo;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::{
    AcaoDecisao, LivroHomologado, enum_de_texto, limite_de, obter_livro, sugestoes_do_arquivo,
    texto_enum,
};
use crate::Resultado;

/// Uma decisão humana sobre `tags`, `estante` ou `autores`, com o que foi sugerido, pronta para treinar.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DecisaoVocabulario {
    pub id: i64,
    pub file_id: i64,
    pub campo: Campo,
    pub acao: AcaoDecisao,
    /// Valor corrigido (listas separadas por `|`).
    pub valor_novo: Option<String>,
    /// Valor que a sugestão tinha.
    pub sugerido_valor: Option<String>,
    pub sugerido_lista: Vec<String>,
}

/// Decisões válidas (não desfeitas) sobre tags, estante e autores, da mais antiga para a mais nova.
///
/// # Errors
/// Falha do SQLite.
pub fn decisoes_de_vocabulario(conn: &Connection) -> Resultado<Vec<DecisaoVocabulario>> {
    let mut st = conn.prepare(
        "SELECT d.id, COALESCE(s.file_id, CASE WHEN d.alvo_tipo = 'arquivo' THEN d.alvo_id END),
                d.campo, d.acao, d.valor_novo, s.valor, s.lista_json
         FROM decisoes_humanas d LEFT JOIN sugestoes s ON s.id = d.sugestao_id
         WHERE d.desfeita_em IS NULL AND d.campo IN ('tags', 'estante', 'autores') ORDER BY d.id",
    )?;
    let linhas = st
        .query_map([], |l| {
            Ok((
                l.get::<_, i64>(0)?,
                l.get::<_, Option<i64>>(1)?,
                l.get::<_, String>(2)?,
                l.get::<_, String>(3)?,
                l.get::<_, Option<String>>(4)?,
                l.get::<_, Option<String>>(5)?,
                l.get::<_, Option<String>>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut v = Vec::with_capacity(linhas.len());
    for (id, file_id, campo, acao, valor_novo, sv, sl) in linhas {
        let Some(file_id) = file_id else { continue };
        v.push(DecisaoVocabulario {
            id,
            file_id,
            campo: enum_de_texto(&campo)?,
            acao: enum_de_texto(&acao)?,
            valor_novo,
            sugerido_valor: sv,
            sugerido_lista: sl
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default(),
        });
    }
    Ok(v)
}

/// Assinatura das decisões humanas: muda quando uma decisão entra ou é desfeita. Barata
/// (índices), serve para invalidar o cache do classificador.
///
/// # Errors
/// Falha do SQLite.
pub fn assinatura_decisoes(conn: &Connection) -> Resultado<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(id), 0), COALESCE(MAX(desfeita_em), 0) FROM decisoes_humanas",
        [],
        |l| Ok((l.get(0)?, l.get(1)?)),
    )?)
}

/// Assinatura do espelho de uma biblioteca: muda a cada sincronização que alterou algo.
///
/// # Errors
/// Falha do SQLite.
pub fn assinatura_espelho(conn: &Connection, biblioteca_id: i64) -> Resultado<(i64, i64, i64)> {
    Ok(conn.query_row(
        "SELECT geracao_espelho,
                (SELECT COALESCE(MAX(id), 0) FROM instantaneos_biblioteca WHERE biblioteca_id = ?1),
                (SELECT COALESCE(MAX(atualizado_em), 0) FROM livros_homologados WHERE biblioteca_id = ?1)
         FROM bibliotecas_calibre WHERE id = ?1",
        [biblioteca_id],
        |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)),
    )?)
}

/// Último valor sugerido (não rejeitado) de um campo do arquivo, qualquer Membro, o mais
/// confiável primeiro: `(valor, lista)`.
///
/// # Errors
/// Falha do SQLite.
pub fn valor_do_arquivo(
    conn: &Connection,
    file_id: i64,
    campo: Campo,
) -> Resultado<Option<(Option<String>, Vec<String>)>> {
    Ok(sugestoes_do_arquivo(conn, file_id)?
        .into_iter()
        .filter(|s| s.campo == campo && s.estado != super::EstadoSugestao::Rejeitada)
        .max_by(|a, b| a.confianca.total_cmp(&b.confianca))
        .map(|s| (s.valor, s.lista)))
}

/// Livros da Homologada que pedem proposta: com a tag `!Catalogar` (sem diferenciar caixa) ou
/// sem nenhuma tag. Keyset por `id`.
///
/// # Errors
/// Falha do SQLite.
pub fn livros_a_catalogar(
    conn: &Connection,
    biblioteca_id: i64,
    apos_id: i64,
    limite: usize,
) -> Resultado<Vec<LivroHomologado>> {
    let limite = limite_de(limite);
    let mut st = conn.prepare(
        "SELECT id FROM livros_homologados
         WHERE biblioteca_id = ?1 AND id > ?2
           AND (tags_json = '[]' OR EXISTS (SELECT 1 FROM json_each(tags_json)
                                            WHERE lower(value) = '!catalogar'))
         ORDER BY id LIMIT ?3",
    )?;
    let ids = st
        .query_map(params![biblioteca_id, apos_id, limite as i64], |l| {
            l.get::<_, i64>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut v = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(l) = obter_livro(conn, id)? {
            v.push(l);
        }
    }
    Ok(v)
}

/// Arquivo do Catálogo ligado a algum formato do livro homologado (o primeiro, por id).
///
/// # Errors
/// Falha do SQLite.
pub fn arquivo_do_livro(conn: &Connection, livro_id: i64) -> Resultado<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT file_id FROM formatos_homologados WHERE livro_id = ?1 AND file_id IS NOT NULL
             ORDER BY id LIMIT 1",
            [livro_id],
            |l| l.get(0),
        )
        .optional()?)
}

/// Livro homologado a que um arquivo do Catálogo pertence (por formato ligado).
///
/// # Errors
/// Falha do SQLite.
pub fn livro_do_arquivo(conn: &Connection, file_id: i64) -> Resultado<Option<LivroHomologado>> {
    let id: Option<i64> = conn
        .query_row(
            "SELECT livro_id FROM formatos_homologados WHERE file_id = ?1 ORDER BY id LIMIT 1",
            [file_id],
            |l| l.get(0),
        )
        .optional()?;
    match id {
        Some(i) => obter_livro(conn, i),
        None => Ok(None),
    }
}

/// Há sugestão (de qualquer estado) do arquivo para esta `versao_regras`?
///
/// # Errors
/// Falha do SQLite.
pub fn tem_sugestao_da_versao(
    conn: &Connection,
    file_id: i64,
    versao_regras: &str,
) -> Resultado<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sugestoes WHERE file_id = ?1 AND versao_regras = ?2)",
        params![file_id, versao_regras],
        |l| l.get::<_, i64>(0),
    )? != 0)
}

/// Livros homologados pelo título principal normalizado (pré-filtro por índice).
///
/// # Errors
/// Falha do SQLite.
pub fn livros_por_titulo_principal(
    conn: &Connection,
    principal: &str,
    limite: usize,
) -> Resultado<Vec<LivroHomologado>> {
    if principal.is_empty() {
        return Ok(Vec::new());
    }
    let mut st = conn.prepare(
        "SELECT id FROM livros_homologados WHERE titulo_principal = ?1 ORDER BY id LIMIT ?2",
    )?;
    let ids = st
        .query_map(params![principal, limite as i64], |l| l.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut v = Vec::new();
    for id in ids {
        if let Some(l) = obter_livro(conn, id)? {
            v.push(l);
        }
    }
    Ok(v)
}

// ---------------------------------------------------------------------------------------------
// Resumo do período (WOW-10)
// ---------------------------------------------------------------------------------------------

/// Jobs concluídos no período `[de, ate]`.
///
/// # Errors
/// Falha do SQLite.
pub fn contar_jobs_concluidos_periodo(conn: &Connection, de: i64, ate: i64) -> Resultado<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM jobs_bibliograficos WHERE estado = 'concluido' AND concluido_em BETWEEN ?1 AND ?2",
        params![de, ate],
        |l| l.get(0),
    )?)
}

/// Arquivos distintos que ganharam sugestões no período.
///
/// # Errors
/// Falha do SQLite.
pub fn contar_classificados_periodo(conn: &Connection, de: i64, ate: i64) -> Resultado<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(DISTINCT file_id) FROM sugestoes WHERE criado_em BETWEEN ?1 AND ?2",
        params![de, ate],
        |l| l.get(0),
    )?)
}

/// Decisões humanas (não desfeitas) no período.
///
/// # Errors
/// Falha do SQLite.
pub fn contar_decisoes_periodo(conn: &Connection, de: i64, ate: i64) -> Resultado<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM decisoes_humanas WHERE desfeita_em IS NULL AND quando BETWEEN ?1 AND ?2",
        params![de, ate],
        |l| l.get(0),
    )?)
}

/// Candidatos de duplicidade surgidos no período; `so_exatas` limita às exatas.
///
/// # Errors
/// Falha do SQLite.
pub fn contar_candidatos_periodo(
    conn: &Connection,
    de: i64,
    ate: i64,
    so_exatas: bool,
) -> Resultado<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM candidatos_duplicidade
         WHERE criado_em BETWEEN ?1 AND ?2 AND (?3 = 0 OR tipo = ?4)",
        params![
            de,
            ate,
            i64::from(so_exatas),
            texto_enum(&dag_biblio::TipoDuplicidade::Exata)
        ],
        |l| l.get(0),
    )?)
}

/// Suspeita de duplicidade do arquivo, para a prioridade da fila: 1,0 se o hash é o de um
/// Formato homologado (economia quase certa), 0,5 se outra cópia no Catálogo tem o mesmo hash,
/// 0 sem hash ou sem repetição. Só índices.
///
/// # Errors
/// Falha do SQLite.
pub fn suspeita_de_duplicidade(conn: &Connection, file_id: i64) -> Resultado<f64> {
    let nivel: i64 = conn.query_row(
        "SELECT CASE
            WHEN EXISTS (SELECT 1 FROM hashes h JOIN formatos_homologados fh
                         ON fh.hash = h.hash AND fh.tamanho = h.size_bytes WHERE h.file_id = ?1) THEN 2
            WHEN EXISTS (SELECT 1 FROM hashes h JOIN hashes o
                         ON o.size_bytes = h.size_bytes AND o.hash = h.hash AND o.file_id <> h.file_id
                         WHERE h.file_id = ?1) THEN 1
            ELSE 0 END",
        [file_id],
        |l| l.get(0),
    )?;
    Ok(match nivel {
        2 => 1.0,
        1 => 0.5,
        _ => 0.0,
    })
}
