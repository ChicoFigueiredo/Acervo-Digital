//! Reconciliação do Catálogo com o disco: Fast Scan, monitor de eventos e volumes offline
//! (A-12, A-13, A-15, A-17..A-19; spec §5 Fast Scan e §12).
//!
//! Tudo aqui recebe uma `&Connection`: o chamador decide a transação. Este módulo só toca no
//! Catálogo; a leitura do filesystem fica no scanner e no monitor.
//!
//! # Garantias
//!
//! * **Remoção só com prova.** Uma entrada só vira `ausente` depois que a raiz foi percorrida
//!   por inteiro, sem bloqueio no caminho dela ([`Bloqueios`]), com o volume conectado. Volume
//!   desconectado, raiz inacessível, permissão negada e varredura interrompida deixam as linhas
//!   como estão.
//! * **Renomeação em vigor, não remoção + criação.** Quando um caminho some e outro aparece no
//!   mesmo volume com o mesmo `(dev, inode, tamanho, mtime)`, a linha antiga é atualizada no
//!   lugar: o `files.id`, o hash e as Marcações são preservados ([`emparelhar`]).
//! * **Volume offline não reescreve linhas** (A-18): a indisponibilidade é derivada de
//!   `volumes.mounted` e exposta nas visões `v_arquivos` e `v_pastas` (`estado_efetivo`).

use std::collections::{HashMap, HashSet};

use dag_core::VolumeDescoberto;
use rusqlite::{Connection, OptionalExtension, params};

use crate::Resultado;
use crate::inventario::{self, NovaPasta, NovoArquivo, registrar_volume};

// ------------------------------------------------------------------ eventos

/// Tipo de evento (`scan_events.event_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipoEvento {
    Criado,
    Alterado,
    Removido,
    Renomeado,
    /// Lacuna: eventos perdidos (overflow do inotify, limite de watches, fila saturada).
    Overflow,
    Montado,
    Desmontado,
}

impl TipoEvento {
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::Criado => "criado",
            Self::Alterado => "alterado",
            Self::Removido => "removido",
            Self::Renomeado => "renomeado",
            Self::Overflow => "overflow",
            Self::Montado => "montado",
            Self::Desmontado => "desmontado",
        }
    }
}

/// Evento a gravar em `scan_events`.
#[derive(Debug, Clone, Copy)]
pub struct NovoEvento<'a> {
    pub volume_id: Option<i64>,
    pub tipo: TipoEvento,
    /// `inotify`, `fast_scan`, `volumes`...
    pub fonte: &'a str,
    pub caminho: Option<&'a [u8]>,
    /// Só em renomeações.
    pub caminho_antigo: Option<&'a [u8]>,
    pub arquivo_id: Option<i64>,
    pub execucao_id: Option<i64>,
    pub observado_em: i64,
    /// `None` = ainda não processado (lacuna aberta).
    pub processado_em: Option<i64>,
    /// Lacuna que exige reconciliação (Fast Scan) antes de ser declarada processada.
    pub reconciliar: bool,
    pub detalhe: Option<&'a str>,
}

impl<'a> NovoEvento<'a> {
    /// Evento já processado, sem lacuna.
    #[must_use]
    pub fn processado(tipo: TipoEvento, fonte: &'a str, em: i64) -> Self {
        Self {
            volume_id: None,
            tipo,
            fonte,
            caminho: None,
            caminho_antigo: None,
            arquivo_id: None,
            execucao_id: None,
            observado_em: em,
            processado_em: Some(em),
            reconciliar: false,
            detalhe: None,
        }
    }
}

/// Grava o evento e devolve o id.
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_evento(conn: &Connection, e: &NovoEvento<'_>) -> Resultado<i64> {
    conn.prepare_cached(
        "INSERT INTO scan_events (volume_id, event_type, source, path_raw, old_path_raw, file_id,
                                  run_id, observed_at, processed_at, reconcile_required, detail)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?
    .execute(params![
        e.volume_id,
        e.tipo.como_str(),
        e.fonte,
        e.caminho,
        e.caminho_antigo,
        e.arquivo_id,
        e.execucao_id,
        e.observado_em,
        e.processado_em,
        i64::from(e.reconciliar),
        e.detalhe
    ])?;
    Ok(conn.last_insert_rowid())
}

/// Declara processados os eventos (lacunas corrigidas por uma reconciliação concluída).
///
/// # Errors
/// Falha do SQLite.
pub fn fechar_eventos(conn: &Connection, ids: &[i64], agora: i64) -> Resultado<()> {
    let mut st = conn.prepare_cached(
        "UPDATE scan_events SET processed_at = ?2 WHERE id = ?1 AND processed_at IS NULL",
    )?;
    for id in ids {
        st.execute(params![id, agora])?;
    }
    Ok(())
}

/// Evento lido de `scan_events`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evento {
    pub id: i64,
    pub volume_id: Option<i64>,
    pub tipo: String,
    pub fonte: String,
    pub caminho: Option<String>,
    pub caminho_antigo: Option<String>,
    pub arquivo_id: Option<i64>,
    pub execucao_id: Option<i64>,
    pub observado_em: i64,
    pub processado_em: Option<i64>,
    pub reconciliar: bool,
    pub detalhe: Option<String>,
}

