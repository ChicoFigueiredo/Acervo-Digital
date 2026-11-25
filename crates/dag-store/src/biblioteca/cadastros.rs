//! Cadastros na biblioteca A Classificar (idempotentes), homologações e backups.

use dag_biblio::PapelBiblioteca;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::{
    AutorAcao, Pagina, enum_de_texto, hex, invalido, limite_de, obter_biblioteca, texto_enum,
};
use crate::Resultado;

/// Chave de idempotência de um cadastro: `dag:<hash BLAKE3 em hex>`. A mesma chave grava-se no
/// livro do Calibre (identificador), então reprocessar nunca cadastra duas vezes.
#[must_use]
pub fn chave_idempotencia(hash: &[u8; 32]) -> String {
    format!("dag:{}", hex(hash))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoCadastro {
    Pendente,
    Cadastrando,
    Cadastrado,
    Falhou,
    Removido,
}

/// Cadastro na A Classificar.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CadastroAClassificar {
    pub id: i64,
    pub file_id: i64,
    pub edicao_id: Option<i64>,
    pub biblioteca_id: i64,
    pub calibre_id: Option<i64>,
    pub chave_idempotencia: String,
    pub estado: EstadoCadastro,
    pub erro: Option<String>,
    pub criado_em: i64,
    pub atualizado_em: i64,
}

const COLUNAS_CAD: &str = "id, file_id, edicao_id, biblioteca_id, calibre_id, chave_idempotencia, estado, erro, criado_em, atualizado_em";

fn cadastro_de_linha(l: &Row<'_>) -> rusqlite::Result<CadastroAClassificar> {
    Ok(CadastroAClassificar {
        id: l.get(0)?,
        file_id: l.get(1)?,
        edicao_id: l.get(2)?,
        biblioteca_id: l.get(3)?,
        calibre_id: l.get(4)?,
        chave_idempotencia: l.get(5)?,
        estado: enum_de_texto(&l.get::<_, String>(6)?)?,
        erro: l.get(7)?,
        criado_em: l.get(8)?,
        atualizado_em: l.get(9)?,
    })
}

/// Registra a intenção de cadastrar o arquivo na A Classificar (idempotente por biblioteca e
/// `dag:<hash>`). Devolve `(cadastro, novo)`. Um cadastro `falhou` volta a `pendente` ao ser
/// registrado de novo; um já `cadastrado` não é tocado.
///
/// # Errors
/// Falha do SQLite, ou `biblioteca_id` que não é uma A Classificar.
pub fn registrar_cadastro(
    conn: &Connection,
    file_id: i64,
    edicao_id: Option<i64>,
    biblioteca_id: i64,
    hash: &[u8; 32],
    agora: i64,
) -> Resultado<(CadastroAClassificar, bool)> {
    match obter_biblioteca(conn, biblioteca_id)? {
        Some(b) if b.papel == PapelBiblioteca::AClassificar => {}
        _ => {
            return Err(invalido(
                "cadastros só vão para uma biblioteca A Classificar",
            ));
        }
    }
    let chave = chave_idempotencia(hash);
    let n = conn.execute(
        "INSERT INTO cadastros_a_classificar (file_id, edicao_id, biblioteca_id, chave_idempotencia, criado_em, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5) ON CONFLICT(biblioteca_id, chave_idempotencia) DO NOTHING",
        params![file_id, edicao_id, biblioteca_id, chave, agora],
    )?;
    if n == 0 {
        conn.execute(
            "UPDATE cadastros_a_classificar SET estado = 'pendente', erro = NULL, atualizado_em = ?3
             WHERE biblioteca_id = ?1 AND chave_idempotencia = ?2 AND estado = 'falhou'",
            params![biblioteca_id, chave, agora],
        )?;
    }
    let c = cadastro_por_chave(conn, biblioteca_id, &chave)?
        .ok_or_else(|| invalido("cadastro sumiu logo após o registro"))?;
    Ok((c, n == 1))
}

