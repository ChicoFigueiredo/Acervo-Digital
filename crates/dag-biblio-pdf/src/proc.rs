//! Execução de ferramentas externas: sem shell, sem arquivos temporários, com timeout (o filho é
//! morto), limite de stdout e entrada por memória.
//!
//! Entrada em memória: os bytes vão para um `memfd` (arquivo anônimo na RAM, nunca no disco, ADR 0006)
//! ligado ao stdin do filho. Ferramentas que pedem um "arquivo" (poppler, djvulibre) recebem
//! `/dev/stdin`, que reabre o memfd do zero e é pesquisável (seekable), coisa que um pipe não é.
//! O poppler aceita `-` para stdin, mas o djvulibre não; `/dev/stdin` serve aos dois.

use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::fs::{MemfdFlags, memfd_create};

/// O que vai para o stdin do filho.
#[derive(Debug)]
pub enum Entrada<'a> {
    Nenhuma,
    /// Arquivo anônimo já criado (reaproveitado entre chamadas).
    Memoria(&'a File),
    /// Bytes avulsos (criam um memfd novo nesta chamada).
    Bytes(&'a [u8]),
}

/// Resultado de uma execução que terminou por conta própria.
#[derive(Debug)]
pub struct Saida {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub sucesso: bool,
    pub codigo: Option<i32>,
    pub duracao: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum ErroProc {
    #[error("não foi possível iniciar {0}: {1}")]
    Inicio(String, String),
    #[error("tempo esgotado ({0:?}); o processo foi encerrado")]
    Timeout(Duration),
    #[error("a saída passou de {0} bytes; o processo foi encerrado")]
    SaidaExcedida(usize),
    #[error("erro de E/S: {0}")]
    Io(String),
}

/// Cria um arquivo anônimo na memória com o conteúdo dado, posicionado no início.
pub fn criar_memfd(nome: &str, dados: &[u8]) -> std::io::Result<File> {
    let fd = memfd_create(nome, MemfdFlags::CLOEXEC)?;
    let mut arquivo = File::from(fd);
    arquivo.write_all(dados)?;
    arquivo.rewind()?;
    Ok(arquivo)
}

fn ler_limitado<R: Read + Send + 'static>(
    mut leitor: R,
    limite: usize,
    excedeu: Arc<AtomicBool>,
    drenar: bool,
) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut pedaco = [0u8; 65_536];
        loop {
            match leitor.read(&mut pedaco) {
                Ok(0) => break,
                Ok(n) => {
                    if buf.len() + n > limite {
                        let cabe = limite.saturating_sub(buf.len());
                        buf.extend_from_slice(&pedaco[..cabe]);
                        if drenar {
                            continue;
                        }
                        excedeu.store(true, Ordering::SeqCst);
                        break;
                    }
                    buf.extend_from_slice(&pedaco[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        buf
    })
}

/// Executa `programa` com `args`. Mata o filho ao estourar `timeout` ou `limite_saida`.
pub fn executar(
    programa: &Path,
    args: &[OsString],
    entrada: Entrada<'_>,
    env: &[(&str, &str)],
    timeout: Duration,
    limite_saida: usize,
) -> Result<Saida, ErroProc> {
    let nome = programa.display().to_string();
    let stdin = match entrada {
        Entrada::Nenhuma => Stdio::null(),
        Entrada::Memoria(f) => Stdio::from(f.try_clone().map_err(|e| ErroProc::Io(e.to_string()))?),
        Entrada::Bytes(b) => {
            Stdio::from(criar_memfd("dag-entrada", b).map_err(|e| ErroProc::Io(e.to_string()))?)
        }
    };
    let mut cmd = Command::new(programa);
    cmd.args(args)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Grupo de processos próprio, para poder matar a árvore inteira no timeout.
        .process_group(0);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let inicio = Instant::now();
    let mut filho = cmd
        .spawn()
        .map_err(|e| ErroProc::Inicio(nome.clone(), e.to_string()))?;
    let excedeu = Arc::new(AtomicBool::new(false));
    let t_out = filho
        .stdout
        .take()
        .map(|s| ler_limitado(s, limite_saida, Arc::clone(&excedeu), false));
    let t_err = filho
        .stderr
        .take()
        .map(|s| ler_limitado(s, 64 * 1024, Arc::new(AtomicBool::new(false)), true));

    let resultado = loop {
        match filho.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(e) => break Err(ErroProc::Io(e.to_string())),
        }
        if excedeu.load(Ordering::SeqCst) {
            break Err(ErroProc::SaidaExcedida(limite_saida));
        }
        if inicio.elapsed() >= timeout {
            break Err(ErroProc::Timeout(timeout));
        }
        thread::sleep(Duration::from_millis(3));
    };
    if resultado.is_err() {
        // Mata o grupo inteiro: um filho que lançou netos não pode segurar os pipes.
        if let Some(pgid) = i32::try_from(filho.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL);
        }
        let _ = filho.kill();
        let _ = filho.wait();
    }
    let stdout = t_out.and_then(|t| t.join().ok()).unwrap_or_default();
    let stderr = t_err.and_then(|t| t.join().ok()).unwrap_or_default();
    let status = resultado?;
    if excedeu.load(Ordering::SeqCst) {
        return Err(ErroProc::SaidaExcedida(limite_saida));
    }
    Ok(Saida {
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        sucesso: status.success(),
        codigo: status.code(),
        duracao: inicio.elapsed(),
    })
}

#[cfg(test)]
mod testes {
    use super::*;

    fn arg(s: &str) -> OsString {
        OsString::from(s)
    }

    #[test]
    fn captura_saida_e_stdin_em_memoria() {
        let s = executar(
            Path::new("/bin/cat"),
            &[],
            Entrada::Bytes(b"ola memfd"),
            &[],
            Duration::from_secs(5),
            1024,
        )
        .unwrap();
        assert!(s.sucesso);
        assert_eq!(s.stdout, b"ola memfd");
    }

    #[test]
    fn timeout_mata_o_filho() {
        let t = Instant::now();
        let e = executar(
            Path::new("/bin/sleep"),
            &[arg("30")],
            Entrada::Nenhuma,
            &[],
            Duration::from_millis(200),
            1024,
        )
        .unwrap_err();
        assert!(matches!(e, ErroProc::Timeout(_)), "{e}");
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn saida_acima_do_limite_mata_o_filho() {
        let e = executar(
            Path::new("/usr/bin/yes"),
            &[],
            Entrada::Nenhuma,
            &[],
            Duration::from_secs(10),
            10_000,
        )
        .unwrap_err();
        assert!(matches!(e, ErroProc::SaidaExcedida(_)), "{e}");
    }

    #[test]
    fn programa_inexistente_e_erro_claro() {
        let e = executar(
            Path::new("/nao/existe"),
            &[],
            Entrada::Nenhuma,
            &[],
            Duration::from_secs(1),
            10,
        )
        .unwrap_err();
        assert!(matches!(e, ErroProc::Inicio(..)));
    }
}
