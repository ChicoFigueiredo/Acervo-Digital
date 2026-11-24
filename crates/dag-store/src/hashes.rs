//! Hash Completo e Verificação de Integridade: fila persistente (`fila_hash`), gravação do hash
//! (`hashes`) e consultas de apoio da etapa de hash do scanner.
//!
//! Tudo recebe uma `&Connection`; o chamador decide a transação. A tabela `hashes` é chaveada
//! por `file_id`: um arquivo físico com vários caminhos (hard links, mesmo volume, `native_dev` e
//! `native_file_id`, `nlink > 1`) é lido uma vez e a linha de hash é gravada para todos os
//! caminhos cuja evidência (tamanho, mtime, ctime) coincide (CA-A19).
//!
//! Reuso (CA-A25): uma linha de `hashes` só vale enquanto tamanho, mtime, ctime e inode do
//! arquivo no inventário continuarem iguais aos guardados com o hash.

use rusqlite::{Connection, OptionalExtension, params};

use crate::Resultado;

/// Natureza do item da fila.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipoFila {
    Hash,
    Verificacao,
}

impl TipoFila {
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::Hash => "hash",
            Self::Verificacao => "verificacao",
        }
    }
}

/// Condição "o arquivo não tem hash válido" (a evidência guardada não bate com o inventário).
const SEM_HASH_VALIDO: &str = "NOT EXISTS (SELECT 1 FROM hashes h WHERE h.file_id = {a}.id
     AND h.size_bytes = {a}.logical_bytes AND h.mtime_ns IS {a}.mtime_ns
     AND h.ctime_ns IS {a}.ctime_ns AND h.native_file_id IS {a}.native_file_id)";

fn sem_hash_valido(alias: &str) -> String {
    SEM_HASH_VALIDO.replace("{a}", alias)
}

/// Resumo da fila montada para uma execução.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResumoFila {
    /// Arquivos regulares ativos vistos na execução.
    pub candidatos: u64,
    /// Candidatos que já têm hash válido (não serão lidos).
    pub reaproveitados: u64,
    /// Leituras físicas enfileiradas (um hard link com vários caminhos conta uma vez).
    pub enfileirados: u64,
    /// Soma dos tamanhos das leituras enfileiradas.
    pub bytes_total: u64,
}

/// Item reservado da fila.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemFila {
    pub fila_id: i64,
    pub file_id: i64,
    pub pasta_id: Option<i64>,
    pub volume_id: i64,
    pub path_raw: Vec<u8>,
    /// Evidência esperada em disco: do inventário (hash) ou do hash guardado (verificação).
    pub tamanho: i64,
    pub mtime_ns: Option<i64>,
    pub ctime_ns: Option<i64>,
    pub native_file_id: Option<i64>,
    /// Hash guardado (só na verificação).
    pub hash_guardado: Option<Vec<u8>>,
}

/// Escopo da Verificação de Integridade.
#[derive(Debug, Clone, Copy)]
pub enum FiltroVerificacao<'a> {
    Tudo,
    Volume(i64),
    /// Caminho bruto absoluto: o próprio arquivo ou tudo abaixo dele.
    Prefixo(&'a [u8]),
}

