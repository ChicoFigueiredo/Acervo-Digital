//! Contêineres (ADR 0006): fila persistente da etapa "Conteúdo de contêineres", Membros,
//! Senhas do Contêiner e Cofre de Senhas (só blobs já cifrados: a cifra e o keyring ficam fora do
//! Catálogo) e Fila de Serviço.
//!
//! Tudo recebe uma `&Connection`; o chamador decide a transação. Nada daqui grava em `files`,
//! `folders` ou `folder_stats`, e nenhuma consulta de espaço recuperável olha `membros`.

use rusqlite::{Connection, OptionalExtension, params};

use crate::Resultado;

/// Extensões que **enfileiram** um arquivo para a tentativa de leitura. É só triagem de custo: o
/// formato, de fato, é sempre decidido pelos bytes mágicos ao ler (um `.dat` com um zip dentro
/// não entra na fila; um `.zip` que não é zip vira `nao_conteiner`).
const EXTENSOES: &[&str] = &[
    "zip", "7z", "rar", "tar", "tgz", "tbz", "tbz2", "txz", "tzst", "gz", "bz2", "xz", "zst",
    "cbz", "cbr", "iso", "img", "raw", "vhdx", "qcow2",
];

/// O nome tem extensão de compactado que a etapa tenta ler (triagem da GUI).
#[must_use]
pub fn nome_de_conteiner(nome: &str) -> bool {
    let n = nome.to_lowercase();
    EXTENSOES
        .iter()
        .any(|e| n.strip_suffix(e).is_some_and(|r| r.ends_with('.')))
}

fn filtro_extensao(alias: &str) -> String {
    EXTENSOES
        .iter()
        .map(|e| format!("lower({alias}.name_display) LIKE '%.{e}'"))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Estado de um Contêiner na fila.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoConteiner {
    Pendente,
    Lendo,
    Lido,
    AguardandoSenha,
    AguardandoChaveiro,
    Falha,
    NaoConteiner,
}

impl EstadoConteiner {
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::Pendente => "pendente",
            Self::Lendo => "lendo",
            Self::Lido => "lido",
            Self::AguardandoSenha => "aguardando_senha",
            Self::AguardandoChaveiro => "aguardando_chaveiro",
            Self::Falha => "falha",
            Self::NaoConteiner => "nao_conteiner",
        }
    }
}

/// O que a montagem da fila fez.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResumoFilaConteineres {
    /// Candidatos novos ou com hash mudado que entraram na fila.
    pub enfileirados: u64,
    /// Contêineres que reaproveitaram os Membros de uma cópia idêntica já lida.
    pub copiados: u64,
}

/// Onde parou uma leitura retomável (Imagem de Disco): ver `retomada_*` na migração 0009.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retomada {
    /// Membros do nível raiz já gravados.
    pub indice: u64,
    /// Próxima `ordem` de Membro.
    pub proxima_ordem: u64,
    /// Bytes de conteúdo já lidos.
    pub bytes: u64,
}

/// Contêiner reservado para leitura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemConteiner {
    /// Cursor válido (mesmo Hash Completo) de uma leitura interrompida.
    pub retomada: Option<Retomada>,
    pub file_id: i64,
    pub volume_id: i64,
    pub path_raw: Vec<u8>,
    pub tamanho: i64,
    pub hash: [u8; 32],
}

/// Membro a gravar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NovoMembro {
    pub ordem: i64,
    pub pai_ordem: Option<i64>,
    pub caminho: Vec<u8>,
    pub tipo: &'static str,
    pub tamanho: Option<i64>,
    pub mtime_ns: Option<i64>,
    pub cifrado: bool,
    pub hash: Option<[u8; 32]>,
    /// `lido`, `cifrado`, `nao_suportado`, `aninhado_grande` ou `erro`.
    pub estado: &'static str,
    pub motivo: Option<String>,
    pub formato_interno: Option<&'static str>,
}

fn limpar(c: &mut Vec<u8>) {
    while c.last() == Some(&0) {
        c.pop();
    }
}

fn exibir(c: &[u8]) -> String {
    String::from_utf8_lossy(c).into_owned()
}

