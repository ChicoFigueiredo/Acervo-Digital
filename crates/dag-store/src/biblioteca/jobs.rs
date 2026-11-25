//! Fila persistente, incremental e retomável de jobs bibliográficos.
//!
//! Prioridade: o chamador calcula `prioridade` (economia potencial, tamanho, novidade); cada hora
//! de espera soma um ponto (aging, sem starvation). Como a ordem de saída é
//! `prioridade * 3600 - enfileirado_em`, que não depende do "agora", ela é indexada e "pegar o
//! próximo" é uma busca em índice (O(log n)), mesmo com centenas de milhares de jobs.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::{Pagina, contador, enum_de_texto, limite_de, texto_enum};
use crate::Resultado;

/// Por que o arquivo entrou na fila.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotivoJob {
    Novo,
    Alterado,
    Reprocessar,
}

/// Estado de um job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoJob {
    Pendente,
    EmAndamento,
    Concluido,
    Falhou,
    /// Precisa de gente (DRM, senha, ilegível): não volta sozinho para a fila.
    Excecao,
}

impl EstadoJob {
    pub const TODOS: [Self; 5] = [
        Self::Pendente,
        Self::EmAndamento,
        Self::Concluido,
        Self::Falhou,
        Self::Excecao,
    ];
}

/// Um job da fila.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Job {
    pub id: i64,
    pub file_id: i64,
    pub motivo: MotivoJob,
    pub prioridade: i64,
    pub enfileirado_em: i64,
    pub estado: EstadoJob,
    pub etapa: Option<String>,
    pub checkpoint_json: Option<String>,
    pub tentativas: i64,
    pub erro: Option<String>,
    pub versao_regras: Option<String>,
    pub iniciado_em: Option<i64>,
    pub atualizado_em: i64,
    pub concluido_em: Option<i64>,
}

const COLUNAS: &str =
    "id, file_id, motivo, prioridade, enfileirado_em, estado, etapa, checkpoint_json,
    tentativas, erro, versao_regras, iniciado_em, atualizado_em, concluido_em";

fn de_linha(l: &Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: l.get(0)?,
        file_id: l.get(1)?,
        motivo: enum_de_texto(&l.get::<_, String>(2)?)?,
        prioridade: l.get(3)?,
        enfileirado_em: l.get(4)?,
        estado: enum_de_texto(&l.get::<_, String>(5)?)?,
        etapa: l.get(6)?,
        checkpoint_json: l.get(7)?,
        tentativas: l.get(8)?,
        erro: l.get(9)?,
        versao_regras: l.get(10)?,
        iniciado_em: l.get(11)?,
        atualizado_em: l.get(12)?,
        concluido_em: l.get(13)?,
    })
}

/// Desfecho de [`enfileirar_job`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enfileirado {
    /// Job novo.
    Criado(i64),
    /// Já havia job ativo (pendente ou em andamento) para o arquivo; nada foi duplicado. Se o
    /// existente estava pendente, ficou com a maior das duas prioridades.
    JaAtivo(i64),
}

impl Enfileirado {
    #[must_use]
    pub const fn id(self) -> i64 {
        match self {
            Self::Criado(i) | Self::JaAtivo(i) => i,
        }
    }
}

/// Enfileira o arquivo. Nunca há dois jobs ativos para o mesmo arquivo (índice único parcial).
///
/// # Errors
/// Falha do SQLite (arquivo inexistente: FK).
pub fn enfileirar_job(
    conn: &Connection,
    file_id: i64,
    motivo: MotivoJob,
    prioridade: i64,
    versao_regras: Option<&str>,
    agora: i64,
) -> Resultado<Enfileirado> {
    let n = conn
        .prepare_cached(
            "INSERT INTO jobs_bibliograficos (file_id, motivo, prioridade, enfileirado_em, versao_regras, atualizado_em)
             VALUES (?1, ?2, ?3, ?4, ?5, ?4)
             ON CONFLICT(file_id) WHERE estado IN ('pendente','em_andamento') DO NOTHING",
        )?
        .execute(params![
            file_id,
            texto_enum(&motivo),
            prioridade,
            agora,
            versao_regras
        ])?;
    if n == 1 {
        return Ok(Enfileirado::Criado(conn.last_insert_rowid()));
    }
    let id: i64 = conn.query_row(
        "SELECT id FROM jobs_bibliograficos WHERE file_id = ?1 AND estado IN ('pendente','em_andamento')",
        [file_id],
        |l| l.get(0),
    )?;
    conn.execute(
        "UPDATE jobs_bibliograficos SET prioridade = max(prioridade, ?2) WHERE id = ?1 AND estado = 'pendente'",
        params![id, prioridade],
    )?;
    Ok(Enfileirado::JaAtivo(id))
}

