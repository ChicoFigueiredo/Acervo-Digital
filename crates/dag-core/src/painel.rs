//! Painel do trabalho em andamento: um modelo de apresentação único, em palavras simples, que a
//! GUI (página Varredura e barra de status), a CLI (`dag status`, `dag jobs`, `--seguir`) e o MCP
//! desenham. Tudo aqui é puro (sem relógio, sem E/S): quem chama passa o que sabe.
//!
//! Responde, de relance: o que está rodando (modo e etapa), onde (agente em segundo plano ou esta
//! janela) e quem iniciou, quantos arquivos há na execução e no Catálogo, quantos já têm hash,
//! quantas threads, o andamento de cada disco físico e o tempo restante.

use crate::formato::{bytes_humanos, inteiro_pt_br};
use crate::{
    EstadoJob, LeituraEmCurso, OrigemJob, PassoAbertura, ProgressoConteineres, ProgressoDiscoHash,
    ProgressoHash, TipoJob, TipoPasso,
};

/// Janela mínima (ms) de medição para a estimativa de tempo restante deixar de ser "estimando…".
pub const JANELA_MINIMA_ETA_MS: u64 = 15_000;

/// Onde o trabalho roda.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Onde {
    /// No agente em segundo plano (sobrevive ao fechamento da janela).
    Agente { pid: Option<u32> },
    /// Neste processo (modo `--sem-agente`): fechar interrompe (e a abertura seguinte retoma).
    Janela,
}

impl Onde {
    /// Frase curta e inequívoca.
    #[must_use]
    pub fn texto(self) -> String {
        match self {
            Self::Agente { pid: Some(p) } => {
                format!("no agente em segundo plano (pid {p}) — pode fechar a janela")
            }
            Self::Agente { pid: None } => {
                "no agente em segundo plano — pode fechar a janela".into()
            }
            Self::Janela => "nesta janela (modo sem agente) — fechar interrompe".into(),
        }
    }
}

/// Totais do Catálogo inteiro.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TotaisCatalogoPainel {
    pub arquivos: u64,
    /// `None` quando a contagem ainda não foi feita.
    pub com_hash: Option<u64>,
}

/// Contagens do inventário (Full Scan) ou da reconciliação (Fast Scan).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ContagemInventario {
    pub pastas: u64,
    pub arquivos: u64,
    pub bytes: u64,
    pub pendentes: u64,
    pub itens_por_s: f64,
    pub erros: u64,
    pub decorrido_ms: u64,
}

impl From<&crate::ProgressoInventario> for ContagemInventario {
    fn from(i: &crate::ProgressoInventario) -> Self {
        Self {
            pastas: i.pastas,
            arquivos: i.arquivos,
            bytes: i.bytes_logicos,
            pendentes: i.diretorios_pendentes,
            itens_por_s: i.entradas_por_segundo,
            erros: i.erros,
            decorrido_ms: i.decorrido_ms,
        }
    }
}

/// Tudo o que o painel precisa saber, já extraído de [`crate::JobInfo`] ou do estado da GUI.
#[derive(Debug, Clone)]
pub struct EntradaPainel<'a> {
    pub tipo: Option<TipoJob>,
    pub origem: Option<OrigemJob>,
    pub estado: Option<EstadoJob>,
    pub pausado: bool,
    pub execucao: Option<i64>,
    pub passo: Option<&'a PassoAbertura>,
    pub etapa_texto: Option<&'a str>,
    /// Full Scan pedido sem hash.
    pub sem_hash: bool,
    pub hash: Option<&'a ProgressoHash>,
    /// Conteúdo de contêineres (etapa depois do hash).
    pub conteineres: Option<&'a ProgressoConteineres>,
    /// O `hash` é a Verificação de Integridade.
    pub verificacao: bool,
    pub inventario: Option<ContagemInventario>,
    /// O `inventario` é a reconciliação do Fast Scan.
    pub reconciliacao: bool,
    pub onde: Onde,
    /// Segundos desde que o trabalho entrou no agente.
    pub decorrido_s: Option<u64>,
    pub catalogo: Option<TotaisCatalogoPainel>,
}

impl EntradaPainel<'_> {
    /// Entrada vazia (sem nada em andamento além do que for preenchido).
    #[must_use]
    pub fn vazia(onde: Onde) -> Self {
        Self {
            tipo: None,
            origem: None,
            estado: None,
            pausado: false,
            execucao: None,
            passo: None,
            etapa_texto: None,
            sem_hash: false,
            hash: None,
            conteineres: None,
            verificacao: false,
            inventario: None,
            reconciliacao: false,
            onde,
            decorrido_s: None,
            catalogo: None,
        }
    }
}

/// Em que etapa o trabalho está, em palavras de usuário.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtapaPainel {
    /// Na fila do agente, ainda sem rodar.
    NaFila,
    /// Ainda sem contagem nenhuma (descobrindo volumes, conferindo o Catálogo).
    Preparando,
    Inventario,
    Reconciliacao,
    /// Montando a fila de hash (pode levar minutos em Catálogos grandes).
    PreparandoHash,
    Hash,
    Verificacao,
    /// Lendo o conteúdo dos compactados (Membros e hashes), depois do Hash Completo.
    Conteineres,
    /// Reconstruindo os Grupos de Duplicados e reavaliando as Marcações (depois do hash). O
    /// resumo do hash que fica no painel já é o final.
    Duplicados,
}

