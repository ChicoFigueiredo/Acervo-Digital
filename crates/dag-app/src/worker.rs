//! Worker da GUI (spec §6 e §7): a GUI nunca escreve no SQLite e nunca inicia um segundo
//! escritor.
//!
//! * **Modo 2/3 (serviço)**: se há um agente em segundo plano (`dag agente`) servindo este
//!   Catálogo no socket, o worker é cliente dele: os pedidos e o progresso (inclusive bytes do
//!   hash) passam pelo socket; pausar, retomar e cancelar vão por uma segunda conexão.
//! * **Modo 1 (sessão)**: senão, uma thread dona do [`Agente`] (e da trava de escritor) recebe
//!   [`Pedido`]s por canal. Se outro processo já é o escritor, o worker abre em leitura e a GUI
//!   diz isso. Pausar, retomar e cancelar usam o [`ControleScan`] compartilhado (a thread está
//!   ocupada executando o scan).

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dag_agent::{Agente, Cliente, ErroAgente, caminho_socket_padrao};
use dag_core::{Autor, Comando, FaseInventario, Pedido, Progresso, Resposta, VolumeDescoberto};
use dag_scanner::{ControleScan, EstadoControle};

use crate::leitor::Acordador;
use crate::progresso::ProgressoScan;

/// Como o worker abriu o Catálogo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModoWorker {
    Iniciando,
    /// Esta GUI é a escritora.
    Escritor,
    /// Um agente em segundo plano (serviço) é o escritor e a GUI é cliente dele.
    Servico,
    /// Outro processo é o escritor (ou o Catálogo só pôde ser aberto para leitura).
    SomenteLeitura {
        motivo: String,
    },
    /// Nem leitura foi possível.
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
            Self::Iniciando => "Worker: iniciando…".into(),
            Self::Escritor => "Worker: escritor (esta sessão)".into(),
            Self::Servico => "Worker: serviço".into(),
            Self::SomenteLeitura { .. } => "Worker: outro processo é o escritor".into(),
            Self::Falhou { .. } => "Worker: falhou".into(),
            Self::Encerrado => "Worker: encerrado".into(),
        }
    }

    /// Esta GUI consegue pedir escritas (ela mesma ou o serviço as executa)?
    #[must_use]
    pub fn pode_escrever(&self) -> bool {
        matches!(self, Self::Escritor | Self::Servico)
    }
}

/// Parâmetros do worker.
#[derive(Debug, Clone)]
pub struct ConfigWorker {
    pub caminho: PathBuf,
    /// Volumes fixos em vez da descoberta do sistema (testes; só no modo sessão).
    pub volumes_fixos: Option<Vec<VolumeDescoberto>>,
    /// Socket do agente em segundo plano; `None` = o padrão (`$DAG_SOCKET`, `$XDG_RUNTIME_DIR`).
    pub socket: Option<PathBuf>,
}

/// Eventos do worker para a GUI.
#[derive(Debug, Clone)]
pub enum Evento {
    /// O Catálogo foi aberto (ou não): modo e alça de controle do scan.
    Modo(ModoWorker),
    /// O worker começou a executar o pedido `id`.
    Iniciado { id: u64 },
    /// Inventário ou reconciliação.
    Progresso(ProgressoScan),
    /// Etapa de Hash Completo (ou verificação): arquivos e bytes reais, sem consultar o
    /// Catálogo.
    Hash(dag_core::ProgressoHash),
    /// Resposta ao pedido `id`.
    Resposta { id: u64, resposta: Box<Resposta> },
}

enum Mensagem {
    Pedido { id: u64, pedido: Pedido },
    Sair,
}

/// Alça para controlar o trabalho em curso.
#[derive(Debug, Clone)]
enum Alca {
    /// O `ControleScan` do agente de sessão.
    Local(ControleScan),
    /// O agente em segundo plano: o estado é o último comando aceito por ele.
    Remota {
        socket: PathBuf,
        estado: Arc<Mutex<EstadoControle>>,
    },
}

