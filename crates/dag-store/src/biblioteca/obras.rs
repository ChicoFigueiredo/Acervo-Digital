//! Obra > Edição > Formato e autores. Tudo é "garantir" (upsert por chave natural), para que
//! reprocessar o mesmo arquivo não duplique nada (CA-B11).

use dag_biblio::FormatoLivro;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{hash_de_blob, normalizar_texto, texto_enum};
use crate::Resultado;

/// Obra a garantir. `chave` é a identidade canônica decidida por quem classifica.
#[derive(Debug, Clone, Copy)]
pub struct NovaObra<'a> {
    pub chave: &'a str,
    pub titulo: &'a str,
    pub idioma_original: Option<&'a str>,
}

/// Garante a Obra (por `chave`) e devolve o id.
///
/// # Errors
/// Falha do SQLite.
pub fn garantir_obra(conn: &Connection, o: &NovaObra<'_>, agora: i64) -> Resultado<i64> {
    Ok(conn.query_row(
        "INSERT INTO obras (chave, titulo, titulo_norm, idioma_original, criado_em, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(chave) DO UPDATE SET
            idioma_original = COALESCE(obras.idioma_original, excluded.idioma_original)
         RETURNING id",
        params![
            o.chave,
            o.titulo,
            normalizar_texto(o.titulo),
            o.idioma_original,
            agora
        ],
        |l| l.get(0),
    )?)
}

/// Edição a garantir.
#[derive(Debug, Clone, Copy, Default)]
pub struct NovaEdicao<'a> {
    pub obra_id: i64,
    pub chave: &'a str,
    pub titulo: &'a str,
    pub editora: Option<&'a str>,
    pub edicao: Option<&'a str>,
    pub idioma: Option<&'a str>,
    pub ano: Option<i32>,
    pub isbn13: Option<&'a str>,
    pub serie: Option<&'a str>,
    pub indice_serie: Option<f64>,
    /// Livro do espelho homologado que é esta Edição, quando já existe.
    pub livro_homologado_id: Option<i64>,
}

/// Edição gravada.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Edicao {
    pub id: i64,
    pub obra_id: i64,
    pub chave: String,
    pub titulo: String,
    pub editora: Option<String>,
    pub edicao: Option<String>,
    pub idioma: Option<String>,
    pub ano: Option<i32>,
    pub isbn13: Option<String>,
    pub serie: Option<String>,
    pub indice_serie: Option<f64>,
    pub livro_homologado_id: Option<i64>,
}

/// Garante a Edição (por `chave`), completando campos que estavam vazios (nunca sobrescreve um
/// valor já conhecido). Devolve o id.
///
/// # Errors
/// Falha do SQLite.
pub fn garantir_edicao(conn: &Connection, e: &NovaEdicao<'_>, agora: i64) -> Resultado<i64> {
    Ok(conn.query_row(
        "INSERT INTO edicoes (obra_id, chave, titulo, editora, edicao, idioma, ano, isbn13, serie,
                indice_serie, livro_homologado_id, criado_em, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
         ON CONFLICT(chave) DO UPDATE SET
            editora = COALESCE(edicoes.editora, excluded.editora),
            edicao = COALESCE(edicoes.edicao, excluded.edicao),
            idioma = COALESCE(edicoes.idioma, excluded.idioma),
            ano = COALESCE(edicoes.ano, excluded.ano),
            isbn13 = COALESCE(edicoes.isbn13, excluded.isbn13),
            serie = COALESCE(edicoes.serie, excluded.serie),
            indice_serie = COALESCE(edicoes.indice_serie, excluded.indice_serie),
            livro_homologado_id = COALESCE(edicoes.livro_homologado_id, excluded.livro_homologado_id)
         RETURNING id",
        params![
            e.obra_id,
            e.chave,
            e.titulo,
            e.editora,
            e.edicao,
            e.idioma,
            e.ano,
            e.isbn13,
            e.serie,
            e.indice_serie,
            e.livro_homologado_id,
            agora
        ],
        |l| l.get(0),
    )?)
}

