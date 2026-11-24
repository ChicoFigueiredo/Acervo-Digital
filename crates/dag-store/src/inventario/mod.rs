//! Escrita e consulta do inventário do Full Scan (A-03..A-06, A-17, A-19).
//!
//! Tudo aqui recebe uma `&Connection`: o chamador decide a transação (lotes curtos de 1–5 mil
//! linhas, via [`Armazem::transacao_imediata`](bancada_armazem::Armazem)). Gravações são
//! idempotentes por `(volume_id, path_raw)`, o que permite refazer a visita de um diretório
//! depois de uma queda sem duplicar linhas.
//!
//! Módulos: [`agregacao`] (folder_stats e totais por volume) e [`consulta`] (leitura paginada).

pub mod agregacao;
pub mod consulta;

use dag_core::{TipoDispositivo, VolumeDescoberto};
use rusqlite::{Connection, OptionalExtension, params};

use crate::Resultado;

// ------------------------------------------------------------------ exibição segura

/// Versão segura para exibir de bytes brutos: UTF-8 válido passa; bytes inválidos e caracteres de
/// controle viram `\xNN`. O valor bruto continua sendo a identidade (`*_raw`).
#[must_use]
pub fn exibir(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    fn empurrar(saida: &mut String, texto: &str) {
        for c in texto.chars() {
            if c.is_control() && c.is_ascii() {
                let _ = write!(saida, "\\x{:02x}", c as u32);
            } else {
                saida.push(c);
            }
        }
    }
    let mut saida = String::with_capacity(bytes.len());
    let mut resto = bytes;
    loop {
        match std::str::from_utf8(resto) {
            Ok(texto) => {
                empurrar(&mut saida, texto);
                break;
            }
            Err(e) => {
                let (bom, depois) = resto.split_at(e.valid_up_to());
                empurrar(&mut saida, &String::from_utf8_lossy(bom));
                let ruins = e.error_len().unwrap_or(depois.len()).min(depois.len());
                for b in &depois[..ruins] {
                    let _ = write!(saida, "\\x{b:02x}");
                }
                resto = &depois[ruins..];
                if resto.is_empty() {
                    break;
                }
            }
        }
    }
    saida
}

// ------------------------------------------------------------------ enums de estado

/// Estado de uma execução (`scan_runs.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoScan {
    EmAndamento,
    Pausada,
    Concluida,
    ConcluidaComErros,
    Cancelada,
    Falhou,
    Interrompida,
}

impl EstadoScan {
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::EmAndamento => "em_andamento",
            Self::Pausada => "pausada",
            Self::Concluida => "concluida",
            Self::ConcluidaComErros => "concluida_com_erros",
            Self::Cancelada => "cancelada",
            Self::Falhou => "falhou",
            Self::Interrompida => "interrompida",
        }
    }

    #[must_use]
    pub fn de_str(s: &str) -> Option<Self> {
        Some(match s {
            "em_andamento" => Self::EmAndamento,
            "pausada" => Self::Pausada,
            "concluida" => Self::Concluida,
            "concluida_com_erros" => Self::ConcluidaComErros,
            "cancelada" => Self::Cancelada,
            "falhou" => Self::Falhou,
            "interrompida" => Self::Interrompida,
            _ => return None,
        })
    }

    /// Estados em que a execução terminou (tem `ended_at`).
    #[must_use]
    pub const fn terminal(self) -> bool {
        !matches!(self, Self::EmAndamento | Self::Pausada)
    }

    /// Estados a partir dos quais [`FaseScan::Inventario`] pode ser retomado.
    #[must_use]
    pub const fn retomavel(self) -> bool {
        matches!(
            self,
            Self::EmAndamento | Self::Pausada | Self::Cancelada | Self::Interrompida | Self::Falhou
        )
    }
}

/// Fase persistida (`scan_runs.phase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaseScan {
    Inventario,
    Estatisticas,
    /// Hash Completo em andamento ou pendente (gravada por `EtapaHash`; o estado da execução
    /// continua o do inventário, `concluida`).
    Hash,
    Verificacao,
    Concluida,
}