/// Cadastro pela chave de idempotência.
///
/// # Errors
/// Falha do SQLite.
pub fn cadastro_por_chave(
    conn: &Connection,
    biblioteca_id: i64,
    chave: &str,
) -> Resultado<Option<CadastroAClassificar>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {COLUNAS_CAD} FROM cadastros_a_classificar WHERE biblioteca_id = ?1 AND chave_idempotencia = ?2"
            ),
            params![biblioteca_id, chave],
            cadastro_de_linha,
        )
        .optional()?)
}

/// Cadastro pelo id.
///
/// # Errors
/// Falha do SQLite.
pub fn obter_cadastro(conn: &Connection, id: i64) -> Resultado<Option<CadastroAClassificar>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUNAS_CAD} FROM cadastros_a_classificar WHERE id = ?1"),
            [id],
            cadastro_de_linha,
        )
        .optional()?)
}

/// Marca o cadastro como em andamento (antes de chamar a interface oficial do Calibre).
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_cadastrando(conn: &Connection, id: i64, agora: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE cadastros_a_classificar SET estado = 'cadastrando', atualizado_em = ?2
         WHERE id = ?1 AND estado IN ('pendente','falhou')",
        params![id, agora],
    )?;
    Ok(())
}

/// Registra o sucesso: o `calibre_id` criado na A Classificar.
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_cadastrado(conn: &Connection, id: i64, calibre_id: i64, agora: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE cadastros_a_classificar SET estado = 'cadastrado', calibre_id = ?2, erro = NULL, atualizado_em = ?3
         WHERE id = ?1",
        params![id, calibre_id, agora],
    )?;
    Ok(())
}

/// Registra a falha do cadastro.
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_cadastro_falhou(conn: &Connection, id: i64, erro: &str, agora: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE cadastros_a_classificar SET estado = 'falhou', erro = ?2, atualizado_em = ?3 WHERE id = ?1",
        params![id, erro, agora],
    )?;
    Ok(())
}

/// Cadastros de um estado, por cursor (`id`).
///
/// # Errors
/// Falha do SQLite.
pub fn listar_cadastros(
    conn: &Connection,
    estado: EstadoCadastro,
    apos_id: Option<i64>,
    limite: usize,
) -> Resultado<Pagina<CadastroAClassificar, i64>> {
    let limite = limite_de(limite);
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS_CAD} FROM cadastros_a_classificar WHERE estado = ?1 AND id > ?2 ORDER BY id LIMIT ?3"
    ))?;
    let linhas = st
        .query_map(
            params![
                texto_enum(&estado),
                apos_id.unwrap_or(0),
                (limite + 1) as i64
            ],
            cadastro_de_linha,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Pagina::de_linhas(linhas, limite, |c| c.id))
}

// ---------------------------------------------------------------------------------------------
// Homologações
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoHomologacao {
    Proposta,
    Aprovada,
    Aplicada,
    Falhou,
    Revertida,
}

/// Homologação: levar um cadastro da A Classificar à Biblioteca Homologada, com aprovação
/// explícita. O DAG só registra; a escrita no Calibre é de outra camada.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Homologacao {
    pub id: i64,
    pub cadastro_id: i64,
    pub biblioteca_destino_id: i64,
    pub aprovado_por: Option<String>,
    pub aprovado_em: Option<i64>,
    pub autor: Option<AutorAcao>,
    pub antes_json: Option<String>,
    pub depois_json: Option<String>,
    pub calibre_id: Option<i64>,
    pub rollback_id: Option<String>,
    pub estado: EstadoHomologacao,
    pub erro: Option<String>,
    pub criado_em: i64,
    pub atualizado_em: i64,
}

const COLUNAS_HOM: &str =
    "id, cadastro_id, biblioteca_destino_id, aprovado_por, aprovado_em, autor, antes_json,
    depois_json, calibre_id, rollback_id, estado, erro, criado_em, atualizado_em";