impl Alca {
    fn estado(&self) -> EstadoControle {
        match self {
            Self::Local(c) => c.estado(),
            Self::Remota { estado, .. } => estado.lock().map_or(EstadoControle::Executando, |e| *e),
        }
    }

    /// Cada pedido novo começa do zero.
    fn reiniciar(&self) {
        match self {
            Self::Local(c) => c.reiniciar(),
            Self::Remota { estado, .. } => {
                if let Ok(mut e) = estado.lock() {
                    *e = EstadoControle::Executando;
                }
            }
        }
    }

    fn agir(&self, local: fn(&ControleScan), comando: Comando, novo: EstadoControle) {
        match self {
            Self::Local(c) => local(c),
            Self::Remota { socket, estado } => {
                let anterior = estado.lock().map_or(EstadoControle::Executando, |e| *e);
                if let Ok(mut e) = estado.lock() {
                    *e = novo;
                }
                let (socket, estado) = (socket.clone(), Arc::clone(estado));
                // Conexão própria: a do pedido está esperando o fim do scan.
                let _ = std::thread::Builder::new()
                    .name("dag-worker-controle".into())
                    .spawn(move || {
                        let ok = Cliente::conectar(&socket)
                            .and_then(|mut c| {
                                c.enviar(
                                    Pedido {
                                        autor: Autor::Gui,
                                        comando,
                                    },
                                    &mut |_| {},
                                )
                            })
                            .is_ok_and(|r| !matches!(r, Resposta::Erro { .. }));
                        if !ok && let Ok(mut e) = estado.lock() {
                            *e = anterior;
                        }
                    });
            }
        }
    }

    fn pausar(&self) {
        self.agir(
            ControleScan::pausar,
            Comando::Pausar { execucao: None },
            EstadoControle::Pausado,
        );
    }

    fn continuar(&self) {
        self.agir(
            ControleScan::continuar,
            Comando::Retomar { execucao: None },
            EstadoControle::Executando,
        );
    }

    fn cancelar(&self) {
        self.agir(
            ControleScan::cancelar,
            Comando::Cancelar { execucao: None },
            EstadoControle::Cancelado,
        );
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
        }
    }

    #[must_use]
    pub fn modo(&self) -> &ModoWorker {
        &self.modo
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

    /// Estado do scan no controle (local ou o último aceito pelo serviço).
    #[must_use]
    pub fn estado_controle(&self) -> Option<EstadoControle> {
        self.alca().map(|a| a.estado())
    }

    pub fn pausar(&self) {
        if let Some(a) = self.alca() {
            a.pausar();
        }
    }

    pub fn continuar(&self) {
        if let Some(a) = self.alca() {
            a.continuar();
        }
    }

    pub fn cancelar(&self) {
        if let Some(a) = self.alca() {
            a.cancelar();
        }
    }

    /// Ao sair, a varredura da própria sessão é cancelada com checkpoint; a de um serviço
    /// continua (ela não pertence a esta janela).
    fn cancelar_ao_sair(&self) {
        if let Some(a) = self.alca().filter(Alca::eh_local) {
            a.cancelar();
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
        if let Evento::Modo(m) = e {
            self.modo = m.clone();
        }
    }

    /// Cancela o que estiver rodando (se for desta sessão), pede para sair e espera a thread
    /// (solta a trava). Com o serviço, a thread pode estar esperando uma resposta longa: não é
    /// esperada.
    pub fn encerrar(mut self) {
        self.cancelar_ao_sair();
        let _ = self.tx.send(Mensagem::Sair);
        let remoto = self.alca().is_some_and(|a| !a.eh_local());
        if let Some(t) = self.thread.take()
            && !remoto
        {
            let _ = t.join();
        }
        self.modo = ModoWorker::Encerrado;
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Fechar a janela: cancela com checkpoint e deixa a thread terminar sozinha.
        self.cancelar_ao_sair();
        let _ = self.tx.send(Mensagem::Sair);
    }
}

/// Onde os pedidos são executados.
#[allow(clippy::large_enum_variant)]
enum Destino {
    Sessao(Agente),
    Servico(Cliente),
    Nenhum,
}

fn abrir(cfg: &ConfigWorker, enc: &Arc<Encaminhador>) -> (Destino, ModoWorker) {
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
    // Já há um agente servindo este Catálogo? Então a GUI é cliente dele, nunca um segundo
    // escritor.
    let socket = cfg.socket.clone().unwrap_or_else(caminho_socket_padrao);
    if let Some(c) = Cliente::conectar_se_serve(&socket, &cfg.caminho) {
        return (Destino::Servico(c), ModoWorker::Servico);
    }
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

/// Converte eventos de progresso do agente em eventos da GUI, com cadência limitada (~8 por
/// segundo por tipo, mais toda troca de fase e o fim do hash).
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
                let fim = h.arquivos_total > 0 && h.arquivos_feitos >= h.arquivos_total;
                if !fim && g.ultimo_hash.elapsed() < CADENCIA {
                    return;
                }
                g.ultimo_hash = Instant::now();
                Evento::Hash(h.clone())
            }
            Progresso::Mensagem { .. } => return,
        };
        drop(g);
        let _ = self.tx.send(ev);
        self.acordador.acordar();
    }
}