/// Mensagens do agente que marcam a etapa de agrupamento (depois do Hash Completo).
const PREFIXOS_DUPLICADOS: [&str; 4] = [
    "[duplicados]",
    "[marcações]",
    "abertura: Grupos de Duplicados",
    "Agrupando duplicados",
];

/// A mensagem de etapa `texto` é a do agrupamento de duplicados? Devolve o nome da etapa.
#[must_use]
pub fn etapa_de_duplicados(texto: &str) -> Option<&'static str> {
    let t = texto.trim_start();
    if !PREFIXOS_DUPLICADOS.iter().any(|p| t.starts_with(p)) {
        return None;
    }
    // "[marcações] N Marcação(ões) ficaram pendentes" já é o fim da etapa, não um passo dela.
    if t.starts_with("[marcações] reavaliando") {
        Some("Reavaliando Marcações")
    } else if t.starts_with("[marcações]") {
        None
    } else {
        Some("Agrupando duplicados")
    }
}

/// Estado de um disco na etapa de hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoDisco {
    Lendo,
    /// Ainda não começou (vaga de disco em paralelo) ou sem leitura em curso.
    Aguardando,
    Concluido,
    Pausado,
}

impl EstadoDisco {
    #[must_use]
    pub fn rotulo(self) -> &'static str {
        match self {
            Self::Lendo => "lendo",
            Self::Aguardando => "aguardando",
            Self::Concluido => "concluído",
            Self::Pausado => "pausado",
        }
    }
}

/// Uma linha da tabela por disco físico.
#[derive(Debug, Clone, PartialEq)]
pub struct PainelDisco {
    pub nome: String,
    pub volumes: Vec<i64>,
    pub threads: u32,
    pub leitores_ativos: u32,
    pub feitos: u64,
    pub total: u64,
    pub pct: Option<f64>,
    pub bytes_por_seg: f64,
    pub estado: EstadoDisco,
    /// Segundos restantes; `None` = "estimando…" (ou concluído).
    pub eta_s: Option<u64>,
    /// Arquivos grandes em leitura agora (bytes lidos de cada um, sobem durante a leitura).
    pub lendo: Vec<LeituraEmCurso>,
}

/// `lendo /caminho/arquivo.iso (12,3 GiB de 205,0 GiB)`.
#[must_use]
pub fn lendo_texto(l: &LeituraEmCurso) -> String {
    format!(
        "lendo {} ({} de {})",
        l.caminho,
        bytes_humanos(l.bytes_lidos),
        bytes_humanos(l.bytes_total)
    )
}

/// Totais da etapa de hash.
#[derive(Debug, Clone, PartialEq)]
pub struct PainelHash {
    pub arquivos_total: u64,
    pub arquivos_feitos: u64,
    pub pendentes: u64,
    /// Arquivos da execução que já têm hash (feitos menos os que deram erro).
    pub com_hash: u64,
    pub reaproveitados: u64,
    pub erros: u64,
    pub pct_arquivos: Option<f64>,
    pub bytes_lidos: u64,
    pub bytes_total: u64,
    pub pct_bytes: Option<f64>,
    pub arquivos_por_min: Option<f64>,
    pub bytes_por_seg: f64,
    /// Tempo restante global em segundos; `None` = "estimando…".
    pub eta_s: Option<u64>,
    pub discos: Vec<PainelDisco>,
    pub threads_por_disco: u32,
    pub max_discos: u32,
}

/// Totais da etapa "Conteúdo de contêineres" (a tabela por disco reaproveita [`PainelHash`]:
/// "arquivos" são Contêineres).
#[derive(Debug, Clone, PartialEq)]
pub struct PainelConteineres {
    pub leitura: PainelHash,
    pub membros: u64,
    pub aguardando_senha: u64,
    pub falhas: u64,
    pub copiados: u64,
}

/// O painel.
#[derive(Debug, Clone, PartialEq)]
pub struct PainelJob {
    pub titulo: String,
    pub etapa: EtapaPainel,
    pub onde: Onde,
    /// "GUI", "CLI", "MCP", "abertura do agente"...
    pub quem: Option<&'static str>,
    pub estado: Option<EstadoJob>,
    pub pausado: bool,
    pub decorrido_s: Option<u64>,
    pub hash: Option<PainelHash>,
    pub conteineres: Option<PainelConteineres>,
    pub inventario: Option<ContagemInventario>,
    pub catalogo: Option<TotaisCatalogoPainel>,
    /// Texto livre da última mudança de etapa (classificação, duplicados...).
    pub etapa_texto: Option<String>,
}

// ------------------------------------------------------------------ funções puras

/// Percentual 0..=100; `None` sem total.
#[must_use]
pub fn percentual(feito: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| (feito as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

/// `41,2%`; `—` sem valor.
#[must_use]
pub fn pct_texto(p: Option<f64>) -> String {
    p.map_or_else(|| "—".into(), |v| format!("{v:.1}%").replace('.', ","))
}

/// Tempo restante em segundos, ou `None` ("estimando…") se a janela de medição é curta demais ou
/// não há vazão. Nunca inventa um valor sem amostras.
#[must_use]
pub fn eta_s(restante: u64, por_seg: f64, janela_ms: u64) -> Option<u64> {
    if janela_ms < JANELA_MINIMA_ETA_MS || !por_seg.is_finite() || por_seg <= 0.0 {
        return None;
    }
    if restante == 0 {
        return Some(0);
    }
    Some((restante as f64 / por_seg).ceil() as u64)
}

/// `1h 11min`, `12min 03s`, `45s`, `2d 3h`.
#[must_use]
pub fn duracao_curta(s: u64) -> String {
    let (d, h, m, seg) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60, s % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m:02}min")
    } else if m > 0 {
        format!("{m}min {seg:02}s")
    } else {
        format!("{seg}s")
    }
}