/// Os `limite` eventos mais recentes (id decrescente).
///
/// # Errors
/// Falha do SQLite.
pub fn listar_eventos(conn: &Connection, limite: u32) -> Resultado<Vec<Evento>> {
    let mut st = conn.prepare(
        "SELECT id, volume_id, event_type, source, path_raw, old_path_raw, file_id, run_id,
                observed_at, processed_at, reconcile_required, detail
         FROM scan_events ORDER BY id DESC LIMIT ?1",
    )?;
    let linhas = st.query_map([i64::from(limite)], |l| {
        let caminho: Option<Vec<u8>> = l.get(4)?;
        let antigo: Option<Vec<u8>> = l.get(5)?;
        Ok(Evento {
            id: l.get(0)?,
            volume_id: l.get(1)?,
            tipo: l.get(2)?,
            fonte: l.get(3)?,
            caminho: caminho.as_deref().map(inventario::exibir),
            caminho_antigo: antigo.as_deref().map(inventario::exibir),
            arquivo_id: l.get(6)?,
            execucao_id: l.get(7)?,
            observado_em: l.get(8)?,
            processado_em: l.get(9)?,
            reconciliar: l.get::<_, i64>(10)? != 0,
            detalhe: l.get(11)?,
        })
    })?;
    Ok(linhas.collect::<Result<_, _>>()?)
}

/// Pendências de reconciliação.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pendencias {
    /// Lacunas (`reconcile_required = 1`) ainda não corrigidas.
    pub lacunas_abertas: u64,
    /// Eventos sem `processed_at` (inclui as lacunas).
    pub eventos_nao_processados: u64,
}

/// Conta lacunas abertas e eventos não processados.
///
/// # Errors
/// Falha do SQLite.
pub fn pendencias(conn: &Connection) -> Resultado<Pendencias> {
    let (a, b): (i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(reconcile_required = 1), 0), COUNT(*) FROM scan_events
         WHERE processed_at IS NULL",
        [],
        |l| Ok((l.get(0)?, l.get(1)?)),
    )?;
    Ok(Pendencias {
        lacunas_abertas: a as u64,
        eventos_nao_processados: b as u64,
    })
}

// ------------------------------------------------------------------ saúde do monitor

/// Linha única de `monitor_saude`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Saude {
    pub run_id: Option<i64>,
    /// `parado`, `iniciando`, `ativo`, `reconciliando`, `encerrando`.
    pub estado: String,
    pub iniciado_em: Option<i64>,
    pub heartbeat: Option<i64>,
    pub backlog: u64,
    pub ultimo_evento_em: Option<i64>,
    pub ultima_reconciliacao: Option<i64>,
    pub watches: u64,
    pub watches_limite: u64,
    pub overflows: u64,
    pub eventos_processados: u64,
    /// Raízes sem cobertura completa de watches (reconciliação periódica cobre o resto).
    pub raizes_parciais: u64,
    pub raizes: Vec<String>,
    pub erro: Option<String>,
}

/// Grava a saúde (sobrescreve a linha única).
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_saude(conn: &Connection, s: &Saude) -> Resultado<()> {
    conn.prepare_cached(
        "UPDATE monitor_saude SET run_id = ?1, estado = ?2, iniciado_em = ?3, heartbeat = ?4,
             backlog = ?5, ultimo_evento_em = ?6, ultima_reconciliacao = ?7, watches = ?8,
             watches_limite = ?9, overflows = ?10, eventos_processados = ?11,
             raizes_parciais = ?12, raizes = ?13, erro = ?14 WHERE id = 1",
    )?
    .execute(params![
        s.run_id,
        s.estado,
        s.iniciado_em,
        s.heartbeat,
        s.backlog as i64,
        s.ultimo_evento_em,
        s.ultima_reconciliacao,
        s.watches as i64,
        s.watches_limite as i64,
        s.overflows as i64,
        s.eventos_processados as i64,
        s.raizes_parciais as i64,
        serde_json::to_string(&s.raizes).unwrap_or_else(|_| "[]".into()),
        s.erro
    ])?;
    Ok(())
}

/// Lê a saúde.
///
/// # Errors
/// Falha do SQLite.
pub fn ler_saude(conn: &Connection) -> Resultado<Saude> {
    Ok(conn.query_row(
        "SELECT run_id, estado, iniciado_em, heartbeat, backlog, ultimo_evento_em,
                ultima_reconciliacao, watches, watches_limite, overflows, eventos_processados,
                raizes_parciais, raizes, erro FROM monitor_saude WHERE id = 1",
        [],
        |l| {
            let raizes: String = l.get(12)?;
            Ok(Saude {
                run_id: l.get(0)?,
                estado: l.get(1)?,
                iniciado_em: l.get(2)?,
                heartbeat: l.get(3)?,
                backlog: l.get::<_, i64>(4)? as u64,
                ultimo_evento_em: l.get(5)?,
                ultima_reconciliacao: l.get(6)?,
                watches: l.get::<_, i64>(7)? as u64,
                watches_limite: l.get::<_, i64>(8)? as u64,
                overflows: l.get::<_, i64>(9)? as u64,
                eventos_processados: l.get::<_, i64>(10)? as u64,
                raizes_parciais: l.get::<_, i64>(11)? as u64,
                raizes: serde_json::from_str(&raizes).unwrap_or_default(),
                erro: l.get(13)?,
            })
        },
    )?)
}