/// Pega o próximo job (maior `prioridade` com aging) e o marca `em_andamento`, numa única
/// instrução atômica: dois trabalhadores nunca recebem o mesmo job.
///
/// # Errors
/// Falha do SQLite.
pub fn pegar_proximo_job(conn: &Connection, agora: i64) -> Resultado<Option<Job>> {
    Ok(conn
        .prepare_cached(&format!(
            "UPDATE jobs_bibliograficos SET estado = 'em_andamento', iniciado_em = ?1, atualizado_em = ?1,
                    tentativas = tentativas + 1
             WHERE id = (SELECT id FROM jobs_bibliograficos WHERE estado = 'pendente'
                         ORDER BY ordem DESC, id DESC LIMIT 1)
             RETURNING {COLUNAS}"
        ))?
        .query_row([agora], de_linha)
        .optional()?)
}

/// Grava a etapa atual e o checkpoint do job em andamento (retomada de onde parou).
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_checkpoint(
    conn: &Connection,
    job_id: i64,
    etapa: &str,
    checkpoint_json: Option<&str>,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "UPDATE jobs_bibliograficos SET etapa = ?2, checkpoint_json = ?3, atualizado_em = ?4 WHERE id = ?1",
        params![job_id, etapa, checkpoint_json, agora],
    )?;
    Ok(())
}

/// Conclui o job (limpa o erro; guarda a versão das regras que o processou, se informada).
///
/// # Errors
/// Falha do SQLite.
pub fn concluir_job(
    conn: &Connection,
    job_id: i64,
    versao_regras: Option<&str>,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "UPDATE jobs_bibliograficos SET estado = 'concluido', erro = NULL, concluido_em = ?2,
                atualizado_em = ?2, versao_regras = COALESCE(?3, versao_regras)
         WHERE id = ?1 AND estado = 'em_andamento'",
        params![job_id, agora, versao_regras],
    )?;
    Ok(())
}

/// Registra uma falha. Com tentativas restantes o job volta a `pendente` (indo para o fim do
/// aging, com o checkpoint preservado); esgotadas, fica `falhou`. Devolve o estado resultante.
///
/// # Errors
/// Falha do SQLite.
pub fn falhar_job(
    conn: &Connection,
    job_id: i64,
    erro: &str,
    max_tentativas: u32,
    agora: i64,
) -> Resultado<EstadoJob> {
    let estado: String = conn.query_row(
        "UPDATE jobs_bibliograficos SET erro = ?2, atualizado_em = ?3,
            estado = CASE WHEN tentativas >= ?4 THEN 'falhou' ELSE 'pendente' END,
            enfileirado_em = CASE WHEN tentativas >= ?4 THEN enfileirado_em ELSE ?3 END,
            concluido_em = CASE WHEN tentativas >= ?4 THEN ?3 ELSE NULL END
         WHERE id = ?1 AND estado = 'em_andamento' RETURNING estado",
        params![job_id, erro, agora, i64::from(max_tentativas)],
        |l| l.get(0),
    )?;
    Ok(enum_de_texto(&estado)?)
}

/// Marca o job como exceção (DRM, senha, ilegível): vai para revisão humana, sem retry.
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_excecao(conn: &Connection, job_id: i64, motivo: &str, agora: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE jobs_bibliograficos SET estado = 'excecao', erro = ?2, atualizado_em = ?3, concluido_em = ?3
         WHERE id = ?1 AND estado IN ('em_andamento','pendente')",
        params![job_id, motivo, agora],
    )?;
    Ok(())
}

