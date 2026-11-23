//! Worker de sessão (spec §7, Modo 1): uma thread dona do [`Agente`] (e da trava de escritor)
//! que recebe [`Pedido`]s por canal e devolve [`Resposta`]s e eventos de progresso. A GUI nunca
//! escreve no SQLite; se outro processo já é o escritor, o worker abre em leitura e a GUI diz
//! isso. Pausar, retomar e cancelar não passam pelo canal (a thread está ocupada executando o
//! scan): usam o [`ControleScan`], que é uma alça atômica compartilhada.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dag_agent::{Agente, ErroAgente};
use dag_core::{Autor, Comando, Pedido, Resposta, VolumeDescoberto};
use dag_scanner::{ControleScan, EstadoControle, FaseProgresso};

use crate::leitor::Acordador;
use crate::progresso::ProgressoScan;

/// Como o worker abriu o Catálogo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModoWorker {
    Iniciando,
    /// Esta GUI é a escritora.
    Escritor,
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
            Self::SomenteLeitura { .. } => "Worker: outro processo é o escritor".into(),
            Self::Falhou { .. } => "Worker: falhou".into(),
            Self::Encerrado => "Worker: encerrado".into(),
        }
    }

    #[must_use]
    pub fn pode_escrever(&self) -> bool {
        matches!(self, Self::Escritor)
    }
}

/// Parâmetros do worker.
#[derive(Debug, Clone)]
pub struct ConfigWorker {
    pub caminho: PathBuf,
    /// Volumes fixos em vez da descoberta do sistema (testes).
    pub volumes_fixos: Option<Vec<VolumeDescoberto>>,
}

/// Eventos do worker para a GUI.
#[derive(Debug, Clone)]
pub enum Evento {
    /// O Catálogo foi aberto (ou não): modo e alça de controle do scan.
    Modo(ModoWorker),
    /// O worker começou a executar o pedido `id`.
    Iniciado {
        id: u64,
    },
    Progresso(ProgressoScan),
    /// Resposta ao pedido `id`.
    Resposta {
        id: u64,
        resposta: Box<Resposta>,
    },
}

enum Mensagem {
    Pedido { id: u64, pedido: Pedido },
    Sair,
}

/// Alça do worker, usada pela thread de interface.
pub struct Worker {
    tx: Sender<Mensagem>,
    rx: Receiver<Evento>,
    controle: Arc<Mutex<Option<ControleScan>>>,
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

    fn controle(&self) -> Option<ControleScan> {
        self.controle.lock().ok().and_then(|g| g.clone())
    }

    /// Estado do scan no controle compartilhado.
    #[must_use]
    pub fn estado_controle(&self) -> Option<EstadoControle> {
        self.controle().map(|c| c.estado())
    }

    pub fn pausar(&self) {
        if let Some(c) = self.controle() {
            c.pausar();
        }
    }

    pub fn continuar(&self) {
        if let Some(c) = self.controle() {
            c.continuar();
        }
    }

    pub fn cancelar(&self) {
        if let Some(c) = self.controle() {
            c.cancelar();
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

    /// Cancela o que estiver rodando, pede para sair e espera a thread (solta a trava).
    pub fn encerrar(mut self) {
        self.cancelar();
        let _ = self.tx.send(Mensagem::Sair);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.modo = ModoWorker::Encerrado;
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Fechar a janela: cancela com checkpoint e deixa a thread terminar sozinha.
        self.cancelar();
        let _ = self.tx.send(Mensagem::Sair);
    }
}

fn abrir(cfg: &ConfigWorker) -> Result<(Agente, ModoWorker), ModoWorker> {
    match Agente::abrir(&cfg.caminho) {
        Ok(a) => Ok((a, ModoWorker::Escritor)),
        Err(ErroAgente::EscritorOcupado(_)) => match Agente::abrir_leitura(&cfg.caminho) {
            Ok(a) => Ok((
                a,
                ModoWorker::SomenteLeitura {
                    motivo: "outro processo do DAG é o escritor deste Catálogo".into(),
                },
            )),
            Err(e) => Err(ModoWorker::Falhou {
                motivo: e.to_string(),
            }),
        },
        Err(e) => Err(ModoWorker::Falhou {
            motivo: e.to_string(),
        }),
    }
}

fn laco(
    cfg: &ConfigWorker,
    rx: &Receiver<Mensagem>,
    tx: &Sender<Evento>,
    controle: &Arc<Mutex<Option<ControleScan>>>,
    acordador: &Acordador,
) {
    let enviar = |e: Evento| {
        let _ = tx.send(e);
        acordador.acordar();
    };
    let (mut agente, modo) = match abrir(cfg) {
        Ok((a, m)) => (Some(a), m),
        Err(m) => (None, m),
    };
    if let Some(a) = agente.take() {
        let tx_prog = tx.clone();
        let ac = acordador.clone();
        let ultimo = Mutex::new((Instant::now(), None::<FaseProgresso>));
        let mut a = a
            .com_autor(Autor::Gui)
            .com_progresso(move |p: &dag_scanner::Progresso| {
                // No máximo ~8 eventos por segundo, mais toda troca de fase.
                let Ok(mut g) = ultimo.lock() else { return };
                let troca = g.1 != Some(p.fase);
                if troca || g.0.elapsed() >= Duration::from_millis(125) {
                    *g = (Instant::now(), Some(p.fase));
                    let _ = tx_prog.send(Evento::Progresso(ProgressoScan::from(p)));
                    ac.acordar();
                }
            });
        if let Some(v) = &cfg.volumes_fixos {
            a = a.com_volumes_fixos(v.clone());
        }
        if let Ok(mut g) = controle.lock() {
            *g = Some(a.controle());
        }
        agente = Some(a);
    }
    enviar(Evento::Modo(modo.clone()));
    while let Ok(m) = rx.recv() {
        match m {
            Mensagem::Sair => break,
            Mensagem::Pedido { id, pedido } => {
                enviar(Evento::Iniciado { id });
                let resposta = match agente.as_mut() {
                    Some(a) => {
                        // Um scan cancelado antes deixa o controle em "cancelado"; cada novo
                        // trabalho começa do zero.
                        a.controle().reiniciar();
                        a.executar_pedido(pedido)
                    }
                    None => Resposta::Erro {
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
            dispositivo: None,
        }
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
            Resposta::Volumes { volumes } => assert_eq!(volumes.len(), 1),
            outra => panic!("{outra:?}"),
        }
        let id = w.enviar(Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec![],
            sem_hash: true,
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
}