/// Monta/atualiza a fila: reinicia o que ficou `lendo`, enfileira candidatos novos e relê os
/// que mudaram de Hash Completo, e copia a lista de Membros de uma cópia idêntica já lida.
///
/// Só entram arquivos ativos com Hash Completo válido; o Hash Completo vem antes (ADR 0006).
///
/// # Errors
/// Falha do SQLite.
pub fn preparar_fila(conn: &Connection, agora: i64) -> Resultado<ResumoFilaConteineres> {
    let mut r = ResumoFilaConteineres::default();
    devolver_lendo(conn, agora)?;
    // Hash mudou: o que foi lido vale para o conteúdo antigo.
    conn.execute(
        "UPDATE conteineres SET estado = 'pendente', hash_lido = NULL, formato = NULL, membros = 0,
                bytes_lidos = 0, motivo = NULL, com_senha = 0, atualizado_em = ?1,
                retomada_indice = NULL, retomada_ordem = NULL, retomada_bytes = NULL, retomada_hash = NULL
         WHERE estado IN ('lido','nao_conteiner','falha','aguardando_senha','aguardando_chaveiro')
           AND file_id IN (SELECT c.file_id FROM conteineres c JOIN hashes h ON h.file_id = c.file_id
                           WHERE c.hash_lido IS NOT h.hash AND c.hash_lido IS NOT NULL)",
        [agora],
    )?;
    let ext = filtro_extensao("f");
    let n = conn.execute(
        &format!(
            "INSERT INTO conteineres (file_id, volume_id, estado, bytes_total, enfileirado_em, atualizado_em)
             SELECT f.id, f.volume_id, 'pendente', f.logical_bytes, ?1, ?1
             FROM files f JOIN hashes h ON h.file_id = f.id AND h.size_bytes = f.logical_bytes
             WHERE f.kind = 'arquivo' AND f.state = 'ativo' AND f.logical_bytes > 0
               AND ({ext})
               AND NOT EXISTS (SELECT 1 FROM conteineres c WHERE c.file_id = f.id)"
        ),
        [agora],
    )?;
    r.enfileirados = n as u64;
    // Cópias idênticas de um Contêiner já lido reaproveitam os Membros.
    let mut st = conn.prepare(
        "SELECT p.file_id, o.file_id FROM conteineres p
         JOIN hashes ph ON ph.file_id = p.file_id
         JOIN conteineres o ON o.estado = 'lido' AND o.hash_lido = ph.hash AND o.file_id <> p.file_id
         WHERE p.estado = 'pendente' AND p.hash_lido IS NULL
         GROUP BY p.file_id",
    )?;
    let pares: Vec<(i64, i64)> = st
        .query_map([], |l| Ok((l.get(0)?, l.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (copia, origem) in pares {
        copiar_membros(conn, origem, copia, agora)?;
        r.copiados += 1;
    }
    Ok(r)
}

/// Copia a lista de Membros e o estado `lido` de `origem` para `destino` (cópia idêntica).
///
/// # Errors
/// Falha do SQLite.
pub fn copiar_membros(conn: &Connection, origem: i64, destino: i64, agora: i64) -> Resultado<()> {
    conn.execute(
        "DELETE FROM membros WHERE conteiner_file_id = ?1",
        [destino],
    )?;
    conn.execute(
        "INSERT INTO membros (conteiner_file_id, ordem, pai_ordem, caminho_raw, caminho_display, tipo,
                              tamanho, mtime_ns, cifrado, hash, estado, motivo, formato_interno)
         SELECT ?2, ordem, pai_ordem, caminho_raw, caminho_display, tipo, tamanho, mtime_ns, cifrado,
                hash, estado, motivo, formato_interno
         FROM membros WHERE conteiner_file_id = ?1",
        params![origem, destino],
    )?;
    conn.execute(
        "UPDATE conteineres SET estado = 'lido', atualizado_em = ?3, motivo = NULL,
            (hash_lido, formato, membros, bytes_lidos, com_senha) =
            (SELECT hash_lido, formato, membros, bytes_lidos, com_senha FROM conteineres WHERE file_id = ?1)
         WHERE file_id = ?2",
        params![origem, destino, agora],
    )?;
    Ok(())
}

/// Depois de ler `origem`, copia os Membros para as cópias idênticas (mesmo Hash Completo) que
/// ainda esperam na fila. Devolve quantas.
///
/// # Errors
/// Falha do SQLite.
pub fn propagar_para_identicas(
    conn: &Connection,
    origem: i64,
    hash: &[u8; 32],
    agora: i64,
) -> Resultado<u64> {
    let mut st = conn.prepare(
        "SELECT c.file_id FROM conteineres c JOIN hashes h ON h.file_id = c.file_id
         WHERE c.estado = 'pendente' AND h.hash = ?1 AND c.file_id <> ?2",
    )?;
    let ids: Vec<i64> = st
        .query_map(params![hash.as_slice(), origem], |l| l.get(0))?
        .collect::<Result<_, _>>()?;
    for id in &ids {
        copiar_membros(conn, origem, *id, agora)?;
    }
    Ok(ids.len() as u64)
}

/// Reserva até `limite` Contêineres pendentes dos `volumes` (vazio = qualquer), com `file_id`
/// maior que `depois_de` e por ordem de id, marcando-os `lendo`. O cursor impede que um item
/// adiado (arquivo que mudou depois do hash) seja reservado de novo na mesma passada.
///
/// # Errors
/// Falha do SQLite.
pub fn reservar(
    conn: &Connection,
    volumes: &[i64],
    depois_de: i64,
    limite: usize,
    agora: i64,
) -> Resultado<Vec<ItemConteiner>> {
    let mut itens = Vec::new();
    let alvos: Vec<Option<i64>> = if volumes.is_empty() {
        vec![None]
    } else {
        volumes.iter().map(|v| Some(*v)).collect()
    };
    for alvo in alvos {
        let faltam = limite.saturating_sub(itens.len());
        if faltam == 0 {
            break;
        }
        let filtro = if alvo.is_some() {
            " AND c.volume_id = ?3"
        } else {
            ""
        };
        let sql = format!(
            "SELECT c.file_id, c.volume_id, f.path_raw, f.logical_bytes, h.hash,
                    CASE WHEN c.retomada_hash = h.hash THEN c.retomada_indice END,
                    c.retomada_ordem, c.retomada_bytes
             FROM conteineres c JOIN files f ON f.id = c.file_id JOIN hashes h ON h.file_id = f.id
             WHERE c.estado = 'pendente' AND c.file_id > ?2{filtro} ORDER BY c.file_id LIMIT ?1"
        );
        let mut st = conn.prepare_cached(&sql)?;
        let mapear = |l: &rusqlite::Row<'_>| -> rusqlite::Result<ItemConteiner> {
            let h: Vec<u8> = l.get(4)?;
            let indice: Option<i64> = l.get(5)?;
            let ordem: Option<i64> = l.get(6)?;
            let bytes: Option<i64> = l.get(7)?;
            Ok(ItemConteiner {
                retomada: match (indice, ordem, bytes) {
                    (Some(i), Some(o), Some(b)) => Some(Retomada {
                        indice: u64::try_from(i).unwrap_or(0),
                        proxima_ordem: u64::try_from(o).unwrap_or(0),
                        bytes: u64::try_from(b).unwrap_or(0),
                    }),
                    _ => None,
                },
                file_id: l.get(0)?,
                volume_id: l.get(1)?,
                path_raw: l.get(2)?,
                tamanho: l.get(3)?,
                hash: h.try_into().unwrap_or([0; 32]),
            })
        };
        let lim = i64::try_from(faltam).unwrap_or(i64::MAX);
        let linhas: Vec<ItemConteiner> = match alvo {
            Some(v) => st
                .query_map(params![lim, depois_de, v], mapear)?
                .collect::<Result<_, _>>()?,
            None => st
                .query_map(params![lim, depois_de], mapear)?
                .collect::<Result<_, _>>()?,
        };
        itens.extend(linhas);
    }
    let mut up = conn.prepare_cached(
        "UPDATE conteineres SET estado = 'lendo', atualizado_em = ?2,
                bytes_lidos = CASE WHEN ?3 THEN retomada_bytes ELSE 0 END
         WHERE file_id = ?1",
    )?;
    for i in &itens {
        if i.retomada.is_none() {
            // Sem cursor válido (ou com o hash mudado): lista parcial velha não vale.
            apagar_membros(conn, i.file_id)?;
        }
        up.execute(params![i.file_id, agora, i.retomada.is_some()])?;
    }
    Ok(itens)
}

/// Devolve à fila o que ficou `lendo` (cancelamento, queda), apagando os Membros parciais.
///
/// # Errors
/// Falha do SQLite.
pub fn devolver_lendo(conn: &Connection, agora: i64) -> Resultado<usize> {
    // Leitura retomável com cursor: os Membros gravados e o cursor foram commitados juntos,
    // então valem e a próxima leitura continua deles.
    conn.execute(
        "DELETE FROM membros WHERE conteiner_file_id IN
            (SELECT file_id FROM conteineres WHERE estado = 'lendo' AND retomada_indice IS NULL)",
        [],
    )?;
    Ok(conn.execute(
        "UPDATE conteineres SET estado = 'pendente', atualizado_em = ?1,
                bytes_lidos = COALESCE(retomada_bytes, 0)
         WHERE estado = 'lendo'",
        [agora],
    )?)
}

/// Volume com Contêineres a ler e o Dispositivo Físico dele.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeConteineres {
    pub volume_id: i64,
    pub dispositivo_id: Option<i64>,
    pub nome_disco: String,
    pub conteineres: u64,
    pub bytes: u64,
}

