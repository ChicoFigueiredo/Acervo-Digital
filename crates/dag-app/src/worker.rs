//! Worker da GUI (spec §6 e §7, ADR 0004): a GUI nunca escreve no SQLite e, por padrão, nunca
//! é o escritor.
//!
//! * **Modo agente (padrão)**: a GUI é cliente do agente em segundo plano (`dag agente`), que é
//!   o único escritor. Se não há agente, o worker o sobe (unidade systemd de usuário, se
//!   instalada; senão processo destacado). Os pedidos vão por conexões próprias, e o que a GUI
//!   mostra (varredura, abertura, monitor) vem da **assinatura** de eventos do agente, então
//!   enxerga e controla trabalhos iniciados por qualquer cliente (CLI, MCP, abertura do agente,
//!   monitor). Se o agente cai ou reinicia, o worker reconecta sozinho.
//! * **Modo em processo (`--sem-agente`, testes)**: uma thread dona do [`Agente`] (e da trava de
//!   escritor) recebe [`Pedido`]s por canal. Se outro processo já é o escritor, o worker abre em
//!   leitura e a GUI diz isso. Pausar, retomar e cancelar usam o [`ControleScan`] compartilhado.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dag_agent::{
    Agente, Cliente, ErroAgente, EventoObservado, Observador, OpcoesLancamento, OpcoesObservador,
    caminho_socket_padrao, garantir_agente, sem_agente_do_ambiente,
};
use dag_core::{
    Autor, Comando, EstadoJob, EventoAgente, FaseInventario, JobInfo, Pedido, Progresso, Resposta,
    SaudeMonitorInfo, StatusConfianca, TipoJob, VisaoJobs, VolumeDescoberto,
};
use dag_scanner::{ControleScan, EstadoControle};

use crate::leitor::Acordador;
use crate::progresso::ProgressoScan;

/// Como o worker chegou ao Catálogo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModoWorker {
    Iniciando,
    /// Sem agente (`--sem-agente`): esta GUI é a escritora, em processo.
    Escritor,
    /// Cliente do agente em segundo plano, o escritor único.
    Servico,
    /// A conexão com o agente caiu (ele encerrou ou reiniciou); o worker reconecta sozinho.
    Reconectando {
        motivo: String,
    },
    /// Outro processo é o escritor (ou o Catálogo só pôde ser aberto para leitura).
    SomenteLeitura {
        motivo: String,
    },
    /// Nem leitura foi possível, ou o agente não pôde ser iniciado.
    Falhou {
        motivo: String,
    },
    Encerrado,
}

impl ModoWorker {
    /// Texto do segmento da barra de status.
    #[must_use]
    pub fn texto_status(&self) -> String {
        match self {
            Self::Iniciando => "Agente: iniciando…".into(),
            Self::Escritor => "Worker: escritor (sem agente)".into(),
            Self::Servico => "Agente: conectado".into(),
            Self::Reconectando { .. } => "Agente desconectado, reconectando…".into(),
            Self::SomenteLeitura { .. } => "Worker: outro processo é o escritor".into(),
            Self::Falhou { .. } => "Agente: indisponível".into(),
            Self::Encerrado => "Worker: encerrado".into(),
        }
    }

    /// Esta GUI consegue pedir escritas (ela mesma ou o agente as executa)?
    #[must_use]
    pub fn pode_escrever(&self) -> bool {
        matches!(self, Self::Escritor | Self::Servico)
    }
}

/// Parâmetros do worker.
#[derive(Debug, Clone)]
pub struct ConfigWorker {
    pub caminho: PathBuf,
    /// Volumes fixos em vez da descoberta do sistema (testes; só no modo em processo).
    pub volumes_fixos: Option<Vec<VolumeDescoberto>>,
    /// Socket do agente em segundo plano; `None` = o padrão (`$DAG_SOCKET`, `$XDG_RUNTIME_DIR`).
    pub socket: Option<PathBuf>,
    /// Retomada de execuções interrompidas e verificação das Raízes ao abrir como escritor
    /// (ADR 0003); só vale no modo em processo, pois com o agente quem faz isso é ele. A GUI não
    /// monitora: nunca marca Ponto de Confiança.
    pub abertura: dag_agent::PoliticaAbertura,
    /// Não usa o agente: abre o Catálogo em processo (`--sem-agente`, testes).
    pub sem_agente: bool,
    /// Como lançar o agente quando não há um; `None` = valores do ambiente.
    pub lancamento: Option<OpcoesLancamento>,
}

impl ConfigWorker {
    #[must_use]
    pub fn novo(caminho: PathBuf) -> Self {
        Self {
            caminho,
            volumes_fixos: None,
            socket: None,
            abertura: dag_agent::PoliticaAbertura::default(),
            sem_agente: sem_agente_do_ambiente(),
            lancamento: None,
        }
    }
}

/// Eventos do worker para a GUI.
#[derive(Debug, Clone)]
pub enum Evento {
    /// O Catálogo foi aberto (ou não): modo e alça de controle do scan.
    Modo(ModoWorker),
    /// O worker começou a executar o pedido `id`.
    Iniciado { id: u64 },
    /// Inventário ou reconciliação do trabalho em andamento (seja de quem for).
    Progresso(ProgressoScan),
    /// Etapa de Hash Completo (ou verificação): arquivos e bytes reais, sem consultar o
    /// Catálogo.
    Hash(dag_core::ProgressoHash),
    /// Passo estruturado da abertura (retomada, passada de verificação): zera o andamento dos
    /// passos anteriores.
    Passo(dag_core::PassoAbertura),
    /// Resposta ao pedido `id`.
    Resposta { id: u64, resposta: Box<Resposta> },
    /// Recuperação ao abrir (execução interrompida retomada, verificação das Raízes). `fim`
    /// `false` = em andamento (faixa de aviso); `true` = relatório final.
    Abertura { texto: String, fim: bool },
    /// Todos os trabalhos do agente agora (em andamento e na fila), quem quer que os tenha
    /// iniciado. Chega no instantâneo e a cada mudança.
    Jobs(Vec<JobInfo>),
    /// Um trabalho terminou (varredura, retomada, verificação), com a resposta que o pedido dele
    /// recebeu; a GUI usa quando o trabalho não foi iniciado por ela.
    JobFinalizado {
        job: JobInfo,
        desfecho: String,
        resposta: Box<Resposta>,
    },
    /// Saúde do monitor do agente.
    Monitor(SaudeMonitorInfo),
    /// Ponto de Confiança por Raiz.
    Confianca(Vec<StatusConfianca>),
    /// Identificação do agente ao conectar (versão, commit, binário): a GUI compara com a sua.
    Versao(Box<dag_agent::InfoServidor>),
    /// O agente vai se reexecutar no lugar (binário atualizado ou pedido do usuário).
    AgenteReiniciando { de: String, para: String },
}