// ------------------------------------------------------------------ volumes (A-18)

/// O que mudou na conectividade dos volumes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumoVolumes {
    /// `st_dev` → `volumes.id` dos volumes conectados (pseudo-filesystems ficam de fora).
    pub conectados: HashMap<u64, i64>,
    /// Voltaram a ficar conectados nesta chamada.
    pub reconectados: Vec<i64>,
    /// Passaram a desconectados nesta chamada.
    pub desconectados: Vec<i64>,
}

/// Registra os volumes descobertos como conectados e marca como desconectados (`mounted = 0`,
/// `last_seen_at` preservado) os que não aparecem. **Nenhuma linha de `files`/`folders` é
/// alterada ou removida**: o estado efetivo vem de `v_arquivos`/`v_pastas`. Grava eventos
/// `montado`/`desmontado` nas transições. Idempotente.
///
/// # Errors
/// Falha do SQLite.
pub fn sincronizar_volumes(
    conn: &Connection,
    descobertos: &[VolumeDescoberto],
    agora: i64,
) -> Resultado<ResumoVolumes> {
    let antes: HashMap<i64, bool> = {
        let mut st = conn.prepare("SELECT id, mounted FROM volumes")?;
        st.query_map([], |l| Ok((l.get(0)?, l.get::<_, i64>(1)? != 0)))?
            .collect::<Result<_, _>>()?
    };
    let mut r = ResumoVolumes::default();
    let mut ids = HashSet::new();
    for v in descobertos.iter().filter(|v| !v.pseudo) {
        let id = registrar_volume(conn, v, agora)?;
        r.conectados.insert(v.st_dev, id);
        if ids.insert(id) && antes.get(&id) == Some(&false) {
            r.reconectados.push(id);
        }
    }
    let mut ordem: Vec<i64> = antes.keys().copied().collect();
    ordem.sort_unstable();
    for id in ordem {
        if ids.contains(&id) {
            continue;
        }
        conn.execute("UPDATE volumes SET mounted = 0 WHERE id = ?1", [id])?;
        if antes[&id] {
            r.desconectados.push(id);
        }
    }
    for (ids, tipo) in [
        (&r.reconectados, TipoEvento::Montado),
        (&r.desconectados, TipoEvento::Desmontado),
    ] {
        for id in ids {
            registrar_evento(
                conn,
                &NovoEvento {
                    volume_id: Some(*id),
                    fonte: "volumes",
                    ..NovoEvento::processado(tipo, "volumes", agora)
                },
            )?;
        }
    }
    Ok(r)
}

// ------------------------------------------------------------------ gravação com comparação

/// O que a gravação de uma entrada do Fast Scan fez.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Efeito {
    /// Identidade, tipo, tamanho, mtime e ctime iguais: só `seen_run_id` foi atualizado.
    Inalterado,
    /// A linha existia e mudou (ou voltou de `ausente`).
    Alterado,
    /// Não havia linha para o caminho.
    Novo,
}

/// Grava um arquivo comparando com a linha existente. Entrada inalterada custa um `UPDATE`
/// de `seen_run_id`, sem reescrever metadados nem tocar em `hashes`.
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_arquivo(conn: &Connection, a: &NovoArquivo<'_>) -> Resultado<Efeito> {
    let tocadas = conn
        .prepare_cached(
            "UPDATE files SET seen_run_id = ?1, state = 'ativo'
             WHERE volume_id = ?2 AND path_raw = ?3 AND folder_id = ?4 AND kind = ?5
               AND native_dev IS ?6 AND native_file_id IS ?7 AND logical_bytes = ?8
               AND mtime_ns IS ?9 AND ctime_ns IS ?10",
        )?
        .execute(params![
            a.run_id,
            a.volume_id,
            a.path_raw,
            a.pasta_id,
            a.tipo.como_str(),
            a.dev as i64,
            a.inode as i64,
            a.bytes_logicos as i64,
            a.mtime_ns,
            a.ctime_ns
        ])?;
    if tocadas > 0 {
        return Ok(Efeito::Inalterado);
    }
    let existia = conn
        .prepare_cached("SELECT 1 FROM files WHERE volume_id = ?1 AND path_raw = ?2")?
        .query_row(params![a.volume_id, a.path_raw], |_| Ok(()))
        .optional()?
        .is_some();
    inventario::upsert_arquivo(conn, a)?;
    if existia {
        return Ok(Efeito::Alterado);
    }
    conn.prepare_cached(
        "UPDATE files SET first_run_id = ?3 WHERE volume_id = ?1 AND path_raw = ?2",
    )?
    .execute(params![a.volume_id, a.path_raw, a.run_id])?;
    Ok(Efeito::Novo)
}