/// Volumes com Contêineres `pendente` ou `lendo`.
///
/// # Errors
/// Falha do SQLite.
pub fn volumes_pendentes(conn: &Connection) -> Resultado<Vec<VolumeConteineres>> {
    let mut st = conn.prepare(
        "SELECT c.volume_id, v.device_id,
                COALESCE(d.rotulo, d.modelo, d.identificador, v.rotulo, 'volume ' || c.volume_id),
                COUNT(*), COALESCE(SUM(c.bytes_total), 0)
         FROM conteineres c LEFT JOIN volumes v ON v.id = c.volume_id
         LEFT JOIN dispositivos d ON d.id = v.device_id
         WHERE c.estado IN ('pendente','lendo') GROUP BY c.volume_id ORDER BY c.volume_id",
    )?;
    let v = st
        .query_map([], |l| {
            Ok(VolumeConteineres {
                volume_id: l.get(0)?,
                dispositivo_id: l.get(1)?,
                nome_disco: l.get(2)?,
                conteineres: l.get::<_, i64>(3)? as u64,
                bytes: l.get::<_, i64>(4)? as u64,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// Apaga os Membros de um Contêiner (antes de ler de novo ou ao descartar uma leitura falha).
///
/// # Errors
/// Falha do SQLite.
pub fn apagar_membros(conn: &Connection, file_id: i64) -> Resultado<()> {
    conn.execute(
        "DELETE FROM membros WHERE conteiner_file_id = ?1",
        [file_id],
    )?;
    conn.execute(
        "UPDATE conteineres SET retomada_indice = NULL, retomada_ordem = NULL,
                retomada_bytes = NULL, retomada_hash = NULL WHERE file_id = ?1",
        [file_id],
    )?;
    Ok(())
}

/// Grava um lote de Membros **e** o ponto de retomada na mesma transação do chamador: os
/// Membros do cursor nunca divergem dele, nem numa queda de energia.
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_lote_retomavel(
    conn: &Connection,
    file_id: i64,
    membros: &[NovoMembro],
    ponto: Retomada,
    hash: &[u8; 32],
    agora: i64,
) -> Resultado<()> {
    inserir_membros(conn, file_id, membros)?;
    conn.execute(
        "UPDATE conteineres SET retomada_indice = ?2, retomada_ordem = ?3, retomada_bytes = ?4,
                retomada_hash = ?5, bytes_lidos = ?4, atualizado_em = ?6 WHERE file_id = ?1",
        params![
            file_id,
            i64::try_from(ponto.indice).unwrap_or(i64::MAX),
            i64::try_from(ponto.proxima_ordem).unwrap_or(i64::MAX),
            i64::try_from(ponto.bytes).unwrap_or(i64::MAX),
            hash.as_slice(),
            agora
        ],
    )?;
    Ok(())
}

/// A leitura interrompida tem cursor (a leitura continua dele; Membros e cursor ficam)?
///
/// # Errors
/// Falha do SQLite.
pub fn tem_retomada(conn: &Connection, file_id: i64) -> Resultado<bool> {
    Ok(conn.query_row(
        "SELECT retomada_indice IS NOT NULL FROM conteineres WHERE file_id = ?1",
        [file_id],
        |l| l.get::<_, i64>(0),
    )? != 0)
}

/// Devolve à fila, preservando Membros e cursor (pausa por cancelamento de leitura retomável).
///
/// # Errors
/// Falha do SQLite.
pub fn devolver_preservando(conn: &Connection, file_id: i64, agora: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE conteineres SET estado = 'pendente', atualizado_em = ?2,
                bytes_lidos = COALESCE(retomada_bytes, 0) WHERE file_id = ?1",
        params![file_id, agora],
    )?;
    Ok(())
}

/// Grava um lote de Membros do Contêiner.
///
/// # Errors
/// Falha do SQLite.
pub fn inserir_membros(conn: &Connection, file_id: i64, membros: &[NovoMembro]) -> Resultado<()> {
    let mut st = conn.prepare_cached(
        "INSERT INTO membros (conteiner_file_id, ordem, pai_ordem, caminho_raw, caminho_display, tipo,
                              tamanho, mtime_ns, cifrado, hash, estado, motivo, formato_interno)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )?;
    for m in membros {
        let mut c = m.caminho.clone();
        limpar(&mut c);
        st.execute(params![
            file_id,
            m.ordem,
            m.pai_ordem,
            c,
            exibir(&c),
            m.tipo,
            m.tamanho,
            m.mtime_ns,
            i64::from(m.cifrado),
            m.hash.as_ref().map(<[u8; 32]>::as_slice),
            m.estado,
            m.motivo,
            m.formato_interno,
        ])?;
    }
    Ok(())
}

/// Registra os bytes lidos até agora (progresso e batimento).
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_bytes(conn: &Connection, file_id: i64, bytes: i64, agora: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE conteineres SET bytes_lidos = ?2, atualizado_em = ?3 WHERE file_id = ?1",
        params![file_id, bytes, agora],
    )?;
    Ok(())
}

/// Conclui a leitura: Contêiner `lido`, Pedidos de Serviço do objeto resolvidos.
///
/// # Errors
/// Falha do SQLite.
#[allow(clippy::too_many_arguments)]
pub fn concluir(
    conn: &Connection,
    file_id: i64,
    hash: &[u8; 32],
    formato: &str,
    membros: i64,
    bytes: i64,
    com_senha: bool,
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "UPDATE conteineres SET estado = 'lido', hash_lido = ?2, formato = ?3, membros = ?4,
                bytes_lidos = ?5, com_senha = ?6, motivo = NULL, atualizado_em = ?7,
                retomada_indice = NULL, retomada_ordem = NULL, retomada_bytes = NULL, retomada_hash = NULL
         WHERE file_id = ?1",
        params![file_id, hash.as_slice(), formato, membros, bytes, i64::from(com_senha), agora],
    )?;
    resolver_pedidos_do_objeto(conn, file_id, agora)?;
    Ok(())
}

/// O arquivo não é um Contêiner (bytes mágicos desconhecidos).
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_nao_conteiner(
    conn: &Connection,
    file_id: i64,
    hash: &[u8; 32],
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "UPDATE conteineres SET estado = 'nao_conteiner', hash_lido = ?2, membros = 0, atualizado_em = ?3
         WHERE file_id = ?1",
        params![file_id, hash.as_slice(), agora],
    )?;
    Ok(())
}