impl FaseScan {
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::Inventario => "inventario",
            Self::Estatisticas => "estatisticas",
            Self::Hash => "hash",
            Self::Verificacao => "verificacao",
            Self::Concluida => "concluida",
        }
    }

    #[must_use]
    pub fn de_str(s: &str) -> Option<Self> {
        Some(match s {
            "inventario" => Self::Inventario,
            "estatisticas" => Self::Estatisticas,
            "hash" => Self::Hash,
            "verificacao" => Self::Verificacao,
            "concluida" => Self::Concluida,
            _ => return None,
        })
    }
}

/// Severidade de `scan_errors`. `Info` = item ignorado de propósito (não torna agregado parcial).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severidade {
    Info,
    Aviso,
    Erro,
    Impeditivo,
}

impl Severidade {
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Aviso => "aviso",
            Self::Erro => "erro",
            Self::Impeditivo => "impeditivo",
        }
    }
}

/// Tipo de entrada gravada em `files.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipoEntrada {
    Arquivo,
    Symlink,
    Fifo,
    Socket,
    DispositivoBloco,
    DispositivoChar,
    Outro,
}

impl TipoEntrada {
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::Arquivo => "arquivo",
            Self::Symlink => "symlink",
            Self::Fifo => "fifo",
            Self::Socket => "socket",
            Self::DispositivoBloco => "dispositivo_bloco",
            Self::DispositivoChar => "dispositivo_char",
            Self::Outro => "outro",
        }
    }
}

// ------------------------------------------------------------------ dispositivos e volumes

const fn tipo_dispositivo(t: TipoDispositivo) -> &'static str {
    match t {
        TipoDispositivo::Desconhecido => "desconhecido",
        TipoDispositivo::Hdd => "hdd",
        TipoDispositivo::Ssd => "ssd",
        TipoDispositivo::Nvme => "nvme",
        TipoDispositivo::Usb => "usb",
        TipoDispositivo::Rede => "rede",
        TipoDispositivo::Virtual => "virtual",
        TipoDispositivo::Outro => "outro",
    }
}

/// Grava (ou atualiza) dispositivo, volume e pontos de montagem de um volume descoberto e o
/// marca como montado. Devolve o `volumes.id`. Idempotente.
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_volume(conn: &Connection, v: &VolumeDescoberto, agora: i64) -> Resultado<i64> {
    let device_id: Option<i64> = match &v.dispositivo {
        Some(d) => Some(conn.query_row(
            "INSERT INTO dispositivos (identificador, modelo, tipo, criado_em, visto_em)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(identificador) DO UPDATE SET
                 modelo = COALESCE(excluded.modelo, dispositivos.modelo),
                 tipo = excluded.tipo, visto_em = excluded.visto_em
             RETURNING id",
            params![d.identificador, d.modelo, tipo_dispositivo(d.tipo), agora],
            |l| l.get(0),
        )?),
        None => None,
    };
    let flags = serde_json::json!({
        "st_dev": v.st_dev,
        "pseudo": v.pseudo,
        "removivel": v.dispositivo.as_ref().map(|d| d.removivel),
    })
    .to_string();
    let volume_id: i64 = conn.query_row(
        "INSERT INTO volumes (platform_id, device_id, rotulo, fs_type, capacity_bytes, free_bytes,
                              mounted, last_seen_at, flags)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8)
         ON CONFLICT(platform_id) DO UPDATE SET
             device_id = COALESCE(excluded.device_id, volumes.device_id),
             rotulo = excluded.rotulo, fs_type = excluded.fs_type,
             capacity_bytes = excluded.capacity_bytes, free_bytes = excluded.free_bytes,
             mounted = 1, last_seen_at = excluded.last_seen_at, flags = excluded.flags
         RETURNING id",
        params![
            v.platform_id,
            device_id,
            v.rotulo,
            v.fs_type,
            v.capacidade_bytes.map(|x| x as i64),
            v.livre_bytes.map(|x| x as i64),
            agora,
            flags
        ],
        |l| l.get(0),
    )?;
    let mut mp = conn.prepare_cached(
        "INSERT INTO mount_points (volume_id, path_raw, path_display, mounted_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(volume_id, path_raw) DO UPDATE SET mounted_at = excluded.mounted_at",
    )?;
    for ponto in &v.pontos_de_montagem {
        mp.execute(params![volume_id, ponto, exibir(ponto), agora])?;
    }
    Ok(volume_id)
}