/// Uma Edição pelo id.
///
/// # Errors
/// Falha do SQLite.
pub fn obter_edicao(conn: &Connection, id: i64) -> Resultado<Option<Edicao>> {
    Ok(conn
        .query_row(
            "SELECT id, obra_id, chave, titulo, editora, edicao, idioma, ano, isbn13, serie, indice_serie,
                    livro_homologado_id FROM edicoes WHERE id = ?1",
            [id],
            |l| {
                Ok(Edicao {
                    id: l.get(0)?,
                    obra_id: l.get(1)?,
                    chave: l.get(2)?,
                    titulo: l.get(3)?,
                    editora: l.get(4)?,
                    edicao: l.get(5)?,
                    idioma: l.get(6)?,
                    ano: l.get(7)?,
                    isbn13: l.get(8)?,
                    serie: l.get(9)?,
                    indice_serie: l.get(10)?,
                    livro_homologado_id: l.get(11)?,
                })
            },
        )
        .optional()?)
}

/// Estado de um Formato bibliográfico.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoFormatoBiblio {
    Candidato,
    AClassificar,
    Homologado,
    Duplicado,
    Descartado,
    Falha,
}

/// Formato bibliográfico a garantir ("o EPUB da edição X").
#[derive(Debug, Clone, Copy)]
pub struct NovoFormatoBiblio {
    pub edicao_id: i64,
    pub formato: FormatoLivro,
    pub file_id: Option<i64>,
    pub hash: [u8; 32],
    pub tamanho: u64,
    pub estado: EstadoFormatoBiblio,
}

/// Formato bibliográfico gravado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FormatoBiblio {
    pub id: i64,
    pub edicao_id: i64,
    pub formato: String,
    pub file_id: Option<i64>,
    pub hash: Option<[u8; 32]>,
    pub tamanho: u64,
    pub estado: String,
}

/// Garante o Formato (por `(edicao, formato, hash)`): repetir não duplica; o estado já avançado
/// (p. ex. `homologado`) **não** é rebaixado por um reprocessamento que traga `candidato`.
///
/// # Errors
/// Falha do SQLite.
pub fn garantir_formato_biblio(
    conn: &Connection,
    f: &NovoFormatoBiblio,
    agora: i64,
) -> Resultado<i64> {
    Ok(conn.query_row(
        "INSERT INTO formatos_bibliograficos (edicao_id, formato, file_id, hash, tamanho, estado, criado_em, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT(edicao_id, formato, hash) DO UPDATE SET
            file_id = COALESCE(excluded.file_id, formatos_bibliograficos.file_id),
            estado = CASE WHEN formatos_bibliograficos.estado = 'candidato' THEN excluded.estado
                          ELSE formatos_bibliograficos.estado END,
            atualizado_em = excluded.atualizado_em
         RETURNING id",
        params![
            f.edicao_id,
            texto_enum(&f.formato),
            f.file_id,
            f.hash.as_slice(),
            f.tamanho as i64,
            texto_enum(&f.estado),
            agora
        ],
        |l| l.get(0),
    )?)
}

/// Muda o estado de um Formato bibliográfico.
///
/// # Errors
/// Falha do SQLite.
pub fn definir_estado_formato_biblio(
    conn: &Connection,
    id: i64,
    estado: EstadoFormatoBiblio,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "UPDATE formatos_bibliograficos SET estado = ?2, atualizado_em = ?3 WHERE id = ?1",
        params![id, texto_enum(&estado), agora],
    )?;
    Ok(())
}

