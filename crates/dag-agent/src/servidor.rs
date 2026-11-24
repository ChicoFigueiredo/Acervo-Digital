//! O agente em segundo plano (spec §6, §7 modos 2–3, §11): único escritor do Catálogo, serve
//! um socket Unix protegido e, opcionalmente, monitora as raízes catalogadas.
//!
//! * Um thread **trabalhador** é o dono do [`Agente`] escritor e executa a fila de pedidos em
//!   série; o monitor roda quando a fila está vazia e cede o lugar a qualquer pedido novo.
//! * O **caminho de controle** não passa pela fila: `Pausar`, `Retomar`, `Cancelar` agem na
//!   alça atômica do trabalho em curso, e `Status`, `SaudeMonitor` e as demais leituras vão para
//!   uma segunda conexão somente-leitura. Respondem na hora mesmo durante uma varredura.
//! * Cada conexão é aceita só se o uid do par (`SO_PEERCRED`) for o do dono do processo.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dag_core::{
    Comando, EstadoJob, FaseInventario, ModoVarredura, OrigemJob, Pedido, Progresso, Resposta,
    TipoJob, VolumeDescoberto,
};
use dag_scanner::{ControleScan, EstadoControle};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::net::sockopt::socket_peercred;

use crate::abertura::PoliticaAbertura;
use crate::agente::{Agente, somente_leitura};
use crate::erro::ErroAgente;
use crate::exibir::progresso_stderr;
use crate::protocolo::{InfoServidor, Quadro, Requisicao, ler_quadro, linha};
use crate::registro::Registro;

/// Intervalo entre novas tentativas de iniciar o monitor com a fila vazia.
const REPETIR_MONITOR: Duration = Duration::from_secs(60);
/// Cadência mínima dos eventos de progresso repassados a cada cliente.
const INTERVALO_PROGRESSO: Duration = Duration::from_millis(100);
/// Cadência da leitura do monitor e do Ponto de Confiança para quem assina eventos.
const INTERVALO_VIGIA: Duration = Duration::from_secs(1);
/// Cadência da batida (sinal de vida) nas assinaturas.
const INTERVALO_BATIDA: Duration = Duration::from_secs(3);
/// Quanto o encerramento espera as conexões entregarem a última resposta.
const ESPERA_CONEXOES: Duration = Duration::from_secs(3);

/// Parâmetros do agente em segundo plano.
#[derive(Debug, Clone)]
pub struct ConfigServidor {
    pub catalogo: PathBuf,
    pub socket: PathBuf,
    /// Monitora as raízes catalogadas (padrão: sim).
    pub monitor: bool,
    /// Volumes fixos em vez da descoberta do sistema (testes).
    pub volumes_fixos: Option<Vec<VolumeDescoberto>>,
    /// Uid aceito no socket; `None` = o uid do próprio processo. Existe para testar a recusa.
    pub uid_aceito: Option<u32>,
    /// O que fazer ao abrir: retomar execuções interrompidas e verificar as Raízes (ADR 0003).
    /// `monitor_ativo` é ajustado de `monitor` ao iniciar.
    pub abertura: PoliticaAbertura,
    /// Paralelismo do hash dado por flag (`dag agente --threads-por-disco/--max-discos`): vence
    /// o persistido e perde para o pedido.
    pub opcoes_hash: dag_core::OpcoesHash,
}

impl ConfigServidor {
    #[must_use]
    pub fn novo(catalogo: impl Into<PathBuf>, socket: impl Into<PathBuf>) -> Self {
        Self {
            catalogo: catalogo.into(),
            socket: socket.into(),
            monitor: true,
            volumes_fixos: None,
            uid_aceito: None,
            abertura: PoliticaAbertura::default(),
            opcoes_hash: dag_core::OpcoesHash::default(),
        }
    }
}