/// Grava uma pasta comparando com a linha existente; devolve o id e o efeito.
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_pasta(conn: &Connection, p: &NovaPasta<'_>) -> Resultado<(i64, Efeito)> {
    let tocada: Option<i64> = conn
        .prepare_cached(
            "UPDATE folders SET seen_run_id = ?1, state = 'ativo'
             WHERE volume_id = ?2 AND path_raw = ?3 AND parent_folder_id IS ?4
               AND native_dev IS ?5 AND native_file_id IS ?6 AND mtime_ns IS ?7 AND attrs = ?8
             RETURNING id",
        )?
        .query_row(
            params![
                p.run_id,
                p.volume_id,
                p.path_raw,
                p.pai,
                p.dev as i64,
                p.inode as i64,
                p.mtime_ns,
                p.attrs
            ],
            |l| l.get(0),
        )
        .optional()?;
    if let Some(id) = tocada {
        return Ok((id, Efeito::Inalterado));
    }
    let existia = conn
        .prepare_cached("SELECT 1 FROM folders WHERE volume_id = ?1 AND path_raw = ?2")?
        .query_row(params![p.volume_id, p.path_raw], |_| Ok(()))
        .optional()?
        .is_some();
    let id = inventario::upsert_pasta(conn, p)?;
    if existia {
        return Ok((id, Efeito::Alterado));
    }
    conn.prepare_cached("UPDATE folders SET first_run_id = ?2 WHERE id = ?1")?
        .execute(params![id, p.run_id])?;
    Ok((id, Efeito::Novo))
}

/// Registra a pasta-raiz de um Fast Scan **sem alterar o pai** de uma pasta que já existe (uma
/// raiz aninhada em outra raiz catalogada continua pendurada nela).
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_raiz(conn: &Connection, p: &NovaPasta<'_>) -> Resultado<i64> {
    let existente: Option<i64> = conn
        .prepare_cached(
            "UPDATE folders SET native_dev = ?3, native_file_id = ?4, attrs = ?5, mtime_ns = ?6,
                 seen_run_id = ?7, state = 'ativo'
             WHERE volume_id = ?1 AND path_raw = ?2 RETURNING id",
        )?
        .query_row(
            params![
                p.volume_id,
                p.path_raw,
                p.dev as i64,
                p.inode as i64,
                p.attrs,
                p.mtime_ns,
                p.run_id
            ],
            |l| l.get(0),
        )
        .optional()?;
    if let Some(id) = existente {
        return Ok(id);
    }
    let id = inventario::upsert_pasta(conn, p)?;
    conn.prepare_cached("UPDATE folders SET first_run_id = ?2 WHERE id = ?1")?
        .execute(params![id, p.run_id])?;
    Ok(id)
}

// ------------------------------------------------------------------ raízes

/// Raiz já catalogada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaizCatalogada {
    pub volume_id: i64,
    pub pasta_id: i64,
    pub path_raw: Vec<u8>,
    pub platform_id: String,
    pub conectado: bool,
}

/// Raízes de varredura conhecidas (pastas sem pai, ativas), das mais curtas para as mais longas.
///
/// # Errors
/// Falha do SQLite.
pub fn raizes_catalogadas(conn: &Connection) -> Resultado<Vec<RaizCatalogada>> {
    let mut st = conn.prepare(
        "SELECT f.volume_id, f.id, f.path_raw, v.platform_id, v.mounted
         FROM folders f JOIN volumes v ON v.id = f.volume_id
         WHERE f.parent_folder_id IS NULL AND f.state = 'ativo'
         ORDER BY length(f.path_raw), f.id",
    )?;
    let linhas = st.query_map([], |l| {
        Ok(RaizCatalogada {
            volume_id: l.get(0)?,
            pasta_id: l.get(1)?,
            path_raw: l.get(2)?,
            platform_id: l.get(3)?,
            conectado: l.get::<_, i64>(4)? != 0,
        })
    })?;
    Ok(linhas.collect::<Result<_, _>>()?)
}

/// Escopo de uma raiz num Fast Scan: o volume e o caminho.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Escopo {
    pub volume_id: i64,
    pub path_raw: Vec<u8>,
}

fn faixa(raiz: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut base = raiz.to_vec();
    if base.last() == Some(&b'/') {
        base.pop();
    }
    let mut lo = base.clone();
    lo.push(b'/');
    let mut hi = base;
    hi.push(b'0');
    (lo, hi)
}

/// `caminho` é `raiz` ou está abaixo dela.
#[must_use]
pub fn dentro_de(caminho: &[u8], raiz: &[u8]) -> bool {
    let mut base = raiz;
    if base.last() == Some(&b'/') {
        base = &base[..base.len() - 1];
    }
    caminho == raiz || (caminho.starts_with(base) && caminho.get(base.len()) == Some(&b'/'))
}

// ------------------------------------------------------------------ bloqueios

/// Caminhos que a varredura não conseguiu visitar por inteiro (erro, exclusão, fronteira de
/// volume, diretório que falhou). Nada abaixo deles pode ser considerado removido.
#[derive(Debug, Clone, Default)]
pub struct Bloqueios {
    exatos: HashSet<Vec<u8>>,
}

