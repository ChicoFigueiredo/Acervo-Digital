//! Acompanha o agente em segundo plano de forma contínua (ADR 0004): conecta, assina o fluxo de
//! eventos e, se o agente cair ou reiniciar, reconecta sozinho e entrega um instantâneo novo.
//!
//! Usado pela GUI (que também lança o agente se faltar) e por `dag status --seguir`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dag_core::EventoAgente;

use crate::cliente::tentar_agente_com_info;
use crate::lancar::{OpcoesLancamento, garantir_agente};
use crate::protocolo::InfoServidor;

/// O agente manda uma batida a cada 3 s; sem nada por tanto tempo, a conexão é dada como morta.
const SILENCIO_MAXIMO: Duration = Duration::from_secs(12);

/// Parâmetros do observador.
#[derive(Debug, Clone)]
pub struct OpcoesObservador {
    pub catalogo: PathBuf,
    pub socket: PathBuf,
    /// `Some`: lança o agente quando ele falta (GUI). `None`: só conecta (`dag status --seguir`).
    pub lancamento: Option<OpcoesLancamento>,
    /// Quantas tentativas de conexão falham antes de o observador lançar o agente. Dá tempo de
    /// um supervisor (systemd) reiniciar o agente que caiu sem competir com ele.
    pub tentativas_antes_de_lancar: u32,
    /// Pausa entre tentativas (cresce até 4x).
    pub intervalo: Duration,
}

impl OpcoesObservador {
    #[must_use]
    pub fn novo(catalogo: PathBuf, socket: PathBuf) -> Self {
        Self {
            catalogo,
            socket,
            lancamento: None,
            tentativas_antes_de_lancar: 3,
            intervalo: Duration::from_millis(500),
        }
    }
}

/// O que o observador entrega.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum EventoObservado {
    /// Conectado a um agente; segue o [`EventoAgente::Instantaneo`].
    Conectado(InfoServidor),
    /// Sem agente (ainda, ou a conexão caiu); o observador continua tentando.
    Desconectado {
        motivo: String,
    },
    Evento(EventoAgente),
}

/// Alça do observador; a thread para ao descartá-la.
#[derive(Debug)]
pub struct Observador {
    rx: Receiver<EventoObservado>,
    parar: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

type Acordar = Arc<dyn Fn() + Send + Sync>;

impl Observador {
    /// Sobe a thread. `acordar` é chamado a cada evento entregue (a GUI acorda o quadro).
    #[must_use]
    pub fn iniciar(opcoes: OpcoesObservador, acordar: Option<Acordar>) -> Self {
        let (tx, rx) = mpsc::channel();
        let parar = Arc::new(AtomicBool::new(false));
        let p = Arc::clone(&parar);
        let thread = std::thread::Builder::new()
            .name("dag-observador".into())
            .spawn(move || laco(&opcoes, &tx, &p, acordar.as_ref()))
            .ok();
        Self { rx, parar, thread }
    }

    /// Espera o próximo evento por até `espera`.
    #[must_use]
    pub fn proximo(&self, espera: Duration) -> Option<EventoObservado> {
        self.rx.recv_timeout(espera).ok()
    }

    /// O próximo evento, se já chegou.
    #[must_use]
    pub fn tentar(&self) -> Option<EventoObservado> {
        self.rx.try_recv().ok()
    }

    /// Para a thread e espera.
    pub fn parar(mut self) {
        self.parar.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Observador {
    fn drop(&mut self) {
        self.parar.store(true, Ordering::SeqCst);
    }
}

fn laco(
    o: &OpcoesObservador,
    tx: &Sender<EventoObservado>,
    parar: &AtomicBool,
    acordar: Option<&Acordar>,
) {
    let enviar = |e: EventoObservado| -> bool {
        let ok = tx.send(e).is_ok();
        if let Some(a) = acordar {
            a();
        }
        ok
    };
    let mut falhas: u32 = 0;
    let mut avisou_queda = false;
    while !parar.load(Ordering::SeqCst) {
        let conectado = tentar_agente_com_info(&o.catalogo, &o.socket).or_else(|| {
            if o.lancamento.is_some() && falhas >= o.tentativas_antes_de_lancar {
                let l = o.lancamento.as_ref()?;
                garantir_agente(l).ok().and_then(|mut c| {
                    let info = c.info().ok()?;
                    Some((c, info))
                })
            } else {
                None
            }
        });
        let Some((cliente, info)) = conectado else {
            if !avisou_queda {
                avisou_queda = true;
                let motivo = if falhas == 0 {
                    "conectando ao agente".to_string()
                } else {
                    format!("agente indisponível em {}", o.socket.display())
                };
                if !enviar(EventoObservado::Desconectado { motivo }) {
                    return;
                }
            }
            falhas = falhas.saturating_add(1);
            esperar(parar, o.intervalo * falhas.clamp(1, 4));
            continue;
        };
        let mut assinatura = match cliente.assinar() {
            Ok(a) => a,
            Err(_) => {
                falhas = falhas.saturating_add(1);
                esperar(parar, o.intervalo);
                continue;
            }
        };
        falhas = 0;
        avisou_queda = false;
        if !enviar(EventoObservado::Conectado(info)) {
            return;
        }
        let mut ultimo = Instant::now();
        while !parar.load(Ordering::SeqCst) {
            match assinatura.proximo(Duration::from_millis(250)) {
                Ok(Some(e)) => {
                    ultimo = Instant::now();
                    if !enviar(EventoObservado::Evento(e)) {
                        return;
                    }
                }
                Ok(None) if ultimo.elapsed() < SILENCIO_MAXIMO => {}
                Ok(None) => {
                    let motivo = "o agente parou de responder".to_string();
                    let _ = enviar(EventoObservado::Desconectado { motivo });
                    avisou_queda = true;
                    break;
                }
                Err(e) => {
                    let _ = enviar(EventoObservado::Desconectado {
                        motivo: e.to_string(),
                    });
                    avisou_queda = true;
                    break;
                }
            }
        }
    }
}

/// Dorme em fatias curtas para atender `parar` logo.
fn esperar(parar: &AtomicBool, total: Duration) {
    let fim = Instant::now() + total;
    while !parar.load(Ordering::SeqCst) && Instant::now() < fim {
        std::thread::sleep(Duration::from_millis(25));
    }
}