/// Monta a fila de hash da execução: descarta a fila antiga de hash, conta os reaproveitados e
/// enfileira uma leitura por arquivo físico que ainda precisa de hash.
///
/// # Errors
/// Falha do SQLite.
pub fn preparar_fila_hash(conn: &Connection, run_id: i64, agora: i64) -> Resultado<ResumoFila> {
    conn.execute("DELETE FROM fila_hash WHERE tipo = 'hash'", [])?;
    let candidatos: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE seen_run_id = ?1 AND kind = 'arquivo' AND state = 'ativo'",
        [run_id],
        |l| l.get(0),
    )?;
    let precisa_f = sem_hash_valido("f");
    let precisa_m = sem_hash_valido("m");
    let sql = format!(
        "INSERT INTO fila_hash (file_id, tipo, state, priority, enqueued_at, updated_at)
         SELECT f.id, 'hash', 'pendente', 0, ?2, ?2 FROM files f
         WHERE f.seen_run_id = ?1 AND f.kind = 'arquivo' AND f.state = 'ativo' AND {precisa_f}
           AND NOT EXISTS (
             SELECT 1 FROM files m
             WHERE f.nlink > 1 AND f.native_file_id IS NOT NULL AND m.id < f.id
               AND m.volume_id = f.volume_id AND m.native_dev IS f.native_dev
               AND m.native_file_id = f.native_file_id
               AND m.seen_run_id = ?1 AND m.kind = 'arquivo' AND m.state = 'ativo'
               AND m.logical_bytes = f.logical_bytes AND m.mtime_ns IS f.mtime_ns
               AND m.ctime_ns IS f.ctime_ns AND {precisa_m})
         ON CONFLICT(file_id) DO UPDATE SET tipo = 'hash', state = 'pendente', attempt_count = 0,
             bytes_lidos = 0, last_error = NULL, enqueued_at = ?2, updated_at = ?2"
    );
    conn.execute(&sql, params![run_id, agora])?;
    let (n, bytes): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(f.logical_bytes), 0) FROM fila_hash q
         JOIN files f ON f.id = q.file_id WHERE q.tipo = 'hash' AND q.state = 'pendente'",
        [],
        |l| Ok((l.get(0)?, l.get(1)?)),
    )?;
    let sem_hash = contar_sem_hash_valido(conn, run_id)?;
    Ok(ResumoFila {
        candidatos: candidatos as u64,
        reaproveitados: (candidatos as u64).saturating_sub(sem_hash),
        enfileirados: n as u64,
        bytes_total: bytes as u64,
    })
}

/// Candidatos da execução sem hash válido, por linha de arquivo (inclui irmãos de hard link).
///
/// # Errors
/// Falha do SQLite.
pub fn contar_sem_hash_valido(conn: &Connection, run_id: i64) -> Resultado<u64> {
    let precisa = sem_hash_valido("f");
    let n: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM files f WHERE f.seen_run_id = ?1 AND f.kind = 'arquivo'
             AND f.state = 'ativo' AND {precisa}"
        ),
        [run_id],
        |l| l.get(0),
    )?;
    Ok(n as u64)
}

/// Enfileira para verificação os arquivos ativos que têm hash, dentro do filtro. Um hard link
/// com vários caminhos entra uma vez. Devolve quantos itens entraram. Pode ser chamada várias
/// vezes (várias raízes); a primeira chamada deve ser precedida de [`limpar_fila`].
///
/// # Errors
/// Falha do SQLite.
pub fn enfileirar_verificacao(
    conn: &Connection,
    filtro: FiltroVerificacao<'_>,
    agora: i64,
) -> Resultado<u64> {
    let (volume, exato, prefixo): (Option<i64>, Option<Vec<u8>>, Option<Vec<u8>>) = match filtro {
        FiltroVerificacao::Tudo => (None, None, None),
        FiltroVerificacao::Volume(v) => (Some(v), None, None),
        FiltroVerificacao::Prefixo(p) => {
            let mut com_barra = p.to_vec();
            if com_barra.last() != Some(&b'/') {
                com_barra.push(b'/');
            }
            (None, Some(p.to_vec()), Some(com_barra))
        }
    };
    let sql =
        "INSERT INTO fila_hash (file_id, tipo, state, priority, enqueued_at, updated_at)
         SELECT f.id, 'verificacao', 'pendente', 0, ?1, ?1 FROM files f
         JOIN hashes h ON h.file_id = f.id
         WHERE f.kind = 'arquivo' AND f.state = 'ativo'
           AND (?2 IS NULL OR f.volume_id = ?2)
           AND (?3 IS NULL OR f.path_raw = ?3 OR substr(f.path_raw, 1, length(?4)) = ?4)
           AND NOT EXISTS (
             SELECT 1 FROM files m JOIN hashes hm ON hm.file_id = m.id
             WHERE f.nlink > 1 AND f.native_file_id IS NOT NULL AND m.id < f.id
               AND m.volume_id = f.volume_id AND m.native_dev IS f.native_dev
               AND m.native_file_id = f.native_file_id AND m.kind = 'arquivo'
               AND m.state = 'ativo' AND hm.size_bytes = h.size_bytes
               AND hm.mtime_ns IS h.mtime_ns AND hm.ctime_ns IS h.ctime_ns
               AND hm.hash = h.hash)
         ON CONFLICT(file_id) DO UPDATE SET tipo = 'verificacao', state = 'pendente',
             attempt_count = 0, bytes_lidos = 0, last_error = NULL, enqueued_at = ?1, updated_at = ?1";
    let mut st = conn.prepare(sql)?;
    let n = st.execute(params![agora, volume, exato, prefixo])?;
    Ok(n as u64)
}

