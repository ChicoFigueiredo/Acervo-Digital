//! Execução do `calibredb` atrás de um trait, para testar sem o Calibre instalado.

use std::collections::VecDeque;
use std::fmt;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::erro::{ErroCalibre, Resultado};

/// Um comando para o `calibredb`. O `stdin` pode carregar a senha (nunca vai em `args`).
#[derive(Clone)]
pub struct Comando {
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Duration,
}

impl fmt::Debug for Comando {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Comando")
            .field("args", &self.args)
            .field("stdin", &self.stdin.as_ref().map(|_| "<oculto>"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl Comando {
    /// Texto para mensagens de erro (sem stdin, portanto sem senha).
    #[must_use]
    pub fn descricao(&self) -> String {
        self.args.join(" ")
    }
}

/// Saída de um processo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaidaProcesso {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl SaidaProcesso {
    #[must_use]
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            status: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    #[must_use]
    pub fn falha(stderr: impl Into<String>) -> Self {
        Self {
            status: Some(1),
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    #[must_use]
    pub fn sucesso(&self) -> bool {
        self.status == Some(0)
    }
}

/// Quem sabe rodar o `calibredb`.
pub trait ExecutorCalibre: Send + Sync + fmt::Debug {
    /// Executa o comando (argumentos já sem o nome do programa).
    ///
    /// # Errors
    /// `CalibredbAusente`, `Timeout` ou E/S. Status de saída diferente de zero NÃO é erro aqui.
    fn executar(&self, cmd: &Comando) -> Resultado<SaidaProcesso>;
}

/// Executor real: roda o binário `calibredb`.
#[derive(Debug, Clone)]
pub struct ExecutorProcesso {
    binario: PathBuf,
    ambiente: Vec<(String, String)>,
    prefixo: Vec<String>,
}

impl Default for ExecutorProcesso {
    fn default() -> Self {
        Self::novo("calibredb")
    }
}

impl ExecutorProcesso {
    /// Usa o binário dado (nome no PATH ou caminho).
    #[must_use]
    pub fn novo(binario: impl Into<PathBuf>) -> Self {
        Self {
            prefixo: Vec::new(),
            binario: binario.into(),
            // Saída estável: inglês, UTF-8. Mensagens de erro são casadas em inglês.
            ambiente: vec![
                ("CALIBRE_OVERRIDE_LANG".into(), "en".into()),
                ("LC_ALL".into(), "C.UTF-8".into()),
                ("PYTHONIOENCODING".into(), "utf-8".into()),
                ("PYTHONUTF8".into(), "1".into()),
            ],
        }
    }

    /// Prefixo do comando (ex.: `["unshare", "-r", "-n"]`). DESLIGADO por padrão.
    ///
    /// `unshare -r -n` dá ao `calibredb` um namespace de rede novo, e portanto um socket Unix
    /// abstrato de lock (`calibre-singleinstance-<uid>-db`) separado: o calibredb local passa a
    /// funcionar com o GUI/`calibre-server` de pé. RISCO: a trava existe para impedir dois
    /// escritores na mesma biblioteca; só é seguro se a biblioteca alvo NÃO estiver aberta por
    /// outro processo Calibre (a A Classificar não é servida pelo server). Nunca usar na
    /// Homologada. Em namespace novo não há rede: não serve para o modo servidor. Usar com a
    /// política de trava `ApenasServidor`.
    #[must_use]
    pub fn com_prefixo(mut self, prefixo: Vec<String>) -> Self {
        self.prefixo = prefixo;
        self
    }

    /// O prefixo `unshare -r -n`.
    #[must_use]
    pub fn prefixo_unshare() -> Vec<String> {
        vec!["unshare".into(), "-r".into(), "-n".into()]
    }

    /// Variável de ambiente extra (p. ex. `CALIBRE_CONFIG_DIRECTORY` em testes).
    #[must_use]
    pub fn com_ambiente(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.ambiente.push((k.into(), v.into()));
        self
    }

    /// Procura o `calibredb` no PATH; `None` se não existir.
    #[must_use]
    pub fn disponivel() -> Option<Self> {
        let e = Self::default();
        let c = Comando {
            args: vec!["--version".into()],
            stdin: None,
            timeout: Duration::from_secs(60),
        };
        match e.executar(&c) {
            Ok(s) if s.sucesso() => Some(e),
            _ => None,
        }
    }
}

impl ExecutorCalibre for ExecutorProcesso {
    fn executar(&self, cmd: &Comando) -> Resultado<SaidaProcesso> {
        let mut c = if let Some((prog, resto)) = self.prefixo.split_first() {
            let mut c = Command::new(prog);
            c.args(resto).arg(&self.binario);
            c
        } else {
            Command::new(&self.binario)
        };
        c.args(&cmd.args)
            .stdin(if cmd.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &self.ambiente {
            c.env(k, v);
        }
        let mut filho = c.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ErroCalibre::CalibredbAusente(format!("{} não encontrado", self.binario.display()))
            } else {
                ErroCalibre::io(format!("executar {}", self.binario.display()), e)
            }
        })?;
        if let (Some(dados), Some(mut si)) = (cmd.stdin.clone(), filho.stdin.take()) {
            std::thread::spawn(move || {
                let _ = si.write_all(&dados);
            });
        }
        let mut so = filho.stdout.take();
        let mut se = filho.stderr.take();
        let t_out = std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(s) = so.as_mut() {
                let _ = s.read_to_end(&mut b);
            }
            b
        });
        let t_err = std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(s) = se.as_mut() {
                let _ = s.read_to_end(&mut b);
            }
            b
        });
        let inicio = Instant::now();
        let status = loop {
            match filho.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {
                    if inicio.elapsed() >= cmd.timeout {
                        let _ = filho.kill();
                        let _ = filho.wait();
                        let _ = t_out.join();
                        let _ = t_err.join();
                        return Err(ErroCalibre::Timeout {
                            comando: cmd.descricao(),
                            segundos: cmd.timeout.as_secs(),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(15));
                }
                Err(e) => return Err(ErroCalibre::io("aguardar calibredb", e)),
            }
        };
        let out = t_out.join().unwrap_or_default();
        let err = t_err.join().unwrap_or_default();
        Ok(SaidaProcesso {
            status: status.code(),
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
        })
    }
}