fn hom_de_linha(l: &Row<'_>) -> rusqlite::Result<Homologacao> {
    let autor: Option<String> = l.get(5)?;
    Ok(Homologacao {
        id: l.get(0)?,
        cadastro_id: l.get(1)?,
        biblioteca_destino_id: l.get(2)?,
        aprovado_por: l.get(3)?,
        aprovado_em: l.get(4)?,
        autor: autor.map(|a| enum_de_texto(&a)).transpose()?,
        antes_json: l.get(6)?,
        depois_json: l.get(7)?,
        calibre_id: l.get(8)?,
        rollback_id: l.get(9)?,
        estado: enum_de_texto(&l.get::<_, String>(10)?)?,
        erro: l.get(11)?,
        criado_em: l.get(12)?,
        atualizado_em: l.get(13)?,
    })
}

/// Propõe a homologação (`antes` = como está, `depois` = como ficará). O destino precisa ser uma
/// biblioteca Homologada.
///
/// # Errors
/// Falha do SQLite, ou destino que não é Homologada.
pub fn propor_homologacao(
    conn: &Connection,
    cadastro_id: i64,
    destino_id: i64,
    antes_json: Option<&str>,
    depois_json: Option<&str>,
    agora: i64,
) -> Resultado<i64> {
    match obter_biblioteca(conn, destino_id)? {
        Some(b) if b.papel == PapelBiblioteca::Homologada => {}
        _ => {
            return Err(invalido(
                "o destino da homologação deve ser uma biblioteca Homologada",
            ));
        }
    }
    conn.execute(
        "INSERT INTO homologacoes (cadastro_id, biblioteca_destino_id, antes_json, depois_json, criado_em, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![cadastro_id, destino_id, antes_json, depois_json, agora],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Registra a aprovação explícita (quem e quando). Só vale a partir de `proposta`.
///
/// # Errors
/// Falha do SQLite, ou homologação que não está `proposta`.
pub fn aprovar_homologacao(
    conn: &Connection,
    id: i64,
    aprovado_por: &str,
    autor: AutorAcao,
    agora: i64,
) -> Resultado<()> {
    let n = conn.execute(
        "UPDATE homologacoes SET estado = 'aprovada', aprovado_por = ?2, autor = ?3, aprovado_em = ?4, atualizado_em = ?4
         WHERE id = ?1 AND estado = 'proposta'",
        params![id, aprovado_por, texto_enum(&autor), agora],
    )?;
    if n == 0 {
        return Err(invalido("só se aprova uma homologação em estado proposta"));
    }
    Ok(())
}

/// Registra a aplicação (já feita pela camada que escreve): `calibre_id` criado no destino e o
/// `rollback_id` para desfazer. Só vale a partir de `aprovada`.
///
/// # Errors
/// Falha do SQLite, ou homologação que não está `aprovada`.
pub fn registrar_aplicacao(
    conn: &Connection,
    id: i64,
    calibre_id: i64,
    rollback_id: &str,
    agora: i64,
) -> Resultado<()> {
    let n = conn.execute(
        "UPDATE homologacoes SET estado = 'aplicada', calibre_id = ?2, rollback_id = ?3, erro = NULL, atualizado_em = ?4
         WHERE id = ?1 AND estado = 'aprovada'",
        params![id, calibre_id, rollback_id, agora],
    )?;
    if n == 0 {
        return Err(invalido(
            "só se registra a aplicação de uma homologação aprovada",
        ));
    }
    Ok(())
}

/// Registra falha da aplicação (volta a `aprovada` não é automático: decisão do operador).
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_homologacao_falhou(
    conn: &Connection,
    id: i64,
    erro: &str,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "UPDATE homologacoes SET estado = 'falhou', erro = ?2, atualizado_em = ?3 WHERE id = ?1",
        params![id, erro, agora],
    )?;
    Ok(())
}

/// Registra a reversão de uma homologação aplicada.
///
/// # Errors
/// Falha do SQLite, ou homologação que não está `aplicada`.
pub fn registrar_reversao(conn: &Connection, id: i64, agora: i64) -> Resultado<()> {
    let n = conn.execute(
        "UPDATE homologacoes SET estado = 'revertida', atualizado_em = ?2 WHERE id = ?1 AND estado = 'aplicada'",
        params![id, agora],
    )?;
    if n == 0 {
        return Err(invalido("só se reverte uma homologação aplicada"));
    }
    Ok(())
}

/// Homologação pelo id.
///
/// # Errors
/// Falha do SQLite.
pub fn obter_homologacao(conn: &Connection, id: i64) -> Resultado<Option<Homologacao>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUNAS_HOM} FROM homologacoes WHERE id = ?1"),
            [id],
            hom_de_linha,
        )
        .optional()?)
}