enum Mensagem {
    Pedido { id: u64, pedido: Pedido },
    Sair,
}

/// Alça para controlar o trabalho em curso.
#[derive(Debug, Clone)]
enum Alca {
    /// O `ControleScan` do agente de sessão (modo em processo).
    Local(ControleScan),
    /// O agente em segundo plano: pausar, retomar e cancelar vão por conexão própria, pelo id do
    /// trabalho.
    Remota { socket: PathBuf },
}

impl Alca {
    /// Pede ao agente; a confirmação chega pelo fluxo de eventos.
    fn agir_remoto(socket: &std::path::Path, comando: Comando) {
        let socket = socket.to_path_buf();
        // Conexão própria: não depende de nenhuma outra estar livre.
        let _ = std::thread::Builder::new()
            .name("dag-worker-controle".into())
            .spawn(move || {
                let _ = Cliente::conectar(&socket).and_then(|mut c| {
                    c.enviar(
                        Pedido {
                            autor: Autor::Gui,
                            comando,
                        },
                        &mut |_| {},
                    )
                });
            });
    }

    fn pausar(&self, job: Option<u64>) {
        match self {
            Self::Local(c) => c.pausar(),
            Self::Remota { socket } => Self::agir_remoto(
                socket,
                job.map_or(Comando::Pausar { execucao: None }, |job| {
                    Comando::PausarJob { job }
                }),
            ),
        }
    }

    fn continuar(&self, job: Option<u64>) {
        match self {
            Self::Local(c) => c.continuar(),
            Self::Remota { socket } => Self::agir_remoto(
                socket,
                job.map_or(
                    Comando::Retomar {
                        execucao: None,
                        threads_por_disco: None,
                        max_discos: None,
                    },
                    |job| Comando::RetomarJob { job },
                ),
            ),
        }
    }

    fn cancelar(&self, job: Option<u64>) {
        match self {
            Self::Local(c) => c.cancelar(),
            Self::Remota { socket } => Self::agir_remoto(
                socket,
                job.map_or(Comando::Cancelar { execucao: None }, |job| {
                    Comando::CancelarJob { job }
                }),
            ),
        }
    }

    fn eh_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }
}

/// Alça do worker, usada pela thread de interface.
pub struct Worker {
    tx: Sender<Mensagem>,
    rx: Receiver<Evento>,
    controle: Arc<Mutex<Option<Alca>>>,
    modo: ModoWorker,
    proximo_id: u64,
    thread: Option<JoinHandle<()>>,
    /// Último conjunto de trabalhos do agente (vazio no modo em processo).
    jobs: Vec<JobInfo>,
    monitor: Option<SaudeMonitorInfo>,
    confianca: Vec<StatusConfianca>,
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker").field("modo", &self.modo).finish()
    }
}

impl Worker {
    /// Sobe a thread; a abertura do Catálogo acontece nela (a GUI não bloqueia).
    #[must_use]
    pub fn iniciar(cfg: ConfigWorker, acordador: Acordador) -> Self {
        let (tx, rx_msg) = mpsc::channel();
        let (tx_ev, rx) = mpsc::channel();
        let controle = Arc::new(Mutex::new(None));
        let controle_thread = Arc::clone(&controle);
        let thread = std::thread::Builder::new()
            .name("dag-worker".into())
            .spawn(move || laco(&cfg, &rx_msg, &tx_ev, &controle_thread, &acordador))
            .ok();
        Self {
            tx,
            rx,
            controle,
            modo: ModoWorker::Iniciando,
            proximo_id: 1,
            thread,
            jobs: Vec::new(),
            monitor: None,
            confianca: Vec::new(),
        }
    }

    #[must_use]
    pub fn modo(&self) -> &ModoWorker {
        &self.modo
    }

    /// Os trabalhos do agente neste instante (em andamento e na fila).
    #[must_use]
    pub fn jobs(&self) -> &[JobInfo] {
        &self.jobs
    }

    /// O trabalho que a GUI acompanha: o primeiro que não é o monitor e já saiu da fila.
    #[must_use]
    pub fn job_em_foco(&self) -> Option<&JobInfo> {
        dag_core::job_em_foco(&self.jobs)
    }

    #[must_use]
    pub fn monitor(&self) -> Option<&SaudeMonitorInfo> {
        self.monitor.as_ref()
    }

    #[must_use]
    pub fn confianca(&self) -> &[StatusConfianca] {
        &self.confianca
    }

    /// Enfileira um comando (autor GUI) e devolve o id do pedido.
    pub fn enviar(&mut self, comando: Comando) -> u64 {
        let id = self.proximo_id;
        self.proximo_id += 1;
        let pedido = Pedido {
            autor: Autor::Gui,
            comando,
        };
        let _ = self.tx.send(Mensagem::Pedido { id, pedido });
        id
    }

    fn alca(&self) -> Option<Alca> {
        self.controle.lock().ok().and_then(|g| g.clone())
    }

    /// Estado do trabalho em foco: no agente vem do fluxo de eventos; em processo, do controle.
    #[must_use]
    pub fn estado_controle(&self) -> Option<EstadoControle> {
        match self.alca()? {
            Alca::Local(c) => Some(c.estado()),
            Alca::Remota { .. } => Some(match self.job_em_foco().map(|j| j.estado) {
                Some(EstadoJob::Pausado) => EstadoControle::Pausado,
                Some(EstadoJob::Cancelando) => EstadoControle::Cancelado,
                _ => EstadoControle::Executando,
            }),
        }
    }

    fn id_em_foco(&self) -> Option<u64> {
        self.job_em_foco().map(|j| j.id)
    }

    /// Pede ao agente em segundo plano que se reexecute no lugar (sem efeito no modo em
    /// processo). A conexão cai e volta sozinha, já na versão nova.
    pub fn reiniciar_agente(&self) {
        if let Some(Alca::Remota { socket }) = self.alca() {
            Alca::agir_remoto(&socket, Comando::ReiniciarAgente);
        }
    }

    pub fn pausar(&self) {
        if let Some(a) = self.alca() {
            a.pausar(self.id_em_foco());
        }
    }

    pub fn continuar(&self) {
        if let Some(a) = self.alca() {
            a.continuar(self.id_em_foco());
        }
    }