/// `7h 12min (estimado)` ou `estimando…`.
#[must_use]
pub fn eta_texto(eta: Option<u64>) -> String {
    eta.map_or_else(
        || "estimando…".into(),
        |s| format!("{} (estimado)", duracao_curta(s)),
    )
}

/// Vazão em bytes por segundo: `210,3 MiB/s`.
#[must_use]
pub fn vazao_texto(bps: f64) -> String {
    format!("{}/s", bytes_humanos(bps.max(0.0) as u64))
}

/// Barra de texto de `largura` células.
#[must_use]
pub fn barra_texto(pct: Option<f64>, largura: usize) -> String {
    let cheias = pct.map_or(0, |p| ((p / 100.0) * largura as f64).round() as usize);
    let cheias = cheias.min(largura);
    format!("[{}{}]", "█".repeat(cheias), "░".repeat(largura - cheias))
}

fn origem_texto(o: OrigemJob) -> &'static str {
    match o {
        OrigemJob::Gui => "GUI",
        OrigemJob::Cli => "CLI",
        OrigemJob::Mcp => "MCP",
        OrigemJob::Abertura => "abertura do agente",
        OrigemJob::Monitor => "monitor",
    }
}

fn disco_do_evento(d: &ProgressoDiscoHash, h: &ProgressoHash, pausado: bool) -> PainelDisco {
    let estado = if d.concluido {
        EstadoDisco::Concluido
    } else if pausado && d.ativo {
        EstadoDisco::Pausado
    } else if d.ativo && d.leitores_ativos > 0 {
        EstadoDisco::Lendo
    } else {
        EstadoDisco::Aguardando
    };
    let restante = d.bytes_total.saturating_sub(d.bytes_lidos);
    PainelDisco {
        nome: d.disco.clone(),
        volumes: d.volumes.clone(),
        threads: d.threads,
        leitores_ativos: d.leitores_ativos,
        feitos: d.arquivos_feitos,
        total: d.arquivos_total,
        pct: percentual(d.arquivos_feitos, d.arquivos_total),
        bytes_por_seg: d.bytes_por_seg,
        estado,
        eta_s: match estado {
            EstadoDisco::Concluido => None,
            _ => eta_s(restante, d.bytes_por_seg, h.janela_ms),
        },
        lendo: d.lendo.clone(),
    }
}

fn painel_hash(h: &ProgressoHash, pausado: bool) -> PainelHash {
    let pendentes = h.arquivos_total.saturating_sub(h.arquivos_feitos);
    let restante = h.bytes_total.saturating_sub(h.bytes_lidos);
    // Sem bytes a ler (só reaproveitados), o tempo restante em arquivos serve.
    let eta = if h.bytes_total > 0 {
        eta_s(restante, h.bytes_por_seg, h.janela_ms)
    } else {
        eta_s(pendentes, h.arquivos_por_seg, h.janela_ms)
    };
    PainelHash {
        arquivos_total: h.arquivos_total,
        arquivos_feitos: h.arquivos_feitos,
        pendentes,
        com_hash: h.arquivos_feitos.saturating_sub(h.erros),
        reaproveitados: h.reaproveitados,
        erros: h.erros,
        pct_arquivos: percentual(h.arquivos_feitos, h.arquivos_total),
        bytes_lidos: h.bytes_lidos,
        bytes_total: h.bytes_total,
        pct_bytes: percentual(h.bytes_lidos, h.bytes_total),
        arquivos_por_min: (h.janela_ms >= JANELA_MINIMA_ETA_MS)
            .then_some(h.arquivos_por_seg * 60.0),
        bytes_por_seg: h.bytes_por_seg,
        eta_s: eta,
        discos: h
            .discos
            .iter()
            .map(|d| disco_do_evento(d, h, pausado))
            .collect(),
        threads_por_disco: h.threads_por_disco,
        max_discos: h.max_discos,
    }
}