/// Esvazia a fila de um tipo (antes de montar a verificação).
///
/// # Errors
/// Falha do SQLite.
pub fn limpar_fila(conn: &Connection, tipo: TipoFila) -> Resultado<()> {
    conn.execute("DELETE FROM fila_hash WHERE tipo = ?1", [tipo.como_str()])?;
    Ok(())
}

/// Totais pendentes (leituras e bytes) de um tipo de fila.
///
/// # Errors
/// Falha do SQLite.
pub fn totais_pendentes(conn: &Connection, tipo: TipoFila) -> Resultado<(u64, u64)> {
    let (n, b): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN q.tipo = 'verificacao'
                    THEN (SELECT size_bytes FROM hashes WHERE file_id = f.id) ELSE f.logical_bytes END), 0)
         FROM fila_hash q JOIN files f ON f.id = q.file_id
         WHERE q.tipo = ?1 AND q.state IN ('pendente','em_andamento','pausado')",
        [tipo.como_str()],
        |l| Ok((l.get(0)?, l.get(1)?)),
    )?;
    Ok((n as u64, b as u64))
}

/// Reserva (lease) até `limite` itens pendentes: passam a `em_andamento`.
///
/// # Errors
/// Falha do SQLite.
pub fn reservar(
    conn: &Connection,
    tipo: TipoFila,
    limite: usize,
    agora: i64,
) -> Resultado<Vec<ItemFila>> {
    reservar_volumes(conn, tipo, &[], limite, agora)
}

/// Como [`reservar`], mas só itens cujos arquivos estão em `volumes` (vazio = qualquer volume).
/// É o que permite ao agendador dar a cada Dispositivo Físico a sua própria sequência de leitura.
///
/// # Errors
/// Falha do SQLite.
pub fn reservar_volumes(
    conn: &Connection,
    tipo: TipoFila,
    volumes: &[i64],
    limite: usize,
    agora: i64,
) -> Resultado<Vec<ItemFila>> {
    let mut itens = Vec::with_capacity(limite);
    {
        let filtro = if volumes.is_empty() {
            String::new()
        } else {
            let ph = vec!["?"; volumes.len()].join(",");
            format!(" AND f.volume_id IN ({ph})")
        };
        let sql = format!(
            "SELECT q.id, f.id, f.folder_id, f.volume_id, f.path_raw,
                    COALESCE(h.size_bytes, f.logical_bytes),
                    CASE WHEN h.file_id IS NULL THEN f.mtime_ns ELSE h.mtime_ns END,
                    CASE WHEN h.file_id IS NULL THEN f.ctime_ns ELSE h.ctime_ns END,
                    CASE WHEN h.file_id IS NULL THEN f.native_file_id ELSE h.native_file_id END,
                    h.hash
             FROM fila_hash q JOIN files f ON f.id = q.file_id
             LEFT JOIN hashes h ON h.file_id = f.id AND q.tipo = 'verificacao'
             WHERE q.tipo = ? AND q.state = 'pendente'{filtro}
             ORDER BY q.priority DESC, q.id LIMIT ?"
        );
        let mut st = conn.prepare_cached(&sql)?;
        let mut args: Vec<rusqlite::types::Value> = vec![tipo.como_str().to_owned().into()];
        args.extend(volumes.iter().map(|v| rusqlite::types::Value::from(*v)));
        args.push((limite as i64).into());
        let mut rows = st.query(rusqlite::params_from_iter(args))?;
        while let Some(l) = rows.next()? {
            itens.push(ItemFila {
                fila_id: l.get(0)?,
                file_id: l.get(1)?,
                pasta_id: l.get(2)?,
                volume_id: l.get(3)?,
                path_raw: l.get(4)?,
                tamanho: l.get(5)?,
                mtime_ns: l.get(6)?,
                ctime_ns: l.get(7)?,
                native_file_id: l.get(8)?,
                hash_guardado: l.get(9)?,
            });
        }
    }
    let mut up = conn.prepare_cached(
        "UPDATE fila_hash SET state = 'em_andamento', attempt_count = attempt_count + 1,
                updated_at = ?2 WHERE id = ?1",
    )?;
    for i in &itens {
        up.execute(params![i.fila_id, agora])?;
    }
    Ok(itens)
}

