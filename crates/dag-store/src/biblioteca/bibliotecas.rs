//! Bibliotecas Calibre conhecidas, perfil, campos e instantâneos do `metadata.db`.

use dag_biblio::PapelBiblioteca;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::{enum_de_texto, hash_de_blob, invalido, texto_enum};
use crate::Resultado;

/// Situação de uma biblioteca.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoBiblioteca {
    Ativa,
    /// Caminho não encontrado/montado na última tentativa (não é remoção).
    Inacessivel,
    Arquivada,
}

/// Biblioteca Calibre registrada.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BibliotecaCalibre {
    pub id: i64,
    pub caminho: String,
    pub papel: PapelBiblioteca,
    pub nome: String,
    pub library_uuid: Option<String>,
    pub versao_calibre: Option<String>,
    pub estado: EstadoBiblioteca,
    pub perfil_json: Option<String>,
    pub criado_em: i64,
    pub atualizado_em: i64,
}

/// Dados para registrar uma biblioteca.
#[derive(Debug, Clone, Copy)]
pub struct NovaBiblioteca<'a> {
    pub caminho: &'a str,
    pub papel: PapelBiblioteca,
    pub nome: &'a str,
    pub library_uuid: Option<&'a str>,
    pub versao_calibre: Option<&'a str>,
}

const COLUNAS: &str = "id, caminho, papel, nome, library_uuid, versao_calibre, estado, perfil_json, criado_em, atualizado_em";

fn de_linha(l: &Row<'_>) -> rusqlite::Result<BibliotecaCalibre> {
    Ok(BibliotecaCalibre {
        id: l.get(0)?,
        caminho: l.get(1)?,
        papel: enum_de_texto(&l.get::<_, String>(2)?)?,
        nome: l.get(3)?,
        library_uuid: l.get(4)?,
        versao_calibre: l.get(5)?,
        estado: enum_de_texto(&l.get::<_, String>(6)?)?,
        perfil_json: l.get(7)?,
        criado_em: l.get(8)?,
        atualizado_em: l.get(9)?,
    })
}

/// Registra a biblioteca (idempotente por caminho) e devolve o id. Um caminho só tem um papel:
/// registrar o mesmo caminho com outro papel é erro. Pode haver várias Homologadas; só uma
/// A Classificar ativa (o índice parcial da migração recusa a segunda).
///
/// # Errors
/// Falha do SQLite, ou caminho já registrado com outro papel.
pub fn registrar_biblioteca(
    conn: &Connection,
    nova: &NovaBiblioteca<'_>,
    agora: i64,
) -> Resultado<i64> {
    let papel = texto_enum(&nova.papel);
    let existente: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, papel FROM bibliotecas_calibre WHERE caminho = ?1",
            [nova.caminho],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )
        .optional()?;
    if let Some((id, p)) = existente {
        if p != papel {
            return Err(invalido(format!(
                "o caminho {} já está registrado como {p}",
                nova.caminho
            )));
        }
        conn.execute(
            "UPDATE bibliotecas_calibre SET nome = ?2, library_uuid = COALESCE(?3, library_uuid),
                    versao_calibre = COALESCE(?4, versao_calibre), atualizado_em = ?5 WHERE id = ?1",
            params![id, nova.nome, nova.library_uuid, nova.versao_calibre, agora],
        )?;
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO bibliotecas_calibre (caminho, papel, nome, library_uuid, versao_calibre, criado_em, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![
            nova.caminho,
            papel,
            nova.nome,
            nova.library_uuid,
            nova.versao_calibre,
            agora
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Grava o perfil (JSON) lido da biblioteca e, se vier, a versão do Calibre.
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_perfil(
    conn: &Connection,
    biblioteca_id: i64,
    perfil_json: &str,
    versao_calibre: Option<&str>,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "UPDATE bibliotecas_calibre SET perfil_json = ?2, versao_calibre = COALESCE(?3, versao_calibre),
                atualizado_em = ?4 WHERE id = ?1",
        params![biblioteca_id, perfil_json, versao_calibre, agora],
    )?;
    Ok(())
}

/// Muda o estado (p. ex. `Inacessivel` quando o disco não está montado).
///
/// # Errors
/// Falha do SQLite.
pub fn definir_estado_biblioteca(
    conn: &Connection,
    biblioteca_id: i64,
    estado: EstadoBiblioteca,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "UPDATE bibliotecas_calibre SET estado = ?2, atualizado_em = ?3 WHERE id = ?1",
        params![biblioteca_id, texto_enum(&estado), agora],
    )?;
    Ok(())
}

/// Uma biblioteca pelo id.
///
/// # Errors
/// Falha do SQLite.
pub fn obter_biblioteca(conn: &Connection, id: i64) -> Resultado<Option<BibliotecaCalibre>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUNAS} FROM bibliotecas_calibre WHERE id = ?1"),
            [id],
            de_linha,
        )
        .optional()?)
}

