//! Full Scan somente-leitura (fase de inventário): percorre as raízes escolhidas, grava pastas,
//! arquivos, symlinks e entradas especiais no Catálogo e agrega os totais. **Não calcula hash**
//! (isso é a etapa [`EtapaHash`]) e **nunca modifica** nada do que varre.
//!
//! # Pipeline
//!
//! ```text
//!  scan_queue (SQLite) ──reserva──▶ coordenador ──Job──▶ N workers (read_dir + lstat)
//!        ▲                              │  ▲                         │ Msg (lotes ≤ 2048 entradas)
//!        └── novas pastas, na mesma ────┘  └──── canal limitado ◀────┘
//!            transação do lote
//! ```
//!
//! * Os workers só leem; um único coordenador é o escritor do Catálogo e agrupa as mensagens em
//!   transações de ~4 mil linhas. Pastas novas entram em `scan_queue` na mesma transação que as
//!   grava, então o checkpoint nunca fica à frente do que foi persistido.
//! * Memória limitada: a árvore nunca fica em RAM. A fronteira de diretórios vive no banco; em
//!   memória há só `reservas_por_vez` diretórios reservados, o canal limitado de resultados e o
//!   lote em montagem. O `parent_folder_id` vem junto com o job (não há mapa caminho→id).
//! * Retomada: diretório `reservada` sem `concluida` é refeito; as gravações são upserts por
//!   `(volume_id, path_raw)`, então refazer não duplica.
//!
//! # Regras
//!
//! Symlinks são entradas (`files.kind = 'symlink'`, alvo bruto em `link_target_raw`) e nunca são
//! seguidos. Uma pasta cujo `(dev, inode)` já foi vista nesta execução por outro caminho (bind
//! mount, alias) não é varrida de novo. Entrar em outro `st_dev` é fronteira de volume: não se
//! cruza (registrado como item ignorado, com o motivo). Pseudo-filesystems são identificados pelo
//! flag `pseudo` do volume, nunca pelo nome da pasta.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod classificador;
mod config;
mod controle;
mod erro;
mod hash;
mod scan;
mod visita;

pub use classificador::{Classificador, FonteEvidencia, SemClassificacao, TipoDetectado};
pub use config::{Casamento, ConfigScan, Exclusao, FaseProgresso, Progresso};
pub use controle::{ControleScan, EstadoControle};
pub use erro::ErroScan;
pub use hash::{
    ConfigHash, Desfecho, EtapaHash, ProgressoHash, ResultadoHash, ResultadoVerificacao,
    VerificacaoIntegridade,
};
pub use scan::{DesfechoScan, FullScan, ResultadoScan};
