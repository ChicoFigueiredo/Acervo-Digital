//! Espelho de leitura da Biblioteca Homologada: livros, formatos, hashes calculados pelo DAG e
//! busca por hash (alerta de duplicata exata, CA-B04).

use dag_biblio::{FormatoCalibre, LivroCalibre, isbn};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use super::{Pagina, hash_de_blob, limite_de, normalizar_texto};
use crate::Resultado;

/// Arquivo de formato de um livro a espelhar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatoEspelho {
    /// Nome do formato como o Calibre o grava (`EPUB`, `PDF`...).
    pub formato: String,
    pub caminho: String,
    pub tamanho: u64,
    pub mtime_ns: Option<i64>,
}

/// Livro a espelhar (uma linha de `books` mais o que pende dela).
#[derive(Debug, Clone, PartialEq)]
pub struct LivroEspelho {
    pub calibre_id: i64,
    pub uuid: Option<String>,
    pub titulo: String,
    pub autores: Vec<String>,
    pub isbn13: Option<String>,
    pub idioma: Option<String>,
    pub serie: Option<String>,
    pub indice: Option<f64>,
    pub editora: Option<String>,
    pub tags: Vec<String>,
    pub estante: Option<String>,
    pub caminho: String,
    pub formatos: Vec<FormatoEspelho>,
}

impl LivroEspelho {
    /// Converte o livro lido do Calibre (`dag_biblio`) para o espelho. O ISBN-13 vem do
    /// identificador `isbn` (ISBN-10 é convertido); a estante é informada por quem a conhece.
    #[must_use]
    pub fn de_calibre(l: &LivroCalibre, estante: Option<String>) -> Self {
        let isbn13 = l
            .identificadores
            .iter()
            .filter(|(t, _)| t.eq_ignore_ascii_case("isbn"))
            .find_map(|(_, v)| isbn::normalizar(v));
        Self {
            calibre_id: l.id,
            uuid: l.uuid.clone(),
            titulo: l.titulo.clone(),
            autores: l.autores.clone(),
            isbn13,
            idioma: l.idiomas.first().cloned(),
            serie: l.serie.clone(),
            indice: l.indice_serie,
            editora: l.editora.clone(),
            tags: l.tags.clone(),
            estante,
            caminho: l.caminho.clone(),
            formatos: l.formatos.iter().map(FormatoEspelho::from).collect(),
        }
    }
}

impl From<&FormatoCalibre> for FormatoEspelho {
    fn from(f: &FormatoCalibre) -> Self {
        Self {
            formato: f.formato.to_ascii_uppercase(),
            caminho: f.caminho_absoluto.clone(),
            tamanho: f.tamanho,
            mtime_ns: None,
        }
    }
}

/// Livro lido do espelho.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LivroHomologado {
    pub id: i64,
    pub biblioteca_id: i64,
    pub calibre_id: i64,
    pub uuid: Option<String>,
    pub titulo: String,
    pub autores: Vec<String>,
    pub isbn13: Option<String>,
    pub idioma: Option<String>,
    pub serie: Option<String>,
    pub indice: Option<f64>,
    pub editora: Option<String>,
    pub tags: Vec<String>,
    pub estante: Option<String>,
    pub caminho: String,
}

const COLUNAS_LIVRO: &str = "id, biblioteca_id, calibre_id, uuid, titulo, autores_json, isbn13, idioma, serie, indice, editora, tags_json, estante, caminho";

fn livro_de_linha(l: &Row<'_>) -> rusqlite::Result<LivroHomologado> {
    let json = |i: usize| -> rusqlite::Result<Vec<String>> {
        Ok(serde_json::from_str(&l.get::<_, String>(i)?).unwrap_or_default())
    };
    Ok(LivroHomologado {
        id: l.get(0)?,
        biblioteca_id: l.get(1)?,
        calibre_id: l.get(2)?,
        uuid: l.get(3)?,
        titulo: l.get(4)?,
        autores: json(5)?,
        isbn13: l.get(6)?,
        idioma: l.get(7)?,
        serie: l.get(8)?,
        indice: l.get(9)?,
        editora: l.get(10)?,
        tags: json(11)?,
        estante: l.get(12)?,
        caminho: l.get(13)?,
    })
}

/// Abre uma nova geração de sincronização do espelho e devolve o número dela.
///
/// # Errors
/// Falha do SQLite.
pub fn iniciar_sincronizacao(conn: &Connection, biblioteca_id: i64) -> Resultado<i64> {
    Ok(conn.query_row(
        "UPDATE bibliotecas_calibre SET geracao_espelho = geracao_espelho + 1 WHERE id = ?1
         RETURNING geracao_espelho",
        [biblioteca_id],
        |l| l.get(0),
    )?)
}