// ------------------------------------------------------------------ execuções

/// Linha de `scan_runs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Execucao {
    pub id: i64,
    pub modo: String,
    pub raizes: String,
    pub iniciada_em: i64,
    pub encerrada_em: Option<i64>,
    pub estado: EstadoScan,
    pub fase: FaseScan,
    pub contadores: String,
    pub checkpoint: Option<String>,
    /// Flag "continuar de onde parou": a execução foi detectada como interrompida e a retomada
    /// automática está ligada (zera ao terminar ou ao ser parada de propósito).
    pub retomada_automatica: bool,
}

/// Cria uma execução `em_andamento` na fase `inventario`.
///
/// # Errors
/// Falha do SQLite.
pub fn criar_execucao(
    conn: &Connection,
    modo: &str,
    raizes_json: &str,
    checkpoint: Option<&str>,
    agora: i64,
) -> Resultado<i64> {
    conn.execute(
        "INSERT INTO scan_runs (mode, roots, started_at, checkpoint) VALUES (?1, ?2, ?3, ?4)",
        params![modo, raizes_json, agora, checkpoint],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Lê uma execução.
///
/// # Errors
/// Falha do SQLite.
pub fn obter_execucao(conn: &Connection, id: i64) -> Resultado<Option<Execucao>> {
    Ok(conn
        .query_row(
            "SELECT id, mode, roots, started_at, ended_at, status, phase, counters, checkpoint,
                    retomada_automatica
             FROM scan_runs WHERE id = ?1",
            [id],
            |l| {
                let estado: String = l.get(5)?;
                let fase: String = l.get(6)?;
                Ok(Execucao {
                    id: l.get(0)?,
                    modo: l.get(1)?,
                    raizes: l.get(2)?,
                    iniciada_em: l.get(3)?,
                    encerrada_em: l.get(4)?,
                    estado: EstadoScan::de_str(&estado).unwrap_or(EstadoScan::Falhou),
                    fase: FaseScan::de_str(&fase).unwrap_or(FaseScan::Inventario),
                    contadores: l.get(7)?,
                    checkpoint: l.get(8)?,
                    retomada_automatica: l.get::<_, i64>(9)? != 0,
                })
            },
        )
        .optional()?)
}

/// Muda o estado (e opcionalmente a fase). Estados terminais gravam `ended_at`; os demais o zeram.
///
/// # Errors
/// Falha do SQLite.
pub fn definir_estado(
    conn: &Connection,
    id: i64,
    estado: EstadoScan,
    fase: Option<FaseScan>,
    agora: i64,
) -> Resultado<()> {
    let fim = estado.terminal().then_some(agora);
    conn.execute(
        "UPDATE scan_runs SET status = ?2, phase = COALESCE(?3, phase), ended_at = ?4 WHERE id = ?1",
        params![id, estado.como_str(), fase.map(FaseScan::como_str), fim],
    )?;
    Ok(())
}

/// Grava os contadores (JSON) da execução.
///
/// # Errors
/// Falha do SQLite.
pub fn salvar_contadores(conn: &Connection, id: i64, json: &str) -> Resultado<()> {
    conn.execute(
        "UPDATE scan_runs SET counters = ?2 WHERE id = ?1",
        params![id, json],
    )?;
    Ok(())
}

/// Ao iniciar o serviço: execuções que ficaram `em_andamento` (processo morto) viram
/// `interrompida`, retomáveis com a fila intacta. Devolve quantas.
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_interrompidas(conn: &Connection, agora: i64) -> Resultado<usize> {
    Ok(conn.execute(
        "UPDATE scan_runs SET status = 'interrompida', ended_at = ?1
         WHERE status IN ('em_andamento','pausada')",
        [agora],
    )?)
}

/// Execução deixada por um processo morto, detectada na abertura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecucaoInterrompida {
    pub id: i64,
    pub modo: String,
    pub raizes: Vec<String>,
    pub fase: FaseScan,
    /// Estava no meio do inventário (fase `inventario`/`estatisticas`), não no hash.
    pub no_inventario: bool,
}

/// Detecta as Execuções Interrompidas. Quem chama já tem a trava de escritor, então nenhum outro
/// processo está rodando: tudo que parece em andamento foi deixado por um processo morto.
///
/// * `em_andamento`/`pausada` viram `interrompida` (qualquer modo; só `full` e `fast` são
///   devolvidas para retomar);
/// * a etapa de hash que parou (execução `concluida` na fase `hash` com fila pendente, sem
///   `parada_deliberada`) é devolvida sem mudar o estado, porque o inventário já terminou.
///
/// Com `retomar_automaticamente` grava a flag `retomada_automatica` nas devolvidas. Idempotente.
///
/// # Errors
/// Falha do SQLite.
pub fn detectar_interrompidas(
    conn: &Connection,
    retomar_automaticamente: bool,
    agora: i64,
) -> Resultado<Vec<ExecucaoInterrompida>> {
    let ler = |sql: &str| -> Resultado<Vec<(i64, String, String, String)>> {
        let mut st = conn.prepare(sql)?;
        let v = st
            .query_map([], |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?, l.get(3)?)))?
            .collect::<Result<_, _>>()?;
        Ok(v)
    };
    let mut saida = Vec::new();
    // Já interrompidas antes e ainda não retomadas (a queda aconteceu de novo durante a retomada).
    let mut candidatas: Vec<(i64, String, String, String, bool)> =
        ler("SELECT id, mode, roots, phase FROM scan_runs
         WHERE status IN ('em_andamento','pausada') ORDER BY id")?
        .into_iter()
        .map(|(i, m, r, f)| (i, m, r, f, true))
        .collect();
    candidatas.extend(
        ler("SELECT id, mode, roots, phase FROM scan_runs
             WHERE status = 'interrompida' AND retomada_automatica = 1 ORDER BY id")?
        .into_iter()
        .map(|(i, m, r, f)| (i, m, r, f, true)),
    );
    // Hash parado: a execução terminou o inventário mas a etapa de hash nunca chegou à fase
    // final (`concluida`). Vale com ou sem linhas na fila persistente: a fila é reconstruída a
    // partir dos arquivos sem hash válido (estado deixado pelo binário antigo, fila esvaziada,
    // queda de energia). Parada deliberada (usuário cancelou o hash) não conta.
    let hash_parado: Option<(i64, String, String, String)> = conn
        .query_row(
            "SELECT id, mode, roots, phase FROM scan_runs
             WHERE phase = 'hash' AND status IN ('concluida','concluida_com_erros')
               AND mode IN ('full','fast') AND parada_deliberada = 0
             ORDER BY id DESC LIMIT 1",
            [],
            |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?, l.get(3)?)),
        )
        .optional()?;
    for (id, modo, raizes, fase, _) in candidatas {
        // Na fase `hash` (a etapa de hash deixa a execução `em_andamento`) o inventário já
        // terminou: a retomada continua só o hash.
        let no_inventario = !matches!(fase.as_str(), "hash" | "verificacao" | "concluida");
        conn.execute(
            "UPDATE scan_runs SET status = 'interrompida', ended_at = COALESCE(ended_at, ?2)
             WHERE id = ?1",
            params![id, agora],
        )?;
        if modo != "full" && modo != "fast" {
            continue;
        }
        saida.push(ExecucaoInterrompida {
            id,
            modo,
            raizes: serde_json::from_str(&raizes).unwrap_or_default(),
            fase: FaseScan::de_str(&fase).unwrap_or(FaseScan::Inventario),
            no_inventario,
        });
    }
    if let Some((id, modo, raizes, fase)) = hash_parado {
        saida.push(ExecucaoInterrompida {
            id,
            modo,
            raizes: serde_json::from_str(&raizes).unwrap_or_default(),
            fase: FaseScan::de_str(&fase).unwrap_or(FaseScan::Hash),
            no_inventario: false,
        });
    }
    for e in &saida {
        conn.execute(
            "UPDATE scan_runs SET retomada_automatica = ?2 WHERE id = ?1",
            params![e.id, i64::from(retomar_automaticamente)],
        )?;
    }
    Ok(saida)
}