fn titulo(e: &EntradaPainel<'_>, etapa: EtapaPainel) -> String {
    let hash = matches!(
        etapa,
        EtapaPainel::Hash | EtapaPainel::PreparandoHash | EtapaPainel::Verificacao
    );
    let exec = e.execucao.map(|x| format!("#{x}"));
    if etapa == EtapaPainel::Conteineres {
        return "Conteúdo de contêineres (compactados)".into();
    }
    if let Some(p) = e.passo {
        match p.tipo {
            TipoPasso::Retomada => {
                let alvo = if hash || !p.no_inventario {
                    "hash"
                } else {
                    "inventário"
                };
                let id = p.execucao.or(e.execucao);
                let modo = if p.modo.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", p.modo)
                };
                return match id {
                    Some(id) => format!("Retomada do {alvo} da execução #{id}{modo}"),
                    None => format!("Retomada do {alvo}{modo}"),
                };
            }
            TipoPasso::Verificacao => {
                let base = format!(
                    "Fast Scan de verificação, passada {} de {}",
                    p.passada.max(1),
                    p.de.max(1)
                );
                return if hash {
                    format!("{base} — hash do que é novo ou mudou")
                } else {
                    base
                };
            }
        }
    }
    match e.tipo {
        Some(TipoJob::Abertura) => "Conferindo o Catálogo ao abrir".into(),
        Some(TipoJob::Retomada) => {
            let alvo = if hash { "hash" } else { "inventário" };
            match &exec {
                Some(x) => format!("Retomada do {alvo} da execução {x}"),
                None => format!("Retomada do {alvo}"),
            }
        }
        Some(TipoJob::VerificacaoIntegridade) => "Verificação de integridade".into(),
        Some(TipoJob::FastScan) => {
            if hash {
                "Fast Scan — hash do que é novo ou mudou".into()
            } else {
                "Fast Scan (reconciliação)".into()
            }
        }
        Some(TipoJob::Monitor) => "Monitor".into(),
        Some(TipoJob::Duplicados) => "Agrupando duplicados".into(),
        Some(TipoJob::Conteineres) => "Conteúdo de contêineres".into(),
        Some(TipoJob::FullScan) | None => {
            if etapa == EtapaPainel::Verificacao {
                "Verificação de integridade".into()
            } else if hash {
                "Hash Completo (Full Scan)".into()
            } else if e.sem_hash {
                "Inventário (Full Scan sem hash)".into()
            } else {
                "Inventário (Full Scan)".into()
            }
        }
    }
}

impl PainelJob {
    /// Monta o painel. A etapa mais avançada com dados manda: hash vale mais que inventário (o
    /// hash só começa depois do inventário), e nunca se mostra texto de inventário durante o hash.
    #[must_use]
    pub fn montar(e: &EntradaPainel<'_>) -> Self {
        let duplicados = e.etapa_texto.and_then(etapa_de_duplicados);
        let etapa = match (e.hash, e.inventario, e.estado) {
            _ if duplicados.is_some() => EtapaPainel::Duplicados,
            _ if e.conteineres.is_some() => EtapaPainel::Conteineres,
            (Some(_), _, _) if e.verificacao => EtapaPainel::Verificacao,
            (Some(h), _, _) if h.preparando => EtapaPainel::PreparandoHash,
            (Some(_), _, _) => EtapaPainel::Hash,
            (None, Some(_), _) if e.reconciliacao => EtapaPainel::Reconciliacao,
            (None, Some(_), _) => EtapaPainel::Inventario,
            (None, None, Some(EstadoJob::Enfileirado)) => EtapaPainel::NaFila,
            (None, None, _) => EtapaPainel::Preparando,
        };
        let hash = e.hash.map(|h| painel_hash(h, e.pausado));
        let conteineres = e.conteineres.map(|c| PainelConteineres {
            leitura: painel_hash(&c.andamento, e.pausado),
            membros: c.membros,
            aguardando_senha: c.aguardando_senha,
            falhas: c.falhas,
            copiados: c.copiados,
        });
        Self {
            titulo: match duplicados {
                // Na rotina de abertura o passo continua sendo o contexto ("Retomada do hash da
                // execução #3 — agrupando duplicados").
                Some(nome) if e.passo.is_some() => {
                    format!("{} — {}", titulo(e, etapa), nome.to_lowercase())
                }
                Some(nome) => nome.to_string(),
                None => titulo(e, etapa),
            },
            etapa,
            onde: e.onde,
            quem: e.origem.map(origem_texto),
            estado: e.estado,
            pausado: e.pausado || e.estado == Some(EstadoJob::Pausado),
            decorrido_s: e.decorrido_s,
            hash,
            conteineres,
            // O inventário só aparece enquanto é a etapa em curso.
            inventario: match etapa {
                EtapaPainel::Inventario | EtapaPainel::Reconciliacao => e.inventario,
                _ => None,
            },
            catalogo: e.catalogo,
            etapa_texto: e.etapa_texto.map(str::to_string),
        }
    }