/// Grava um lote de livros na `geracao` (upsert idempotente por `(biblioteca, calibre_id)`).
/// Hash já calculado de um formato **só** é mantido se tamanho e mtime não mudaram. Formatos que
/// o livro perdeu são removidos. Devolve o `id` de cada livro, na ordem do lote.
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_lote(
    conn: &Connection,
    biblioteca_id: i64,
    geracao: i64,
    livros: &[LivroEspelho],
    agora: i64,
) -> Resultado<Vec<i64>> {
    let mut up_livro = conn.prepare_cached(
        "INSERT INTO livros_homologados (biblioteca_id, calibre_id, uuid, titulo, titulo_norm, autores_json,
                isbn13, idioma, serie, indice, editora, tags_json, estante, caminho, geracao, atualizado_em,
                titulo_principal)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
         ON CONFLICT(biblioteca_id, calibre_id) DO UPDATE SET
            uuid = excluded.uuid, titulo = excluded.titulo, titulo_norm = excluded.titulo_norm,
            autores_json = excluded.autores_json, isbn13 = excluded.isbn13, idioma = excluded.idioma,
            serie = excluded.serie, indice = excluded.indice, editora = excluded.editora,
            tags_json = excluded.tags_json, estante = excluded.estante, caminho = excluded.caminho,
            geracao = excluded.geracao, titulo_principal = excluded.titulo_principal,
            atualizado_em = CASE WHEN
                livros_homologados.uuid IS excluded.uuid AND livros_homologados.titulo = excluded.titulo
                AND livros_homologados.autores_json = excluded.autores_json
                AND livros_homologados.isbn13 IS excluded.isbn13 AND livros_homologados.idioma IS excluded.idioma
                AND livros_homologados.serie IS excluded.serie AND livros_homologados.indice IS excluded.indice
                AND livros_homologados.editora IS excluded.editora AND livros_homologados.tags_json = excluded.tags_json
                AND livros_homologados.estante IS excluded.estante AND livros_homologados.caminho = excluded.caminho
                THEN livros_homologados.atualizado_em ELSE excluded.atualizado_em END
         RETURNING id",
    )?;
    let mut up_formato = conn.prepare_cached(
        "INSERT INTO formatos_homologados (livro_id, biblioteca_id, formato, caminho, tamanho, mtime_ns)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(livro_id, formato) DO UPDATE SET
            caminho = excluded.caminho,
            hash = CASE WHEN formatos_homologados.tamanho = excluded.tamanho
                         AND formatos_homologados.mtime_ns IS excluded.mtime_ns
                        THEN formatos_homologados.hash ELSE NULL END,
            hash_em = CASE WHEN formatos_homologados.tamanho = excluded.tamanho
                         AND formatos_homologados.mtime_ns IS excluded.mtime_ns
                        THEN formatos_homologados.hash_em ELSE NULL END,
            tamanho = excluded.tamanho, mtime_ns = excluded.mtime_ns",
    )?;
    let mut ids = Vec::with_capacity(livros.len());
    for l in livros {
        let id: i64 = up_livro.query_row(
            params![
                biblioteca_id,
                l.calibre_id,
                l.uuid,
                l.titulo,
                normalizar_texto(&l.titulo),
                serde_json::to_string(&l.autores)?,
                l.isbn13,
                l.idioma,
                l.serie,
                l.indice,
                l.editora,
                serde_json::to_string(&l.tags)?,
                l.estante,
                l.caminho,
                geracao,
                agora,
                super::titulo_principal(&l.titulo)
            ],
            |r| r.get(0),
        )?;
        for f in &l.formatos {
            up_formato.execute(params![
                id,
                biblioteca_id,
                f.formato,
                f.caminho,
                f.tamanho as i64,
                f.mtime_ns
            ])?;
        }
        let nomes: Vec<&str> = l.formatos.iter().map(|f| f.formato.as_str()).collect();
        conn.prepare_cached(
            "DELETE FROM formatos_homologados WHERE livro_id = ?1
             AND formato NOT IN (SELECT value FROM json_each(?2))",
        )?
        .execute(params![id, serde_json::to_string(&nomes)?])?;
        ids.push(id);
    }
    Ok(ids)
}

/// Remove os livros que não foram vistos na `geracao` (sumiram da biblioteca) e, em cascata, seus
/// formatos. Só chame depois de gravar **todos** os lotes da geração.
///
/// # Errors
/// Falha do SQLite.
pub fn remover_ausentes(conn: &Connection, biblioteca_id: i64, geracao: i64) -> Resultado<u64> {
    Ok(conn.execute(
        "DELETE FROM livros_homologados WHERE biblioteca_id = ?1 AND geracao < ?2",
        params![biblioteca_id, geracao],
    )? as u64)
}