    pub fn cancelar(&self) {
        if let Some(a) = self.alca() {
            a.cancelar(self.id_em_foco());
        }
    }

    /// Cancela um trabalho do agente pelo id (um que ainda está na fila, por exemplo).
    pub fn cancelar_job(&self, job: u64) {
        if let Some(a) = self.alca() {
            a.cancelar(Some(job));
        }
    }

    /// Ao sair, a varredura da própria sessão (modo em processo) é cancelada com checkpoint; a
    /// do agente continua (ela não pertence a esta janela).
    fn cancelar_ao_sair(&self) {
        if let Some(a) = self.alca().filter(Alca::eh_local) {
            a.cancelar(None);
        }
    }

    /// Recolhe todos os eventos já disponíveis, sem bloquear.
    pub fn drenar(&mut self) -> Vec<Evento> {
        let mut v = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            self.aplicar(&e);
            v.push(e);
        }
        v
    }

    /// Espera um evento por até `limite` (testes).
    pub fn esperar(&mut self, limite: Duration) -> Option<Evento> {
        match self.rx.recv_timeout(limite) {
            Ok(e) => {
                self.aplicar(&e);
                Some(e)
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
        }
    }

    fn aplicar(&mut self, e: &Evento) {
        match e {
            Evento::Modo(m) => self.modo = m.clone(),
            Evento::Jobs(j) => self.jobs.clone_from(j),
            Evento::Monitor(m) => self.monitor = Some(m.clone()),
            Evento::Confianca(c) => self.confianca.clone_from(c),
            _ => {}
        }
    }

    /// Cancela o que estiver rodando (se for desta sessão), pede para sair e espera a thread
    /// (solta a trava no modo em processo).
    pub fn encerrar(mut self) {
        self.cancelar_ao_sair();
        let _ = self.tx.send(Mensagem::Sair);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.modo = ModoWorker::Encerrado;
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Fechar a janela: no modo em processo cancela com checkpoint; com o agente nada é
        // cancelado (o trabalho é dele) e a thread só deixa de acompanhar.
        self.cancelar_ao_sair();
        let _ = self.tx.send(Mensagem::Sair);
    }
}

/// Onde os pedidos são executados no modo em processo.
#[allow(clippy::large_enum_variant)]
enum Destino {
    Sessao(Agente),
    Nenhum,
}

fn abrir_em_processo(cfg: &ConfigWorker, enc: &Arc<Encaminhador>) -> (Destino, ModoWorker) {
    let sessao = |a: Agente| {
        let e = Arc::clone(enc);
        let mut a = a
            .com_autor(Autor::Gui)
            .com_progresso(move |p: &Progresso| e.encaminhar(p));
        if let Some(v) = &cfg.volumes_fixos {
            a = a.com_volumes_fixos(v.clone());
        }
        Destino::Sessao(a)
    };
    match Agente::abrir(&cfg.caminho) {
        Ok(a) => (sessao(a), ModoWorker::Escritor),
        Err(ErroAgente::EscritorOcupado(_)) => match Agente::abrir_leitura(&cfg.caminho) {
            Ok(a) => (
                sessao(a),
                ModoWorker::SomenteLeitura {
                    motivo: "outro processo do DAG é o escritor deste Catálogo".into(),
                },
            ),
            Err(e) => (
                Destino::Nenhum,
                ModoWorker::Falhou {
                    motivo: e.to_string(),
                },
            ),
        },
        Err(e) => (
            Destino::Nenhum,
            ModoWorker::Falhou {
                motivo: e.to_string(),
            },
        ),
    }
}

/// Converte eventos de progresso do agente de sessão em eventos da GUI, com cadência limitada
/// (~8 por segundo por tipo, mais toda troca de fase e o fim do hash).
struct Encaminhador {
    tx: Sender<Evento>,
    acordador: Acordador,
    estado: Mutex<EstadoEnc>,
}

struct EstadoEnc {
    ultimo_inventario: Instant,
    fase: Option<FaseInventario>,
    ultimo_hash: Instant,
}

const CADENCIA: Duration = Duration::from_millis(125);

impl Encaminhador {
    fn novo(tx: Sender<Evento>, acordador: Acordador) -> Self {
        let agora = Instant::now();
        Self {
            tx,
            acordador,
            estado: Mutex::new(EstadoEnc {
                ultimo_inventario: agora.checked_sub(CADENCIA).unwrap_or(agora),
                fase: None,
                ultimo_hash: agora.checked_sub(CADENCIA).unwrap_or(agora),
            }),
        }
    }

    fn encaminhar(&self, p: &Progresso) {
        let Ok(mut g) = self.estado.lock() else {
            return;
        };
        let ev = match p {
            Progresso::Inventario(i) | Progresso::Reconciliacao(i) => {
                let troca = g.fase != Some(i.fase);
                if !troca && g.ultimo_inventario.elapsed() < CADENCIA {
                    return;
                }
                g.fase = Some(i.fase);
                g.ultimo_inventario = Instant::now();
                Evento::Progresso(ProgressoScan::from(i))
            }
            Progresso::Hash(h) | Progresso::Verificacao(h) => {
                // A montagem da fila e o primeiro quadro com os totais passam sempre.
                let fim = h.preparando
                    || h.janela_ms == 0
                    || (h.arquivos_total > 0 && h.arquivos_feitos >= h.arquivos_total);
                if !fim && g.ultimo_hash.elapsed() < CADENCIA {
                    return;
                }
                g.ultimo_hash = Instant::now();
                Evento::Hash(h.clone())
            }
            Progresso::Passo(p) => Evento::Passo(p.clone()),
            // O andamento dos contêineres chega pelo painel do trabalho (JobInfo).
            Progresso::Conteineres(_) => return,
            // Passos da recuperação ao abrir (retomada, passadas de verificação) viram o texto da
            // faixa/barra de status; as demais mensagens são só do terminal.
            Progresso::Mensagem { texto } => match texto.strip_prefix("abertura: ") {
                Some(t) => Evento::Abertura {
                    texto: t.to_string(),
                    fim: false,
                },
                None => return,
            },
        };
        drop(g);
        let _ = self.tx.send(ev);
        self.acordador.acordar();
    }
}

const TEXTO_ABERTURA: &str = "Conferindo o Catálogo: execuções interrompidas e mudanças no disco…";

/// Traduz o fluxo do agente nos eventos da GUI, mantendo a visão dos trabalhos.
#[derive(Default)]
struct Tradutor {
    visao: VisaoJobs,
    ja_conectou: bool,
}

