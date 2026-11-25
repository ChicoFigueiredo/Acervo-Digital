//! Leitura do Catálogo fora da thread de interface: uma thread dona de uma conexão
//! **somente-leitura** executa consultas pequenas (uma página ou um nível, nunca o catálogo
//! inteiro) e devolve o resultado por canal. A UI só pergunta "já chegou?" a cada quadro.

use std::fmt::Display;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use bancada_casca::egui;
use dag_store::consulta_gui::abrir_leitura;
use rusqlite::Connection;

/// Acorda a UI quando algo chegou de outra thread (resultado de consulta, evento do worker).
/// Antes do primeiro quadro não há `Context`: acordar vira um não-fazer-nada.
#[derive(Clone, Default)]
pub struct Acordador(Arc<Mutex<Option<egui::Context>>>);

impl std::fmt::Debug for Acordador {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Acordador")
    }
}

impl Acordador {
    pub fn definir(&self, ctx: &egui::Context) {
        if let Ok(mut g) = self.0.lock()
            && g.is_none()
        {
            *g = Some(ctx.clone());
        }
    }

    pub fn acordar(&self) {
        if let Ok(g) = self.0.lock()
            && let Some(ctx) = g.as_ref()
        {
            ctx.request_repaint();
        }
    }
}

/// Texto de erro para a UI.
pub fn erro<E: Display>(e: E) -> String {
    e.to_string()
}

type Tarefa = Box<dyn FnOnce(&mut Conexao) + Send>;

/// Conexão preguiçosa: o arquivo pode ainda não existir quando a GUI abre.
struct Conexao {
    caminho: PathBuf,
    conn: Option<Connection>,
}

impl Conexao {
    fn obter(&mut self) -> Result<&Connection, String> {
        if self.conn.is_none() {
            if !self.caminho.is_file() {
                return Err(format!(
                    "O Catálogo ainda não existe em {}. Atualize os volumes ou faça uma varredura.",
                    self.caminho.display()
                ));
            }
            self.conn = Some(abrir_leitura(&self.caminho).map_err(erro)?);
        }
        self.conn.as_ref().ok_or_else(|| "sem conexão".to_string())
    }
}

/// Resultado futuro de uma consulta.
#[derive(Debug)]
pub struct Pendente<T>(Receiver<Result<T, String>>);

impl<T> Pendente<T> {
    /// `Some` quando o resultado chegou (uma única vez).
    pub fn tentar(&mut self) -> Option<Result<T, String>> {
        match self.0.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("o leitor do Catálogo parou".into())),
        }
    }

    /// Espera o resultado (testes).
    pub fn esperar(self, limite: std::time::Duration) -> Result<T, String> {
        self.0
            .recv_timeout(limite)
            .unwrap_or_else(|_| Err("tempo esgotado".into()))
    }
}

/// A thread leitora. Descartá-la encerra a thread.
pub struct Leitor {
    tx: Option<Sender<Tarefa>>,
    acordador: Acordador,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Leitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Leitor")
    }
}

impl Leitor {
    #[must_use]
    pub fn novo(caminho: impl Into<PathBuf>, acordador: Acordador) -> Self {
        let (tx, rx) = mpsc::channel::<Tarefa>();
        let mut conexao = Conexao {
            caminho: caminho.into(),
            conn: None,
        };
        let thread = std::thread::Builder::new()
            .name("dag-leitor".into())
            .spawn(move || {
                while let Ok(tarefa) = rx.recv() {
                    tarefa(&mut conexao);
                }
            })
            .ok();
        Self {
            tx: Some(tx),
            acordador,
            thread,
        }
    }

    /// Agenda `f` na thread leitora. O resultado chega pelo [`Pendente`]; a UI é acordada.
    pub fn consultar<T, F>(&self, f: F) -> Pendente<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, String> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let acordador = self.acordador.clone();
        let tarefa: Tarefa = Box::new(move |c: &mut Conexao| {
            let r = c.obter().and_then(f);
            // Se a conexão falhou por erro do SQLite, descarta para reabrir na próxima.
            if r.is_err() {
                c.conn = None;
            }
            let _ = tx.send(r);
            acordador.acordar();
        });
        if let Some(t) = &self.tx {
            // Se a thread já encerrou, o Pendente verá o canal fechado.
            let _ = t.send(tarefa);
        }
        Pendente(rx)
    }
}