/// Homologações de um cadastro (mais nova primeiro).
///
/// # Errors
/// Falha do SQLite.
pub fn homologacoes_do_cadastro(
    conn: &Connection,
    cadastro_id: i64,
) -> Resultado<Vec<Homologacao>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS_HOM} FROM homologacoes WHERE cadastro_id = ?1 ORDER BY id DESC"
    ))?;
    let v = st
        .query_map([cadastro_id], hom_de_linha)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

// ---------------------------------------------------------------------------------------------
// Backups (mínimo: a Cobertura da Fase A faz o trabalho pesado)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificacaoBackup {
    Pendente,
    Ok,
    Falhou,
}

/// Registro de backup de uma biblioteca.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BackupBiblioteca {
    pub id: i64,
    pub biblioteca_id: i64,
    pub manifesto_json: String,
    pub criado_em: i64,
    pub verificacao: VerificacaoBackup,
    pub verificado_em: Option<i64>,
    pub restauracao_json: Option<String>,
    pub restauracao_em: Option<i64>,
    pub cobertura_json: Option<String>,
}

/// Registra o manifesto de um backup.
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_backup(
    conn: &Connection,
    biblioteca_id: i64,
    manifesto_json: &str,
    agora: i64,
) -> Resultado<i64> {
    conn.execute(
        "INSERT INTO backups_biblioteca (biblioteca_id, manifesto_json, criado_em) VALUES (?1, ?2, ?3)",
        params![biblioteca_id, manifesto_json, agora],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Registra o resultado da verificação do backup.
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_verificacao_backup(
    conn: &Connection,
    id: i64,
    resultado: VerificacaoBackup,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "UPDATE backups_biblioteca SET verificacao_estado = ?2, verificado_em = ?3 WHERE id = ?1",
        params![id, texto_enum(&resultado), agora],
    )?;
    Ok(())
}

/// Registra a restauração amostral e/ou a Cobertura (JSON livre da camada que mede).
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_restauracao_e_cobertura(
    conn: &Connection,
    id: i64,
    restauracao_json: Option<&str>,
    cobertura_json: Option<&str>,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "UPDATE backups_biblioteca SET restauracao_json = COALESCE(?2, restauracao_json),
                restauracao_em = CASE WHEN ?2 IS NULL THEN restauracao_em ELSE ?4 END,
                cobertura_json = COALESCE(?3, cobertura_json) WHERE id = ?1",
        params![id, restauracao_json, cobertura_json, agora],
    )?;
    Ok(())
}

/// O backup mais recente da biblioteca.
///
/// # Errors
/// Falha do SQLite.
pub fn ultimo_backup(conn: &Connection, biblioteca_id: i64) -> Resultado<Option<BackupBiblioteca>> {
    Ok(conn
        .query_row(
            "SELECT id, biblioteca_id, manifesto_json, criado_em, verificacao_estado, verificado_em,
                    restauracao_json, restauracao_em, cobertura_json
             FROM backups_biblioteca WHERE biblioteca_id = ?1 ORDER BY criado_em DESC, id DESC LIMIT 1",
            [biblioteca_id],
            |l| {
                Ok(BackupBiblioteca {
                    id: l.get(0)?,
                    biblioteca_id: l.get(1)?,
                    manifesto_json: l.get(2)?,
                    criado_em: l.get(3)?,
                    verificacao: enum_de_texto(&l.get::<_, String>(4)?)?,
                    verificado_em: l.get(5)?,
                    restauracao_json: l.get(6)?,
                    restauracao_em: l.get(7)?,
                    cobertura_json: l.get(8)?,
                })
            },
        )
        .optional()?)
}
