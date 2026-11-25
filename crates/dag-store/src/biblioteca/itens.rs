//! Itens de revisão por Membro de Contêiner. Um compactado com vários livros gera um item por
//! livro, ligado ao Contêiner (`file_id`) e ao caminho interno.
//!
//! O Membro é codificado no sufixo `|m:<caminho interno>` da `versao_regras` das extrações e
//! sugestões do item: as chaves únicas existentes (`(file_id, versao_regras)` e
//! `(file_id, campo, origem, modelo, versao_regras)`) passam a valer por Membro sem alterar o
//! esquema. Arquivos comuns usam a `versao_regras` pura (Membro vazio).

use rusqlite::{Connection, params};
use serde::Serialize;

use super::curadoria::{
    COLUNAS_DECISAO, COLUNAS_SUGESTAO, DecisaoHumana, Extracao, Sugestao, decisao_de_linha,
    extracao_de_linha, sugestao_de_linha,
};
use super::{Pagina, limite_de};
use crate::Resultado;

const SEP: &str = "|m:";

/// `versao_regras` do item: a pura para arquivo comum, com sufixo para Membro.
#[must_use]
pub fn versao_do_item(base: &str, membro: &str) -> String {
    if membro.is_empty() {
        base.to_owned()
    } else {
        format!("{base}{SEP}{membro}")
    }
}

/// Membro (caminho interno) de uma `versao_regras` de item; vazio para arquivo comum.
#[must_use]
pub fn membro_da_versao(versao: &str) -> &str {
    versao.split_once(SEP).map_or("", |(_, m)| m)
}

/// Um item de revisão: arquivo (ou Contêiner) e Membro, com as sugestões pendentes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemRevisaoMembro {
    pub file_id: i64,
    /// Vazio = o próprio arquivo.
    pub membro: String,
    pub pendentes: i64,
    pub maior_confianca: f64,
    pub menor_confianca: f64,
}