    /// Nome curto da etapa de agrupamento em curso (só faz sentido com `EtapaPainel::Duplicados`).
    fn nome_do_agrupamento(&self) -> &'static str {
        self.etapa_texto
            .as_deref()
            .and_then(etapa_de_duplicados)
            .unwrap_or("Agrupando duplicados")
    }

    /// Percentual global quando existe denominador (hash); inventário não tem.
    #[must_use]
    pub fn pct_global(&self) -> Option<f64> {
        if let Some(c) = &self.conteineres {
            return c.leitura.pct_arquivos;
        }
        self.hash.as_ref().and_then(|h| h.pct_arquivos)
    }

    /// Uma linha para a barra de status.
    #[must_use]
    pub fn linha_status(&self) -> String {
        let mut t = self.titulo.clone();
        if self.pausado {
            t.push_str(" — pausado");
        }
        if let Some(c) = &self.conteineres {
            t.push_str(&format!(
                " — {} ({} de {} contêineres, {} membros) · {}",
                pct_texto(c.leitura.pct_arquivos),
                inteiro_pt_br(c.leitura.arquivos_feitos),
                inteiro_pt_br(c.leitura.arquivos_total),
                inteiro_pt_br(c.membros),
                vazao_texto(c.leitura.bytes_por_seg)
            ));
            if let Some(s) = c.leitura.eta_s {
                t.push_str(&format!(" · faltam ~{}", duracao_curta(s)));
            }
            return t;
        }
        match (&self.hash, &self.inventario) {
            (Some(h), _) if self.etapa == EtapaPainel::Duplicados => {
                t.push_str(&format!(
                    "… — hash concluído: {} de {}, {} erro(s)",
                    inteiro_pt_br(h.com_hash),
                    inteiro_pt_br(h.arquivos_total),
                    inteiro_pt_br(h.erros)
                ));
            }
            (None, _) if self.etapa == EtapaPainel::Duplicados => t.push('…'),
            (Some(_), _) if self.etapa == EtapaPainel::PreparandoHash => {
                t.push_str(" — montando a fila…");
            }
            (Some(h), _) => {
                t.push_str(&format!(
                    " — {} ({} de {}) · {}",
                    pct_texto(h.pct_arquivos),
                    inteiro_pt_br(h.com_hash),
                    inteiro_pt_br(h.arquivos_total),
                    vazao_texto(h.bytes_por_seg)
                ));
                if let Some(s) = h.eta_s {
                    t.push_str(&format!(" · faltam ~{}", duracao_curta(s)));
                }
            }
            (None, Some(i)) => t.push_str(&format!(
                " — {} pastas, {} arquivos (sem total ainda)",
                inteiro_pt_br(i.pastas),
                inteiro_pt_br(i.arquivos)
            )),
            (None, None) => {}
        }
        t
    }

    /// Uma linha curta para listas de trabalhos.
    #[must_use]
    pub fn resumo(&self) -> String {
        if let Some(c) = &self.conteineres {
            return format!(
                "contêineres {} de {} ({}), {} membros, {} lidos",
                inteiro_pt_br(c.leitura.arquivos_feitos),
                inteiro_pt_br(c.leitura.arquivos_total),
                pct_texto(c.leitura.pct_arquivos),
                inteiro_pt_br(c.membros),
                bytes_humanos(c.leitura.bytes_lidos)
            );
        }
        match (&self.hash, &self.inventario) {
            (Some(h), _) if self.etapa == EtapaPainel::Duplicados => format!(
                "{}… (hash concluído: {} de {} arquivos, {} erro(s))",
                self.nome_do_agrupamento(),
                inteiro_pt_br(h.com_hash),
                inteiro_pt_br(h.arquivos_total),
                inteiro_pt_br(h.erros)
            ),
            (None, _) if self.etapa == EtapaPainel::Duplicados => {
                format!("{}…", self.nome_do_agrupamento())
            }
            (Some(_), _) if self.etapa == EtapaPainel::PreparandoHash => {
                "montando a fila de hash…".into()
            }
            (Some(h), _) => format!(
                "hash {} de {} arquivos ({}), {} de {} lidos",
                inteiro_pt_br(h.com_hash),
                inteiro_pt_br(h.arquivos_total),
                pct_texto(h.pct_arquivos),
                bytes_humanos(h.bytes_lidos),
                bytes_humanos(h.bytes_total)
            ),
            (None, Some(i)) => format!(
                "inventário: {} pastas, {} arquivos (sem total ainda)",
                inteiro_pt_br(i.pastas),
                inteiro_pt_br(i.arquivos)
            ),
            (None, None) => self.etapa_texto.clone().unwrap_or_default(),
        }
    }

    /// O painel completo em texto (CLI e MCP), uma linha por item.
    #[must_use]
    pub fn linhas(&self) -> Vec<String> {
        let mut v = Vec::new();
        let mut cab = format!("Em andamento: {}", self.titulo);
        if self.pausado {
            cab.push_str(" [pausado]");
        }
        if let Some(s) = self.decorrido_s {
            cab.push_str(&format!(" — há {}", duracao_curta(s)));
        }
        v.push(cab);
        let mut onde = format!("  Onde: {}", self.onde.texto());
        if let Some(q) = self.quem {
            onde.push_str(&format!(" · iniciado por: {q}"));
        }
        v.push(onde);
        match self.etapa {
            EtapaPainel::NaFila => {
                v.push("  Na fila do agente, esperando o trabalho em curso.".into())
            }
            EtapaPainel::Preparando => v.push(format!(
                "  Preparando (descobrindo volumes e conferindo o Catálogo){}",
                self.etapa_texto
                    .as_deref()
                    .map_or_else(String::new, |t| format!(": {t}"))
            )),
            EtapaPainel::PreparandoHash => v.push(
                "  Montando a fila de hash (pode levar minutos em Catálogos grandes)…".into(),
            ),
            EtapaPainel::Duplicados => v.push(format!(
                "  Etapa atual: {}… (pode levar minutos em Catálogos grandes). O hash abaixo já terminou.",
                self.nome_do_agrupamento().to_lowercase()
            )),
            _ => {}
        }
        if let Some(i) = &self.inventario {
            v.push(format!(
                "  Encontrados até agora (ainda sem total): {} pastas, {} arquivos, {} | {} itens/s | {} pastas por visitar",
                inteiro_pt_br(i.pastas),
                inteiro_pt_br(i.arquivos),
                bytes_humanos(i.bytes),
                inteiro_pt_br(i.itens_por_s as u64),
                inteiro_pt_br(i.pendentes)
            ));
            if i.erros > 0 {
                v.push(format!("  Erros: {}", inteiro_pt_br(i.erros)));
            }
        }
        if let Some(c) = &self.conteineres {
            v.push(format!(
                "  Conteúdo de contêineres: {} de {} lidos ({}) · {} membros · {} aguardando senha · {} com falha de leitura · {} cópias idênticas reaproveitadas",
                inteiro_pt_br(c.leitura.arquivos_feitos),
                inteiro_pt_br(c.leitura.arquivos_total),
                pct_texto(c.leitura.pct_arquivos),
                inteiro_pt_br(c.membros),
                inteiro_pt_br(c.aguardando_senha),
                inteiro_pt_br(c.falhas),
                inteiro_pt_br(c.copiados)
            ));
            v.extend(
                linhas_hash(&c.leitura)
                    .into_iter()
                    .filter(|l| !l.starts_with("  Arquivos da execução")),
            );
        } else if let Some(h) = self
            .hash
            .as_ref()
            .filter(|h| self.etapa != EtapaPainel::PreparandoHash || h.arquivos_total > 0)
        {
            v.extend(linhas_hash(h));
        }
        if let Some(c) = &self.catalogo {
            v.push(format!(
                "  Catálogo: {} arquivos{}",
                inteiro_pt_br(c.arquivos),
                c.com_hash.map_or_else(String::new, |h| format!(
                    ", {} com hash ({})",
                    inteiro_pt_br(h),
                    pct_texto(percentual(h, c.arquivos))
                ))
            ));
        }
        v
    }
}