/// O uid do par é o permitido?
#[must_use]
pub fn uid_autorizado(uid_do_par: u32, uid_aceito: u32) -> bool {
    uid_do_par == uid_aceito
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ocupacao {
    Ocioso,
    /// Pedido de um cliente em execução.
    Trabalho,
    /// Monitor do próprio agente (cede a qualquer pedido).
    Monitor,
}

#[derive(Debug)]
struct Estado {
    ocupacao: Ocupacao,
    /// Execução de varredura em curso, quando já conhecida.
    execucao: Option<i64>,
    /// Trabalho em curso no registro (ADR 0004), quando há um.
    job: Option<u64>,
}

enum Saida {
    Progresso(Progresso),
    Resposta(Box<Resposta>),
}

struct Trabalho {
    pedido: Pedido,
    saida: Sender<Saida>,
    /// Trabalho cadastrado no registro (só os longos: varreduras, retomadas, verificações).
    job: Option<u64>,
}

struct Compartilhado {
    catalogo: PathBuf,
    monitor: bool,
    abertura: PoliticaAbertura,
    uid: u32,
    parar: AtomicBool,
    controle: ControleScan,
    estado: Mutex<Estado>,
    fila: Mutex<Option<Sender<Trabalho>>>,
    leitor: Mutex<Option<Agente>>,
    conexoes: AtomicUsize,
    registro: Registro,
    /// Quem espera na fila de cada trabalho ainda não iniciado (para cancelá-lo na fila).
    pendentes: Mutex<HashMap<u64, Sender<Saida>>>,
    /// Trabalhos cancelados na fila: o trabalhador os pula.
    cancelados: Mutex<HashSet<u64>>,
}

fn travar<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Agente em execução. Descartar (ou [`encerrar`](Self::encerrar)) faz o encerramento limpo.
#[derive(Debug)]
pub struct Servidor {
    comp: Arc<Compartilhado>,
    socket: PathBuf,
    aceitador: Option<JoinHandle<()>>,
    trabalhador: Option<JoinHandle<()>>,
    vigia: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Compartilhado {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compartilhado")
            .field("catalogo", &self.catalogo)
            .finish_non_exhaustive()
    }
}

impl Servidor {
    /// Toma a trava de escritor, abre o socket e sobe as threads.
    ///
    /// # Errors
    /// [`ErroAgente::EscritorOcupado`] se outro processo já escreve; E/S do socket; se já há um
    /// agente respondendo no socket.
    pub fn iniciar(cfg: ConfigServidor) -> Result<Servidor, ErroAgente> {
        let mut agente = Agente::abrir(&cfg.catalogo)?
            .silencioso()
            .com_opcoes_hash(cfg.opcoes_hash)?;
        if let Some(v) = &cfg.volumes_fixos {
            agente = agente.com_volumes_fixos(v.clone());
        }
        let listener = preparar_socket(&cfg.socket)?;
        let catalogo = fs::canonicalize(&cfg.catalogo).unwrap_or_else(|_| cfg.catalogo.clone());
        let uid_proprio = rustix::process::getuid().as_raw();
        let (tx, rx) = mpsc::channel();
        let comp = Arc::new(Compartilhado {
            catalogo,
            monitor: cfg.monitor,
            abertura: PoliticaAbertura {
                monitor_ativo: cfg.monitor,
                ..cfg.abertura.clone()
            },
            uid: cfg.uid_aceito.unwrap_or(uid_proprio),
            parar: AtomicBool::new(false),
            controle: agente.controle(),
            estado: Mutex::new(Estado {
                ocupacao: Ocupacao::Ocioso,
                execucao: None,
                job: None,
            }),
            fila: Mutex::new(Some(tx)),
            leitor: Mutex::new(
                Agente::abrir_leitura(&cfg.catalogo)
                    .ok()
                    .map(Agente::silencioso),
            ),
            conexoes: AtomicUsize::new(0),
            registro: Registro::default(),
            pendentes: Mutex::new(HashMap::new()),
            cancelados: Mutex::new(HashSet::new()),
        });
        // A rotina de abertura é o primeiro trabalho do agente; já consta no instantâneo de quem
        // assina antes mesmo de a thread de trabalho começar.
        let job_abertura = comp.registro.novo_job(
            TipoJob::Abertura,
            OrigemJob::Abertura,
            Vec::new(),
            None,
            EstadoJob::Executando,
        );
        let trabalhador = {
            let comp = Arc::clone(&comp);
            std::thread::Builder::new()
                .name("dag-agente-trabalho".into())
                .spawn(move || trabalhar(agente, &rx, &comp, job_abertura))
                .map_err(|e| ErroAgente::es("iniciar thread de trabalho", e))?
        };
        let vigia = {
            let comp = Arc::clone(&comp);
            std::thread::Builder::new()
                .name("dag-agente-vigia".into())
                .spawn(move || vigiar(&comp))
                .map_err(|e| ErroAgente::es("iniciar thread de vigia", e))?
        };
        let aceitador = {
            let comp = Arc::clone(&comp);
            std::thread::Builder::new()
                .name("dag-agente-socket".into())
                .spawn(move || aceitar(&listener, &comp))
                .map_err(|e| ErroAgente::es("iniciar thread do socket", e))?
        };
        eprintln!(
            "agente: servindo {} em {} (monitor: {})",
            comp.catalogo.display(),
            cfg.socket.display(),
            if cfg.monitor { "sim" } else { "não" }
        );
        Ok(Servidor {
            comp,
            socket: cfg.socket,
            aceitador: Some(aceitador),
            trabalhador: Some(trabalhador),
            vigia: Some(vigia),
        })
    }

    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// As threads principais ainda estão de pé?
    #[must_use]
    pub fn ativo(&self) -> bool {
        self.trabalhador.as_ref().is_some_and(|t| !t.is_finished())
            && self.aceitador.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Encerramento limpo: cancela o trabalho em curso (a fila de retomada é gravada), recusa os
    /// pedidos pendentes, consolida o WAL, solta a trava e remove o socket.
    pub fn encerrar(mut self) {
        self.parar_tudo();
    }

    fn parar_tudo(&mut self) {
        if self.comp.parar.swap(true, Ordering::SeqCst) {
            return;
        }
        // Fecha a fila e cancela com o mesmo critério do enfileiramento (sob a trava).
        {
            let _e = travar(&self.comp.estado);
            travar(&self.comp.fila).take();
            // Encerramento, não decisão do usuário: o que estava rodando fica `interrompida` e
            // continua sozinho na próxima abertura.
            self.comp.controle.encerrar();
        }
        if let Some(t) = self.aceitador.take() {
            let _ = t.join();
        }
        if let Some(t) = self.trabalhador.take() {
            let _ = t.join();
        }
        if let Some(t) = self.vigia.take() {
            let _ = t.join();
        }
        let fim = Instant::now() + ESPERA_CONEXOES;
        while self.comp.conexoes.load(Ordering::SeqCst) > 0 && Instant::now() < fim {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = fs::remove_file(&self.socket);
        eprintln!("agente: encerrado");
    }
}

impl Drop for Servidor {
    fn drop(&mut self) {
        self.parar_tudo();
    }
}

// ---------------------------------------------------------------------- socket

/// Cria o diretório (0700) e o socket (0600). Um socket antigo só é removido se ninguém atende.
fn preparar_socket(socket: &Path) -> Result<UnixListener, ErroAgente> {
    let nome = |e: std::io::Error| ErroAgente::es(format!("socket {}", socket.display()), e);
    if let Some(pai) = socket.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(pai)
            .map_err(nome)?;
        // O diretório do DAG é só nosso: aperta se foi criado com permissões frouxas.
        if pai.file_name().is_some_and(|n| n == "dag")
            && let Ok(m) = fs::metadata(pai)
            && m.uid() == rustix::process::getuid().as_raw()
            && m.mode() & 0o077 != 0
        {
            fs::set_permissions(pai, fs::Permissions::from_mode(0o700)).map_err(nome)?;
        }
    }
    if let Ok(m) = fs::symlink_metadata(socket) {
        if !m.file_type().is_socket() {
            return Err(ErroAgente::Invalido(format!(
                "{} existe e não é um socket; remova-o ou escolha outro caminho",
                socket.display()
            )));
        }
        match UnixStream::connect(socket) {
            Ok(_) => {
                return Err(ErroAgente::Invalido(format!(
                    "já há um agente respondendo em {}",
                    socket.display()
                )));
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                ) =>
            {
                fs::remove_file(socket).map_err(nome)?;
            }
            Err(e) => return Err(nome(e)),
        }
    }
    let listener = UnixListener::bind(socket).map_err(nome)?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600)).map_err(nome)?;
    listener.set_nonblocking(true).map_err(nome)?;
    Ok(listener)
}