/// Falha de leitura: estado `falha` e Pedido de Serviço `falha_leitura`.
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_falha(conn: &Connection, file_id: i64, motivo: &str, agora: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE conteineres SET estado = 'falha', motivo = ?2, atualizado_em = ?3 WHERE file_id = ?1",
        params![file_id, motivo, agora],
    )?;
    criar_pedido(conn, "falha_leitura", file_id, motivo, agora)?;
    Ok(())
}

/// Precisa de senha (`aguardando_senha`) ou o chaveiro está trancado (`aguardando_chaveiro`).
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_aguardando(
    conn: &Connection,
    file_id: i64,
    chaveiro: bool,
    motivo: &str,
    agora: i64,
) -> Resultado<()> {
    let estado = if chaveiro {
        EstadoConteiner::AguardandoChaveiro
    } else {
        EstadoConteiner::AguardandoSenha
    };
    conn.execute(
        "UPDATE conteineres SET estado = ?2, motivo = ?3, atualizado_em = ?4 WHERE file_id = ?1",
        params![file_id, estado.como_str(), motivo, agora],
    )?;
    criar_pedido(conn, "pedido_senha", file_id, motivo, agora)?;
    Ok(())
}

/// Devolve à fila os Contêineres que esperavam o chaveiro.
///
/// # Errors
/// Falha do SQLite.
pub fn reenfileirar_aguardando_chaveiro(conn: &Connection, agora: i64) -> Resultado<usize> {
    let n = conn.execute(
        "UPDATE conteineres SET estado = 'pendente', motivo = NULL, atualizado_em = ?1
         WHERE estado = 'aguardando_chaveiro'",
        [agora],
    )?;
    conn.execute(
        "UPDATE pedidos_servico SET estado = 'resolvido', resolvido_em = ?1
         WHERE estado = 'aberto' AND tipo = 'pedido_senha' AND file_id IN
           (SELECT file_id FROM conteineres WHERE estado = 'pendente')",
        [agora],
    )?;
    Ok(n)
}