/// Uma biblioteca pelo caminho.
///
/// # Errors
/// Falha do SQLite.
pub fn biblioteca_por_caminho(
    conn: &Connection,
    caminho: &str,
) -> Resultado<Option<BibliotecaCalibre>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUNAS} FROM bibliotecas_calibre WHERE caminho = ?1"),
            [caminho],
            de_linha,
        )
        .optional()?)
}

/// Bibliotecas registradas, filtradas por papel (poucas linhas: sem paginação).
///
/// # Errors
/// Falha do SQLite.
pub fn listar_bibliotecas(
    conn: &Connection,
    papel: Option<PapelBiblioteca>,
) -> Resultado<Vec<BibliotecaCalibre>> {
    let p = papel.map(|p| texto_enum(&p));
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS} FROM bibliotecas_calibre WHERE (?1 IS NULL OR papel = ?1) ORDER BY id"
    ))?;
    let linhas = st
        .query_map([p], de_linha)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(linhas)
}

/// Um campo (coluna) do Calibre e se o DAG pode clonar o valor para a A Classificar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampoCalibre {
    pub rotulo: String,
    pub nome: String,
    pub datatype: String,
    pub multiplo: bool,
    pub clonavel: bool,
    pub motivo: Option<String>,
}

/// Substitui o conjunto de campos da biblioteca (o perfil é sempre lido por inteiro).
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_campos(
    conn: &Connection,
    biblioteca_id: i64,
    campos: &[CampoCalibre],
) -> Resultado<()> {
    conn.execute(
        "DELETE FROM campos_calibre WHERE biblioteca_id = ?1",
        [biblioteca_id],
    )?;
    let mut st = conn.prepare_cached(
        "INSERT INTO campos_calibre (biblioteca_id, rotulo, nome, datatype, multiplo, clonavel, motivo)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(biblioteca_id, rotulo) DO UPDATE SET nome = excluded.nome,
            datatype = excluded.datatype, multiplo = excluded.multiplo,
            clonavel = excluded.clonavel, motivo = excluded.motivo",
    )?;
    for c in campos {
        st.execute(params![
            biblioteca_id,
            c.rotulo,
            c.nome,
            c.datatype,
            c.multiplo,
            c.clonavel,
            c.motivo
        ])?;
    }
    Ok(())
}