/// Resumo de uma sincronização completa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumoSincronizacao {
    pub geracao: i64,
    pub livros: u64,
    pub removidos: u64,
}

/// Sincroniza o espelho inteiro (geração nova, lotes de 1000, remoção dos ausentes). Idempotente:
/// repetir com o mesmo conteúdo não muda nenhuma linha além da geração. O chamador deve envolver
/// a chamada numa transação se quiser tudo-ou-nada.
///
/// # Errors
/// Falha do SQLite.
pub fn sincronizar_espelho(
    conn: &Connection,
    biblioteca_id: i64,
    livros: impl IntoIterator<Item = LivroEspelho>,
    agora: i64,
) -> Resultado<ResumoSincronizacao> {
    let geracao = iniciar_sincronizacao(conn, biblioteca_id)?;
    let mut total = 0_u64;
    let mut lote = Vec::with_capacity(1000);
    for l in livros {
        lote.push(l);
        if lote.len() == 1000 {
            total += lote.len() as u64;
            gravar_lote(conn, biblioteca_id, geracao, &lote, agora)?;
            lote.clear();
        }
    }
    if !lote.is_empty() {
        total += lote.len() as u64;
        gravar_lote(conn, biblioteca_id, geracao, &lote, agora)?;
    }
    let removidos = remover_ausentes(conn, biblioteca_id, geracao)?;
    Ok(ResumoSincronizacao {
        geracao,
        livros: total,
        removidos,
    })
}

/// Formato homologado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FormatoHomologado {
    pub id: i64,
    pub livro_id: i64,
    pub biblioteca_id: i64,
    pub formato: String,
    pub caminho: String,
    pub tamanho: u64,
    pub mtime_ns: Option<i64>,
    pub hash: Option<[u8; 32]>,
    pub hash_em: Option<i64>,
    pub file_id: Option<i64>,
}

fn formato_de_linha(l: &Row<'_>) -> rusqlite::Result<FormatoHomologado> {
    Ok(FormatoHomologado {
        id: l.get(0)?,
        livro_id: l.get(1)?,
        biblioteca_id: l.get(2)?,
        formato: l.get(3)?,
        caminho: l.get(4)?,
        tamanho: l.get::<_, i64>(5)? as u64,
        mtime_ns: l.get(6)?,
        hash: hash_de_blob(l.get(7)?),
        hash_em: l.get(8)?,
        file_id: l.get(9)?,
    })
}

const COLUNAS_FORMATO: &str =
    "id, livro_id, biblioteca_id, formato, caminho, tamanho, mtime_ns, hash, hash_em, file_id";

/// Formatos de um livro.
///
/// # Errors
/// Falha do SQLite.
pub fn formatos_do_livro(conn: &Connection, livro_id: i64) -> Resultado<Vec<FormatoHomologado>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS_FORMATO} FROM formatos_homologados WHERE livro_id = ?1 ORDER BY formato"
    ))?;
    let v = st
        .query_map([livro_id], formato_de_linha)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Formatos ainda sem hash, por cursor `id` (para o agente calcular o hash em fatias).
///
/// # Errors
/// Falha do SQLite.
pub fn formatos_sem_hash(
    conn: &Connection,
    biblioteca_id: i64,
    apos_id: i64,
    limite: usize,
) -> Resultado<Vec<FormatoHomologado>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS_FORMATO} FROM formatos_homologados
         WHERE biblioteca_id = ?1 AND hash IS NULL AND id > ?2 ORDER BY id LIMIT ?3"
    ))?;
    let v = st
        .query_map(
            params![biblioteca_id, apos_id, limite_de(limite) as i64],
            formato_de_linha,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Grava o hash BLAKE3 calculado pelo DAG para o formato. Só vale se `tamanho` coincide com o
/// do espelho (o arquivo pode ter mudado entre a leitura e a gravação); devolve se gravou.
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_hash_formato(
    conn: &Connection,
    formato_id: i64,
    hash: &[u8; 32],
    tamanho: u64,
    agora: i64,
) -> Resultado<bool> {
    Ok(conn.execute(
        "UPDATE formatos_homologados SET hash = ?2, hash_em = ?3 WHERE id = ?1 AND tamanho = ?4",
        params![formato_id, hash.as_slice(), agora, tamanho as i64],
    )? == 1)
}

/// Liga o formato homologado ao arquivo correspondente do Catálogo (`files`).
///
/// # Errors
/// Falha do SQLite (FK).
pub fn ligar_arquivo_catalogo(
    conn: &Connection,
    formato_id: i64,
    file_id: Option<i64>,
) -> Resultado<()> {
    conn.execute(
        "UPDATE formatos_homologados SET file_id = ?2 WHERE id = ?1",
        params![formato_id, file_id],
    )?;
    Ok(())
}

/// Formato homologado achado por hash, com o livro a que pertence.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AchadoPorHash {
    pub formato: FormatoHomologado,
    pub livro: LivroHomologado,
}