fn linhas_hash(h: &PainelHash) -> Vec<String> {
    let mut v = Vec::new();
    if h.threads_por_disco > 0 || h.max_discos > 0 {
        v.push(format!(
            "  Configuração: {} thread(s) por disco, até {} disco(s) em paralelo",
            h.threads_por_disco, h.max_discos
        ));
    }
    v.push(format!(
        "  Arquivos da execução: {} com hash de {} ({}) · pendentes {} · reaproveitados {} · erros {}",
        inteiro_pt_br(h.com_hash),
        inteiro_pt_br(h.arquivos_total),
        pct_texto(h.pct_arquivos),
        inteiro_pt_br(h.pendentes),
        inteiro_pt_br(h.reaproveitados),
        inteiro_pt_br(h.erros)
    ));
    v.push(format!(
        "  {} {}",
        barra_texto(h.pct_arquivos, 30),
        pct_texto(h.pct_arquivos)
    ));
    v.push(format!(
        "  Bytes lidos nesta etapa: {} de {} ({})",
        bytes_humanos(h.bytes_lidos),
        bytes_humanos(h.bytes_total),
        pct_texto(h.pct_bytes)
    ));
    v.push(format!(
        "  Vazão: {} · {} · tempo restante: {}",
        h.arquivos_por_min.map_or_else(
            || "estimando arquivos/min…".to_string(),
            |a| format!("{} arquivos/min", inteiro_pt_br(a as u64))
        ),
        vazao_texto(h.bytes_por_seg),
        eta_texto(h.eta_s)
    ));
    if !h.discos.is_empty() {
        let cab = [
            "DISCO", "VOLUMES", "THREADS", "ARQUIVOS", "%", "MB/s", "ESTADO", "ETA",
        ];
        let linhas: Vec<Vec<String>> = h
            .discos
            .iter()
            .map(|d| {
                vec![
                    d.nome.clone(),
                    d.volumes
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                    format!("{}/{}", d.leitores_ativos, d.threads),
                    format!("{}/{}", inteiro_pt_br(d.feitos), inteiro_pt_br(d.total)),
                    pct_texto(d.pct),
                    format!("{:.1}", d.bytes_por_seg / (1024.0 * 1024.0)).replace('.', ","),
                    d.estado.rotulo().to_string(),
                    match d.estado {
                        EstadoDisco::Concluido => "—".to_string(),
                        _ => d
                            .eta_s
                            .map_or_else(|| "estimando…".to_string(), duracao_curta),
                    },
                ]
            })
            .collect();
        v.push("  Por disco físico (threads = leitores ativos/configurados):".into());
        for l in tabela_texto(
            &cab,
            &[false, false, true, true, true, true, false, true],
            &linhas,
        ) {
            v.push(format!("    {l}"));
        }
        // Arquivos grandes: o contador de arquivos só anda quando um termina, então mostra o
        // andamento de dentro do arquivo.
        for d in &h.discos {
            for l in &d.lendo {
                v.push(format!("    {}: {}", d.nome, lendo_texto(l)));
            }
        }
    }
    v
}