fn aceitar(listener: &UnixListener, comp: &Arc<Compartilhado>) {
    let espera = Timespec {
        tv_sec: 0,
        tv_nsec: 200_000_000,
    };
    while !comp.parar.load(Ordering::SeqCst) {
        let mut fds = [PollFd::new(listener, PollFlags::IN)];
        match poll(&mut fds, Some(&espera)) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => break,
        }
        loop {
            match listener.accept() {
                Ok((stream, _)) => admitir(stream, comp),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    eprintln!("agente: accept falhou: {e}");
                    break;
                }
            }
        }
    }
}

/// Confere o uid do par e entrega a conexão a uma thread.
fn admitir(stream: UnixStream, comp: &Arc<Compartilhado>) {
    let _ = stream.set_nonblocking(false);
    let par = socket_peercred(&stream).map(|c| c.uid.as_raw());
    if !par.as_ref().is_ok_and(|u| uid_autorizado(*u, comp.uid)) {
        eprintln!("agente: conexão recusada (uid do par: {par:?})");
        recusar(stream);
        return;
    }
    comp.conexoes.fetch_add(1, Ordering::SeqCst);
    let comp = Arc::clone(comp);
    let r = std::thread::Builder::new()
        .name("dag-agente-conexao".into())
        .spawn({
            let comp = Arc::clone(&comp);
            move || {
                atender(stream, &comp);
                comp.conexoes.fetch_sub(1, Ordering::SeqCst);
            }
        });
    if r.is_err() {
        comp.conexoes.fetch_sub(1, Ordering::SeqCst);
    }
}