type Roteiro = Box<dyn Fn(&Comando) -> Resultado<SaidaProcesso> + Send + Sync>;

/// Executor falso: registra as chamadas e responde por uma função ou por uma fila de respostas.
/// Serve aos testes sem Calibre instalado.
pub struct ExecutorFalso {
    chamadas: Mutex<Vec<Comando>>,
    fila: Mutex<VecDeque<Resultado<SaidaProcesso>>>,
    funcao: Option<Roteiro>,
}

impl fmt::Debug for ExecutorFalso {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutorFalso")
            .field("chamadas", &self.chamadas.lock().map(|c| c.len()).ok())
            .finish_non_exhaustive()
    }
}

impl ExecutorFalso {
    /// Responde com a fila (na ordem); fila vazia responde sucesso sem saída.
    #[must_use]
    pub fn com_fila(respostas: Vec<Resultado<SaidaProcesso>>) -> Self {
        Self {
            chamadas: Mutex::new(Vec::new()),
            fila: Mutex::new(respostas.into()),
            funcao: None,
        }
    }

    /// Responde chamando a função.
    #[must_use]
    pub fn com_funcao(
        f: impl Fn(&Comando) -> Resultado<SaidaProcesso> + Send + Sync + 'static,
    ) -> Self {
        Self {
            chamadas: Mutex::new(Vec::new()),
            fila: Mutex::new(VecDeque::new()),
            funcao: Some(Box::new(f)),
        }
    }

    /// Argumentos de todas as chamadas feitas.
    #[must_use]
    pub fn chamadas(&self) -> Vec<Vec<String>> {
        self.chamadas
            .lock()
            .map(|c| c.iter().map(|x| x.args.clone()).collect())
            .unwrap_or_default()
    }
}

impl ExecutorCalibre for ExecutorFalso {
    fn executar(&self, cmd: &Comando) -> Resultado<SaidaProcesso> {
        if let Ok(mut c) = self.chamadas.lock() {
            c.push(cmd.clone());
        }
        if let Some(f) = &self.funcao {
            return f(cmd);
        }
        self.fila
            .lock()
            .ok()
            .and_then(|mut f| f.pop_front())
            .unwrap_or_else(|| Ok(SaidaProcesso::ok("")))
    }
}

impl<T: ExecutorCalibre + ?Sized> ExecutorCalibre for std::sync::Arc<T> {
    fn executar(&self, cmd: &Comando) -> Resultado<SaidaProcesso> {
        (**self).executar(cmd)
    }
}