/// Campos da biblioteca, por rótulo.
///
/// # Errors
/// Falha do SQLite.
pub fn listar_campos(conn: &Connection, biblioteca_id: i64) -> Resultado<Vec<CampoCalibre>> {
    let mut st = conn.prepare(
        "SELECT rotulo, nome, datatype, multiplo, clonavel, motivo FROM campos_calibre
         WHERE biblioteca_id = ?1 ORDER BY rotulo",
    )?;
    let linhas = st
        .query_map([biblioteca_id], |l| {
            Ok(CampoCalibre {
                rotulo: l.get(0)?,
                nome: l.get(1)?,
                datatype: l.get(2)?,
                multiplo: l.get(3)?,
                clonavel: l.get(4)?,
                motivo: l.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(linhas)
}

/// Resultado da comparação de um instantâneo com o anterior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultadoComparacao {
    Primeiro,
    Igual,
    Alterado,
    Incompativel,
}

/// Instantâneo a gravar. `resultado = None` calcula `primeiro`/`igual`/`alterado` pelo hash do
/// `metadata.db` em relação ao instantâneo anterior.
#[derive(Debug, Clone, Default)]
pub struct NovoInstantaneo {
    pub quando: i64,
    pub hash_metadata_db: Option<[u8; 32]>,
    pub tamanho_metadata_db: Option<u64>,
    pub livros: u64,
    pub formatos: u64,
    pub manifesto_json: Option<String>,
    pub resultado: Option<ResultadoComparacao>,
    pub comparacao_json: Option<String>,
}

/// Instantâneo gravado.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InstantaneoBiblioteca {
    pub id: i64,
    pub biblioteca_id: i64,
    pub quando: i64,
    pub hash_metadata_db: Option<[u8; 32]>,
    pub livros: i64,
    pub formatos: i64,
    pub resultado: ResultadoComparacao,
    pub comparacao_json: Option<String>,
}

/// Grava um instantâneo e devolve-o (com o resultado da comparação).
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_instantaneo(
    conn: &Connection,
    biblioteca_id: i64,
    novo: &NovoInstantaneo,
) -> Resultado<InstantaneoBiblioteca> {
    let anterior = ultimo_instantaneo(conn, biblioteca_id)?;
    let resultado = novo.resultado.unwrap_or_else(|| match &anterior {
        None => ResultadoComparacao::Primeiro,
        Some(a) if a.hash_metadata_db.is_some() && a.hash_metadata_db == novo.hash_metadata_db => {
            ResultadoComparacao::Igual
        }
        Some(_) => ResultadoComparacao::Alterado,
    });
    conn.execute(
        "INSERT INTO instantaneos_biblioteca (biblioteca_id, quando, hash_metadata_db, tamanho_metadata_db,
                livros, formatos, manifesto_json, resultado, comparacao_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            biblioteca_id,
            novo.quando,
            novo.hash_metadata_db.as_ref().map(<[u8; 32]>::as_slice),
            novo.tamanho_metadata_db.map(|t| t as i64),
            novo.livros as i64,
            novo.formatos as i64,
            novo.manifesto_json,
            texto_enum(&resultado),
            novo.comparacao_json
        ],
    )?;
    Ok(InstantaneoBiblioteca {
        id: conn.last_insert_rowid(),
        biblioteca_id,
        quando: novo.quando,
        hash_metadata_db: novo.hash_metadata_db,
        livros: novo.livros as i64,
        formatos: novo.formatos as i64,
        resultado,
        comparacao_json: novo.comparacao_json.clone(),
    })
}

/// O instantâneo mais recente da biblioteca.
///
/// # Errors
/// Falha do SQLite.
pub fn ultimo_instantaneo(
    conn: &Connection,
    biblioteca_id: i64,
) -> Resultado<Option<InstantaneoBiblioteca>> {
    Ok(conn
        .query_row(
            "SELECT id, biblioteca_id, quando, hash_metadata_db, livros, formatos, resultado, comparacao_json
             FROM instantaneos_biblioteca WHERE biblioteca_id = ?1 ORDER BY quando DESC, id DESC LIMIT 1",
            [biblioteca_id],
            |l| {
                Ok(InstantaneoBiblioteca {
                    id: l.get(0)?,
                    biblioteca_id: l.get(1)?,
                    quando: l.get(2)?,
                    hash_metadata_db: hash_de_blob(l.get(3)?),
                    livros: l.get(4)?,
                    formatos: l.get(5)?,
                    resultado: enum_de_texto(&l.get::<_, String>(6)?)?,
                    comparacao_json: l.get(7)?,
                })
            },
        )
        .optional()?)
}