fn recusar(mut stream: UnixStream) {
    let q = Quadro::de_resposta(
        0,
        Resposta::Erro {
            codigo: "nao_autorizado".into(),
            mensagem: "conexão recusada: o usuário do processo não é o dono do agente".into(),
        },
    );
    if let Ok(b) = linha(&q) {
        let _ = stream.write_all(&b);
    }
}

// ---------------------------------------------------------------------- conexão

fn atender(stream: UnixStream, comp: &Arc<Compartilhado>) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut leitor = BufReader::new(clone);
    let mut saida = stream;
    let mut buf = Vec::new();
    while !comp.parar.load(Ordering::SeqCst) {
        let bytes = match ler_quadro(&mut leitor, &mut buf) {
            Ok(Some(b)) => b,
            Ok(None) => return,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(_) => return,
        };
        let req: Requisicao = match serde_json::from_slice(&bytes) {
            Ok(r) => r,
            Err(e) => {
                let q = Quadro::de_resposta(
                    0,
                    Resposta::Erro {
                        codigo: "quadro_invalido".into(),
                        mensagem: format!("quadro inválido: {e}"),
                    },
                );
                if escrever(&mut saida, &q).is_err() {
                    return;
                }
                continue;
            }
        };
        let id = req.id;
        if req.assinar {
            // A conexão passa a ser só do fluxo de eventos até um dos lados fechar.
            servir_assinatura(comp, id, &mut saida);
            return;
        }
        let ok = match req.pedido {
            None => escrever(
                &mut saida,
                &Quadro::de_servidor(
                    id,
                    InfoServidor {
                        catalogo: comp.catalogo.display().to_string(),
                        pid: std::process::id(),
                        versao: env!("CARGO_PKG_VERSION").into(),
                        monitor: comp.monitor,
                    },
                ),
            )
            .is_ok(),
            Some(p) => servir(comp, id, p, &mut saida),
        };
        if !ok {
            return;
        }
    }
}

fn escrever(s: &mut UnixStream, q: &Quadro) -> std::io::Result<()> {
    let b = linha(q)?;
    s.write_all(&b)?;
    s.flush()
}