impl Tradutor {
    /// Progresso numérico do trabalho em foco (o único que executa).
    fn progresso_de(p: &Progresso, saida: &mut Vec<Evento>) {
        match p {
            Progresso::Inventario(i) | Progresso::Reconciliacao(i) => {
                saida.push(Evento::Progresso(ProgressoScan::from(i)));
            }
            Progresso::Hash(h) | Progresso::Verificacao(h) => saida.push(Evento::Hash(h.clone())),
            Progresso::Passo(p) => saida.push(Evento::Passo(p.clone())),
            Progresso::Mensagem { .. } | Progresso::Conteineres(_) => {}
        }
    }

    fn texto_abertura(etapa: Option<&str>) -> String {
        etapa
            .and_then(|e| e.strip_prefix("abertura: "))
            .unwrap_or(TEXTO_ABERTURA)
            .to_string()
    }

    fn traduzir(&mut self, ev: EventoObservado) -> Vec<Evento> {
        let mut out = Vec::new();
        match ev {
            EventoObservado::Conectado(info) => {
                self.ja_conectou = true;
                out.push(Evento::Versao(Box::new(info)));
                out.push(Evento::Modo(ModoWorker::Servico));
            }
            EventoObservado::Desconectado { motivo } => {
                // Sem conexão não se sabe o que roda: a visão zera até o próximo instantâneo.
                self.visao = VisaoJobs::default();
                if self.ja_conectou {
                    out.push(Evento::Modo(ModoWorker::Reconectando { motivo }));
                    out.push(Evento::Jobs(Vec::new()));
                }
            }
            EventoObservado::Evento(e) => self.evento(&e, &mut out),
        }
        out
    }

    fn evento(&mut self, e: &EventoAgente, out: &mut Vec<Evento>) {
        let fim = self.visao.aplicar(e);
        match e {
            EventoAgente::Instantaneo {
                monitor, confianca, ..
            } => {
                out.push(Evento::Jobs(self.visao.jobs.clone()));
                if let Some(a) = self.visao.jobs.iter().find(|j| j.tipo == TipoJob::Abertura) {
                    out.push(Evento::Abertura {
                        texto: Self::texto_abertura(a.etapa.as_deref()),
                        fim: false,
                    });
                }
                if let Some(f) = self.visao.em_foco() {
                    if let Some(p) = &f.passo {
                        out.push(Evento::Passo(p.clone()));
                    }
                    for p in &f.progresso {
                        Self::progresso_de(p, out);
                    }
                }
                if let Some(m) = monitor {
                    out.push(Evento::Monitor(m.clone()));
                }
                out.push(Evento::Confianca(confianca.clone()));
            }
            EventoAgente::JobEnfileirado { .. } | EventoAgente::JobAtualizado { .. } => {
                out.push(Evento::Jobs(self.visao.jobs.clone()));
            }
            EventoAgente::JobIniciado { job } => {
                out.push(Evento::Jobs(self.visao.jobs.clone()));
                if job.tipo == TipoJob::Abertura {
                    out.push(Evento::Abertura {
                        texto: TEXTO_ABERTURA.into(),
                        fim: false,
                    });
                }
            }
            EventoAgente::JobProgresso {
                job, progresso: p, ..
            } => {
                let Some(j) = self.visao.job(*job) else {
                    return;
                };
                if j.tipo == TipoJob::Abertura
                    && let Progresso::Mensagem { texto } = p
                    && let Some(t) = texto.strip_prefix("abertura: ")
                {
                    out.push(Evento::Abertura {
                        texto: t.to_string(),
                        fim: false,
                    });
                }
                if matches!(p, Progresso::Passo(_)) {
                    // O passo vive no trabalho: a GUI o lê da lista de trabalhos também.
                    out.push(Evento::Jobs(self.visao.jobs.clone()));
                }
                if self.visao.em_foco().is_some_and(|f| f.id == *job) {
                    Self::progresso_de(p, out);
                }
            }
            EventoAgente::JobFinalizado {
                job,
                desfecho,
                resposta,
            } => {
                out.push(Evento::Jobs(self.visao.jobs.clone()));
                if let Some(j) = fim {
                    match j.tipo {
                        TipoJob::Abertura => {
                            let texto = match &**resposta {
                                Resposta::Ok { mensagem } => mensagem.clone().unwrap_or_default(),
                                Resposta::Erro { mensagem, .. } => {
                                    format!("Recuperação ao abrir falhou: {mensagem}")
                                }
                                _ => String::new(),
                            };
                            out.push(Evento::Abertura { texto, fim: true });
                        }
                        TipoJob::Monitor => {}
                        _ => out.push(Evento::JobFinalizado {
                            job: job.clone(),
                            desfecho: desfecho.clone(),
                            resposta: resposta.clone(),
                        }),
                    }
                }
            }
            EventoAgente::Monitor { saude } => out.push(Evento::Monitor(saude.clone())),
            EventoAgente::PontoConfianca { confianca } => {
                out.push(Evento::Confianca(confianca.clone()));
            }
            EventoAgente::Batida => {}
            EventoAgente::AgenteReiniciando { de, para } => {
                out.push(Evento::AgenteReiniciando {
                    de: de.clone(),
                    para: para.clone(),
                });
            }
        }
    }
}

/// Acompanha o agente: traduz o fluxo de eventos para a GUI até `parar`.
fn encaminhar_eventos(
    obs: &Observador,
    tx: &Sender<Evento>,
    acordador: &Acordador,
    parar: &AtomicBool,
) {
    let mut t = Tradutor::default();
    while !parar.load(Ordering::SeqCst) {
        let Some(ev) = obs.proximo(Duration::from_millis(100)) else {
            continue;
        };
        let saida = t.traduzir(ev);
        if saida.is_empty() {
            continue;
        }
        for e in saida {
            if tx.send(e).is_err() {
                return;
            }
        }
        acordador.acordar();
    }
}