/// Formatos de uma Edição.
///
/// # Errors
/// Falha do SQLite.
pub fn formatos_da_edicao(conn: &Connection, edicao_id: i64) -> Resultado<Vec<FormatoBiblio>> {
    let mut st = conn.prepare(
        "SELECT id, edicao_id, formato, file_id, hash, tamanho, estado FROM formatos_bibliograficos
         WHERE edicao_id = ?1 ORDER BY formato, id",
    )?;
    let v = st
        .query_map([edicao_id], |l| {
            Ok(FormatoBiblio {
                id: l.get(0)?,
                edicao_id: l.get(1)?,
                formato: l.get(2)?,
                file_id: l.get(3)?,
                hash: hash_de_blob(l.get(4)?),
                tamanho: l.get::<_, i64>(5)? as u64,
                estado: l.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Autor a garantir. Homônimos distintos usam `desambiguacao` diferente.
#[derive(Debug, Clone, Default)]
pub struct NovoAutor<'a> {
    pub nome: &'a str,
    pub desambiguacao: &'a str,
    pub ordenacao: Option<&'a str>,
    pub variantes: Vec<String>,
}

/// Garante o autor (por nome normalizado + desambiguação), somando variantes novas.
///
/// # Errors
/// Falha do SQLite.
pub fn garantir_autor(conn: &Connection, a: &NovoAutor<'_>, agora: i64) -> Resultado<i64> {
    let norm = normalizar_texto(a.nome);
    let id: i64 = conn.query_row(
        "INSERT INTO autores_biblio (nome, nome_norm, desambiguacao, ordenacao, variantes_json, criado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(nome_norm, desambiguacao) DO UPDATE SET
            ordenacao = COALESCE(autores_biblio.ordenacao, excluded.ordenacao)
         RETURNING id",
        params![
            a.nome,
            norm,
            a.desambiguacao,
            a.ordenacao,
            serde_json::to_string(&a.variantes)?,
            agora
        ],
        |l| l.get(0),
    )?;
    if !a.variantes.is_empty() {
        let atuais: String = conn.query_row(
            "SELECT variantes_json FROM autores_biblio WHERE id = ?1",
            [id],
            |l| l.get(0),
        )?;
        let mut v: Vec<String> = serde_json::from_str(&atuais).unwrap_or_default();
        let antes = v.len();
        for n in &a.variantes {
            if !v.contains(n) {
                v.push(n.clone());
            }
        }
        if v.len() != antes {
            conn.execute(
                "UPDATE autores_biblio SET variantes_json = ?2 WHERE id = ?1",
                params![id, serde_json::to_string(&v)?],
            )?;
        }
    }
    Ok(id)
}

/// Papel de um autor numa Edição.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PapelAutoria {
    Autor,
    Tradutor,
    Organizador,
    Ilustrador,
    Prefaciador,
    Outro,
}

/// Vincula (idempotente) um autor a uma Edição.
///
/// # Errors
/// Falha do SQLite.
pub fn vincular_autoria(
    conn: &Connection,
    edicao_id: i64,
    autor_id: i64,
    papel: PapelAutoria,
    ordem: i64,
) -> Resultado<()> {
    conn.execute(
        "INSERT INTO vinculo_autoria (edicao_id, autor_id, papel, ordem) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(edicao_id, autor_id, papel) DO UPDATE SET ordem = excluded.ordem",
        params![edicao_id, autor_id, texto_enum(&papel), ordem],
    )?;
    Ok(())
}

/// Autores de uma Edição `(nome, papel)`, na ordem de autoria.
///
/// # Errors
/// Falha do SQLite.
pub fn autores_da_edicao(conn: &Connection, edicao_id: i64) -> Resultado<Vec<(String, String)>> {
    let mut st = conn.prepare(
        "SELECT a.nome, v.papel FROM vinculo_autoria v JOIN autores_biblio a ON a.id = v.autor_id
         WHERE v.edicao_id = ?1 ORDER BY v.ordem, a.id",
    )?;
    let v = st
        .query_map([edicao_id], |l| Ok((l.get(0)?, l.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}
