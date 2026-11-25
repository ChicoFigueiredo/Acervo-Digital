//! Gerenciador opcional do servidor de IA.
//!
//! * `llama-server` (llama.cpp): inicia com `-ngl` para GPU, cai para CPU se a GPU faltar
//!   memória, espera `/health`, detecta queda/OOM e devolve erro sem derrubar o agente (CA-B15).
//! * Ollama: o serviço é do usuário. Só verificamos que responde e que o modelo existe; se não
//!   existir, pedimos `ollama pull <modelo>`. Nunca baixamos modelos sozinhos nem paramos o serviço.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cliente::{ClienteLlm, ConfigCliente, InfoServidor};
use crate::erro::{ErroIa, Resultado, indica_falta_de_memoria};
use crate::modelo::{Backend, PapelModelo, PerfilModelo};

/// Como iniciar o `llama-server`.
#[derive(Debug, Clone)]
pub struct ConfigLlamaServer {
    /// Executável (`llama-server` no PATH ou caminho completo).
    pub binario: PathBuf,
    pub modelo_gguf: PathBuf,
    /// Projetor multimodal (`--mmproj`), se o modelo for multimodal.
    pub mmproj: Option<PathBuf>,
    pub porta: u16,
    pub n_ctx: u32,
    /// Camadas na GPU; 99 = todas; 0 = CPU.
    pub ngl: i32,
    pub threads: Option<u32>,
    /// Argumentos adicionais crus (ex.: `--jinja`, `-fa`).
    pub extras: Vec<String>,
    pub tempo_limite_inicio: Duration,
    /// Tenta de novo só com CPU se a GPU falhar.
    pub fallback_cpu: bool,
}

impl ConfigLlamaServer {
    #[must_use]
    pub fn novo(modelo_gguf: PathBuf, porta: u16) -> Self {
        Self {
            binario: PathBuf::from("llama-server"),
            modelo_gguf,
            mmproj: None,
            porta,
            n_ctx: 8192,
            ngl: 99,
            threads: None,
            extras: Vec::new(),
            tempo_limite_inicio: Duration::from_secs(180),
            fallback_cpu: true,
        }
    }

    /// Argumentos da linha de comando para um dado `-ngl`.
    #[must_use]
    pub fn argumentos(&self, ngl: i32) -> Vec<String> {
        let mut a: Vec<String> = vec![
            "-m".into(),
            self.modelo_gguf.to_string_lossy().into_owned(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            self.porta.to_string(),
            "-c".into(),
            self.n_ctx.to_string(),
            "-ngl".into(),
            ngl.to_string(),
            // Uma fatia só: processamos um livro por vez e o KV cache não se divide em 4.
            "-np".into(),
            "1".into(),
        ];
        if let Some(m) = &self.mmproj {
            a.push("--mmproj".into());
            a.push(m.to_string_lossy().into_owned());
        }
        if let Some(t) = self.threads {
            a.push("-t".into());
            a.push(t.to_string());
        }
        a.extend(self.extras.iter().cloned());
        a
    }
}

/// Onde o modelo está rodando.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModoExecucao {
    Gpu { ngl: i32 },
    Cpu,
}

/// Porta TCP livre em 127.0.0.1 (o llama-server não aceita porta 0).
///
/// # Errors
/// `Io` se não conseguir abrir um socket local.
pub fn porta_livre() -> Resultado<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Processo `llama-server` gerenciado.
#[derive(Debug)]
pub struct ServidorLlama {
    filho: Child,
    cfg: ConfigLlamaServer,
    modo: ModoExecucao,
    cauda_stderr: Arc<Mutex<VecDeque<String>>>,
}

fn cliente_saude(porta: u16) -> Resultado<ClienteLlm> {
    let mut p = PerfilModelo::ollama("saude", false, PapelModelo::Primario);
    p.backend = Backend::OpenAi;
    p.endpoint = format!("http://127.0.0.1:{porta}");
    ClienteLlm::novo(
        p,
        ConfigCliente {
            timeout: Duration::from_secs(2),
            timeout_conexao: Duration::from_secs(1),
            tentativas: 1,
            ..ConfigCliente::default()
        },
    )
}