/// Modo agente: garante o agente, acompanha o fluxo e despacha os pedidos em conexões próprias.
fn laco_agente(
    cfg: &ConfigWorker,
    rx: &Receiver<Mensagem>,
    tx: &Sender<Evento>,
    controle: &Arc<Mutex<Option<Alca>>>,
    acordador: &Acordador,
    lancar: bool,
) {
    let socket = cfg.socket.clone().unwrap_or_else(caminho_socket_padrao);
    let lanc = cfg
        .lancamento
        .clone()
        .unwrap_or_else(|| OpcoesLancamento::do_ambiente(&cfg.caminho, &socket));
    let enviar = |e: Evento| {
        let _ = tx.send(e);
        acordador.acordar();
    };
    let mut motivo_falha: Option<String> = None;
    if lancar && let Err(e) = garantir_agente(&lanc) {
        let motivo = e.to_string();
        enviar(Evento::Modo(ModoWorker::Falhou {
            motivo: motivo.clone(),
        }));
        motivo_falha = Some(motivo);
    }
    if let Ok(mut g) = controle.lock() {
        *g = Some(Alca::Remota {
            socket: socket.clone(),
        });
    }
    // O observador reconecta (e relança o agente, se for o caso) sozinho.
    let mut opcoes = OpcoesObservador::novo(cfg.caminho.clone(), socket.clone());
    if lancar {
        opcoes.lancamento = Some(lanc);
    }
    let obs = Observador::iniciar(opcoes, None);
    let parar = Arc::new(AtomicBool::new(false));
    let encaminhador = {
        let (tx, ac, parar) = (tx.clone(), acordador.clone(), Arc::clone(&parar));
        std::thread::Builder::new()
            .name("dag-worker-eventos".into())
            .spawn(move || encaminhar_eventos(&obs, &tx, &ac, &parar))
            .ok()
    };
    while let Ok(m) = rx.recv() {
        match m {
            Mensagem::Sair => break,
            Mensagem::Pedido { id, pedido } => {
                enviar(Evento::Iniciado { id });
                let (tx, ac) = (tx.clone(), acordador.clone());
                let (socket, catalogo) = (socket.clone(), cfg.caminho.clone());
                let falha = motivo_falha.clone();
                let r = std::thread::Builder::new()
                    .name("dag-worker-pedido".into())
                    .spawn(move || {
                        let resposta = match Cliente::conectar_se_serve(&socket, &catalogo) {
                            Some(mut c) => c
                                .enviar(pedido, &mut |_| {})
                                .unwrap_or_else(|e| e.para_resposta()),
                            None => Resposta::Erro {
                                codigo: "agente_indisponivel".into(),
                                mensagem: falha.unwrap_or_else(|| {
                                    "O agente está desconectado; a GUI está reconectando.".into()
                                }),
                            },
                        };
                        let _ = tx.send(Evento::Resposta {
                            id,
                            resposta: Box::new(resposta),
                        });
                        ac.acordar();
                    });
                if r.is_err() {
                    enviar(Evento::Resposta {
                        id,
                        resposta: Box::new(Resposta::Erro {
                            codigo: "worker_indisponivel".into(),
                            mensagem: "Não foi possível iniciar a thread do pedido.".into(),
                        }),
                    });
                }
            }
        }
    }
    parar.store(true, Ordering::SeqCst);
    if let Some(t) = encaminhador {
        let _ = t.join();
    }
}