/// Fila de revisão por item `(arquivo, Membro)`, em keyset `(file_id, membro)`.
///
/// # Errors
/// Falha do SQLite.
pub fn listar_revisao_itens(
    conn: &Connection,
    apos: Option<(i64, String)>,
    limite: usize,
) -> Resultado<Pagina<ItemRevisaoMembro, (i64, String)>> {
    let limite = limite_de(limite);
    let (f, m) = apos.unwrap_or((0, String::new()));
    let mut st = conn.prepare(
        "SELECT file_id,
                CASE WHEN instr(versao_regras, '|m:') > 0
                     THEN substr(versao_regras, instr(versao_regras, '|m:') + 3) ELSE '' END AS membro,
                COUNT(*), MAX(confianca), MIN(confianca)
         FROM sugestoes WHERE estado = 'pendente' AND file_id >= ?1
         GROUP BY file_id, membro
         HAVING file_id > ?1 OR membro > ?2
         ORDER BY file_id, membro LIMIT ?3",
    )?;
    let linhas = st
        .query_map(params![f, m, (limite + 1) as i64], |l| {
            Ok(ItemRevisaoMembro {
                file_id: l.get(0)?,
                membro: l.get(1)?,
                pendentes: l.get(2)?,
                maior_confianca: l.get(3)?,
                menor_confianca: l.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Pagina::de_linhas(linhas, limite, |i| {
        (i.file_id, i.membro.clone())
    }))
}

/// Sugestões do item `(arquivo, Membro)`, por campo e confiança decrescente.
///
/// # Errors
/// Falha do SQLite.
pub fn sugestoes_do_item(
    conn: &Connection,
    file_id: i64,
    membro: &str,
) -> Resultado<Vec<Sugestao>> {
    Ok(super::sugestoes_do_arquivo(conn, file_id)?
        .into_iter()
        .filter(|s| membro_da_versao(&s.versao_regras) == membro)
        .collect())
}

/// Membros que têm sugestões (de qualquer estado) neste arquivo; `""` para o próprio arquivo.
///
/// # Errors
/// Falha do SQLite.
pub fn membros_com_sugestoes(conn: &Connection, file_id: i64) -> Resultado<Vec<String>> {
    let mut v: Vec<String> = super::sugestoes_do_arquivo(conn, file_id)?
        .iter()
        .map(|s| membro_da_versao(&s.versao_regras).to_owned())
        .collect();
    v.sort();
    v.dedup();
    Ok(v)
}

/// Extrações vigentes (mesmo hash) do arquivo para a versão base de regras: uma por Membro.
/// Vazio = precisa extrair.
///
/// # Errors
/// Falha do SQLite.
pub fn extracoes_vigentes_do_arquivo(
    conn: &Connection,
    file_id: i64,
    versao_base: &str,
    hash_atual: &[u8; 32],
) -> Resultado<Vec<(String, Extracao)>> {
    let mut st = conn.prepare(
        "SELECT id, job_id, file_id, formato, estado, resultado, compressao, paginas_total, paginas_ocr,
                tempo_ms, hash_arquivo, versao_regras, criado_em
         FROM extracoes
         WHERE file_id = ?1 AND hash_arquivo = ?3
           AND (versao_regras = ?2 OR substr(versao_regras, 1, length(?2) + 3) = ?2 || '|m:')
         ORDER BY versao_regras",
    )?;
    let v = st
        .query_map(
            params![file_id, versao_base, hash_atual.as_slice()],
            extracao_de_linha,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v.into_iter()
        .map(|e| (membro_da_versao(&e.versao_regras).to_owned(), e))
        .collect())
}

/// Decisões humanas válidas (não desfeitas) do item: as ligadas a sugestões dele e, para o
/// próprio arquivo, as avulsas (sem sugestão), da mais nova para a mais antiga.
///
/// # Errors
/// Falha do SQLite.
pub fn decisoes_do_item(
    conn: &Connection,
    file_id: i64,
    versao_item: &str,
) -> Resultado<Vec<DecisaoHumana>> {
    let avulsas = i64::from(membro_da_versao(versao_item).is_empty());
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS_DECISAO} FROM decisoes_humanas d
         WHERE d.desfeita_em IS NULL AND (
               d.sugestao_id IN (SELECT id FROM sugestoes WHERE file_id = ?1 AND versao_regras = ?2)
            OR (?3 = 1 AND d.sugestao_id IS NULL AND d.alvo_tipo = 'arquivo' AND d.alvo_id = ?1))
         ORDER BY d.id DESC"
    ))?;
    let v = st
        .query_map(params![file_id, versao_item, avulsas], decisao_de_linha)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Sugestões pendentes de uma Edição (para decidir um Membro por `AlvoRevisao::Edicao`).
///
/// # Errors
/// Falha do SQLite.
pub fn sugestoes_pendentes_da_edicao(
    conn: &Connection,
    edicao_id: i64,
) -> Resultado<Vec<Sugestao>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS_SUGESTAO} FROM sugestoes WHERE edicao_id = ?1 AND estado = 'pendente'
         ORDER BY campo, confianca DESC, id"
    ))?;
    let v = st
        .query_map([edicao_id], sugestao_de_linha)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Muda o estado de uma sugestão (usado para guardar como decidida a que veio de uma decisão
/// humana já registrada: reprocessar não pede para o humano decidir de novo).
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_estado_sugestao(
    conn: &Connection,
    sugestao_id: i64,
    estado: super::EstadoSugestao,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "UPDATE sugestoes SET estado = ?2, atualizado_em = ?3 WHERE id = ?1",
        params![sugestao_id, super::texto_enum(&estado), agora],
    )?;
    Ok(())
}

