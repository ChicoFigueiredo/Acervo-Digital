//! Execução de programas externos (`systemctl`, `loginctl`, `pkexec`) atrás de um trait, para
//! que os testes usem um falso. Sempre vetor de argumentos fixo: nunca passa por shell.

use std::io::{self, Read, Write};
use std::process::{Command, Stdio};

/// Limite do que se lê da saída de um programa (proteção de memória).
pub const MAX_SAIDA: u64 = 512 * 1024 * 1024;

/// Resultado de um programa que terminou.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SaidaProcesso {
    /// `None` se morreu por sinal.
    pub codigo: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl SaidaProcesso {
    #[must_use]
    pub fn sucesso(&self) -> bool {
        self.codigo == Some(0)
    }

    #[must_use]
    pub fn stdout_texto(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    #[must_use]
    pub fn stderr_texto(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }
}

/// Executa `programa` com `args` (e `stdin` opcional) e espera terminar.
///
/// # Errors
/// [`io::Error`] se o programa não pôde ser iniciado (`NotFound` quando não existe).
pub trait ExecutorSistema: Send + Sync {
    fn executar(
        &self,
        programa: &str,
        args: &[&str],
        stdin: Option<&[u8]>,
    ) -> io::Result<SaidaProcesso>;
}

/// Implementação real, com `std::process::Command`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ExecutorReal;

impl ExecutorSistema for ExecutorReal {
    fn executar(
        &self,
        programa: &str,
        args: &[&str],
        stdin: Option<&[u8]>,
    ) -> io::Result<SaidaProcesso> {
        let mut cmd = Command::new(programa);
        cmd.args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut filho = cmd.spawn()?;
        let mut pipe_in = filho.stdin.take();
        let mut pipe_out = filho.stdout.take();
        let mut pipe_err = filho.stderr.take();
        let dados = stdin.map(<[u8]>::to_vec);
        let (stdout, stderr) = std::thread::scope(|s| {
            if let (Some(mut p), Some(d)) = (pipe_in.take(), dados) {
                s.spawn(move || {
                    let _ = p.write_all(&d);
                });
            }
            let err = s.spawn(move || {
                let mut v = Vec::new();
                if let Some(p) = pipe_err.as_mut() {
                    let _ = p.take(MAX_SAIDA).read_to_end(&mut v);
                }
                v
            });
            let mut v = Vec::new();
            if let Some(p) = pipe_out.as_mut() {
                let _ = p.take(MAX_SAIDA).read_to_end(&mut v);
            }
            (v, err.join().unwrap_or_default())
        });
        let status = filho.wait()?;
        Ok(SaidaProcesso {
            codigo: status.code(),
            stdout,
            stderr,
        })
    }
}