/// Volume com trabalho pendente na fila, com o Dispositivo Físico que o hospeda (se conhecido).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumePendente {
    pub volume_id: i64,
    pub dispositivo_id: Option<i64>,
    /// Nome para exibição do disco (rótulo, modelo ou identificador do dispositivo; sem
    /// dispositivo, o rótulo do volume ou `volume N`).
    pub nome_disco: String,
    pub arquivos: u64,
    pub bytes: u64,
}

/// Volumes com itens pendentes (`pendente`, `em_andamento`, `pausado`) de um tipo de fila, em
/// ordem de id do volume. Volumes sem dispositivo conhecido aparecem com `dispositivo_id = None`.
///
/// # Errors
/// Falha do SQLite.
pub fn volumes_pendentes(conn: &Connection, tipo: TipoFila) -> Resultado<Vec<VolumePendente>> {
    let mut st = conn.prepare(
        "SELECT f.volume_id, v.device_id,
                COALESCE(d.rotulo, d.modelo, d.identificador, v.rotulo, 'volume ' || f.volume_id),
                COUNT(*),
                COALESCE(SUM(CASE WHEN q.tipo = 'verificacao'
                    THEN (SELECT size_bytes FROM hashes WHERE file_id = f.id) ELSE f.logical_bytes END), 0)
         FROM fila_hash q JOIN files f ON f.id = q.file_id
         LEFT JOIN volumes v ON v.id = f.volume_id
         LEFT JOIN dispositivos d ON d.id = v.device_id
         WHERE q.tipo = ?1 AND q.state IN ('pendente','em_andamento','pausado')
         GROUP BY f.volume_id ORDER BY f.volume_id",
    )?;
    let linhas = st
        .query_map([tipo.como_str()], |l| {
            Ok(VolumePendente {
                volume_id: l.get(0)?,
                dispositivo_id: l.get(1)?,
                nome_disco: l.get(2)?,
                arquivos: l.get::<_, i64>(3)? as u64,
                bytes: l.get::<_, i64>(4)? as u64,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(linhas)
}

/// Andamento da fila de hash de um disco (para `dag status`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AndamentoDisco {
    /// Dispositivo físico; `None` para volumes sem dispositivo conhecido (um grupo por volume).
    pub dispositivo_id: Option<i64>,
    pub volume_ids: Vec<i64>,
    pub nome: String,
    pub arquivos_feitos: u64,
    pub arquivos_pendentes: u64,
    pub arquivos_com_erro: u64,
    pub bytes_feitos: u64,
    pub bytes_pendentes: u64,
}

/// Fotografia da fila de hash para o `status`: por disco e o instante da última atividade.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AndamentoFila {
    pub discos: Vec<AndamentoDisco>,
    /// Maior `updated_at` da fila (segundos Unix), se a fila não estiver vazia.
    pub ultima_atividade: Option<i64>,
    /// Bytes concluídos com `updated_at >= desde` (para a vazão recente).
    pub bytes_recentes: u64,
}

/// Lê o andamento da fila de `tipo`, agrupando os volumes por Dispositivo Físico. Só leitura.
///
/// # Errors
/// Falha do SQLite.
pub fn andamento_fila(conn: &Connection, tipo: TipoFila, desde: i64) -> Resultado<AndamentoFila> {
    let mut st = conn.prepare(
        "SELECT f.volume_id, v.device_id,
                COALESCE(d.rotulo, d.modelo, d.identificador, v.rotulo, 'volume ' || f.volume_id),
                q.state, COUNT(*), COALESCE(SUM(f.logical_bytes), 0),
                COALESCE(SUM(CASE WHEN q.updated_at >= ?2 THEN q.bytes_lidos ELSE 0 END), 0)
         FROM fila_hash q JOIN files f ON f.id = q.file_id
         LEFT JOIN volumes v ON v.id = f.volume_id
         LEFT JOIN dispositivos d ON d.id = v.device_id
         WHERE q.tipo = ?1 GROUP BY f.volume_id, q.state ORDER BY f.volume_id",
    )?;
    let mut rows = st.query(params![tipo.como_str(), desde])?;
    let mut discos: Vec<AndamentoDisco> = Vec::new();
    let mut recentes: u64 = 0;
    while let Some(l) = rows.next()? {
        let (vol, dev, nome, estado): (i64, Option<i64>, String, String) =
            (l.get(0)?, l.get(1)?, l.get(2)?, l.get(3)?);
        let (n, bytes, rec) = (
            l.get::<_, i64>(4)? as u64,
            l.get::<_, i64>(5)? as u64,
            l.get::<_, i64>(6)? as u64,
        );
        let pos = discos.iter().position(|d| match (dev, d.dispositivo_id) {
            (Some(a), Some(b)) => a == b,
            _ => dev.is_none() && d.dispositivo_id.is_none() && d.volume_ids == [vol],
        });
        let d = if let Some(i) = pos {
            &mut discos[i]
        } else {
            discos.push(AndamentoDisco {
                dispositivo_id: dev,
                nome,
                ..AndamentoDisco::default()
            });
            discos.last_mut().expect("acabou de inserir")
        };
        if !d.volume_ids.contains(&vol) {
            d.volume_ids.push(vol);
        }
        match estado.as_str() {
            "concluido" => {
                d.arquivos_feitos += n;
                d.bytes_feitos += bytes;
                recentes += rec;
            }
            "erro" => d.arquivos_com_erro += n,
            _ => {
                d.arquivos_pendentes += n;
                d.bytes_pendentes += bytes;
            }
        }
    }
    let ultima: Option<i64> = conn.query_row(
        "SELECT MAX(updated_at) FROM fila_hash WHERE tipo = ?1",
        [tipo.como_str()],
        |l| l.get(0),
    )?;
    Ok(AndamentoFila {
        discos,
        ultima_atividade: ultima,
        bytes_recentes: recentes,
    })
}

/// Devolve à fila (`pendente`) os itens reservados e não concluídos (retomada/cancelamento).
///
/// # Errors
/// Falha do SQLite.
pub fn liberar_reservas(conn: &Connection, tipo: TipoFila, agora: i64) -> Resultado<usize> {
    Ok(conn.execute(
        "UPDATE fila_hash SET state = 'pendente', updated_at = ?2
         WHERE tipo = ?1 AND state IN ('em_andamento','pausado')",
        params![tipo.como_str(), agora],
    )?)
}

/// Devolve um item específico à fila.
///
/// # Errors
/// Falha do SQLite.
pub fn devolver_item(conn: &Connection, fila_id: i64, agora: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE fila_hash SET state = 'pendente', updated_at = ?2 WHERE id = ?1",
        params![fila_id, agora],
    )?;
    Ok(())
}

/// Marca o item como falho (não grava hash).
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_erro(conn: &Connection, fila_id: i64, motivo: &str, agora: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE fila_hash SET state = 'erro', last_error = ?2, updated_at = ?3 WHERE id = ?1",
        params![fila_id, motivo, agora],
    )?;
    Ok(())
}