/// Entrega o instantâneo e depois cada evento ao assinante, até ele sair ou o agente encerrar.
fn servir_assinatura(comp: &Arc<Compartilhado>, id: u64, saida: &mut UnixStream) {
    let (monitor, confianca) = comp.ler_monitor();
    let (instantaneo, rx) = comp.registro.assinar(monitor, confianca);
    if escrever(saida, &Quadro::de_evento(id, instantaneo)).is_err() {
        return;
    }
    while !comp.parar.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(e) => {
                if escrever(saida, &Quadro::de_evento(id, e)).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            // O registro desligou este assinante (lento demais): fecha para ele reassinar.
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Atende um pedido; `false` se a conexão quebrou.
fn servir(comp: &Arc<Compartilhado>, id: u64, pedido: Pedido, saida: &mut UnixStream) -> bool {
    if let Some(r) = comp.imediato(&pedido) {
        return escrever(saida, &Quadro::de_resposta(id, r)).is_ok();
    }
    let (tx, rx) = mpsc::channel();
    if let Err(r) = comp.enfileirar(pedido, tx) {
        return escrever(saida, &Quadro::de_resposta(id, *r)).is_ok();
    }
    let mut vivo = true;
    while let Ok(s) = rx.recv() {
        let (q, fim) = match s {
            Saida::Progresso(p) => (Quadro::de_progresso(id, p), false),
            Saida::Resposta(r) => (Quadro::de_resposta(id, *r), true),
        };
        // Cliente que sumiu não interrompe o trabalho; só deixa de receber.
        if vivo && escrever(saida, &q).is_err() {
            vivo = false;
        }
        if fim {
            return vivo;
        }
    }
    false
}

#[derive(Clone, Copy)]
enum Controle {
    Pausar,
    Continuar,
    Cancelar,
}

/// Que trabalho registrado um pedido vira (só os longos), com as raízes e a execução, se já
/// conhecidas.
fn tipo_do_job(pedido: &Pedido) -> Option<(TipoJob, Vec<String>, Option<i64>)> {
    match &pedido.comando {
        Comando::IniciarScan { modo, raizes, .. } => Some((
            match modo {
                ModoVarredura::FullScan => TipoJob::FullScan,
                ModoVarredura::FastScan => TipoJob::FastScan,
            },
            raizes.clone(),
            None,
        )),
        Comando::Retomar {
            execucao: Some(id), ..
        } => Some((TipoJob::Retomada, Vec::new(), Some(id.0))),
        Comando::VerificarIntegridade { raizes, .. } => {
            Some((TipoJob::VerificacaoIntegridade, raizes.clone(), None))
        }
        _ => None,
    }
}

impl Compartilhado {
    /// Resposta imediata (caminho de controle e leituras), ou `None` se o pedido vai à fila.
    fn imediato(&self, pedido: &Pedido) -> Option<Resposta> {
        match &pedido.comando {
            Comando::Pausar { execucao } => Some(self.controlar(Controle::Pausar, *execucao, None)),
            Comando::Cancelar { execucao } => {
                Some(self.controlar(Controle::Cancelar, *execucao, None))
            }
            Comando::Retomar { execucao: None, .. } => {
                Some(self.controlar(Controle::Continuar, None, None))
            }
            // Com id: retoma o trabalho pausado em memória; senão é retomada de execução
            // gravada no Catálogo, que vai à fila como qualquer varredura.
            Comando::Retomar {
                execucao: Some(id), ..
            } => {
                let e = travar(&self.estado);
                let pausado = e.ocupacao == Ocupacao::Trabalho
                    && self.controle.estado() == EstadoControle::Pausado
                    && e.execucao.is_none_or(|a| a == id.0);
                drop(e);
                pausado.then(|| self.controlar(Controle::Continuar, Some(*id), None))
            }
            Comando::PausarJob { job } => Some(self.controlar_job(Controle::Pausar, *job)),
            Comando::RetomarJob { job } => Some(self.controlar_job(Controle::Continuar, *job)),
            Comando::CancelarJob { job } => Some(self.controlar_job(Controle::Cancelar, *job)),
            Comando::ListarJobs => Some(Resposta::Jobs {
                jobs: self.registro.jobs(),
            }),
            Comando::Monitorar { .. } => Some(if self.monitor {
                Resposta::Ok {
                    mensagem: Some(
                        "o agente já monitora as raízes catalogadas (veja `dag status`)".into(),
                    ),
                }
            } else {
                ErroAgente::Invalido(
                    "o agente foi iniciado com --sem-monitor; reinicie-o sem a opção".into(),
                )
                .para_resposta()
            }),
            c if somente_leitura(c) => Some(self.ler(pedido.clone())),
            _ => None,
        }
    }

    /// Pausa, retoma ou cancela o trabalho em curso (`execucao` e `job` só conferem que é o
    /// esperado).
    fn controlar(
        &self,
        acao: Controle,
        execucao: Option<dag_core::ExecucaoId>,
        job: Option<u64>,
    ) -> Resposta {
        let e = travar(&self.estado);
        self.controlar_em_curso(&e, acao, execucao, job)
    }

    fn controlar_em_curso(
        &self,
        e: &Estado,
        acao: Controle,
        execucao: Option<dag_core::ExecucaoId>,
        job: Option<u64>,
    ) -> Resposta {
        if e.ocupacao != Ocupacao::Trabalho {
            return ErroAgente::NadaEmExecucao.para_resposta();
        }
        if let (Some(pedida), Some(atual)) = (execucao, e.execucao)
            && pedida.0 != atual
        {
            return ErroAgente::NaoEncontrado(format!(
                "a execução {} não está em andamento (a atual é a {atual})",
                pedida.0
            ))
            .para_resposta();
        }
        if let (Some(pedido), Some(atual)) = (job, e.job)
            && pedido != atual
        {
            return ErroAgente::NaoEncontrado(format!(
                "o trabalho {pedido} não está em andamento (o atual é o {atual})"
            ))
            .para_resposta();
        }
        let (mensagem, estado) = match acao {
            Controle::Pausar => {
                self.controle.pausar();
                (
                    "varredura pausando (checkpoint no próximo ponto seguro)",
                    EstadoJob::Pausado,
                )
            }
            Controle::Continuar => {
                self.controle.continuar();
                ("varredura retomada", EstadoJob::Executando)
            }
            Controle::Cancelar => {
                self.controle.cancelar();
                (
                    "varredura cancelando (o que já foi lido é gravado; dá para retomar)",
                    EstadoJob::Cancelando,
                )
            }
        };
        if let Some(j) = e.job {
            self.registro.definir_estado(j, estado);
        }
        Resposta::Ok {
            mensagem: Some(mensagem.into()),
        }
    }

    /// Controle por id de trabalho (qualquer cliente age em qualquer trabalho).
    fn controlar_job(&self, acao: Controle, job: u64) -> Resposta {
        // Segura a trava do estado: o trabalhador tira o trabalho da fila sob a mesma trava.
        let e = travar(&self.estado);
        let Some(info) = self.registro.job(job) else {
            return ErroAgente::NaoEncontrado(format!(
                "o trabalho {job} não existe ou já terminou (veja `dag jobs`)"
            ))
            .para_resposta();
        };
        if e.job == Some(job) {
            return self.controlar_em_curso(&e, acao, None, Some(job));
        }
        if info.estado != EstadoJob::Enfileirado {
            return ErroAgente::NaoEncontrado(format!("o trabalho {job} não está em andamento"))
                .para_resposta();
        }
        if !matches!(acao, Controle::Cancelar) {
            return ErroAgente::Invalido(format!(
                "o trabalho {job} ainda está na fila; só dá para cancelá-lo"
            ))
            .para_resposta();
        }
        travar(&self.cancelados).insert(job);
        let resposta = Resposta::Erro {
            codigo: "cancelado".into(),
            mensagem: format!("o trabalho {job} foi cancelado antes de começar"),
        };
        if let Some(tx) = travar(&self.pendentes).remove(&job) {
            let _ = tx.send(Saida::Resposta(Box::new(resposta.clone())));
        }
        self.registro.finalizar(job, &resposta);
        Resposta::Ok {
            mensagem: Some(format!("trabalho {job} cancelado na fila")),
        }
    }

    fn ler(&self, pedido: Pedido) -> Resposta {
        let mut g = travar(&self.leitor);
        if g.is_none() {
            match Agente::abrir_leitura(&self.catalogo) {
                Ok(a) => *g = Some(a.silencioso()),
                Err(e) => return e.para_resposta(),
            }
        }
        match g.as_mut() {
            Some(a) => a.executar_pedido(pedido),
            None => ErroAgente::Invalido("leitor indisponível".into()).para_resposta(),
        }
    }

    /// Saúde do monitor e Ponto de Confiança, lidos agora (para o fluxo de eventos).
    fn ler_monitor(
        &self,
    ) -> (
        Option<dag_core::SaudeMonitorInfo>,
        Vec<dag_core::StatusConfianca>,
    ) {
        let mut g = travar(&self.leitor);
        if g.is_none() {
            *g = Agente::abrir_leitura(&self.catalogo)
                .ok()
                .map(Agente::silencioso);
        }
        match g.as_ref().map(Agente::monitor_e_confianca) {
            Some(Ok((s, c))) => (Some(s), c),
            _ => (None, Vec::new()),
        }
    }

    /// Põe o pedido na fila (e no registro, se for um trabalho longo); o monitor interno cede o
    /// lugar.
    fn enfileirar(&self, pedido: Pedido, saida: Sender<Saida>) -> Result<(), Box<Resposta>> {
        let e = travar(&self.estado);
        let fila = travar(&self.fila);
        let Some(tx) = fila.as_ref() else {
            return Err(Box::new(encerrando()));
        };
        let job = tipo_do_job(&pedido).map(|(tipo, raizes, execucao)| {
            self.registro.novo_job(
                tipo,
                OrigemJob::from(pedido.autor),
                raizes,
                execucao,
                EstadoJob::Enfileirado,
            )
        });
        if let Some(j) = job {
            travar(&self.pendentes).insert(j, saida.clone());
        }
        if tx.send(Trabalho { pedido, saida, job }).is_err() {
            if let Some(j) = job {
                travar(&self.pendentes).remove(&j);
                self.registro.finalizar(j, &encerrando());
            }
            return Err(Box::new(encerrando()));
        }
        if e.ocupacao == Ocupacao::Monitor {
            self.controle.cancelar();
        }
        Ok(())
    }
}

fn encerrando() -> Resposta {
    Resposta::Erro {
        codigo: "agente_encerrando".into(),
        mensagem: "o agente está encerrando".into(),
    }
}

// ---------------------------------------------------------------------- vigia

/// Lê o monitor e o Ponto de Confiança de tempos em tempos e os publica a quem assina (só há
/// leitura quando alguém assinou), mais a batida de sinal de vida.
fn vigiar(comp: &Arc<Compartilhado>) {
    let mut ultima_leitura = Instant::now()
        .checked_sub(INTERVALO_VIGIA)
        .unwrap_or_else(Instant::now);
    let mut ultima_batida = Instant::now();
    while !comp.parar.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));
        if !comp.registro.tem_assinantes() {
            continue;
        }
        if ultima_leitura.elapsed() >= INTERVALO_VIGIA {
            ultima_leitura = Instant::now();
            if let (Some(saude), confianca) = comp.ler_monitor() {
                comp.registro.observar_monitor(saude, confianca);
            }
        }
        if ultima_batida.elapsed() >= INTERVALO_BATIDA {
            ultima_batida = Instant::now();
            comp.registro.batida();
        }
    }
}