/// Um Contêiner lido (Fase A) que tem Membros com extensão de livro: só estes entram na fila.
///
/// # Errors
/// Falha do SQLite.
pub fn conteiner_tem_livros(conn: &Connection, file_id: i64) -> Resultado<bool> {
    Ok(conn
        .prepare_cached(
            "SELECT EXISTS (
               SELECT 1 FROM conteineres c JOIN membros m ON m.conteiner_file_id = c.file_id
               WHERE c.file_id = ?1 AND c.estado = 'lido' AND m.tipo = 'arquivo'
                 AND (lower(m.caminho_display) LIKE '%.epub' OR lower(m.caminho_display) LIKE '%.mobi'
                   OR lower(m.caminho_display) LIKE '%.azw3' OR lower(m.caminho_display) LIKE '%.azw'
                   OR lower(m.caminho_display) LIKE '%.pdf' OR lower(m.caminho_display) LIKE '%.djvu'
                   OR lower(m.caminho_display) LIKE '%.djv' OR lower(m.caminho_display) LIKE '%.fb2'
                   OR lower(m.caminho_display) LIKE '%.cbz' OR lower(m.caminho_display) LIKE '%.cbr'
                   OR lower(m.caminho_display) LIKE '%.cb7' OR lower(m.caminho_display) LIKE '%.kfx'))",
        )?
        .query_row([file_id], |l| l.get::<_, i64>(0))?
        != 0)
}

/// Uma linha da lista de cadastros: o livro (arquivo) com seu cadastro, se já houver.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LinhaCadastro {
    pub file_id: i64,
    pub cadastro_id: Option<i64>,
    pub estado: Option<super::EstadoCadastro>,
    pub calibre_id: Option<i64>,
    pub erro: Option<String>,
    /// Concluído na fila, sem revisão pendente e ainda não cadastrado.
    pub pronto: bool,
}

/// Livros com cadastro na A Classificar e os prontos para cadastrar, em keyset por `file_id`.
/// Com `estado`, só os cadastros nesse estado (`Pendente` inclui os prontos sem cadastro ainda).
///
/// # Errors
/// Falha do SQLite.
pub fn listar_cadastros_e_prontos(
    conn: &Connection,
    estado: Option<super::EstadoCadastro>,
    apos_file_id: Option<i64>,
    limite: usize,
) -> Resultado<Pagina<LinhaCadastro, i64>> {
    let limite = limite_de(limite);
    let filtro = estado.map(|e| super::texto_enum(&e));
    let mut st = conn.prepare(
        "WITH alvos(file_id) AS (
            SELECT file_id FROM cadastros_a_classificar
            UNION SELECT file_id FROM jobs_bibliograficos WHERE estado = 'concluido')
         SELECT a.file_id, c.id, c.estado, c.calibre_id, c.erro,
                ((c.id IS NULL OR c.estado IN ('pendente','falhou'))
                 AND EXISTS (SELECT 1 FROM jobs_bibliograficos j
                             WHERE j.file_id = a.file_id AND j.estado = 'concluido')
                 AND NOT EXISTS (SELECT 1 FROM sugestoes s
                                 WHERE s.file_id = a.file_id
                                   AND (s.estado = 'pendente' OR s.versao_regras LIKE '%|h'))) AS pronto
         FROM alvos a
         LEFT JOIN cadastros_a_classificar c ON c.id =
              (SELECT MAX(id) FROM cadastros_a_classificar WHERE file_id = a.file_id)
         WHERE a.file_id > ?1
           AND (?2 IS NULL OR c.estado = ?2 OR (?2 = 'pendente' AND c.id IS NULL))
         ORDER BY a.file_id LIMIT ?3",
    )?;
    let linhas = st
        .query_map(
            params![apos_file_id.unwrap_or(0), filtro, (limite + 1) as i64],
            |l| {
                let estado: Option<String> = l.get(2)?;
                Ok(LinhaCadastro {
                    file_id: l.get(0)?,
                    cadastro_id: l.get(1)?,
                    estado: estado.and_then(|e| super::enum_de_texto(&e).ok()),
                    calibre_id: l.get(3)?,
                    erro: l.get(4)?,
                    pronto: l.get::<_, i64>(5)? != 0,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Pagina::de_linhas(linhas, limite, |i| i.file_id))
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn versao_do_item_ida_e_volta() {
        assert_eq!(versao_do_item("b1", ""), "b1");
        let v = versao_do_item("b1", "a.zip!/x/livro.epub");
        assert_eq!(v, "b1|m:a.zip!/x/livro.epub");
        assert_eq!(membro_da_versao(&v), "a.zip!/x/livro.epub");
        assert_eq!(membro_da_versao("b1"), "");
    }
}