fn laco(
    cfg: &ConfigWorker,
    rx: &Receiver<Mensagem>,
    tx: &Sender<Evento>,
    controle: &Arc<Mutex<Option<Alca>>>,
    acordador: &Acordador,
) {
    // `--sem-agente` só vale se não houver agente: havendo um, ele é o escritor e a GUI o segue.
    let socket = cfg.socket.clone().unwrap_or_else(caminho_socket_padrao);
    let agente_ja_existe = Cliente::conectar_se_serve(&socket, &cfg.caminho).is_some();
    if !cfg.sem_agente || agente_ja_existe {
        laco_agente(cfg, rx, tx, controle, acordador, !cfg.sem_agente);
        return;
    }
    let enviar = |e: Evento| {
        let _ = tx.send(e);
        acordador.acordar();
    };
    let enc = Arc::new(Encaminhador::novo(tx.clone(), acordador.clone()));
    let (mut destino, modo) = abrir_em_processo(cfg, &enc);
    let alca = match &destino {
        Destino::Sessao(a) => Some(Alca::Local(a.controle())),
        Destino::Nenhum => None,
    };
    if let Ok(mut g) = controle.lock() {
        g.clone_from(&alca);
    }
    enviar(Evento::Modo(modo.clone()));
    if modo == ModoWorker::Escritor
        && let Destino::Sessao(a) = &mut destino
    {
        // Sem agente a GUI é a escritora: o que o processo anterior deixou pela metade continua
        // daqui.
        enviar(Evento::Abertura {
            texto: TEXTO_ABERTURA.into(),
            fim: false,
        });
        let r = a.recuperar_ao_abrir(&cfg.abertura);
        let (texto, fim) = match r {
            Ok(r) if r.relevante() => (format!("{r}"), true),
            Ok(_) => (String::new(), true),
            Err(e) => (format!("Recuperação ao abrir falhou: {e}"), true),
        };
        enviar(Evento::Abertura { texto, fim });
    }
    while let Ok(m) = rx.recv() {
        match m {
            Mensagem::Sair => break,
            Mensagem::Pedido { id, pedido } => {
                enviar(Evento::Iniciado { id });
                // Um scan cancelado antes deixa o controle em "cancelado"; cada novo trabalho
                // começa do zero.
                if let Some(Alca::Local(c)) = &alca {
                    c.reiniciar();
                }
                let resposta = match &mut destino {
                    Destino::Sessao(a) => a.executar_pedido(pedido),
                    Destino::Nenhum => Resposta::Erro {
                        codigo: "worker_indisponivel".into(),
                        mensagem: match &modo {
                            ModoWorker::Falhou { motivo } => {
                                format!("O worker não abriu o Catálogo: {motivo}")
                            }
                            _ => "O worker não está disponível.".into(),
                        },
                    },
                };
                enviar(Evento::Resposta {
                    id,
                    resposta: Box::new(resposta),
                });
            }
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_agent::GuardaEscritor;
    use dag_core::{ModoVarredura, OrigemJob};
    use std::fs;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    const T: Duration = Duration::from_secs(60);

    fn volume_de(p: &Path) -> VolumeDescoberto {
        let dev = fs::metadata(p).unwrap().dev();
        VolumeDescoberto {
            platform_id: format!("teste-dev-{dev}"),
            st_dev: dev,
            rotulo: Some("teste".into()),
            fs_type: "teste".into(),
            capacidade_bytes: Some(1000),
            livre_bytes: Some(400),
            pontos_de_montagem: vec![p.as_os_str().as_bytes().to_vec()],
            pseudo: false,
            classe: dag_core::ClasseVolume::Fisico,
            sistema: false,
            dispositivo: None,
        }
    }

    fn sem_socket() -> PathBuf {
        PathBuf::from("/nonexistent/dag-teste/agente.sock")
    }

    fn escrever(p: &Path, bytes: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::File::create(p)
            .unwrap()
            .write_all(&vec![b'x'; bytes])
            .unwrap();
    }

    /// Worker em processo (`--sem-agente`), como nos testes que não sobem agente.
    fn em_processo(catalogo: &Path, raiz: &Path) -> Worker {
        Worker::iniciar(
            ConfigWorker {
                caminho: catalogo.to_path_buf(),
                volumes_fixos: Some(vec![volume_de(raiz)]),
                socket: Some(sem_socket()),
                abertura: dag_agent::PoliticaAbertura::desligada(),
                sem_agente: true,
                lancamento: None,
            },
            Acordador::default(),
        )
    }

    /// Worker cliente de um agente já de pé.
    fn cliente_do_agente(catalogo: &Path, socket: &Path) -> Worker {
        Worker::iniciar(
            ConfigWorker {
                caminho: catalogo.to_path_buf(),
                volumes_fixos: None,
                socket: Some(socket.to_path_buf()),
                abertura: dag_agent::PoliticaAbertura::desligada(),
                sem_agente: false,
                lancamento: Some(OpcoesLancamento {
                    catalogo: catalogo.to_path_buf(),
                    socket: socket.to_path_buf(),
                    // Nunca lança de verdade nos testes: o agente é o do próprio teste.
                    executavel: Some(PathBuf::from("/nonexistent/dag")),
                    usar_systemd: false,
                    espera: Duration::from_millis(300),
                    argumentos_agente: Vec::new(),
                }),
            },
            Acordador::default(),
        )
    }

    /// Espera o evento que satisfaz `pred`, acumulando os anteriores.
    fn esperar_ate(w: &mut Worker, pred: impl Fn(&Evento) -> bool) -> (Evento, Vec<Evento>) {
        let fim = Instant::now() + T;
        let mut antes = Vec::new();
        while Instant::now() < fim {
            if let Some(e) = w.esperar(Duration::from_millis(200)) {
                if pred(&e) {
                    return (e, antes);
                }
                antes.push(e);
            }
        }
        panic!("evento esperado não chegou; recebidos: {antes:?}");
    }

    fn cenario() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let raiz = fs::canonicalize(tmp.path()).unwrap().join("arvore");
        escrever(&raiz.join("big.pdf"), 9000);
        escrever(&raiz.join("sub/x.bin"), 3000);
        escrever(&raiz.join("sub/y.txt"), 2000);
        escrever(&raiz.join("copia/big.pdf"), 9000);
        let catalogo = tmp.path().join("dados/catalogo.sqlite");
        (tmp, raiz, catalogo)
    }

    fn cenario_grande() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let c = cenario();
        for d in 0..120 {
            for f in 0..60 {
                escrever(&c.1.join(format!("g{d}/f{f}.txt")), 64 + f);
            }
        }
        c
    }

    fn scan(raiz: &Path, sem_hash: bool) -> Comando {
        Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec![raiz.to_str().unwrap().into()],
            sem_hash,
            elevar: false,
            releitura_total: false,
            threads_por_disco: None,
            max_discos: None,
        }
    }

    fn servidor(raiz: &Path, catalogo: &Path, socket: &Path) -> dag_agent::Servidor {
        let mut cfg = dag_agent::ConfigServidor::novo(catalogo, socket);
        cfg.monitor = false;
        cfg.volumes_fixos = Some(vec![volume_de(raiz)]);
        dag_agent::Servidor::iniciar(cfg).unwrap()
    }

    fn resposta_de(w: &mut Worker, id: u64) -> Resposta {
        let (e, _) = esperar_ate(
            w,
            |e| matches!(e, Evento::Resposta { id: i, .. } if *i == id),
        );
        let Evento::Resposta { resposta, .. } = e else {
            unreachable!()
        };
        *resposta
    }

    #[test]
    fn worker_executa_full_scan_com_progresso_e_conclusao() {
        let (_tmp, raiz, catalogo) = cenario();
        let mut w = em_processo(&catalogo, &raiz);
        let (e, _) = esperar_ate(&mut w, |e| matches!(e, Evento::Modo(_)));
        assert!(matches!(e, Evento::Modo(ModoWorker::Escritor)));
        assert!(w.modo().pode_escrever());
        assert!(w.estado_controle().is_some());

        let id = w.enviar(scan(&raiz, false));
        let (fim, antes) = esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Resposta { id: i, .. } if *i == id),
        );
        // Iniciado e ao menos um evento de progresso (a troca de fase sempre é enviada).
        assert!(
            antes
                .iter()
                .any(|e| matches!(e, Evento::Iniciado { id: i } if *i == id))
        );
        let progressos: Vec<&ProgressoScan> = antes
            .iter()
            .filter_map(|e| match e {
                Evento::Progresso(p) => Some(p),
                _ => None,
            })
            .collect();
        assert!(
            !progressos.is_empty(),
            "sem eventos de progresso: {antes:?}"
        );
        assert_eq!(progressos.last().unwrap().arquivos, 4);
        let Evento::Resposta { resposta, .. } = fim else {
            unreachable!()
        };
        match *resposta {
            Resposta::ScanConcluido {
                desfecho,
                arquivos,
                bytes_logicos,
                hash,
                duplicados,
                ..
            } => {
                assert_eq!(desfecho, "concluida");
                assert_eq!(arquivos, 4);
                assert_eq!(bytes_logicos, 23_000);
                assert!(hash.is_some());
                assert_eq!(duplicados.unwrap().grupos, 1, "big.pdf e a cópia");
            }
            outra => panic!("resposta inesperada: {outra:?}"),
        }
        w.encerrar();
    }

    #[test]
    fn worker_responde_volumes_e_erro_de_comando_invalido() {
        let (_tmp, raiz, catalogo) = cenario();
        let mut w = em_processo(&catalogo, &raiz);
        let id = w.enviar(Comando::ListarVolumes);
        match resposta_de(&mut w, id) {
            Resposta::Volumes { volumes, .. } => assert_eq!(volumes.len(), 1),
            outra => panic!("{outra:?}"),
        }
        let id = w.enviar(Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec![],
            sem_hash: true,
            elevar: false,
            releitura_total: false,
            threads_por_disco: None,
            max_discos: None,
        });
        assert!(matches!(resposta_de(&mut w, id), Resposta::Erro { .. }));
        w.encerrar();
    }

    #[test]
    fn com_o_escritor_ocupado_o_worker_abre_em_leitura() {
        let (_tmp, raiz, catalogo) = cenario();
        // Outro "processo" (aqui, outra abertura do mesmo arquivo de trava) é o escritor.
        let dono = Agente::abrir(&catalogo).unwrap().silencioso();
        let mut w = em_processo(&catalogo, &raiz);
        let (e, _) = esperar_ate(&mut w, |e| matches!(e, Evento::Modo(_)));
        let Evento::Modo(m) = e else { unreachable!() };
        assert!(matches!(m, ModoWorker::SomenteLeitura { .. }), "{m:?}");
        assert_eq!(m.texto_status(), "Worker: outro processo é o escritor");
        assert!(!m.pode_escrever());
        // Comando de escrita é recusado; leitura funciona.
        let id = w.enviar(Comando::ListarVolumes);
        let r = resposta_de(&mut w, id);
        assert!(matches!(r, Resposta::Erro { .. }), "{r:?}");
        let id = w.enviar(Comando::Status { execucao: None });
        let r = resposta_de(&mut w, id);
        assert!(matches!(r, Resposta::Status { .. }), "{r:?}");
        w.encerrar();
        drop(dono);
        let _ = GuardaEscritor::adquirir(&catalogo).unwrap();
    }

    #[test]
    fn sem_catalogo_e_com_trava_ocupada_o_worker_falha_com_mensagem() {
        let tmp = tempfile::tempdir().unwrap();
        let catalogo = tmp.path().join("c.sqlite");
        let _guarda = GuardaEscritor::adquirir(&catalogo).unwrap();
        let mut w = Worker::iniciar(
            ConfigWorker {
                caminho: catalogo,
                volumes_fixos: None,
                socket: Some(sem_socket()),
                abertura: dag_agent::PoliticaAbertura::desligada(),
                sem_agente: true,
                lancamento: None,
            },
            Acordador::default(),
        );
        let (e, _) = esperar_ate(&mut w, |e| matches!(e, Evento::Modo(_)));
        let Evento::Modo(m) = e else { unreachable!() };
        assert!(matches!(m, ModoWorker::Falhou { .. }), "{m:?}");
        let id = w.enviar(Comando::ListarVolumes);
        assert!(
            matches!(resposta_de(&mut w, id), Resposta::Erro { codigo, .. } if codigo == "worker_indisponivel")
        );
        w.encerrar();
    }

    #[test]
    fn controle_cancelado_antes_do_pedido_nao_impede_o_proximo_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let raiz = fs::canonicalize(tmp.path()).unwrap().join("grande");
        for i in 0..300 {
            escrever(&raiz.join(format!("d{}/f{i}.txt", i % 30)), 10);
        }
        let mut w = em_processo(&tmp.path().join("c.sqlite"), &raiz);
        esperar_ate(&mut w, |e| matches!(e, Evento::Modo(_)));
        w.cancelar();
        // Cancelamento antes do início vale para o scan em curso; o worker reinicia o controle
        // ao começar cada pedido, então este roda normalmente.
        let id = w.enviar(scan(&raiz, true));
        let r = resposta_de(&mut w, id);
        assert!(
            matches!(&r, Resposta::ScanConcluido { desfecho, arquivos: 300, .. } if desfecho == "concluida"),
            "{r:?}"
        );
        w.encerrar();
    }

    #[test]
    fn gui_escritora_sem_agente_retoma_a_execucao_interrompida_ao_abrir_e_avisa() {
        let (_tmp, raiz, catalogo) = cenario();
        {
            let mut a = Agente::abrir(&catalogo)
                .unwrap()
                .silencioso()
                .com_volumes_fixos(vec![volume_de(&raiz)]);
            let r = a.executar(scan(&raiz, false));
            assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
        }
        // Processo morto no meio do hash.
        {
            let cat = dag_store::Catalogo::abrir(&catalogo).unwrap();
            cat.armazem()
                .conexao()
                .execute_batch(
                    "UPDATE scan_runs SET phase = 'hash';
                     DELETE FROM hashes WHERE file_id IN
                        (SELECT id FROM files WHERE kind = 'arquivo' ORDER BY id LIMIT 2);
                     UPDATE fila_hash SET state = 'pendente'
                        WHERE file_id IN (SELECT id FROM files WHERE kind = 'arquivo' ORDER BY id LIMIT 2);",
                )
                .unwrap();
        }
        let mut w = Worker::iniciar(
            ConfigWorker {
                caminho: catalogo.clone(),
                volumes_fixos: Some(vec![volume_de(&raiz)]),
                socket: Some(sem_socket()),
                abertura: dag_agent::PoliticaAbertura::default(),
                sem_agente: true,
                lancamento: None,
            },
            Acordador::default(),
        );
        let (fim, _) = esperar_ate(&mut w, |e| matches!(e, Evento::Abertura { fim: true, .. }));
        let Evento::Abertura { texto, .. } = fim else {
            unreachable!()
        };
        assert!(
            texto.contains("1 interrompida(s), 1 retomada(s)"),
            "{texto}"
        );
        w.encerrar();
        let cat = dag_store::Catalogo::abrir(&catalogo).unwrap();
        let n: i64 = cat
            .armazem()
            .conexao()
            .query_row("SELECT COUNT(*) FROM hashes", [], |l| l.get(0))
            .unwrap();
        assert_eq!(n, 4);
    }

    #[test]
    fn worker_e_cliente_do_agente_sem_segundo_escritor_e_recebe_hash_com_bytes() {
        let (tmp, raiz, catalogo) = cenario();
        let socket = tmp.path().join("run/agente.sock");
        let srv = servidor(&raiz, &catalogo, &socket);
        let mut w = cliente_do_agente(&catalogo, &socket);
        let (e, _) = esperar_ate(&mut w, |e| matches!(e, Evento::Modo(_)));
        let Evento::Modo(m) = e else { unreachable!() };
        assert_eq!(m, ModoWorker::Servico);
        assert_eq!(m.texto_status(), "Agente: conectado");
        assert!(m.pode_escrever());
        // A GUI não abriu um segundo escritor: a trava é do agente.
        assert!(matches!(
            Agente::abrir(&catalogo).unwrap_err(),
            ErroAgente::EscritorOcupado(_)
        ));
        let id = w.enviar(scan(&raiz, false));
        let (fim, antes) = esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Resposta { id: i, .. } if *i == id),
        );
        assert!(antes.iter().any(|e| matches!(e, Evento::Progresso(_))));
        let h: Vec<_> = antes
            .iter()
            .filter_map(|e| match e {
                Evento::Hash(h) => Some(h),
                _ => None,
            })
            .collect();
        assert!(!h.is_empty(), "sem progresso de hash: {antes:?}");
        assert!(h.last().unwrap().bytes_lidos > 0);
        // O job que a GUI iniciou aparece com a origem dela.
        assert!(antes.iter().any(|e| matches!(e, Evento::Jobs(j)
            if j.iter().any(|j| j.origem == OrigemJob::Gui && j.tipo == TipoJob::FullScan))));
        let Evento::Resposta { resposta, .. } = fim else {
            unreachable!()
        };
        assert!(matches!(
            *resposta,
            Resposta::ScanConcluido { arquivos: 4, .. }
        ));
        w.encerrar();
        srv.encerrar();
    }

    #[test]
    fn gui_ve_o_job_da_cli_com_progresso_e_o_pausa_retoma_e_cancela() {
        let (tmp, raiz, catalogo) = cenario_grande();
        let socket = tmp.path().join("run/agente.sock");
        let srv = servidor(&raiz, &catalogo, &socket);
        let mut w = cliente_do_agente(&catalogo, &socket);
        esperar_ate(&mut w, |e| matches!(e, Evento::Modo(ModoWorker::Servico)));

        // Outro cliente (a CLI) inicia a varredura; a GUI não pediu nada.
        let (s, c) = (socket.clone(), scan(&raiz, false));
        let cli = std::thread::spawn(move || {
            Cliente::conectar(&s)
                .unwrap()
                .enviar(
                    Pedido {
                        autor: Autor::Cli,
                        comando: c,
                    },
                    &mut |_| {},
                )
                .unwrap()
        });
        esperar_ate(&mut w, |e| {
            matches!(e, Evento::Jobs(j) if j.iter().any(|j| j.origem == OrigemJob::Cli
                && j.tipo == TipoJob::FullScan && j.estado == EstadoJob::Executando))
        });
        let foco = w.job_em_foco().unwrap().clone();
        assert_eq!(foco.origem, OrigemJob::Cli);
        esperar_ate(&mut w, |e| matches!(e, Evento::Progresso(_)));
        assert_eq!(w.estado_controle(), Some(EstadoControle::Executando));

        w.pausar();
        esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Jobs(j) if j.iter().any(|j| j.id == foco.id && j.estado == EstadoJob::Pausado)),
        );
        assert_eq!(w.estado_controle(), Some(EstadoControle::Pausado));
        w.continuar();
        esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Jobs(j) if j.iter().any(|j| j.id == foco.id && j.estado == EstadoJob::Executando)),
        );
        w.cancelar();
        let (fim, _) = esperar_ate(&mut w, |e| matches!(e, Evento::JobFinalizado { .. }));
        let Evento::JobFinalizado { job, desfecho, .. } = fim else {
            unreachable!()
        };
        assert_eq!((job.id, desfecho.as_str()), (foco.id, "cancelada"));
        assert!(matches!(
            cli.join().unwrap(),
            Resposta::ScanConcluido { desfecho, .. } if desfecho == "cancelada"
        ));
        assert!(w.job_em_foco().is_none());
        w.encerrar();
        srv.encerrar();
    }

    #[test]
    fn pausar_e_cancelar_o_proprio_scan_vao_pelo_agente() {
        let (tmp, raiz, catalogo) = cenario_grande();
        let socket = tmp.path().join("run/agente.sock");
        let srv = servidor(&raiz, &catalogo, &socket);
        let mut w = cliente_do_agente(&catalogo, &socket);
        esperar_ate(&mut w, |e| matches!(e, Evento::Modo(ModoWorker::Servico)));
        let id = w.enviar(scan(&raiz, false));
        // Pausa assim que o scan começa a reportar.
        esperar_ate(&mut w, |e| matches!(e, Evento::Progresso(_)));
        w.pausar();
        esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Progresso(p) if p.fase == crate::progresso::FaseScan::Pausado),
        );
        assert_eq!(w.estado_controle(), Some(EstadoControle::Pausado));
        w.cancelar();
        let r = resposta_de(&mut w, id);
        assert!(
            matches!(&r, Resposta::ScanConcluido { desfecho, .. } if desfecho == "cancelada"),
            "{r:?}"
        );
        w.encerrar();
        srv.encerrar();
    }

    #[test]
    fn a_gui_reconecta_quando_o_agente_reinicia() {
        let (tmp, raiz, catalogo) = cenario();
        let socket = tmp.path().join("run/agente.sock");
        let srv = servidor(&raiz, &catalogo, &socket);
        let mut w = cliente_do_agente(&catalogo, &socket);
        esperar_ate(&mut w, |e| matches!(e, Evento::Modo(ModoWorker::Servico)));
        srv.encerrar();
        let (e, _) = esperar_ate(&mut w, |e| {
            matches!(e, Evento::Modo(ModoWorker::Reconectando { .. }))
        });
        let Evento::Modo(m) = e else { unreachable!() };
        assert_eq!(m.texto_status(), "Agente desconectado, reconectando…");
        assert!(!w.modo().pode_escrever());
        // Pedidos durante a queda recebem erro claro, sem travar.
        let id = w.enviar(Comando::ListarVolumes);
        assert!(matches!(
            resposta_de(&mut w, id),
            Resposta::Erro { codigo, .. } if codigo == "agente_indisponivel"
        ));
        let srv = servidor(&raiz, &catalogo, &socket);
        esperar_ate(&mut w, |e| matches!(e, Evento::Modo(ModoWorker::Servico)));
        let id = w.enviar(Comando::ListarVolumes);
        assert!(matches!(resposta_de(&mut w, id), Resposta::Volumes { .. }));
        w.encerrar();
        srv.encerrar();
    }

    #[test]
    fn sem_o_executavel_dag_a_gui_falha_com_mensagem_clara_e_sugere_sem_agente() {
        let (tmp, _raiz, catalogo) = cenario();
        let socket = tmp.path().join("run/agente.sock");
        let mut w = cliente_do_agente(&catalogo, &socket);
        let (e, _) = esperar_ate(&mut w, |e| matches!(e, Evento::Modo(_)));
        let Evento::Modo(ModoWorker::Falhou { motivo }) = e else {
            panic!("{e:?}")
        };
        assert!(
            motivo.contains("não encontrei o executável `dag`"),
            "{motivo}"
        );
        assert!(motivo.contains("--sem-agente"), "{motivo}");
        w.encerrar();
    }
}