// ---------------------------------------------------------------------- trabalho

enum Passo {
    Sair,
    Trabalho(Trabalho),
    /// Cancelado na fila antes de começar: já foi respondido.
    Pular,
    Monitor,
    Esperar,
}

fn trabalhar(
    mut agente: Agente,
    rx: &Receiver<Trabalho>,
    comp: &Arc<Compartilhado>,
    job_abertura: u64,
) {
    let mut tentar_monitor = comp.monitor;
    let mut pendente: Option<Trabalho> = None;
    let mut ultimo_erro_monitor: Option<String> = None;
    recuperar_ao_abrir(&mut agente, comp, job_abertura);
    loop {
        let passo = {
            let mut e = travar(&comp.estado);
            if comp.parar.load(Ordering::SeqCst) {
                Passo::Sair
            } else {
                let proximo = pendente.take().map_or_else(|| rx.try_recv(), Ok);
                match proximo {
                    Ok(t) => {
                        if let Some(j) = t.job {
                            travar(&comp.pendentes).remove(&j);
                        }
                        if t.job.is_some_and(|j| travar(&comp.cancelados).remove(&j)) {
                            Passo::Pular
                        } else {
                            e.ocupacao = Ocupacao::Trabalho;
                            e.execucao = None;
                            e.job = t.job;
                            comp.controle.reiniciar();
                            if let Some(j) = t.job {
                                comp.registro.iniciar(j);
                            }
                            Passo::Trabalho(t)
                        }
                    }
                    Err(mpsc::TryRecvError::Disconnected) => Passo::Sair,
                    Err(mpsc::TryRecvError::Empty) if tentar_monitor => {
                        tentar_monitor = false;
                        e.ocupacao = Ocupacao::Monitor;
                        e.execucao = None;
                        e.job = None;
                        comp.controle.reiniciar();
                        Passo::Monitor
                    }
                    Err(mpsc::TryRecvError::Empty) => Passo::Esperar,
                }
            }
        };
        match passo {
            Passo::Sair => break,
            Passo::Pular => {}
            Passo::Trabalho(t) => {
                rodar(&mut agente, comp, t);
                tentar_monitor = comp.monitor;
            }
            Passo::Monitor => {
                rodar_monitor(&mut agente, comp, &mut ultimo_erro_monitor);
                let mut e = travar(&comp.estado);
                e.ocupacao = Ocupacao::Ocioso;
                e.job = None;
            }
            Passo::Esperar => match rx.recv_timeout(REPETIR_MONITOR) {
                Ok(t) => pendente = Some(t),
                Err(RecvTimeoutError::Timeout) => tentar_monitor = comp.monitor,
                Err(RecvTimeoutError::Disconnected) => break,
            },
        }
    }
    // Pedidos que ficaram na fila não rodam: o cliente é avisado.
    let mut sobras: Vec<Trabalho> = pendente.into_iter().collect();
    while let Ok(t) = rx.try_recv() {
        sobras.push(t);
    }
    for t in sobras {
        if let Some(j) = t.job {
            travar(&comp.pendentes).remove(&j);
            if !travar(&comp.cancelados).remove(&j) {
                comp.registro.finalizar(j, &encerrando());
            }
        }
        let _ = t.saida.send(Saida::Resposta(Box::new(encerrando())));
    }
    agente.checkpoint();
}