fn marcar_concluido(conn: &Connection, fila_id: i64, bytes: u64, agora: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE fila_hash SET state = 'concluido', bytes_lidos = ?2, last_error = NULL,
                updated_at = ?3 WHERE id = ?1",
        params![fila_id, bytes as i64, agora],
    )?;
    Ok(())
}

/// Arquivos que são o mesmo arquivo físico de `item` (inclusive ele): mesmo volume, `native_dev`
/// e `native_file_id`, `nlink > 1`, ativos, e com a mesma evidência (tamanho, mtime, ctime) do
/// inventário.
fn irmaos(conn: &Connection, file_id: i64) -> Resultado<Vec<i64>> {
    let mut st = conn.prepare_cached(
        "SELECT s.id FROM files f JOIN files s
           ON f.nlink > 1 AND f.native_file_id IS NOT NULL AND s.volume_id = f.volume_id
          AND s.native_dev IS f.native_dev AND s.native_file_id = f.native_file_id
          AND s.kind = 'arquivo' AND s.state = 'ativo' AND s.logical_bytes = f.logical_bytes
          AND s.mtime_ns IS f.mtime_ns AND s.ctime_ns IS f.ctime_ns
         WHERE f.id = ?1",
    )?;
    let mut ids: Vec<i64> = st
        .query_map([file_id], |l| l.get(0))?
        .collect::<Result<_, _>>()?;
    if !ids.contains(&file_id) {
        ids.push(file_id);
    }
    Ok(ids)
}