/// Devolve à fila os Contêineres que esperavam senha (o Cofre ganhou uma senha nova).
///
/// # Errors
/// Falha do SQLite.
pub fn reenfileirar_aguardando_senha(conn: &Connection, agora: i64) -> Resultado<usize> {
    let n = conn.execute(
        "UPDATE conteineres SET estado = 'pendente', motivo = NULL, atualizado_em = ?1
         WHERE estado = 'aguardando_senha'",
        [agora],
    )?;
    conn.execute(
        "UPDATE pedidos_servico SET estado = 'resolvido', resolvido_em = ?1
         WHERE estado = 'aberto' AND tipo = 'pedido_senha' AND file_id IN
           (SELECT file_id FROM conteineres WHERE estado = 'pendente')",
        [agora],
    )?;
    Ok(n)
}

/// Reenfileira um Contêiner (e as cópias idênticas, pelo Hash Completo) para nova tentativa, por
/// exemplo depois de fornecida uma senha ou de "tentar de novo". Devolve quantos voltaram à fila.
///
/// # Errors
/// Falha do SQLite.
pub fn reenfileirar(conn: &Connection, file_id: i64, agora: i64) -> Resultado<usize> {
    let n = conn.execute(
        "UPDATE conteineres SET estado = 'pendente', motivo = NULL, hash_lido = NULL, membros = 0,
                bytes_lidos = 0, atualizado_em = ?2,
                retomada_indice = NULL, retomada_ordem = NULL, retomada_bytes = NULL, retomada_hash = NULL
         WHERE (estado = 'lido' AND file_id = ?1  -- tentar de novo numa imagem lida com avisos
                OR estado IN ('aguardando_senha','aguardando_chaveiro','falha')) AND file_id IN (
             SELECT ?1 UNION
             SELECT c.file_id FROM hashes h0 JOIN hashes h ON h.hash = h0.hash AND h.size_bytes = h0.size_bytes
                  JOIN conteineres c ON c.file_id = h.file_id WHERE h0.file_id = ?1)",
        params![file_id, agora],
    )?;
    conn.execute(
        "DELETE FROM membros WHERE conteiner_file_id IN (SELECT file_id FROM conteineres WHERE estado = 'pendente')
           AND conteiner_file_id IN (SELECT ?1 UNION SELECT c.file_id FROM hashes h0
                JOIN hashes h ON h.hash = h0.hash AND h.size_bytes = h0.size_bytes
                JOIN conteineres c ON c.file_id = h.file_id WHERE h0.file_id = ?1)",
        [file_id],
    )?;
    conn.execute(
        "UPDATE pedidos_servico SET estado = 'resolvido', resolvido_em = ?2
         WHERE estado = 'aberto' AND file_id IN (SELECT file_id FROM conteineres WHERE estado = 'pendente')",
        params![file_id, agora],
    )?;
    Ok(n)
}