/// Callback de progresso de um trabalho: guarda no registro (que avisa quem assina) e, se
/// `saida` existir, repassa a quem pediu, ambos na mesma cadência limitada.
fn progresso_do_job(
    comp: &Arc<Compartilhado>,
    job: Option<u64>,
    saida: Option<Sender<Saida>>,
    base: Option<crate::agente::CallbackProgresso>,
) -> crate::agente::CallbackProgresso {
    let limitador = Mutex::new(Limitador::default());
    let c = Arc::clone(comp);
    Arc::new(move |p: &Progresso| {
        if let Progresso::Inventario(i) | Progresso::Reconciliacao(i) = p {
            travar(&c.estado).execucao = Some(i.execucao.0);
            if let Some(j) = job {
                c.registro.definir_execucao(j, i.execucao.0);
            }
        }
        if let Some(b) = &base {
            b(p);
        }
        if travar(&limitador).permitir(p) {
            if let Some(j) = job {
                c.registro.progresso(j, p);
            }
            if let Some(tx) = &saida {
                let _ = tx.send(Saida::Progresso(p.clone()));
            }
        }
    })
}

/// Execuções interrompidas, retomada e verificação das Raízes antes de atender pedidos e
/// monitorar. Pedidos que chegam esperam na fila; `Cancelar` e o encerramento a interrompem.
fn recuperar_ao_abrir(agente: &mut Agente, comp: &Arc<Compartilhado>, job: u64) {
    if comp.parar.load(Ordering::SeqCst) {
        comp.registro.finalizar(job, &encerrando());
        return;
    }
    {
        let mut e = travar(&comp.estado);
        e.ocupacao = Ocupacao::Trabalho;
        e.execucao = None;
        e.job = Some(job);
        comp.controle.reiniciar();
    }
    agente.definir_progresso(Some(progresso_do_job(
        comp,
        Some(job),
        None,
        Some(progresso_stderr()),
    )));
    let resposta = match agente.recuperar_ao_abrir(&comp.abertura) {
        Ok(r) if r.relevante() => {
            eprintln!("agente: {r}");
            for a in &r.avisos {
                eprintln!("agente: aviso: {a}");
            }
            Resposta::Ok {
                mensagem: Some(r.to_string()),
            }
        }
        Ok(_) => Resposta::Ok { mensagem: None },
        Err(e) => {
            eprintln!("agente: recuperação ao abrir falhou: {e}");
            e.para_resposta()
        }
    };
    agente.definir_progresso(None);
    {
        let mut e = travar(&comp.estado);
        e.ocupacao = Ocupacao::Ocioso;
        e.execucao = None;
        e.job = None;
    }
    comp.registro.finalizar(job, &resposta);
}