/// Zera a flag "continuar de onde parou" (a execução terminou ou foi parada de propósito).
///
/// # Errors
/// Falha do SQLite.
pub fn limpar_retomada_automatica(conn: &Connection, id: i64) -> Resultado<()> {
    conn.execute(
        "UPDATE scan_runs SET retomada_automatica = 0 WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

/// Contadores exatos recalculados a partir das tabelas (não dependem de passes repetidos).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContadoresDb {
    pub pastas: u64,
    pub arquivos: u64,
    pub especiais: u64,
    pub bytes_logicos: u64,
    pub ignorados: u64,
    /// Problemas de severidade `aviso` ou acima.
    pub erros: u64,
}

impl ContadoresDb {
    /// JSON gravado em `scan_runs.counters`.
    #[must_use]
    pub fn json(&self) -> String {
        serde_json::json!({
            "pastas": self.pastas, "arquivos": self.arquivos, "especiais": self.especiais,
            "bytes_logicos": self.bytes_logicos, "ignorados": self.ignorados, "erros": self.erros,
        })
        .to_string()
    }
}

/// Recalcula os contadores de uma execução a partir de `folders`, `files` e `scan_errors`.
///
/// # Errors
/// Falha do SQLite.
pub fn contadores_da_execucao(conn: &Connection, run_id: i64) -> Resultado<ContadoresDb> {
    let n = |sql: &str| -> Resultado<u64> {
        Ok(conn.query_row(sql, [run_id], |l| l.get::<_, i64>(0))? as u64)
    };
    let (arquivos, especiais, bytes): (i64, i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(kind = 'arquivo'), 0), COALESCE(SUM(kind <> 'arquivo'), 0),
                COALESCE(SUM(logical_bytes), 0) FROM files WHERE seen_run_id = ?1",
        [run_id],
        |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)),
    )?;
    Ok(ContadoresDb {
        pastas: n("SELECT COUNT(*) FROM folders WHERE seen_run_id = ?1")?,
        arquivos: arquivos as u64,
        especiais: especiais as u64,
        bytes_logicos: bytes as u64,
        ignorados: n("SELECT COUNT(*) FROM scan_errors WHERE run_id = ?1 AND severity = 'info'")?,
        erros: n("SELECT COUNT(*) FROM scan_errors WHERE run_id = ?1 AND severity <> 'info'")?,
    })
}

