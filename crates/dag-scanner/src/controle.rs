//! Controle atômico de pausa/cancelamento, compartilhável entre threads.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

const EXECUTANDO: u8 = 0;
const PAUSADO: u8 = 1;
const CANCELADO: u8 = 2;

/// Estado pedido ao scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoControle {
    Executando,
    Pausado,
    Cancelado,
}

/// Alça de controle. Clone e passe para outra thread; `executar`/`retomar` nunca a reiniciam
/// (um cancelamento pedido antes do início vale). Para retomar depois de cancelar, use um
/// controle novo (ou [`reiniciar`](Self::reiniciar)).
#[derive(Debug, Clone, Default)]
pub struct ControleScan(Arc<AtomicU8>);

impl ControleScan {
    #[must_use]
    pub fn novo() -> Self {
        Self::default()
    }

    /// Pausa: o scan termina os diretórios em andamento, grava o lote e espera.
    pub fn pausar(&self) {
        let _ = self
            .0
            .compare_exchange(EXECUTANDO, PAUSADO, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Continua depois de pausar (não desfaz cancelamento).
    pub fn continuar(&self) {
        let _ = self
            .0
            .compare_exchange(PAUSADO, EXECUTANDO, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Cancela: o scan para depois de gravar o que já leu; a fila persiste e pode ser retomada.
    pub fn cancelar(&self) {
        self.0.store(CANCELADO, Ordering::SeqCst);
    }

    /// Volta para `Executando`, inclusive depois de cancelar.
    pub fn reiniciar(&self) {
        self.0.store(EXECUTANDO, Ordering::SeqCst);
    }

    #[must_use]
    pub fn estado(&self) -> EstadoControle {
        match self.0.load(Ordering::SeqCst) {
            EXECUTANDO => EstadoControle::Executando,
            PAUSADO => EstadoControle::Pausado,
            _ => EstadoControle::Cancelado,
        }
    }

    #[must_use]
    pub fn cancelado(&self) -> bool {
        self.estado() == EstadoControle::Cancelado
    }
}