impl Drop for Leitor {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Um dado carregado sob demanda: mantém o último valor bom enquanto o novo carrega.
#[derive(Debug)]
pub struct Carga<T> {
    pub dado: Option<T>,
    pub erro: Option<String>,
    pendente: Option<Pendente<T>>,
}

impl<T> Default for Carga<T> {
    fn default() -> Self {
        Self {
            dado: None,
            erro: None,
            pendente: None,
        }
    }
}

impl<T> Carga<T> {
    /// Passa a esperar `p` (descarta uma consulta anterior ainda em voo).
    pub fn pedir(&mut self, p: Pendente<T>) {
        self.pendente = Some(p);
    }

    #[must_use]
    pub fn carregando(&self) -> bool {
        self.pendente.is_some()
    }

    /// Recolhe o resultado, se chegou. `true` quando o dado (ou o erro) mudou neste quadro.
    pub fn atualizar(&mut self) -> bool {
        let Some(p) = self.pendente.as_mut() else {
            return false;
        };
        match p.tentar() {
            None => false,
            Some(r) => {
                self.pendente = None;
                match r {
                    Ok(v) => {
                        self.dado = Some(v);
                        self.erro = None;
                    }
                    Err(e) => self.erro = Some(e),
                }
                true
            }
        }
    }

    /// Como [`Self::atualizar`], mas entrega o valor novo a `f` em vez de substituir `dado`
    /// (para anexar páginas).
    pub fn atualizar_com(&mut self, f: impl FnOnce(&mut Option<T>, T)) -> bool {
        let Some(p) = self.pendente.as_mut() else {
            return false;
        };
        match p.tentar() {
            None => false,
            Some(r) => {
                self.pendente = None;
                match r {
                    Ok(v) => {
                        f(&mut self.dado, v);
                        self.erro = None;
                    }
                    Err(e) => self.erro = Some(e),
                }
                true
            }
        }
    }

    pub fn limpar(&mut self) {
        self.dado = None;
        self.erro = None;
        self.pendente = None;
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_store::Catalogo;
    use std::time::Duration;

    const T: Duration = Duration::from_secs(10);

    #[test]
    fn consulta_em_catalogo_inexistente_devolve_erro_claro() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Leitor::novo(tmp.path().join("nao.sqlite"), Acordador::default());
        let e = l
            .consultar(|c| dag_store::consulta_gui::totais_do_catalogo(c).map_err(erro))
            .esperar(T)
            .unwrap_err();
        assert!(e.contains("ainda não existe"), "{e}");
    }

    #[test]
    fn leitor_abre_so_leitura_depois_que_o_catalogo_aparece() {
        let tmp = tempfile::tempdir().unwrap();
        let caminho = tmp.path().join("c.sqlite");
        let l = Leitor::novo(&caminho, Acordador::default());
        assert!(l.consultar(|_| Ok(1)).esperar(T).is_err());
        let _c = Catalogo::abrir(&caminho).unwrap();
        let t = l
            .consultar(|c| dag_store::consulta_gui::totais_do_catalogo(c).map_err(erro))
            .esperar(T)
            .unwrap();
        assert_eq!(t.volumes, 0);
        // Escrever pela conexão do leitor é impossível.
        let r = l
            .consultar(|c| c.execute("DELETE FROM volumes", []).map_err(erro))
            .esperar(T);
        assert!(r.is_err());
    }

    #[test]
    fn carga_guarda_o_ultimo_valor_bom_e_o_erro() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Leitor::novo(tmp.path().join("x.sqlite"), Acordador::default());
        let mut c: Carga<u32> = Carga::default();
        c.pedir(l.consultar(|_| Ok(7)));
        // Sem catálogo → erro.
        let mut mudou = false;
        for _ in 0..200 {
            if c.atualizar() {
                mudou = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(mudou && c.erro.is_some() && c.dado.is_none());
        assert!(!c.carregando());
    }
}