fn laco(
    cfg: &ConfigWorker,
    rx: &Receiver<Mensagem>,
    tx: &Sender<Evento>,
    controle: &Arc<Mutex<Option<Alca>>>,
    acordador: &Acordador,
) {
    let enviar = |e: Evento| {
        let _ = tx.send(e);
        acordador.acordar();
    };
    let enc = Arc::new(Encaminhador::novo(tx.clone(), acordador.clone()));
    let (mut destino, modo) = abrir(cfg, &enc);
    let alca = match &destino {
        Destino::Sessao(a) => Some(Alca::Local(a.controle())),
        Destino::Servico(c) => Some(Alca::Remota {
            socket: c.socket().to_path_buf(),
            estado: Arc::new(Mutex::new(EstadoControle::Executando)),
        }),
        Destino::Nenhum => None,
    };
    if let Ok(mut g) = controle.lock() {
        g.clone_from(&alca);
    }
    enviar(Evento::Modo(modo.clone()));
    while let Ok(m) = rx.recv() {
        match m {
            Mensagem::Sair => break,
            Mensagem::Pedido { id, pedido } => {
                enviar(Evento::Iniciado { id });
                // Um scan cancelado antes deixa o controle em "cancelado"; cada novo trabalho
                // começa do zero.
                if let Some(a) = &alca {
                    a.reiniciar();
                }
                let resposta = match &mut destino {
                    Destino::Sessao(a) => a.executar_pedido(pedido),
                    Destino::Servico(c) => c
                        .enviar(pedido, &mut |p| enc.encaminhar(p))
                        .unwrap_or_else(|e| e.para_resposta()),
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
    use dag_core::ModoVarredura;
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

    #[test]
    fn worker_executa_full_scan_com_progresso_e_conclusao() {
        let (_tmp, raiz, catalogo) = cenario();
        let mut w = Worker::iniciar(
            ConfigWorker {
                caminho: catalogo.clone(),
                volumes_fixos: Some(vec![volume_de(&raiz)]),
                socket: Some(sem_socket()),
            },
            Acordador::default(),
        );
        let (e, _) = esperar_ate(&mut w, |e| matches!(e, Evento::Modo(_)));
        assert!(matches!(e, Evento::Modo(ModoWorker::Escritor)));
        assert!(w.modo().pode_escrever());
        assert!(w.estado_controle().is_some());

        let id = w.enviar(Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec![raiz.to_str().unwrap().into()],
            sem_hash: false,
            elevar: false,
        });
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
        let ultimo = progressos.last().unwrap();
        assert_eq!(ultimo.arquivos, 4);
        let Evento::Resposta { resposta, .. } = fim else {
            unreachable!()
        };
        let resposta = *resposta;
        match resposta {
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
        let mut w = Worker::iniciar(
            ConfigWorker {
                caminho: catalogo,
                volumes_fixos: Some(vec![volume_de(&raiz)]),
                socket: Some(sem_socket()),
            },
            Acordador::default(),
        );
        let id = w.enviar(Comando::ListarVolumes);
        let (e, _) = esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Resposta { id: i, .. } if *i == id),
        );
        let Evento::Resposta { resposta, .. } = e else {
            unreachable!()
        };
        let resposta = *resposta;
        match resposta {
            Resposta::Volumes { volumes, .. } => assert_eq!(volumes.len(), 1),
            outra => panic!("{outra:?}"),
        }
        let id = w.enviar(Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec![],
            sem_hash: true,
            elevar: false,
        });
        let (e, _) = esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Resposta { id: i, .. } if *i == id),
        );
        let Evento::Resposta { resposta, .. } = e else {
            unreachable!()
        };
        let resposta = *resposta;
        assert!(matches!(resposta, Resposta::Erro { .. }));
        w.encerrar();
    }

    #[test]
    fn com_o_escritor_ocupado_o_worker_abre_em_leitura() {
        let (_tmp, raiz, catalogo) = cenario();
        // Outro "processo" (aqui, outra abertura do mesmo arquivo de trava) é o escritor.
        let dono = Agente::abrir(&catalogo).unwrap().silencioso();
        let mut w = Worker::iniciar(
            ConfigWorker {
                caminho: catalogo.clone(),
                volumes_fixos: Some(vec![volume_de(&raiz)]),
                socket: Some(sem_socket()),
            },
            Acordador::default(),
        );
        let (e, _) = esperar_ate(&mut w, |e| matches!(e, Evento::Modo(_)));
        let Evento::Modo(m) = e else { unreachable!() };
        assert!(matches!(m, ModoWorker::SomenteLeitura { .. }), "{m:?}");
        assert_eq!(m.texto_status(), "Worker: outro processo é o escritor");
        assert!(!m.pode_escrever());
        // Comando de escrita é recusado; leitura funciona.
        let id = w.enviar(Comando::ListarVolumes);
        let (e, _) = esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Resposta { id: i, .. } if *i == id),
        );
        let Evento::Resposta { resposta, .. } = e else {
            unreachable!()
        };
        let resposta = *resposta;
        assert!(matches!(resposta, Resposta::Erro { .. }), "{resposta:?}");
        let id = w.enviar(Comando::Status { execucao: None });
        let (e, _) = esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Resposta { id: i, .. } if *i == id),
        );
        let Evento::Resposta { resposta, .. } = e else {
            unreachable!()
        };
        let resposta = *resposta;
        assert!(matches!(resposta, Resposta::Status { .. }), "{resposta:?}");
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
            },
            Acordador::default(),
        );
        let (e, _) = esperar_ate(&mut w, |e| matches!(e, Evento::Modo(_)));
        let Evento::Modo(m) = e else { unreachable!() };
        assert!(matches!(m, ModoWorker::Falhou { .. }), "{m:?}");
        let id = w.enviar(Comando::ListarVolumes);
        let (e, _) = esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Resposta { id: i, .. } if *i == id),
        );
        let Evento::Resposta { resposta, .. } = e else {
            unreachable!()
        };
        let resposta = *resposta;
        assert!(
            matches!(resposta, Resposta::Erro { codigo, .. } if codigo == "worker_indisponivel")
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
        let catalogo = tmp.path().join("c.sqlite");
        let mut w = Worker::iniciar(
            ConfigWorker {
                caminho: catalogo,
                volumes_fixos: Some(vec![volume_de(&raiz)]),
                socket: Some(sem_socket()),
            },
            Acordador::default(),
        );
        esperar_ate(&mut w, |e| matches!(e, Evento::Modo(_)));
        w.cancelar();
        // Cancelamento antes do início vale para o scan em curso; o worker reinicia o controle
        // ao começar cada pedido, então este roda normalmente.
        let id = w.enviar(Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec![raiz.to_str().unwrap().into()],
            sem_hash: true,
            elevar: false,
        });
        let (e, _) = esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Resposta { id: i, .. } if *i == id),
        );
        let Evento::Resposta { resposta, .. } = e else {
            unreachable!()
        };
        let resposta = *resposta;
        assert!(
            matches!(&resposta, Resposta::ScanConcluido { desfecho, arquivos: 300, .. } if desfecho == "concluida"),
            "{resposta:?}"
        );
        w.encerrar();
    }

    fn servidor(raiz: &Path, catalogo: &Path, socket: &Path) -> dag_agent::Servidor {
        let mut cfg = dag_agent::ConfigServidor::novo(catalogo, socket);
        cfg.monitor = false;
        cfg.volumes_fixos = Some(vec![volume_de(raiz)]);
        dag_agent::Servidor::iniciar(cfg).unwrap()
    }

    #[test]
    fn worker_conecta_ao_servico_sem_segundo_escritor_e_recebe_hash_com_bytes() {
        let (tmp, raiz, catalogo) = cenario();
        let socket = tmp.path().join("run/agente.sock");
        let srv = servidor(&raiz, &catalogo, &socket);
        let mut w = Worker::iniciar(
            ConfigWorker {
                caminho: catalogo.clone(),
                volumes_fixos: None,
                socket: Some(socket.clone()),
            },
            Acordador::default(),
        );
        let (e, _) = esperar_ate(&mut w, |e| matches!(e, Evento::Modo(_)));
        let Evento::Modo(m) = e else { unreachable!() };
        assert_eq!(m, ModoWorker::Servico);
        assert_eq!(m.texto_status(), "Worker: serviço");
        assert!(m.pode_escrever());
        // A GUI não abriu um segundo escritor: a trava é do serviço.
        assert!(matches!(
            Agente::abrir(&catalogo).unwrap_err(),
            ErroAgente::EscritorOcupado(_)
        ));
        let id = w.enviar(Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec![raiz.to_str().unwrap().into()],
            sem_hash: false,
            elevar: false,
        });
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
    fn pausar_e_cancelar_vao_pelo_socket_no_modo_servico() {
        let (tmp, raiz, catalogo) = cenario();
        for d in 0..120 {
            for f in 0..60 {
                escrever(&raiz.join(format!("g{d}/f{f}.txt")), 64 + f);
            }
        }
        let socket = tmp.path().join("run/agente.sock");
        let srv = servidor(&raiz, &catalogo, &socket);
        let mut w = Worker::iniciar(
            ConfigWorker {
                caminho: catalogo,
                volumes_fixos: None,
                socket: Some(socket),
            },
            Acordador::default(),
        );
        esperar_ate(&mut w, |e| matches!(e, Evento::Modo(ModoWorker::Servico)));
        let id = w.enviar(Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec![raiz.to_str().unwrap().into()],
            sem_hash: false,
            elevar: false,
        });
        // Pausa assim que o scan começa a reportar.
        esperar_ate(&mut w, |e| matches!(e, Evento::Progresso(_)));
        w.pausar();
        assert_eq!(w.estado_controle(), Some(EstadoControle::Pausado));
        esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Progresso(p) if p.fase == crate::progresso::FaseScan::Pausado),
        );
        w.cancelar();
        let (fim, _) = esperar_ate(
            &mut w,
            |e| matches!(e, Evento::Resposta { id: i, .. } if *i == id),
        );
        let Evento::Resposta { resposta, .. } = fim else {
            unreachable!()
        };
        assert!(
            matches!(&*resposta, Resposta::ScanConcluido { desfecho, .. } if desfecho == "cancelada"),
            "{resposta:?}"
        );
        w.encerrar();
        srv.encerrar();
    }
}