impl Bloqueios {
    #[must_use]
    pub fn vazio() -> Self {
        Self::default()
    }

    pub fn adicionar(&mut self, caminho: &[u8]) {
        self.exatos.insert(caminho.to_vec());
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.exatos.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.exatos.is_empty()
    }

    /// O caminho, ou algum ancestral dele, está bloqueado?
    #[must_use]
    pub fn bloqueia(&self, caminho: &[u8]) -> bool {
        if self.exatos.is_empty() {
            return false;
        }
        let mut atual = caminho;
        loop {
            if self.exatos.contains(atual) {
                return true;
            }
            match atual.iter().rposition(|b| *b == b'/') {
                Some(0) => return self.exatos.contains(&b"/"[..]),
                Some(i) => atual = &atual[..i],
                None => return false,
            }
        }
    }

    /// Bloqueios de uma execução: todo caminho com problema registrado (qualquer severidade,
    /// inclusive itens ignorados de propósito) e todo diretório da fila que falhou.
    ///
    /// # Errors
    /// Falha do SQLite.
    pub fn da_execucao(conn: &Connection, run_id: i64) -> Resultado<Self> {
        let mut b = Self::default();
        let mut st = conn.prepare(
            "SELECT path_raw FROM scan_errors WHERE run_id = ?1 AND path_raw IS NOT NULL
             UNION
             SELECT path_raw FROM scan_queue WHERE run_id = ?1 AND lease_state = 'falhou'",
        )?;
        let linhas = st.query_map([run_id], |l| l.get::<_, Vec<u8>>(0))?;
        for p in linhas {
            b.exatos.insert(p?);
        }
        Ok(b)
    }
}

// ------------------------------------------------------------------ candidatos a renomeação

/// Evidência de identidade de uma entrada (para renomeação).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cand {
    pub id: i64,
    pub volume_id: i64,
    pub dev: i64,
    pub inode: i64,
    pub tamanho: i64,
    pub mtime_ns: i64,
    pub nome: Vec<u8>,
}

type Chave = (i64, i64, i64, i64, i64);

impl Cand {
    fn chave(&self) -> Chave {
        (
            self.volume_id,
            self.dev,
            self.inode,
            self.tamanho,
            self.mtime_ns,
        )
    }

    fn chave_pasta(&self) -> Chave {
        (self.volume_id, self.dev, self.inode, 0, 0)
    }
}

fn unicos_por_nome(v: &[&Cand]) -> bool {
    let nomes: HashSet<&[u8]> = v.iter().map(|c| c.nome.as_slice()).collect();
    nomes.len() == v.len()
}

/// Emparelha caminhos sumidos com caminhos novos do mesmo volume que têm o mesmo
/// `(dev, inode, tamanho, mtime)`. Par 1:1 por chave; com vários de cada lado (hard links
/// movidos juntos) só emparelha pelo mesmo nome, e só se os nomes forem únicos nos dois lados.
/// Ambiguidade nunca vira renomeação: cai em remoção + criação, que é seguro. Devolve
/// `(id_antigo, id_novo)`.
#[must_use]
pub fn emparelhar(velhos: &[Cand], novos: &[Cand]) -> Vec<(i64, i64)> {
    parear(velhos, novos, Cand::chave, |_, _| true)
}

/// Como [`emparelhar`] para pastas: chave `(volume, dev, inode)`; o par só vale se o nome é
/// o mesmo ou o `mtime` é o mesmo (mitiga reuso de inode por um diretório novo e não
/// relacionado).
#[must_use]
pub fn emparelhar_pastas(velhas: &[Cand], novas: &[Cand]) -> Vec<(i64, i64)> {
    parear(velhas, novas, Cand::chave_pasta, |v, n| {
        v.nome == n.nome || v.mtime_ns == n.mtime_ns
    })
}

fn parear(
    velhos: &[Cand],
    novos: &[Cand],
    chave: impl Fn(&Cand) -> Chave,
    valido: impl Fn(&Cand, &Cand) -> bool,
) -> Vec<(i64, i64)> {
    let mut por_chave_n: HashMap<Chave, Vec<&Cand>> = HashMap::new();
    for n in novos {
        por_chave_n.entry(chave(n)).or_default().push(n);
    }
    let mut por_chave_v: HashMap<Chave, Vec<&Cand>> = HashMap::new();
    for v in velhos {
        por_chave_v.entry(chave(v)).or_default().push(v);
    }
    let mut pares = Vec::new();
    for (k, vs) in &por_chave_v {
        let Some(ns) = por_chave_n.get(k) else {
            continue;
        };
        if vs.len() == 1 && ns.len() == 1 {
            if valido(vs[0], ns[0]) {
                pares.push((vs[0].id, ns[0].id));
            }
            continue;
        }
        if !unicos_por_nome(vs) || !unicos_por_nome(ns) {
            continue;
        }
        for v in vs {
            if let Some(n) = ns.iter().find(|n| n.nome == v.nome)
                && valido(v, n)
            {
                pares.push((v.id, n.id));
            }
        }
    }
    pares.sort_unstable();
    pares
}