// ------------------------------------------------------------------ pastas e arquivos

/// Pasta a gravar (upsert por `(volume_id, path_raw)`).
#[derive(Debug, Clone, Copy)]
pub struct NovaPasta<'a> {
    pub volume_id: i64,
    pub pai: Option<i64>,
    pub path_raw: &'a [u8],
    pub name_raw: &'a [u8],
    pub dev: u64,
    pub inode: u64,
    pub attrs: &'a str,
    pub mtime_ns: Option<i64>,
    pub run_id: i64,
}

/// Grava a pasta e devolve o `folders.id` (o mesmo se já existia).
///
/// # Errors
/// Falha do SQLite.
pub fn upsert_pasta(conn: &Connection, p: &NovaPasta<'_>) -> Resultado<i64> {
    let mut st = conn.prepare_cached(
        "INSERT INTO folders (volume_id, parent_folder_id, path_raw, path_display, name_raw,
                              name_display, native_dev, native_file_id, attrs, mtime_ns, seen_run_id, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'ativo')
         ON CONFLICT(volume_id, path_raw) DO UPDATE SET
             parent_folder_id = excluded.parent_folder_id, native_dev = excluded.native_dev,
             native_file_id = excluded.native_file_id, attrs = excluded.attrs,
             mtime_ns = excluded.mtime_ns, seen_run_id = excluded.seen_run_id, state = 'ativo'
         RETURNING id",
    )?;
    Ok(st.query_row(
        params![
            p.volume_id,
            p.pai,
            p.path_raw,
            exibir(p.path_raw),
            p.name_raw,
            exibir(p.name_raw),
            p.dev as i64,
            p.inode as i64,
            p.attrs,
            p.mtime_ns,
            p.run_id
        ],
        |l| l.get(0),
    )?)
}