/// Procura formatos homologados com este Hash Completo (e, se informado, o mesmo tamanho): é a
/// base do alerta "Você já tem este livro" (CA-B04), em O(log n) pelo índice `(hash, tamanho)`.
///
/// # Errors
/// Falha do SQLite.
pub fn buscar_por_hash(
    conn: &Connection,
    hash: &[u8; 32],
    tamanho: Option<u64>,
) -> Resultado<Vec<AchadoPorHash>> {
    let mut st = conn.prepare(
        "SELECT f.id, f.livro_id, f.biblioteca_id, f.formato, f.caminho, f.tamanho, f.mtime_ns, f.hash,
                f.hash_em, f.file_id,
                l.id, l.biblioteca_id, l.calibre_id, l.uuid, l.titulo, l.autores_json, l.isbn13, l.idioma,
                l.serie, l.indice, l.editora, l.tags_json, l.estante, l.caminho
         FROM formatos_homologados f JOIN livros_homologados l ON l.id = f.livro_id
         WHERE f.hash = ?1 AND (?2 IS NULL OR f.tamanho = ?2) ORDER BY f.id",
    )?;
    let v = st
        .query_map(params![hash.as_slice(), tamanho.map(|t| t as i64)], |l| {
            let formato = formato_de_linha(l)?;
            let json = |i: usize| -> rusqlite::Result<Vec<String>> {
                Ok(serde_json::from_str(&l.get::<_, String>(i)?).unwrap_or_default())
            };
            let livro = LivroHomologado {
                id: l.get(10)?,
                biblioteca_id: l.get(11)?,
                calibre_id: l.get(12)?,
                uuid: l.get(13)?,
                titulo: l.get(14)?,
                autores: json(15)?,
                isbn13: l.get(16)?,
                idioma: l.get(17)?,
                serie: l.get(18)?,
                indice: l.get(19)?,
                editora: l.get(20)?,
                tags: json(21)?,
                estante: l.get(22)?,
                caminho: l.get(23)?,
            };
            Ok(AchadoPorHash { formato, livro })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Livros homologados com este ISBN-13 (índice parcial).
///
/// # Errors
/// Falha do SQLite.
pub fn buscar_livros_por_isbn(conn: &Connection, isbn13: &str) -> Resultado<Vec<LivroHomologado>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS_LIVRO} FROM livros_homologados WHERE isbn13 = ?1 ORDER BY id"
    ))?;
    let v = st
        .query_map([isbn13], livro_de_linha)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Um livro do espelho pelo id.
///
/// # Errors
/// Falha do SQLite.
pub fn obter_livro(conn: &Connection, id: i64) -> Resultado<Option<LivroHomologado>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUNAS_LIVRO} FROM livros_homologados WHERE id = ?1"),
            [id],
            livro_de_linha,
        )
        .optional()?)
}

/// Livros de uma biblioteca, paginados por cursor (`id` do último livro da página anterior).
///
/// # Errors
/// Falha do SQLite.
pub fn listar_livros(
    conn: &Connection,
    biblioteca_id: i64,
    apos_id: Option<i64>,
    limite: usize,
) -> Resultado<Pagina<LivroHomologado, i64>> {
    let limite = limite_de(limite);
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS_LIVRO} FROM livros_homologados
         WHERE biblioteca_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3"
    ))?;
    let linhas = st
        .query_map(
            params![biblioteca_id, apos_id.unwrap_or(0), (limite + 1) as i64],
            livro_de_linha,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Pagina::de_linhas(linhas, limite, |l| l.id))
}

/// Total de livros do espelho (contador mantido).
///
/// # Errors
/// Falha do SQLite.
pub fn contar_livros(conn: &Connection, biblioteca_id: i64) -> Resultado<i64> {
    super::contador(conn, &format!("biblio_livros:{biblioteca_id}"))
}

/// Total de formatos do espelho (contador mantido).
///
/// # Errors
/// Falha do SQLite.
pub fn contar_formatos(conn: &Connection, biblioteca_id: i64) -> Resultado<i64> {
    super::contador(conn, &format!("biblio_formatos:{biblioteca_id}"))
}