/// Grava o hash completo (só chamar depois da leitura inteira e da conferência de estabilidade).
/// Aplica a todos os caminhos do mesmo arquivo físico e conclui o item da fila. Devolve quantas
/// linhas de arquivo receberam o hash.
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_hash(
    conn: &Connection,
    item: &ItemFila,
    hash: &[u8; 32],
    bytes_lidos: u64,
    agora: i64,
) -> Resultado<u64> {
    let ids = irmaos(conn, item.file_id)?;
    let mut st = conn.prepare_cached(
        "INSERT OR REPLACE INTO hashes
            (file_id, algoritmo, hash, hashed_at, size_bytes, mtime_ns, ctime_ns, native_file_id)
         SELECT id, 'blake3', ?2, ?3, logical_bytes, mtime_ns, ctime_ns, native_file_id
         FROM files WHERE id = ?1",
    )?;
    let mut n = 0;
    for id in ids {
        n += st.execute(params![id, hash.as_slice(), agora])? as u64;
    }
    marcar_concluido(conn, item.fila_id, bytes_lidos, agora)?;
    Ok(n)
}

/// Registra o resultado da verificação para o arquivo e seus caminhos irmãos. O hash guardado
/// nunca é alterado. Devolve os `file_id` marcados.
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_verificacao(
    conn: &Connection,
    item: &ItemFila,
    divergente: bool,
    bytes_lidos: u64,
    agora: i64,
) -> Resultado<Vec<i64>> {
    let estado = if divergente { "divergente" } else { "ok" };
    let mut marcados = Vec::new();
    let mut st = conn.prepare_cached(
        "UPDATE hashes SET verified_at = ?2, verification_state = ?3
         WHERE file_id = ?1 AND size_bytes = ?4 AND mtime_ns IS ?5 AND ctime_ns IS ?6 AND hash = ?7",
    )?;
    for id in irmaos(conn, item.file_id)? {
        let n = st.execute(params![
            id,
            agora,
            estado,
            item.tamanho,
            item.mtime_ns,
            item.ctime_ns,
            item.hash_guardado
        ])?;
        if n > 0 {
            marcados.push(id);
        }
    }
    marcar_concluido(conn, item.fila_id, bytes_lidos, agora)?;
    Ok(marcados)
}

/// Fase atual da execução (`scan_runs.phase`) como texto.
///
/// # Errors
/// Falha do SQLite.
pub fn fase_da_execucao(conn: &Connection, run_id: i64) -> Resultado<Option<String>> {
    Ok(conn
        .query_row("SELECT phase FROM scan_runs WHERE id = ?1", [run_id], |l| {
            l.get(0)
        })
        .optional()?)
}

/// Define `scan_runs.phase` (`hash`, `verificacao`, `concluida`...), sem tocar no estado.
///
/// # Errors
/// Falha do SQLite (inclusive fase fora do conjunto permitido).
pub fn definir_fase(conn: &Connection, run_id: i64, fase: &str) -> Resultado<()> {
    conn.execute(
        "UPDATE scan_runs SET phase = ?2 WHERE id = ?1",
        params![run_id, fase],
    )?;
    Ok(())
}

/// Grava um bloco de contadores em `scan_runs.counters` sob a chave dada (JSON mesclado).
///
/// # Errors
/// Falha do SQLite.
pub fn salvar_contadores_etapa(
    conn: &Connection,
    run_id: i64,
    chave: &str,
    json: &str,
) -> Resultado<()> {
    conn.execute(
        "UPDATE scan_runs SET counters = json_set(CASE WHEN json_valid(counters) THEN counters ELSE '{}' END,
                '$.' || ?2, json(?3)) WHERE id = ?1",
        params![run_id, chave, json],
    )?;
    Ok(())
}

/// Hash guardado de um arquivo, se houver.
///
/// # Errors
/// Falha do SQLite.
pub fn hash_do_arquivo(conn: &Connection, file_id: i64) -> Resultado<Option<[u8; 32]>> {
    let v: Option<Vec<u8>> = conn
        .query_row(
            "SELECT hash FROM hashes WHERE file_id = ?1",
            [file_id],
            |l| l.get(0),
        )
        .optional()?;
    Ok(v.and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok()))
}