/// Recuperação após queda: todo job `em_andamento` (órfão) volta a `pendente`, mantendo etapa e
/// checkpoint. Devolve quantos voltaram. Chamar na abertura do agente.
///
/// # Errors
/// Falha do SQLite.
pub fn retomar_orfaos(conn: &Connection, agora: i64) -> Resultado<u64> {
    Ok(conn.execute(
        "UPDATE jobs_bibliograficos SET estado = 'pendente', atualizado_em = ?1 WHERE estado = 'em_andamento'",
        [agora],
    )? as u64)
}

/// Muda a prioridade de um job pendente (a ordem de saída acompanha).
///
/// # Errors
/// Falha do SQLite.
pub fn alterar_prioridade(conn: &Connection, job_id: i64, prioridade: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE jobs_bibliograficos SET prioridade = ?2 WHERE id = ?1 AND estado = 'pendente'",
        params![job_id, prioridade],
    )?;
    Ok(())
}

/// O arquivo já foi processado com esta versão de regras (job concluído)? Quem enfileira usa
/// isto para pular o que não mudou (hash e decisão anterior reaproveitados, CA-B11).
///
/// # Errors
/// Falha do SQLite.
pub fn processado_com_versao(
    conn: &Connection,
    file_id: i64,
    versao_regras: &str,
) -> Resultado<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM jobs_bibliograficos WHERE file_id = ?1 AND estado = 'concluido' AND versao_regras = ?2 LIMIT 1",
            params![file_id, versao_regras],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Um job pelo id.
///
/// # Errors
/// Falha do SQLite.
pub fn obter_job(conn: &Connection, id: i64) -> Resultado<Option<Job>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUNAS} FROM jobs_bibliograficos WHERE id = ?1"),
            [id],
            de_linha,
        )
        .optional()?)
}

/// Histórico de jobs de um arquivo (mais novo primeiro).
///
/// # Errors
/// Falha do SQLite.
pub fn jobs_do_arquivo(conn: &Connection, file_id: i64) -> Resultado<Vec<Job>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS} FROM jobs_bibliograficos WHERE file_id = ?1 ORDER BY id DESC"
    ))?;
    let v = st
        .query_map([file_id], de_linha)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Contagem de jobs por estado (contadores mantidos, sem varrer a tabela).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ContagemJobs {
    pub pendentes: i64,
    pub em_andamento: i64,
    pub concluidos: i64,
    pub falhos: i64,
    pub excecoes: i64,
}

/// Lê a contagem por estado.
///
/// # Errors
/// Falha do SQLite.
pub fn contagem_jobs(conn: &Connection) -> Resultado<ContagemJobs> {
    let c = |e: EstadoJob| contador(conn, &format!("biblio_jobs:{}", texto_enum(&e)));
    Ok(ContagemJobs {
        pendentes: c(EstadoJob::Pendente)?,
        em_andamento: c(EstadoJob::EmAndamento)?,
        concluidos: c(EstadoJob::Concluido)?,
        falhos: c(EstadoJob::Falhou)?,
        excecoes: c(EstadoJob::Excecao)?,
    })
}

/// Cursor da fila: `(ordem, id)` do último item da página anterior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorJob {
    pub ordem: i64,
    pub id: i64,
}

/// Lista a fila de um estado, na ordem em que sairia (maior `ordem` primeiro), por cursor keyset.
///
/// # Errors
/// Falha do SQLite.
pub fn listar_jobs(
    conn: &Connection,
    estado: EstadoJob,
    apos: Option<CursorJob>,
    limite: usize,
) -> Resultado<Pagina<Job, CursorJob>> {
    let limite = limite_de(limite);
    let (ordem, id) = apos.map_or((i64::MAX, i64::MAX), |c| (c.ordem, c.id));
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS}, ordem FROM jobs_bibliograficos
         WHERE estado = ?1 AND (ordem, id) < (?2, ?3) ORDER BY ordem DESC, id DESC LIMIT ?4"
    ))?;
    let linhas = st
        .query_map(
            params![texto_enum(&estado), ordem, id, (limite + 1) as i64],
            |l| Ok((de_linha(l)?, l.get::<_, i64>(14)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Pagina::de_linhas(linhas, limite, |(j, o)| CursorJob {
        ordem: *o,
        id: j.id,
    })
    .mapear(|(j, _)| j))
}
