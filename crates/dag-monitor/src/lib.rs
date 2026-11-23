//! `dag-monitor`: monitoramento em tempo real das raízes catalogadas (A-13, A-15; spec §6, §12.3–4).
//!
//! # Princípios
//!
//! * **Eventos são dicas, não verdade.** O inotify só diz "algo mudou neste diretório". Cada lote
//!   coalescido dispara uma **reconciliação direcionada** (`read_dir` + `lstat` comparados ao
//!   Catálogo, em `dag-scanner`) aplicada pelo escritor único. Nenhum evento é gravado "na fé".
//! * **Nenhuma perda é silenciosa (CA-A11).** `IN_Q_OVERFLOW`, limite de watches, fila saturada
//!   e raiz desmontada viram uma **lacuna** em `scan_events` com `reconcile_required = 1`; um
//!   Fast Scan da raiz corrige e só então a lacuna ganha `processed_at`.
//! * **Recursos limitados.** Watches recursivos com teto (metade de
//!   `fs.inotify.max_user_watches` por padrão); fila de diretórios sujos com teto (acima dele o
//!   que sobra vira lacuna e Fast Scan); coalescência por diretório com debounce e espera máxima.
//!   A thread leitora nunca bloqueia (para o kernel não estourar a fila de eventos).
//! * **Escritor único.** O monitor roda dentro do processo que detém a trava do Catálogo
//!   (`dag-agent`); um segundo agente não consegue iniciar outro monitor.
//! * **Somente leitura sobre o acervo**: só `inotify_add_watch`, `read_dir`, `lstat`.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod config;
mod erro;
mod fila;
mod monitor;
mod observador;

pub use config::ConfigMonitor;
pub use erro::ErroMonitor;
pub use fila::{Lacuna, TipoLacuna};
pub use monitor::{InjetorMonitor, Monitor, PosReconciliacao, ResumoMonitor};