impl ServidorLlama {
    fn lancar(cfg: &ConfigLlamaServer, ngl: i32) -> Resultado<Self> {
        let mut filho = Command::new(&cfg.binario)
            .args(cfg.argumentos(ngl))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                ErroIa::Servidor(format!(
                    "não foi possível executar `{}`: {e} (llama.cpp instalado? use Ollama como alternativa)",
                    cfg.binario.display()
                ))
            })?;
        let cauda = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(err) = filho.stderr.take() {
            let c = cauda.clone();
            std::thread::spawn(move || {
                for linha in BufReader::new(err).lines().map_while(Result::ok) {
                    if let Ok(mut q) = c.lock() {
                        if q.len() >= 60 {
                            q.pop_front();
                        }
                        q.push_back(linha);
                    }
                }
            });
        }
        Ok(Self {
            filho,
            cfg: cfg.clone(),
            modo: if ngl == 0 {
                ModoExecucao::Cpu
            } else {
                ModoExecucao::Gpu { ngl }
            },
            cauda_stderr: cauda,
        })
    }

    /// Últimas linhas do stderr do processo.
    #[must_use]
    pub fn cauda_stderr(&self) -> String {
        self.cauda_stderr
            .lock()
            .map(|q| q.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }

    fn erro_de_saida(&self, status: &std::process::ExitStatus) -> ErroIa {
        let cauda = self.cauda_stderr();
        // Dá tempo ao leitor de stderr de esvaziar o pipe.
        let cauda = if cauda.is_empty() {
            std::thread::sleep(Duration::from_millis(100));
            self.cauda_stderr()
        } else {
            cauda
        };
        if indica_falta_de_memoria(&cauda) {
            ErroIa::MemoriaInsuficiente(format!(
                "llama-server saiu ({status}); {}",
                resumo_cauda(&cauda)
            ))
        } else {
            ErroIa::Servidor(format!(
                "llama-server saiu ({status}); {}",
                resumo_cauda(&cauda)
            ))
        }
    }

    fn esperar_pronto(&mut self) -> Resultado<()> {
        let cliente = cliente_saude(self.cfg.porta)?;
        let limite = Instant::now() + self.cfg.tempo_limite_inicio;
        loop {
            if let Some(status) = self.filho.try_wait()? {
                return Err(self.erro_de_saida(&status));
            }
            if cliente.saude().is_ok() {
                return Ok(());
            }
            if Instant::now() >= limite {
                let _ = self.filho.kill();
                let _ = self.filho.wait();
                return Err(ErroIa::Servidor(format!(
                    "llama-server não ficou saudável em {:?}; {}",
                    self.cfg.tempo_limite_inicio,
                    resumo_cauda(&self.cauda_stderr())
                )));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Inicia o servidor e espera o `/health`. Com GPU primeiro e, se falhar (OOM, saída
    /// inesperada ou tempo), CPU quando `fallback_cpu`.
    ///
    /// # Errors
    /// `Servidor`/`MemoriaInsuficiente` com o resumo do stderr.
    pub fn iniciar(cfg: &ConfigLlamaServer) -> Resultado<Self> {
        let mut falha_gpu: Option<ErroIa> = None;
        if cfg.ngl != 0 {
            match Self::lancar(cfg, cfg.ngl).and_then(|mut s| s.esperar_pronto().map(|()| s)) {
                Ok(s) => return Ok(s),
                Err(e) if cfg.fallback_cpu => falha_gpu = Some(e),
                Err(e) => return Err(e),
            }
        }
        let mut s = Self::lancar(cfg, 0)?;
        match s.esperar_pronto() {
            Ok(()) => Ok(s),
            Err(e) => Err(match falha_gpu {
                Some(g) => ErroIa::Servidor(format!("GPU falhou ({g}) e CPU também ({e})")),
                None => e,
            }),
        }
    }

    #[must_use]
    pub fn modo(&self) -> ModoExecucao {
        self.modo
    }

    #[must_use]
    pub fn porta(&self) -> u16 {
        self.cfg.porta
    }

    #[must_use]
    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.cfg.porta)
    }

    /// O processo ainda está de pé? Se caiu (inclusive por OOM), devolve o erro com o stderr.
    ///
    /// # Errors
    /// `MemoriaInsuficiente` ou `Servidor` descrevendo a queda.
    pub fn verificar_vivo(&mut self) -> Resultado<()> {
        match self.filho.try_wait()? {
            None => Ok(()),
            Some(status) => Err(self.erro_de_saida(&status)),
        }
    }

    /// `/health` responde?
    ///
    /// # Errors
    /// Falha de transporte/HTTP.
    pub fn saude(&self) -> Resultado<()> {
        cliente_saude(self.cfg.porta)?.saude()
    }

    /// Reinicia só com CPU (depois de uma queda por OOM na GPU).
    ///
    /// # Errors
    /// Se também falhar em CPU.
    pub fn reiniciar_em_cpu(&mut self) -> Resultado<()> {
        self.parar();
        let mut novo = Self::lancar(&self.cfg, 0)?;
        novo.esperar_pronto()?;
        // `Drop` do antigo já não tem processo; troca por inteiro.
        std::mem::swap(self, &mut novo);
        Ok(())
    }

    /// Encerra o processo (idempotente).
    pub fn parar(&mut self) {
        let _ = self.filho.kill();
        let _ = self.filho.wait();
    }
}

impl Drop for ServidorLlama {
    fn drop(&mut self) {
        self.parar();
    }
}

fn resumo_cauda(c: &str) -> String {
    let ultimas: Vec<&str> = c.lines().rev().take(5).collect();
    ultimas.into_iter().rev().collect::<Vec<_>>().join(" | ")
}

/// Serviço Ollama do usuário: só verificação, nunca download nem parada.
#[derive(Debug)]
pub struct ServicoOllama {
    cliente: ClienteLlm,
}

impl ServicoOllama {
    /// Verifica que o serviço responde e que o modelo está instalado.
    ///
    /// # Errors
    /// `Transporte` se o Ollama não estiver rodando; `ModeloAusente` (com a instrução
    /// `ollama pull`) se o modelo não existir.
    pub fn verificar(perfil: &PerfilModelo, cfg: ConfigCliente) -> Resultado<(Self, InfoServidor)> {
        let mut p = perfil.clone();
        p.backend = Backend::Ollama;
        let cliente = ClienteLlm::novo(p, cfg)?;
        cliente.saude().map_err(|e| {
            ErroIa::Servidor(format!(
                "o serviço Ollama não respondeu ({e}); inicie com `ollama serve`"
            ))
        })?;
        let info = cliente.info_servidor()?;
        Ok((Self { cliente }, info))
    }

    /// Ainda responde?
    ///
    /// # Errors
    /// Falha de transporte.
    pub fn verificar_vivo(&self) -> Resultado<()> {
        self.cliente.saude()
    }

    #[must_use]
    pub fn cliente(&self) -> &ClienteLlm {
        &self.cliente
    }
}

/// Backend pronto para uso, qualquer que seja.
#[derive(Debug)]
pub enum Servidor {
    Llama(ServidorLlama),
    Ollama(ServicoOllama),
}

impl Servidor {
    /// Verifica a saúde do backend.
    ///
    /// # Errors
    /// Queda do processo ou serviço fora do ar.
    pub fn verificar_vivo(&mut self) -> Resultado<()> {
        match self {
            Self::Llama(s) => s.verificar_vivo(),
            Self::Ollama(s) => s.verificar_vivo(),
        }
    }

    /// Para o que o DAG iniciou (o serviço Ollama do usuário nunca é parado).
    pub fn parar(&mut self) {
        if let Self::Llama(s) = self {
            s.parar();
        }
    }
}