/// Arquivo (ou symlink/especial) a gravar (upsert por `(volume_id, path_raw)`).
#[derive(Debug, Clone, Copy)]
pub struct NovoArquivo<'a> {
    pub volume_id: i64,
    pub pasta_id: i64,
    pub path_raw: &'a [u8],
    pub name_raw: &'a [u8],
    pub tipo: TipoEntrada,
    pub dev: u64,
    pub inode: u64,
    pub nlink: u64,
    pub bytes_logicos: u64,
    /// `None` = o sistema não informou alocação (nunca zero inventado).
    pub bytes_alocados: Option<u64>,
    pub mtime_ns: Option<i64>,
    pub ctime_ns: Option<i64>,
    pub birthtime_ns: Option<i64>,
    pub attrs: &'a str,
    pub alvo_symlink: Option<&'a [u8]>,
    pub mime_id: Option<i64>,
    pub run_id: i64,
}

/// Grava a entrada de arquivo.
///
/// # Errors
/// Falha do SQLite.
pub fn upsert_arquivo(conn: &Connection, a: &NovoArquivo<'_>) -> Resultado<()> {
    let mut st = conn.prepare_cached(
        "INSERT INTO files (volume_id, folder_id, path_raw, path_display, name_raw, name_display,
                            native_dev, native_file_id, nlink, logical_bytes, allocated_bytes,
                            mtime_ns, ctime_ns, birthtime_ns, attrs, kind, link_target_raw,
                            link_target_display, mime_id, seen_run_id, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                 ?19, ?20, 'ativo')
         ON CONFLICT(volume_id, path_raw) DO UPDATE SET
             folder_id = excluded.folder_id, native_dev = excluded.native_dev,
             native_file_id = excluded.native_file_id, nlink = excluded.nlink,
             logical_bytes = excluded.logical_bytes, allocated_bytes = excluded.allocated_bytes,
             mtime_ns = excluded.mtime_ns, ctime_ns = excluded.ctime_ns,
             birthtime_ns = excluded.birthtime_ns, attrs = excluded.attrs, kind = excluded.kind,
             link_target_raw = excluded.link_target_raw,
             link_target_display = excluded.link_target_display,
             mime_id = COALESCE(excluded.mime_id, files.mime_id),
             hardlink_group = NULL, seen_run_id = excluded.seen_run_id, state = 'ativo'",
    )?;
    st.execute(params![
        a.volume_id,
        a.pasta_id,
        a.path_raw,
        exibir(a.path_raw),
        a.name_raw,
        exibir(a.name_raw),
        a.dev as i64,
        a.inode as i64,
        a.nlink.max(1) as i64,
        a.bytes_logicos as i64,
        a.bytes_alocados.map(|x| x as i64),
        a.mtime_ns,
        a.ctime_ns,
        a.birthtime_ns,
        a.attrs,
        a.tipo.como_str(),
        a.alvo_symlink,
        a.alvo_symlink.map(exibir),
        a.mime_id,
        a.run_id
    ])?;
    Ok(())
}