/// Itens sumidos de um escopo.
#[derive(Debug, Default)]
pub struct Sumidos {
    /// Todos os ids sumidos (candidatos à remoção).
    pub ids: Vec<i64>,
    /// Os que têm evidência completa para renomeação (limitado por `cap`).
    pub cands: Vec<Cand>,
    /// `cands` foi truncado pelo limite (renomeação fica limitada; remoção não).
    pub truncado: bool,
}

/// Tabela alvo das consultas de reconciliação.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tabela {
    Arquivos,
    Pastas,
}

impl Tabela {
    const fn nome(self) -> &'static str {
        match self {
            Self::Arquivos => "files",
            Self::Pastas => "folders",
        }
    }

    const fn tamanho(self) -> &'static str {
        match self {
            Self::Arquivos => "logical_bytes",
            Self::Pastas => "0",
        }
    }
}

/// Entradas ativas dos escopos que a execução `run_id` **não viu** e que não estão sob
/// bloqueio: o que, após uma reconciliação completa, deixou de existir.
///
/// # Errors
/// Falha do SQLite.
pub fn coletar_sumidos(
    conn: &Connection,
    tabela: Tabela,
    run_id: i64,
    escopos: &[Escopo],
    bloq: &Bloqueios,
    cap: usize,
) -> Resultado<Sumidos> {
    let mut out = Sumidos::default();
    let sql = format!(
        "SELECT id, volume_id, native_dev, native_file_id, {}, mtime_ns, name_raw, path_raw
         FROM {} WHERE volume_id = ?1 AND state = 'ativo'
           AND (seen_run_id IS NULL OR seen_run_id <> ?2)
           AND (path_raw = ?3 OR (path_raw >= ?4 AND path_raw < ?5))",
        tabela.tamanho(),
        tabela.nome()
    );
    let mut st = conn.prepare(&sql)?;
    for e in escopos {
        let (lo, hi) = faixa(&e.path_raw);
        let mut linhas = st.query(params![e.volume_id, run_id, e.path_raw, lo, hi])?;
        while let Some(l) = linhas.next()? {
            let path: Vec<u8> = l.get(7)?;
            if bloq.bloqueia(&path) {
                continue;
            }
            let id: i64 = l.get(0)?;
            out.ids.push(id);
            let dev: Option<i64> = l.get(2)?;
            let inode: Option<i64> = l.get(3)?;
            let mtime: Option<i64> = l.get(5)?;
            let pasta = tabela == Tabela::Pastas;
            if let (Some(dev), Some(inode)) = (dev, inode) {
                if mtime.is_none() && !pasta {
                    continue;
                }
                if out.cands.len() >= cap {
                    out.truncado = true;
                    continue;
                }
                out.cands.push(Cand {
                    id,
                    volume_id: l.get(1)?,
                    dev,
                    inode,
                    tamanho: l.get(4)?,
                    mtime_ns: mtime.unwrap_or(i64::MIN),
                    nome: l.get(6)?,
                });
            }
        }
    }
    Ok(out)
}