fn rodar(agente: &mut Agente, comp: &Arc<Compartilhado>, t: Trabalho) {
    let Trabalho { pedido, saida, job } = t;
    agente.definir_progresso(Some(progresso_do_job(comp, job, Some(saida.clone()), None)));
    let r = agente.executar_pedido(pedido);
    agente.definir_progresso(None);
    {
        let mut e = travar(&comp.estado);
        e.ocupacao = Ocupacao::Ocioso;
        e.execucao = None;
        e.job = None;
    }
    if let Some(j) = job {
        comp.registro.finalizar(j, &r);
    }
    let _ = saida.send(Saida::Resposta(Box::new(r)));
}

fn rodar_monitor(agente: &mut Agente, comp: &Arc<Compartilhado>, ultimo_erro: &mut Option<String>) {
    // Só aparece como trabalho se a tentativa anterior não falhou do mesmo jeito (sem monitor
    // possível, a repetição a cada minuto não vira ruído no fluxo de eventos).
    let job = ultimo_erro.is_none().then(|| {
        let j = comp.registro.novo_job(
            TipoJob::Monitor,
            OrigemJob::Monitor,
            Vec::new(),
            None,
            EstadoJob::Executando,
        );
        travar(&comp.estado).job = Some(j);
        j
    });
    agente.definir_progresso(Some(progresso_stderr()));
    let r = agente.executar(Comando::Monitorar {
        raizes: Vec::new(),
        reconciliar_a_cada_s: None,
        sem_hash: false,
    });
    agente.definir_progresso(None);
    if let Some(j) = job {
        comp.registro.finalizar(j, &r);
    }
    match r {
        Resposta::Erro { mensagem, .. } => {
            if ultimo_erro.as_ref() != Some(&mensagem) {
                eprintln!("agente: monitor não iniciado: {mensagem}");
                *ultimo_erro = Some(mensagem);
            }
        }
        _ => {
            *ultimo_erro = None;
            if !comp.parar.load(Ordering::SeqCst) {
                eprintln!("agente: monitor pausado para atender um pedido");
            }
        }
    }
}

/// Limita a cadência do progresso (por tipo), sem perder trocas de fase nem o fim.
#[derive(Default)]
struct Limitador {
    ultimo: [Option<Instant>; 4],
    fase: [Option<FaseInventario>; 2],
}

impl Limitador {
    fn permitir(&mut self, p: &Progresso) -> bool {
        let (slot, importante) = match p {
            Progresso::Mensagem { .. } | Progresso::Passo(_) => return true,
            Progresso::Inventario(i) => {
                let novo = self.fase[0] != Some(i.fase);
                self.fase[0] = Some(i.fase);
                (0, novo)
            }
            Progresso::Reconciliacao(i) => {
                let novo = self.fase[1] != Some(i.fase);
                self.fase[1] = Some(i.fase);
                (1, novo)
            }
            // A montagem da fila e o primeiro quadro com os totais definitivos passam sempre.
            Progresso::Hash(h) => (
                2,
                h.preparando
                    || h.janela_ms == 0
                    || (h.arquivos_total > 0 && h.arquivos_feitos >= h.arquivos_total),
            ),
            Progresso::Verificacao(h) => (
                3,
                h.preparando
                    || h.janela_ms == 0
                    || (h.arquivos_total > 0 && h.arquivos_feitos >= h.arquivos_total),
            ),
        };
        let agora = Instant::now();
        let vencido =
            self.ultimo[slot].is_none_or(|t| agora.duration_since(t) >= INTERVALO_PROGRESSO);
        if importante || vencido {
            self.ultimo[slot] = Some(agora);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn so_o_proprio_uid_e_aceito() {
        assert!(uid_autorizado(1000, 1000));
        assert!(!uid_autorizado(1001, 1000));
        assert!(!uid_autorizado(0, 1000));
    }
}
