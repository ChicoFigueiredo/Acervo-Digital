//! Impressão digital textual por item (B-14, migração 0013). Evidência de hipótese de mesma
//! Edição; nunca autoriza descarte.

use dag_biblio::impressao::Impressao;
use rusqlite::{Connection, OptionalExtension, params};

use crate::Resultado;

/// Grava (substitui) a impressão do item `(file_id, membro)` e seus índices LSH.
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_impressao(
    conn: &Connection,
    file_id: i64,
    membro: &str,
    imp: &Impressao,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "DELETE FROM impressoes_texto WHERE file_id = ?1 AND membro = ?2",
        params![file_id, membro],
    )?;
    conn.execute(
        "INSERT INTO impressoes_texto (file_id, membro, assinatura, shingles, criado_em)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![file_id, membro, imp.para_bytes(), imp.shingles, agora],
    )?;
    let mut st = conn.prepare_cached(
        "INSERT OR IGNORE INTO impressoes_bandas (chave, file_id, membro) VALUES (?1, ?2, ?3)",
    )?;
    for k in imp.chaves_lsh() {
        st.execute(params![k, file_id, membro])?;
    }
    Ok(())
}

/// Impressão gravada do item, se houver.
///
/// # Errors
/// Falha do SQLite.
pub fn impressao_do_item(
    conn: &Connection,
    file_id: i64,
    membro: &str,
) -> Resultado<Option<Impressao>> {
    let b: Option<Vec<u8>> = conn
        .query_row(
            "SELECT assinatura FROM impressoes_texto WHERE file_id = ?1 AND membro = ?2",
            params![file_id, membro],
            |l| l.get(0),
        )
        .optional()?;
    Ok(b.and_then(|b| Impressao::de_bytes(&b)))
}

/// Um item com impressão parecida (pré-filtro LSH).
#[derive(Debug, Clone, PartialEq)]
pub struct CandidatoImpressao {
    pub file_id: i64,
    pub membro: String,
    /// Em quantas bandas coincidiu.
    pub bandas_em_comum: u32,
    pub impressao: Impressao,
}

/// Itens de OUTROS arquivos que coincidem em alguma banda LSH com `imp`, os que mais coincidem
/// primeiro, no máximo `limite`. Usa o índice por chave: custo proporcional às colisões, não ao
/// número de livros.
///
/// # Errors
/// Falha do SQLite.
pub fn candidatos_por_impressao(
    conn: &Connection,
    file_id: i64,
    imp: &Impressao,
    minimo_bandas: u32,
    limite: usize,
) -> Resultado<Vec<CandidatoImpressao>> {
    let mut linhas: std::collections::HashMap<(i64, String), u32> =
        std::collections::HashMap::new();
    {
        let mut st = conn.prepare_cached(
            "SELECT file_id, membro FROM impressoes_bandas WHERE chave = ?1 AND file_id <> ?2",
        )?;
        for k in imp.chaves_lsh() {
            let it = st.query_map(params![k, file_id], |l| {
                Ok((l.get::<_, i64>(0)?, l.get::<_, String>(1)?))
            })?;
            for r in it {
                *linhas.entry(r?).or_insert(0) += 1;
            }
        }
    }
    let mut ordem: Vec<((i64, String), u32)> = linhas
        .into_iter()
        .filter(|(_, n)| *n >= minimo_bandas)
        .collect();
    ordem.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ordem.truncate(limite);
    let mut saida = Vec::with_capacity(ordem.len());
    for ((f, m), n) in ordem {
        if let Some(impressao) = impressao_do_item(conn, f, &m)? {
            saida.push(CandidatoImpressao {
                file_id: f,
                membro: m,
                bandas_em_comum: n,
                impressao,
            });
        }
    }
    Ok(saida)
}