/// Linhas criadas pela execução (`first_run_id`) cuja chave bate com alguma das `sumidas`.
///
/// # Errors
/// Falha do SQLite.
pub fn coletar_novos_com_chave(
    conn: &Connection,
    tabela: Tabela,
    run_id: i64,
    sumidos: &[Cand],
) -> Resultado<Vec<Cand>> {
    let pasta = tabela == Tabela::Pastas;
    let chaves: HashSet<Chave> = sumidos
        .iter()
        .map(|c| if pasta { c.chave_pasta() } else { c.chave() })
        .collect();
    let mut out = Vec::new();
    if chaves.is_empty() {
        return Ok(out);
    }
    let sql = format!(
        "SELECT id, volume_id, native_dev, native_file_id, {}, mtime_ns, name_raw
         FROM {} WHERE first_run_id = ?1 AND state = 'ativo'
           AND native_dev IS NOT NULL AND native_file_id IS NOT NULL",
        tabela.tamanho(),
        tabela.nome()
    );
    let mut st = conn.prepare(&sql)?;
    let mut linhas = st.query([run_id])?;
    while let Some(l) = linhas.next()? {
        let mtime: Option<i64> = l.get(5)?;
        if mtime.is_none() && !pasta {
            continue;
        }
        let c = Cand {
            id: l.get(0)?,
            volume_id: l.get(1)?,
            dev: l.get(2)?,
            inode: l.get(3)?,
            tamanho: l.get(4)?,
            mtime_ns: mtime.unwrap_or(i64::MIN),
            nome: l.get(6)?,
        };
        let k = if pasta { c.chave_pasta() } else { c.chave() };
        if chaves.contains(&k) {
            out.push(c);
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ aplicar renomeação

/// Dados do caminho novo a aplicar sobre a linha antiga.
struct LinhaArquivo {
    folder_id: i64,
    path_raw: Vec<u8>,
    path_display: String,
    name_raw: Vec<u8>,
    name_display: String,
    nlink: Option<i64>,
    allocated: Option<i64>,
    ctime: Option<i64>,
    birth: Option<i64>,
    attrs: String,
    mime_id: Option<i64>,
    kind: String,
    alvo_raw: Option<Vec<u8>>,
    alvo_display: Option<String>,
}

/// A linha `velho` assume o caminho e os metadados da linha `novo` (que é apagada): mantém
/// `files.id`, `hashes`, grupos e Marcações. A evidência de ctime do hash acompanha o novo
/// ctime (renomear altera o ctime sem alterar o conteúdo; tamanho, mtime e inode já batem).
/// Devolve o novo caminho bruto.
///
/// # Errors
/// Falha do SQLite.
pub fn aplicar_renomeacao_arquivo(
    conn: &Connection,
    velho: i64,
    novo: i64,
    run_id: i64,
    fonte: &str,
    agora: i64,
    registrar: bool,
) -> Resultado<Vec<u8>> {
    let n = conn.query_row(
        "SELECT folder_id, path_raw, path_display, name_raw, name_display, nlink, allocated_bytes,
                ctime_ns, birthtime_ns, attrs, mime_id, kind, link_target_raw, link_target_display
         FROM files WHERE id = ?1",
        [novo],
        |l| {
            Ok(LinhaArquivo {
                folder_id: l.get(0)?,
                path_raw: l.get(1)?,
                path_display: l.get(2)?,
                name_raw: l.get(3)?,
                name_display: l.get(4)?,
                nlink: l.get(5)?,
                allocated: l.get(6)?,
                ctime: l.get(7)?,
                birth: l.get(8)?,
                attrs: l.get(9)?,
                mime_id: l.get(10)?,
                kind: l.get(11)?,
                alvo_raw: l.get(12)?,
                alvo_display: l.get(13)?,
            })
        },
    )?;
    let (volume_id, antigo): (i64, Vec<u8>) = conn.query_row(
        "SELECT volume_id, path_raw FROM files WHERE id = ?1",
        [velho],
        |l| Ok((l.get(0)?, l.get(1)?)),
    )?;
    conn.execute("DELETE FROM files WHERE id = ?1", [novo])?;
    conn.execute(
        "UPDATE files SET folder_id = ?2, path_raw = ?3, path_display = ?4, name_raw = ?5,
             name_display = ?6, nlink = ?7, allocated_bytes = ?8, ctime_ns = ?9, birthtime_ns = ?10,
             attrs = ?11, mime_id = COALESCE(?12, mime_id), kind = ?13, link_target_raw = ?14,
             link_target_display = ?15, hardlink_group = NULL, seen_run_id = ?16, state = 'ativo'
         WHERE id = ?1",
        params![
            velho,
            n.folder_id,
            n.path_raw,
            n.path_display,
            n.name_raw,
            n.name_display,
            n.nlink,
            n.allocated,
            n.ctime,
            n.birth,
            n.attrs,
            n.mime_id,
            n.kind,
            n.alvo_raw,
            n.alvo_display,
            run_id
        ],
    )?;
    conn.execute(
        "UPDATE hashes SET ctime_ns = ?2 WHERE file_id = ?1",
        params![velho, n.ctime],
    )?;
    if registrar {
        registrar_evento(
            conn,
            &NovoEvento {
                volume_id: Some(volume_id),
                caminho: Some(&n.path_raw),
                caminho_antigo: Some(&antigo),
                arquivo_id: Some(velho),
                execucao_id: Some(run_id),
                ..NovoEvento::processado(TipoEvento::Renomeado, fonte, agora)
            },
        )?;
    }
    Ok(n.path_raw)
}

/// Como [`aplicar_renomeacao_arquivo`] para pastas: os filhos da pasta nova passam para a
/// antiga, a nova é apagada e a antiga assume caminho, pai e metadados.
///
/// # Errors
/// Falha do SQLite.
pub fn aplicar_renomeacao_pasta(
    conn: &Connection,
    velha: i64,
    nova: i64,
    run_id: i64,
    fonte: &str,
    agora: i64,
) -> Resultado<()> {
    type Dados = (
        Option<i64>,
        Vec<u8>,
        String,
        Vec<u8>,
        String,
        String,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    );
    let n: Dados = conn.query_row(
        "SELECT parent_folder_id, path_raw, path_display, name_raw, name_display, attrs, mtime_ns,
                native_dev, native_file_id FROM folders WHERE id = ?1",
        [nova],
        |l| {
            Ok((
                l.get(0)?,
                l.get(1)?,
                l.get(2)?,
                l.get(3)?,
                l.get(4)?,
                l.get(5)?,
                l.get(6)?,
                l.get(7)?,
                l.get(8)?,
            ))
        },
    )?;
    let (volume_id, antigo): (i64, Vec<u8>) = conn.query_row(
        "SELECT volume_id, path_raw FROM folders WHERE id = ?1",
        [velha],
        |l| Ok((l.get(0)?, l.get(1)?)),
    )?;
    conn.execute(
        "UPDATE files SET folder_id = ?1 WHERE folder_id = ?2",
        [velha, nova],
    )?;
    conn.execute(
        "UPDATE folders SET parent_folder_id = ?1 WHERE parent_folder_id = ?2",
        [velha, nova],
    )?;
    conn.execute(
        "UPDATE scan_queue SET directory_id = ?1 WHERE directory_id = ?2",
        [velha, nova],
    )?;
    conn.execute(
        "UPDATE scan_errors SET folder_id = ?1 WHERE folder_id = ?2",
        [velha, nova],
    )?;
    conn.execute("DELETE FROM folders WHERE id = ?1", [nova])?;
    // O novo pai pode ser a própria pasta apagada (já repontado acima) ou outra pasta nova.
    let pai = n.0.filter(|p| *p != nova && *p != velha);
    conn.execute(
        "UPDATE folders SET parent_folder_id = ?2, path_raw = ?3, path_display = ?4, name_raw = ?5,
             name_display = ?6, attrs = ?7, mtime_ns = ?8, native_dev = ?9, native_file_id = ?10,
             seen_run_id = ?11, state = 'ativo' WHERE id = ?1",
        params![velha, pai, n.1, n.2, n.3, n.4, n.5, n.6, n.7, n.8, run_id],
    )?;
    registrar_evento(
        conn,
        &NovoEvento {
            volume_id: Some(volume_id),
            caminho: Some(&n.1),
            caminho_antigo: Some(&antigo),
            execucao_id: Some(run_id),
            detalhe: Some("pasta"),
            ..NovoEvento::processado(TipoEvento::Renomeado, fonte, agora)
        },
    )?;
    Ok(())
}

/// Marca uma linha como `ausente` (só se estiver `ativo`). Devolve o caminho se mudou.
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_ausente(conn: &Connection, tabela: Tabela, id: i64) -> Resultado<Option<Vec<u8>>> {
    let sql = format!(
        "UPDATE {} SET state = 'ausente' WHERE id = ?1 AND state = 'ativo' RETURNING path_raw",
        tabela.nome()
    );
    Ok(conn
        .prepare_cached(&sql)?
        .query_row([id], |l| l.get(0))
        .optional()?)
}

/// Marca como `ausente` tudo que está ativo sob `raiz` (pasta removida, confirmada pelo
/// diretório pai listado com sucesso). Devolve quantas linhas (arquivos + pastas).
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_subarvore_ausente(conn: &Connection, volume_id: i64, raiz: &[u8]) -> Resultado<u64> {
    let (lo, hi) = faixa(raiz);
    let mut n = 0;
    for t in ["files", "folders"] {
        n += conn.execute(
            &format!(
                "UPDATE {t} SET state = 'ausente' WHERE volume_id = ?1 AND state = 'ativo'
                 AND (path_raw = ?2 OR (path_raw >= ?3 AND path_raw < ?4))"
            ),
            params![volume_id, raiz, lo, hi],
        )? as u64;
    }
    Ok(n)
}

#[cfg(test)]
mod testes {
    use super::*;

    fn c(id: i64, ino: i64, nome: &str) -> Cand {
        Cand {
            id,
            volume_id: 1,
            dev: 1,
            inode: ino,
            tamanho: 10,
            mtime_ns: 5,
            nome: nome.as_bytes().to_vec(),
        }
    }

    #[test]
    fn bloqueio_vale_para_ancestrais() {
        let mut b = Bloqueios::vazio();
        b.adicionar(b"/a/b");
        assert!(b.bloqueia(b"/a/b"));
        assert!(b.bloqueia(b"/a/b/c/d"));
        assert!(!b.bloqueia(b"/a/bc"));
        assert!(!b.bloqueia(b"/a"));
        b.adicionar(b"/");
        assert!(b.bloqueia(b"/qualquer"));
    }

    #[test]
    fn dentro_de_respeita_limite_de_componente() {
        assert!(dentro_de(b"/a/b", b"/a"));
        assert!(dentro_de(b"/a", b"/a"));
        assert!(!dentro_de(b"/ab", b"/a"));
        assert!(dentro_de(b"/x", b"/"));
    }

    #[test]
    fn pares_unicos_e_ambiguos() {
        assert_eq!(emparelhar(&[c(1, 7, "a")], &[c(2, 7, "b")]), [(1, 2)]);
        // Inode diferente: sem par.
        assert!(emparelhar(&[c(1, 7, "a")], &[c(2, 8, "a")]).is_empty());
        // Dois lados com 2 itens: só por nome único.
        let v = [c(1, 7, "a"), c(2, 7, "b")];
        let n = [c(3, 7, "b"), c(4, 7, "a")];
        assert_eq!(emparelhar(&v, &n), [(1, 4), (2, 3)]);
        // Nomes repetidos: ambíguo, sem par.
        let v = [c(1, 7, "a"), c(2, 7, "a")];
        let n = [c(3, 7, "a"), c(4, 7, "a")];
        assert!(emparelhar(&v, &n).is_empty());
    }

    #[test]
    fn pasta_exige_mesmo_nome_ou_mtime() {
        let mut v = c(1, 9, "x");
        v.mtime_ns = 1;
        let mut n = c(2, 9, "y");
        n.mtime_ns = 2;
        assert!(emparelhar_pastas(&[v.clone()], &[n.clone()]).is_empty());
        n.mtime_ns = 1;
        assert_eq!(emparelhar_pastas(&[v], &[n]), [(1, 2)]);
    }
}