/// Tabela de texto alinhada (colunas separadas por dois espaços).
#[must_use]
pub fn tabela_texto(cab: &[&str], direita: &[bool], linhas: &[Vec<String>]) -> Vec<String> {
    let n = cab.len();
    let mut larg: Vec<usize> = cab.iter().map(|c| c.chars().count()).collect();
    for l in linhas {
        for (i, c) in l.iter().enumerate().take(n) {
            larg[i] = larg[i].max(c.chars().count());
        }
    }
    let fmt = |cols: Vec<&str>| {
        cols.iter()
            .enumerate()
            .map(|(i, c)| {
                let pad = larg[i].saturating_sub(c.chars().count());
                if direita.get(i).copied().unwrap_or(false) {
                    format!("{}{c}", " ".repeat(pad))
                } else {
                    format!("{c}{}", " ".repeat(pad))
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut out = vec![fmt(cab.to_vec())];
    for l in linhas {
        out.push(fmt(l.iter().map(String::as_str).collect()));
    }
    out
}

#[cfg(test)]
mod testes {
    use super::*;

    fn disco(
        nome: &str,
        feitos: u64,
        total: u64,
        lidos: u64,
        bytes: u64,
        bps: f64,
    ) -> ProgressoDiscoHash {
        ProgressoDiscoHash {
            disco: nome.into(),
            volumes: vec![1, 2],
            threads: 2,
            leitores_ativos: 2,
            arquivos_feitos: feitos,
            arquivos_total: total,
            bytes_lidos: lidos,
            bytes_total: bytes,
            bytes_por_seg: bps,
            ativo: true,
            concluido: false,
            lendo: Vec::new(),
        }
    }

    fn hash(janela_ms: u64) -> ProgressoHash {
        ProgressoHash {
            arquivos_feitos: 8_000_000,
            arquivos_total: 19_700_000,
            bytes_lidos: 5 << 40,
            bytes_total: 12 << 40,
            bytes_por_seg: 200.0 * 1024.0 * 1024.0,
            reaproveitados: 7_900_000,
            execucao: Some(1),
            threads_por_disco: 2,
            max_discos: 4,
            erros: 3,
            arquivos_por_seg: 200.0,
            janela_ms,
            discos: vec![
                disco(
                    "WD Red 4TB",
                    100,
                    400,
                    1 << 40,
                    4 << 40,
                    100.0 * 1024.0 * 1024.0,
                ),
                ProgressoDiscoHash {
                    concluido: true,
                    ativo: false,
                    ..disco("Seagate", 10, 10, 1, 1, 0.0)
                },
            ],
            ..ProgressoHash::default()
        }
    }

    fn entrada<'a>(
        h: Option<&'a ProgressoHash>,
        passo: Option<&'a PassoAbertura>,
    ) -> EntradaPainel<'a> {
        EntradaPainel {
            tipo: Some(TipoJob::Abertura),
            origem: Some(OrigemJob::Abertura),
            estado: Some(EstadoJob::Executando),
            execucao: Some(1),
            passo,
            hash: h,
            decorrido_s: Some(4272),
            catalogo: Some(TotaisCatalogoPainel {
                arquivos: 19_700_000,
                com_hash: Some(8_100_000),
            }),
            ..EntradaPainel::vazia(Onde::Agente { pid: Some(4242) })
        }
    }

    fn passo_retomada() -> PassoAbertura {
        PassoAbertura {
            tipo: TipoPasso::Retomada,
            execucao: Some(1),
            modo: "Full Scan".into(),
            no_inventario: false,
            passada: 0,
            de: 0,
        }
    }

    #[test]
    fn duracoes_e_eta() {
        assert_eq!(duracao_curta(45), "45s");
        assert_eq!(duracao_curta(723), "12min 03s");
        assert_eq!(duracao_curta(4272), "1h 11min");
        assert_eq!(duracao_curta(90_000), "1d 1h");
        assert_eq!(eta_s(1000, 10.0, 20_000), Some(100));
        assert_eq!(eta_s(1000, 10.0, 5_000), None, "poucas amostras");
        assert_eq!(eta_s(1000, 0.0, 60_000), None, "sem vazão");
        assert_eq!(eta_s(0, 0.0, 60_000), None);
        assert_eq!(eta_texto(None), "estimando…");
        assert_eq!(eta_texto(Some(4272)), "1h 11min (estimado)");
    }

    #[test]
    fn percentuais_e_barra() {
        assert_eq!(percentual(1, 4), Some(25.0));
        assert_eq!(percentual(1, 0), None);
        assert_eq!(percentual(9, 4), Some(100.0));
        assert_eq!(pct_texto(Some(41.23)), "41,2%");
        assert_eq!(pct_texto(None), "—");
        assert_eq!(barra_texto(Some(50.0), 10), "[█████░░░░░]");
        assert_eq!(barra_texto(None, 4), "[░░░░]");
    }

    #[test]
    fn titulo_da_retomada_de_hash_da_abertura() {
        let h = hash(30_000);
        let p = passo_retomada();
        let painel = PainelJob::montar(&entrada(Some(&h), Some(&p)));
        assert_eq!(painel.titulo, "Retomada do hash da execução #1 (Full Scan)");
        assert_eq!(painel.etapa, EtapaPainel::Hash);
        assert!(
            painel.inventario.is_none(),
            "nunca inventário durante o hash"
        );
    }

    #[test]
    fn titulos_das_outras_etapas() {
        let v = PassoAbertura {
            tipo: TipoPasso::Verificacao,
            passada: 1,
            de: 3,
            ..passo_retomada()
        };
        let inv = ContagemInventario {
            pastas: 5,
            arquivos: 9,
            ..Default::default()
        };
        let mut e = entrada(None, Some(&v));
        e.inventario = Some(inv);
        e.reconciliacao = true;
        assert_eq!(
            PainelJob::montar(&e).titulo,
            "Fast Scan de verificação, passada 1 de 3"
        );
        let mut e = EntradaPainel::vazia(Onde::Janela);
        e.tipo = Some(TipoJob::FullScan);
        e.inventario = Some(inv);
        let p = PainelJob::montar(&e);
        assert_eq!(p.titulo, "Inventário (Full Scan)");
        assert_eq!(p.etapa, EtapaPainel::Inventario);
        assert!(p.inventario.is_some());
        e.tipo = Some(TipoJob::VerificacaoIntegridade);
        assert_eq!(PainelJob::montar(&e).titulo, "Verificação de integridade");
        let h = hash(1);
        let mut e = EntradaPainel::vazia(Onde::Janela);
        e.hash = Some(&h);
        e.verificacao = true;
        assert_eq!(PainelJob::montar(&e).titulo, "Verificação de integridade");
        assert_eq!(PainelJob::montar(&e).etapa, EtapaPainel::Verificacao);
    }

    #[test]
    fn onde_e_quem() {
        let h = hash(30_000);
        let p = passo_retomada();
        let painel = PainelJob::montar(&entrada(Some(&h), Some(&p)));
        let t = painel.linhas().join("\n");
        assert!(
            t.contains("no agente em segundo plano (pid 4242) — pode fechar a janela"),
            "{t}"
        );
        assert!(t.contains("iniciado por: abertura do agente"), "{t}");
        let mut e = entrada(Some(&h), Some(&p));
        e.onde = Onde::Janela;
        let t = PainelJob::montar(&e).linhas().join("\n");
        assert!(
            t.contains("nesta janela (modo sem agente) — fechar interrompe"),
            "{t}"
        );
    }

    #[test]
    fn totais_do_hash_e_tabela_por_disco() {
        let h = hash(30_000);
        let p = passo_retomada();
        let painel = PainelJob::montar(&entrada(Some(&h), Some(&p)));
        let ph = painel.hash.as_ref().unwrap();
        assert_eq!(ph.com_hash, 7_999_997);
        assert_eq!(ph.pendentes, 11_700_000);
        assert_eq!(ph.discos[0].estado, EstadoDisco::Lendo);
        assert_eq!(ph.discos[1].estado, EstadoDisco::Concluido);
        assert!(ph.eta_s.is_some());
        let t = painel.linhas().join("\n");
        assert!(
            t.contains("2 thread(s) por disco, até 4 disco(s) em paralelo"),
            "{t}"
        );
        assert!(t.contains("7.999.997 com hash de 19.700.000"), "{t}");
        assert!(t.contains("pendentes 11.700.000"), "{t}");
        assert!(t.contains("reaproveitados 7.900.000"), "{t}");
        assert!(t.contains("erros 3"), "{t}");
        assert!(t.contains("WD Red 4TB"), "{t}");
        assert!(t.contains("2/2"), "{t}");
        assert!(t.contains("lendo"), "{t}");
        assert!(t.contains("concluído"), "{t}");
        assert!(
            t.contains("Catálogo: 19.700.000 arquivos, 8.100.000 com hash"),
            "{t}"
        );
        assert!(!t.contains("Encontrados até agora"), "{t}");
    }

    #[test]
    fn eta_global_diz_estimando_ate_ter_amostras() {
        let h = hash(2_000);
        let p = passo_retomada();
        let painel = PainelJob::montar(&entrada(Some(&h), Some(&p)));
        assert_eq!(painel.hash.as_ref().unwrap().eta_s, None);
        let t = painel.linhas().join("\n");
        assert!(t.contains("tempo restante: estimando…"), "{t}");
        assert!(painel.linha_status().contains("Retomada do hash"));
        assert!(!painel.linha_status().contains("faltam"));
    }

    #[test]
    fn inventario_mostra_contagens_sem_total() {
        let mut e = EntradaPainel::vazia(Onde::Janela);
        e.tipo = Some(TipoJob::FullScan);
        e.inventario = Some(ContagemInventario {
            pastas: 10,
            arquivos: 1234,
            bytes: 1 << 20,
            itens_por_s: 500.0,
            ..Default::default()
        });
        let t = PainelJob::montar(&e).linhas().join("\n");
        assert!(t.contains("ainda sem total"), "{t}");
        assert!(t.contains("1.234 arquivos"), "{t}");
        assert_eq!(PainelJob::montar(&e).pct_global(), None);
    }

    #[test]
    fn preparando_a_fila_nao_mostra_texto_de_inventario() {
        let h = ProgressoHash {
            preparando: true,
            ..ProgressoHash::default()
        };
        let p = passo_retomada();
        let painel = PainelJob::montar(&entrada(Some(&h), Some(&p)));
        assert_eq!(painel.etapa, EtapaPainel::PreparandoHash);
        let t = painel.linhas().join("\n");
        assert!(t.contains("Montando a fila de hash"), "{t}");
        assert!(!t.contains("inventário"), "{t}");
    }

    #[test]
    fn disco_aguardando_vaga() {
        let mut h = hash(30_000);
        h.discos.push(ProgressoDiscoHash {
            ativo: false,
            leitores_ativos: 0,
            threads: 0,
            ..disco("Ext", 0, 5, 0, 10, 0.0)
        });
        let p = passo_retomada();
        let painel = PainelJob::montar(&entrada(Some(&h), Some(&p)));
        assert_eq!(
            painel.hash.unwrap().discos[2].estado,
            EstadoDisco::Aguardando
        );
    }

    #[test]
    fn arquivo_grande_em_leitura_aparece_no_painel_com_gb_lidos() {
        let mut h = hash(30_000);
        h.discos[0].lendo = vec![LeituraEmCurso {
            caminho: "/mnt/a/backup.img".into(),
            bytes_lidos: 12 << 30,
            bytes_total: 205 << 30,
        }];
        let p = passo_retomada();
        let painel = PainelJob::montar(&entrada(Some(&h), Some(&p)));
        assert_eq!(painel.hash.as_ref().unwrap().discos[0].lendo.len(), 1);
        let t = painel.linhas().join("\n");
        assert!(
            t.contains("lendo /mnt/a/backup.img (12,0 GiB de 205,0 GiB)"),
            "{t}"
        );
    }
}