/// Totais da etapa para o painel e o status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AndamentoConteineres {
    pub pendentes: u64,
    pub lendo: u64,
    pub lidos: u64,
    pub aguardando_senha: u64,
    pub aguardando_chaveiro: u64,
    pub falhas: u64,
    pub nao_conteineres: u64,
    /// Membros de todos os Contêineres lidos.
    pub membros: u64,
    pub bytes_lidos: u64,
    pub bytes_pendentes: u64,
}

/// Contagens por estado (um `GROUP BY` sobre `conteineres`, que é pequena em relação a `files`).
///
/// # Errors
/// Falha do SQLite.
pub fn andamento(conn: &Connection) -> Resultado<AndamentoConteineres> {
    let mut a = AndamentoConteineres::default();
    let mut st = conn.prepare(
        "SELECT estado, COUNT(*), COALESCE(SUM(membros),0), COALESCE(SUM(bytes_lidos),0),
                COALESCE(SUM(bytes_total),0) FROM conteineres GROUP BY estado",
    )?;
    let mut rows = st.query([])?;
    while let Some(l) = rows.next()? {
        let estado: String = l.get(0)?;
        let n = l.get::<_, i64>(1)? as u64;
        let membros = l.get::<_, i64>(2)? as u64;
        let lidos = l.get::<_, i64>(3)? as u64;
        let total = l.get::<_, i64>(4)? as u64;
        match estado.as_str() {
            "pendente" => {
                a.pendentes = n;
                a.bytes_pendentes += total;
            }
            "lendo" => {
                a.lendo = n;
                a.bytes_lidos += lidos;
                a.bytes_pendentes += total.saturating_sub(lidos);
            }
            "lido" => {
                a.lidos = n;
                a.membros = membros;
                a.bytes_lidos += lidos;
            }
            "aguardando_senha" => a.aguardando_senha = n,
            "aguardando_chaveiro" => a.aguardando_chaveiro = n,
            "falha" => a.falhas = n,
            "nao_conteiner" => a.nao_conteineres = n,
            _ => {}
        }
    }
    Ok(a)
}

/// Há Contêineres a ler (`pendente` ou `lendo`)?
///
/// # Errors
/// Falha do SQLite.
pub fn ha_pendentes(conn: &Connection) -> Resultado<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM conteineres WHERE estado IN ('pendente','lendo'))",
        [],
        |l| l.get::<_, i64>(0),
    )? != 0)
}

// ---------------------------------------------------------------- senhas e Cofre (só blobs)

/// Liga a senha cifrada ao Hash Completo do Contêiner.
///
/// # Errors
/// Falha do SQLite.
pub fn guardar_senha(
    conn: &Connection,
    hash: &[u8; 32],
    cifrada: &[u8],
    agora: i64,
) -> Resultado<()> {
    conn.execute(
        "INSERT INTO senhas_conteiner (hash, cifrada, criada_em) VALUES (?1, ?2, ?3)
         ON CONFLICT(hash) DO UPDATE SET cifrada = ?2, criada_em = ?3",
        params![hash.as_slice(), cifrada, agora],
    )?;
    Ok(())
}

/// Senha cifrada ligada ao hash, se houver.
///
/// # Errors
/// Falha do SQLite.
pub fn senha_cifrada(conn: &Connection, hash: &[u8; 32]) -> Resultado<Option<Vec<u8>>> {
    Ok(conn
        .query_row(
            "SELECT cifrada FROM senhas_conteiner WHERE hash = ?1",
            [hash.as_slice()],
            |l| l.get(0),
        )
        .optional()?)
}

/// Esquece a senha ligada ao hash.
///
/// # Errors
/// Falha do SQLite.
pub fn apagar_senha(conn: &Connection, hash: &[u8; 32]) -> Resultado<bool> {
    Ok(conn.execute(
        "DELETE FROM senhas_conteiner WHERE hash = ?1",
        [hash.as_slice()],
    )? > 0)
}

