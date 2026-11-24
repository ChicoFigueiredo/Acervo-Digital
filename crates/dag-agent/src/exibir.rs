//! Progresso legível em stderr (CLI): uma linha reescrita num terminal; sem terminal, no
//! máximo uma linha a cada 2 s por etapa (mais a final).

use std::io::{IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use dag_core::formato::{bytes_humanos, inteiro_pt_br};
use dag_core::{FaseInventario, Progresso, ProgressoHash, ProgressoInventario};

/// Formata e imprime eventos de progresso em stderr.
#[derive(Debug)]
pub struct ExibidorStderr {
    tty: bool,
    inicio: Instant,
    ultimo_ms: [u64; 4],
    linha_aberta: bool,
}

impl Default for ExibidorStderr {
    fn default() -> Self {
        Self::novo()
    }
}

impl ExibidorStderr {
    #[must_use]
    pub fn novo() -> Self {
        Self {
            tty: std::io::stderr().is_terminal(),
            inicio: Instant::now(),
            ultimo_ms: [0; 4],
            linha_aberta: false,
        }
    }

    /// Mostra o evento (aplicando o limite de cadência).
    pub fn mostrar(&mut self, p: &Progresso) {
        let (slot, fim, linha) = match p {
            Progresso::Mensagem { texto } => {
                let mut e = std::io::stderr().lock();
                if self.linha_aberta {
                    let _ = writeln!(e);
                    self.linha_aberta = false;
                }
                let _ = writeln!(e, "{texto}");
                return;
            }
            Progresso::Inventario(i) => (
                0,
                i.fase == FaseInventario::Concluido,
                linha_inventario("inventário", i),
            ),
            Progresso::Reconciliacao(i) => (
                1,
                i.fase == FaseInventario::Concluido,
                linha_inventario("reconciliação", i),
            ),
            Progresso::Hash(h) => (
                2,
                h.arquivos_total > 0 && h.arquivos_feitos >= h.arquivos_total,
                linha_hash("hash", h),
            ),
            Progresso::Verificacao(h) => (
                3,
                h.arquivos_total > 0 && h.arquivos_feitos >= h.arquivos_total,
                linha_hash("verificação", h),
            ),
        };
        let agora_ms = u64::try_from(self.inicio.elapsed().as_millis()).unwrap_or(u64::MAX);
        if !self.tty && !fim && agora_ms < self.ultimo_ms[slot] + 2000 {
            return;
        }
        self.ultimo_ms[slot] = agora_ms;
        let mut e = std::io::stderr().lock();
        if self.tty {
            let _ = write!(e, "\r\x1b[2K{linha}{}", if fim { "\n" } else { "" });
            self.linha_aberta = !fim;
        } else {
            let _ = writeln!(e, "{linha}");
        }
    }
}

fn linha_inventario(rotulo: &str, i: &ProgressoInventario) -> String {
    let fase = match i.fase {
        FaseInventario::Inventario => rotulo,
        FaseInventario::Pausado => "pausado",
        FaseInventario::Agregacao => "agregando",
        FaseInventario::Concluido => "concluído",
    };
    format!(
        "[{fase}] {} pastas, {} arquivos, {} | {} pendentes | {} itens/s",
        inteiro_pt_br(i.pastas),
        inteiro_pt_br(i.arquivos),
        bytes_humanos(i.bytes_logicos),
        inteiro_pt_br(i.diretorios_pendentes),
        inteiro_pt_br(i.entradas_por_segundo as u64),
    )
}

fn linha_hash(rotulo: &str, p: &ProgressoHash) -> String {
    let ativos: Vec<String> = p
        .discos
        .iter()
        .filter(|d| d.ativo)
        .map(|d| format!("{} {}/s", d.disco, bytes_humanos(d.bytes_por_seg as u64)))
        .collect();
    let discos = if ativos.is_empty() {
        String::new()
    } else {
        format!(" | discos: {}", ativos.join("; "))
    };
    format!(
        "[{rotulo}] {}/{} arquivos, {} de {} lidos | {}/s | {} reaproveitados{discos}",
        inteiro_pt_br(p.arquivos_feitos),
        inteiro_pt_br(p.arquivos_total),
        bytes_humanos(p.bytes_lidos),
        bytes_humanos(p.bytes_total),
        bytes_humanos(p.bytes_por_seg as u64),
        inteiro_pt_br(p.reaproveitados),
    )
}

/// Destino de progresso do [`Agente`](crate::Agente) que escreve em stderr (padrão).
pub(crate) fn progresso_stderr() -> Arc<dyn Fn(&Progresso) + Send + Sync> {
    let e = Mutex::new(ExibidorStderr::novo());
    Arc::new(move |p: &Progresso| {
        if let Ok(mut e) = e.lock() {
            e.mostrar(p);
        }
    })
}
