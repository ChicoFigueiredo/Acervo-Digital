//! Fila compartilhada entre a thread leitora do inotify e o escritor: diretórios sujos
//! coalescidos, lacunas e contadores. Tudo limitado; nada aqui bloqueia a leitora por muito tempo.

use std::collections::{HashMap, HashSet};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Por que o monitor não pode confiar que viu tudo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipoLacuna {
    /// `IN_Q_OVERFLOW`: o kernel descartou eventos.
    Overflow,
    /// Teto de watches (ou `max_user_watches` do sistema) atingido: parte da árvore sem watch.
    LimiteDeWatches,
    /// Fila de diretórios sujos cheia: o detalhe foi descartado.
    FilaSaturada,
    /// Raiz sumiu (desmontada, renomeada, `st_dev` diferente).
    RaizDesmontada,
    /// Injetada por teste ou operador.
    Injetada,
}

impl TipoLacuna {
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::Overflow => "overflow",
            Self::LimiteDeWatches => "limite_de_watches",
            Self::FilaSaturada => "fila_saturada",
            Self::RaizDesmontada => "raiz_desmontada",
            Self::Injetada => "injetada",
        }
    }
}

/// Lacuna detectada, a ser gravada e corrigida pelo escritor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lacuna {
    pub tipo: TipoLacuna,
    /// Índice da raiz afetada; `None` = todas (não dá para saber).
    pub raiz: Option<usize>,
    pub detalhe: String,
    pub observado_em: i64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Sujo {
    pub primeiro: Instant,
    pub ultimo: Instant,
    pub observado_em: i64,
}

#[derive(Debug, Default)]
pub(crate) struct EstadoFila {
    pub sujos: HashMap<Vec<u8>, Sujo>,
    pub lacunas: Vec<Lacuna>,
    pub saturadas: HashSet<usize>,
    pub ultimo_evento: Option<i64>,
    pub eventos_brutos: u64,
    pub overflows: u64,
    pub watches: usize,
    pub watches_limite: usize,
    pub parciais: HashSet<usize>,
    /// Raízes cujos watches precisam ser refeitos (voltaram depois de desmontadas).
    pub rearmar: Vec<usize>,
    pub encerrar: bool,
    pub erro: Option<String>,
}

pub(crate) struct Fila {
    estado: Mutex<EstadoFila>,
    pub acordar: Condvar,
    max_dirs: usize,
}

pub(crate) fn agora() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

impl Fila {
    pub fn nova(max_dirs: usize) -> Self {
        Self {
            estado: Mutex::new(EstadoFila::default()),
            acordar: Condvar::new(),
            max_dirs: max_dirs.max(1),
        }
    }

    pub fn travar(&self) -> MutexGuard<'_, EstadoFila> {
        self.estado.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Dica de que `dir` mudou. Coalesce por diretório; com a fila cheia vira lacuna da raiz.
    pub fn marcar(&self, dir: &[u8], raiz: usize) {
        let agora_i = Instant::now();
        let t = agora();
        let mut e = self.travar();
        e.eventos_brutos += 1;
        e.ultimo_evento = Some(t);
        if let Some(s) = e.sujos.get_mut(dir) {
            s.ultimo = agora_i;
        } else if e.sujos.len() >= self.max_dirs {
            if e.saturadas.insert(raiz) {
                e.lacunas.push(Lacuna {
                    tipo: TipoLacuna::FilaSaturada,
                    raiz: Some(raiz),
                    detalhe: format!(
                        "fila com {} diretórios sujos: detalhe descartado",
                        self.max_dirs
                    ),
                    observado_em: t,
                });
            }
        } else {
            e.sujos.insert(
                dir.to_vec(),
                Sujo {
                    primeiro: agora_i,
                    ultimo: agora_i,
                    observado_em: t,
                },
            );
        }
        drop(e);
        self.acordar.notify_all();
    }

    pub fn lacuna(&self, l: Lacuna) {
        let mut e = self.travar();
        if l.tipo == TipoLacuna::Overflow {
            e.overflows += 1;
        }
        e.ultimo_evento = Some(l.observado_em);
        e.lacunas.push(l);
        drop(e);
        self.acordar.notify_all();
    }

    pub fn encerrar(&self) {
        self.travar().encerrar = true;
        self.acordar.notify_all();
    }

    pub fn encerrando(&self) -> bool {
        self.travar().encerrar
    }
}