/// Inclui uma senha (já cifrada) no Cofre.
///
/// # Errors
/// Falha do SQLite.
pub fn cofre_adicionar(
    conn: &Connection,
    rotulo: &str,
    cifrada: &[u8],
    agora: i64,
) -> Resultado<i64> {
    conn.execute(
        "INSERT INTO cofre_senhas (rotulo, cifrada, criada_em) VALUES (?1, ?2, ?3)",
        params![rotulo, cifrada, agora],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Rótulos do Cofre (nunca os valores).
///
/// # Errors
/// Falha do SQLite.
pub fn cofre_rotulos(conn: &Connection) -> Resultado<Vec<(i64, String)>> {
    let mut st = conn.prepare("SELECT id, rotulo FROM cofre_senhas ORDER BY id")?;
    let v = st
        .query_map([], |l| Ok((l.get(0)?, l.get(1)?)))?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// Blobs cifrados do Cofre, em ordem de inclusão (só quem tem a chave decifra).
///
/// # Errors
/// Falha do SQLite.
pub fn cofre_cifradas(conn: &Connection) -> Resultado<Vec<(i64, Vec<u8>)>> {
    let mut st = conn.prepare("SELECT id, cifrada FROM cofre_senhas ORDER BY id")?;
    let v = st
        .query_map([], |l| Ok((l.get(0)?, l.get(1)?)))?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// Remove uma senha do Cofre.
///
/// # Errors
/// Falha do SQLite.
pub fn cofre_remover(conn: &Connection, id: i64) -> Resultado<bool> {
    Ok(conn.execute("DELETE FROM cofre_senhas WHERE id = ?1", [id])? > 0)
}

// ---------------------------------------------------------------- Fila de Serviço

/// Um Pedido de Serviço.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PedidoServico {
    pub id: i64,
    /// `pedido_senha` ou `falha_leitura`.
    pub tipo: String,
    pub file_id: Option<i64>,
    /// Caminho de exibição do arquivo objeto.
    pub objeto: Option<String>,
    pub motivo: String,
    /// `aberto`, `resolvido` ou `dispensado`.
    pub estado: String,
    pub criado_em: i64,
    pub resolvido_em: Option<i64>,
}

/// Abre um Pedido de Serviço (no máximo um aberto por tipo e objeto: repetir só atualiza o motivo).
///
/// # Errors
/// Falha do SQLite.
pub fn criar_pedido(
    conn: &Connection,
    tipo: &str,
    file_id: i64,
    motivo: &str,
    agora: i64,
) -> Resultado<i64> {
    conn.execute(
        "INSERT INTO pedidos_servico (tipo, file_id, motivo, criado_em) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(tipo, file_id) WHERE estado = 'aberto' DO UPDATE SET motivo = ?3",
        params![tipo, file_id, motivo, agora],
    )?;
    Ok(conn.query_row(
        "SELECT id FROM pedidos_servico WHERE tipo = ?1 AND file_id = ?2 AND estado = 'aberto'",
        params![tipo, file_id],
        |l| l.get(0),
    )?)
}

fn resolver_pedidos_do_objeto(conn: &Connection, file_id: i64, agora: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE pedidos_servico SET estado = 'resolvido', resolvido_em = ?2
         WHERE file_id = ?1 AND estado = 'aberto'",
        params![file_id, agora],
    )?;
    Ok(())
}

fn mapear_pedido(l: &rusqlite::Row<'_>) -> rusqlite::Result<PedidoServico> {
    Ok(PedidoServico {
        id: l.get(0)?,
        tipo: l.get(1)?,
        file_id: l.get(2)?,
        objeto: l.get(3)?,
        motivo: l.get(4)?,
        estado: l.get(5)?,
        criado_em: l.get(6)?,
        resolvido_em: l.get(7)?,
    })
}

const SELECT_PEDIDO: &str = "SELECT p.id, p.tipo, p.file_id, f.path_display, p.motivo, p.estado,
        p.criado_em, p.resolvido_em FROM pedidos_servico p LEFT JOIN files f ON f.id = p.file_id";

/// Lista pedidos (só os abertos, ou todos), do mais antigo ao mais novo.
///
/// # Errors
/// Falha do SQLite.
pub fn listar_pedidos(conn: &Connection, so_abertos: bool) -> Resultado<Vec<PedidoServico>> {
    let filtro = if so_abertos {
        " WHERE p.estado = 'aberto'"
    } else {
        ""
    };
    let mut st = conn.prepare(&format!("{SELECT_PEDIDO}{filtro} ORDER BY p.id"))?;
    let v = st.query_map([], mapear_pedido)?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// Um pedido pelo id.
///
/// # Errors
/// Falha do SQLite.
pub fn pedido(conn: &Connection, id: i64) -> Resultado<Option<PedidoServico>> {
    Ok(conn
        .query_row(
            &format!("{SELECT_PEDIDO} WHERE p.id = ?1"),
            [id],
            mapear_pedido,
        )
        .optional()?)
}

/// Quantos pedidos abertos (contador da GUI).
///
/// # Errors
/// Falha do SQLite.
pub fn contar_abertos(conn: &Connection) -> Resultado<u64> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pedidos_servico WHERE estado = 'aberto'",
        [],
        |l| l.get(0),
    )?;
    Ok(n as u64)
}

/// Muda o estado de um pedido aberto. Devolve se algo mudou.
///
/// # Errors
/// Falha do SQLite.
pub fn encerrar_pedido(conn: &Connection, id: i64, estado: &str, agora: i64) -> Resultado<bool> {
    Ok(conn.execute(
        "UPDATE pedidos_servico SET estado = ?2, resolvido_em = ?3 WHERE id = ?1 AND estado = 'aberto'",
        params![id, estado, agora],
    )? > 0)
}

// ---------------------------------------------------------------- consulta de Membros

/// Entrada de uma pasta virtual dentro de um Contêiner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntradaMembro {
    pub nome: Vec<u8>,
    /// Pasta (explícita ou implícita no caminho) ou Contêiner aninhado: dá para abrir.
    pub navegavel: bool,
    pub tamanho: u64,
    pub arquivos: u64,
    pub mtime_ns: Option<i64>,
    /// `ordem` do Membro aninhado que é Contêiner (para navegar para dentro dele).
    pub aninhado_ordem: Option<i64>,
    pub hash: Option<Vec<u8>>,
    pub estado: Option<String>,
}