/// Grava (ou reaproveita) um tipo de arquivo identificado por extensão/MIME.
///
/// # Errors
/// Falha do SQLite.
#[allow(clippy::too_many_arguments)]
pub fn obter_tipo_de_arquivo(
    conn: &Connection,
    extensao: Option<&str>,
    mime: Option<&str>,
    descricao: Option<&str>,
    categoria: Option<&str>,
    fonte: &str,
    confianca: Option<f64>,
) -> Resultado<i64> {
    let existente: Option<i64> = conn
        .query_row(
            "SELECT id FROM file_types WHERE extension IS ?1 AND mime IS ?2 AND evidence_source = ?3",
            params![extensao, mime, fonte],
            |l| l.get(0),
        )
        .optional()?;
    if let Some(id) = existente {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO file_types (extension, mime, human_description, broad_category, evidence_source, confidence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![extensao, mime, descricao, categoria, fonte, confianca],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Já existe, nesta execução, outra pasta com o mesmo `(dev, inode)` em caminho diferente?
/// (bind mount ou alias de diretório: não é varrida de novo, evitando ciclo e contagem dupla.)
///
/// # Errors
/// Falha do SQLite.
pub fn pasta_ja_vista_por_outro_caminho(
    conn: &Connection,
    volume_id: i64,
    dev: u64,
    inode: u64,
    run_id: i64,
    path_raw: &[u8],
) -> Resultado<bool> {
    let mut st = conn.prepare_cached(
        "SELECT 1 FROM folders WHERE volume_id = ?1 AND native_dev = ?2 AND native_file_id = ?3
           AND seen_run_id = ?4 AND path_raw <> ?5 LIMIT 1",
    )?;
    Ok(st
        .query_row(
            params![volume_id, dev as i64, inode as i64, run_id, path_raw],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

// ------------------------------------------------------------------ fila persistente (checkpoint)

/// Diretório reservado para visita.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiretorioPendente {
    pub fila_id: i64,
    pub pasta_id: i64,
    pub path_raw: Vec<u8>,
    pub volume_id: i64,
    pub dev: u64,
}

/// Como terminou a visita de um diretório.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FimDiretorio {
    Concluido,
    Falhou(String),
    /// Visita abortada (cancelamento): volta para a fila.
    Liberar,
}

/// Enfileira um diretório (idempotente; não reabre diretório já concluído). `true` se inseriu.
///
/// # Errors
/// Falha do SQLite.
pub fn enfileirar_diretorio(
    conn: &Connection,
    run_id: i64,
    pasta_id: i64,
    path_raw: &[u8],
    agora: i64,
) -> Resultado<bool> {
    let mut st = conn.prepare_cached(
        "INSERT OR IGNORE INTO scan_queue (run_id, directory_id, path_raw, phase, lease_state, scheduled_at)
         VALUES (?1, ?2, ?3, 'inventario', 'livre', ?4)",
    )?;
    Ok(st.execute(params![run_id, pasta_id, path_raw, agora])? > 0)
}

/// Reserva até `limite` diretórios livres (os mais recentes primeiro, o que mantém a fronteira
/// pequena) e os marca `reservada`.
///
/// # Errors
/// Falha do SQLite.
pub fn reservar_diretorios(
    conn: &Connection,
    run_id: i64,
    limite: usize,
) -> Resultado<Vec<DiretorioPendente>> {
    let itens: Vec<DiretorioPendente> = {
        let mut st = conn.prepare_cached(
            "SELECT q.id, q.directory_id, q.path_raw, f.volume_id, f.native_dev
             FROM scan_queue q JOIN folders f ON f.id = q.directory_id
             WHERE q.run_id = ?1 AND q.phase = 'inventario' AND q.lease_state = 'livre'
             ORDER BY q.id DESC LIMIT ?2",
        )?;
        st.query_map(params![run_id, limite as i64], |l| {
            Ok(DiretorioPendente {
                fila_id: l.get(0)?,
                pasta_id: l.get(1)?,
                path_raw: l.get(2)?,
                volume_id: l.get(3)?,
                dev: l.get::<_, i64>(4)? as u64,
            })
        })?
        .collect::<Result<_, _>>()?
    };
    let mut up = conn.prepare_cached(
        "UPDATE scan_queue SET lease_state = 'reservada', attempt_count = attempt_count + 1 WHERE id = ?1",
    )?;
    for d in &itens {
        up.execute([d.fila_id])?;
    }
    Ok(itens)
}

/// Registra o fim da visita de um diretório da fila.
///
/// # Errors
/// Falha do SQLite.
pub fn finalizar_diretorio(conn: &Connection, fila_id: i64, fim: &FimDiretorio) -> Resultado<()> {
    match fim {
        FimDiretorio::Concluido => conn
            .prepare_cached(
                "UPDATE scan_queue SET lease_state = 'concluida', last_error = NULL WHERE id = ?1",
            )?
            .execute([fila_id])?,
        FimDiretorio::Falhou(msg) => conn
            .prepare_cached(
                "UPDATE scan_queue SET lease_state = 'falhou', last_error = ?2 WHERE id = ?1",
            )?
            .execute(params![fila_id, msg])?,
        FimDiretorio::Liberar => conn
            .prepare_cached("UPDATE scan_queue SET lease_state = 'livre' WHERE id = ?1")?
            .execute([fila_id])?,
    };
    Ok(())
}

/// Devolve à fila tudo que estava `reservada` (cancelamento, pausa longa ou retomada após queda).
///
/// # Errors
/// Falha do SQLite.
pub fn liberar_reservas(conn: &Connection, run_id: i64) -> Resultado<usize> {
    Ok(conn.execute(
        "UPDATE scan_queue SET lease_state = 'livre'
         WHERE run_id = ?1 AND phase = 'inventario' AND lease_state = 'reservada'",
        [run_id],
    )?)
}

/// Situação da fila de uma execução.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SituacaoFila {
    pub livres: u64,
    pub reservadas: u64,
    pub concluidas: u64,
    pub falhas: u64,
}

/// Conta a fila por estado.
///
/// # Errors
/// Falha do SQLite.
pub fn situacao_da_fila(conn: &Connection, run_id: i64) -> Resultado<SituacaoFila> {
    let mut s = SituacaoFila::default();
    let mut st = conn.prepare_cached(
        "SELECT lease_state, COUNT(*) FROM scan_queue WHERE run_id = ?1 AND phase = 'inventario' GROUP BY lease_state",
    )?;
    let linhas = st.query_map([run_id], |l| {
        Ok((l.get::<_, String>(0)?, l.get::<_, i64>(1)? as u64))
    })?;
    for r in linhas {
        let (estado, n) = r?;
        match estado.as_str() {
            "livre" => s.livres = n,
            "reservada" => s.reservadas = n,
            "concluida" => s.concluidas = n,
            _ => s.falhas += n,
        }
    }
    Ok(s)
}

// ------------------------------------------------------------------ erros e itens ignorados

/// Problema ou item ignorado a registrar em `scan_errors`.
#[derive(Debug, Clone, Copy)]
pub struct NovoProblema<'a> {
    pub run_id: i64,
    /// Pasta em que ocorreu (a lida, ou a que contém a entrada).
    pub pasta_id: Option<i64>,
    pub path_raw: Option<&'a [u8]>,
    pub fase: &'a str,
    pub codigo: &'a str,
    pub motivo: Option<&'a str>,
    pub severidade: Severidade,
    pub observado_em: i64,
}

/// Registra o problema (idempotente por execução, fase, código e caminho).
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_problema(conn: &Connection, p: &NovoProblema<'_>) -> Resultado<()> {
    let mut st = conn.prepare_cached(
        "INSERT OR IGNORE INTO scan_errors (run_id, folder_id, path_raw, path_display, phase,
                                            error_code, reason, observed_at, severity)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    st.execute(params![
        p.run_id,
        p.pasta_id,
        p.path_raw,
        p.path_raw.map(exibir),
        p.fase,
        p.codigo,
        p.motivo,
        p.observado_em,
        p.severidade.como_str()
    ])?;
    Ok(())
}