/// Filhos diretos de `prefixo` (vazio ou terminado em `/`) no Contêiner `file_id`, no espaço de
/// nomes do Membro pai `pai_ordem` (`None` = raiz do Contêiner), em ordem de nome, a partir de
/// `depois` (exclusivo). O caminho do Membro é plano no formato; as pastas virtuais saem dos
/// segmentos.
///
/// # Errors
/// Falha do SQLite.
pub fn listar_filhos(
    conn: &Connection,
    file_id: i64,
    pai_ordem: Option<i64>,
    prefixo: &[u8],
    depois: Option<&[u8]>,
    limite: u32,
) -> Resultado<Vec<EntradaMembro>> {
    let plen = i64::try_from(prefixo.len()).unwrap_or(0);
    let superior: Option<Vec<u8>> = prefixo.split_last().map(|(u, resto)| {
        let mut v = resto.to_vec();
        v.push(u.saturating_add(1));
        v
    });
    let mut st = conn.prepare(
        "SELECT seg, MAX(dir), COALESCE(SUM(CASE WHEN tipo = 'arquivo' THEN tamanho END), 0),
                SUM(tipo = 'arquivo'), MAX(mtime_ns),
                MAX(CASE WHEN formato_interno IS NOT NULL AND NOT dir THEN ordem END),
                MAX(CASE WHEN NOT dir THEN hash END), MAX(CASE WHEN NOT dir THEN estado END)
         FROM (
            SELECT tipo, tamanho, mtime_ns, formato_interno, ordem, hash, estado,
                   CASE WHEN instr(rest, x'2f') = 0 THEN rest ELSE substr(rest, 1, instr(rest, x'2f') - 1) END AS seg,
                   (instr(rest, x'2f') > 0 OR tipo = 'pasta') AS dir
            FROM (SELECT tipo, tamanho, mtime_ns, formato_interno, ordem, hash, estado,
                         substr(caminho_raw, ?3 + 1) AS rest
                  FROM membros
                  WHERE conteiner_file_id = ?1 AND pai_ordem IS ?2
                    AND caminho_raw >= ?4 AND (?5 IS NULL OR caminho_raw < ?5))
            WHERE length(rest) > 0)
         WHERE length(seg) > 0 AND (?6 IS NULL OR seg > ?6)
         GROUP BY seg ORDER BY seg LIMIT ?7",
    )?;
    let v = st
        .query_map(
            params![
                file_id,
                pai_ordem,
                plen,
                prefixo,
                superior,
                depois,
                i64::from(limite)
            ],
            |l| {
                let dir: i64 = l.get(1)?;
                let aninhado: Option<i64> = l.get(5)?;
                Ok(EntradaMembro {
                    nome: l.get(0)?,
                    navegavel: dir != 0 || aninhado.is_some(),
                    tamanho: l.get::<_, i64>(2)?.max(0) as u64,
                    arquivos: l.get::<_, Option<i64>>(3)?.unwrap_or(0).max(0) as u64,
                    mtime_ns: l.get(4)?,
                    aninhado_ordem: aninhado,
                    hash: l.get(6)?,
                    estado: l.get(7)?,
                })
            },
        )?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// Contêiner com Membros lidos (selo e navegação do Explorador).
///
/// # Errors
/// Falha do SQLite.
pub fn estado_do_conteiner(
    conn: &Connection,
    file_id: i64,
) -> Resultado<Option<(String, Option<String>, i64)>> {
    Ok(conn
        .query_row(
            "SELECT estado, formato, membros FROM conteineres WHERE file_id = ?1",
            [file_id],
            |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)),
        )
        .optional()?)
}

/// Resolve um arquivo por caminho bruto no volume (para os comandos que recebem caminho).
///
/// # Errors
/// Falha do SQLite.
pub fn arquivo_por_caminho(conn: &Connection, path_raw: &[u8]) -> Resultado<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM files WHERE path_raw = ?1 AND state = 'ativo' ORDER BY id LIMIT 1",
            [path_raw],
            |l| l.get(0),
        )
        .optional()?)
}

/// Hash Completo de um arquivo, se houver.
///
/// # Errors
/// Falha do SQLite.
pub fn hash_do_arquivo(conn: &Connection, file_id: i64) -> Resultado<Option<[u8; 32]>> {
    Ok(conn
        .query_row(
            "SELECT hash FROM hashes WHERE file_id = ?1",
            [file_id],
            |l| l.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .and_then(|v| v.try_into().ok()))
}
