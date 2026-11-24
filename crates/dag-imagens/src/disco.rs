//! Abstração de disco lógico, visões (partições) e fluxos de conteúdo.

use crate::erro::{Result, corrompido};
use std::cell::RefCell;
use std::io::{self, Read, Seek, SeekFrom};
use std::rc::Rc;

/// Limite de alocação para leituras de metadados em memória.
pub(crate) const MAX_METADADO: u64 = 256 * 1024 * 1024;
/// Limite de trechos (runs) por arquivo.
pub(crate) const MAX_RUNS: usize = 4_000_000;

/// Disco lógico de leitura aleatória (somente leitura).
pub(crate) trait Disco {
    fn tamanho(&self) -> u64;
    /// Lê exatamente `buf.len()` bytes em `pos`; fora do disco é erro.
    fn ler(&mut self, pos: u64, buf: &mut [u8]) -> Result<()>;
}

/// Disco bruto sobre um fluxo `Read + Seek`.
pub(crate) struct Bruto<R> {
    r: R,
    len: u64,
}

impl<R: Read + Seek> Bruto<R> {
    pub(crate) fn novo(mut r: R) -> Result<Self> {
        let len = r.seek(SeekFrom::End(0))?;
        Ok(Self { r, len })
    }
}

impl<R: Read + Seek> Disco for Bruto<R> {
    fn tamanho(&self) -> u64 {
        self.len
    }
    fn ler(&mut self, pos: u64, buf: &mut [u8]) -> Result<()> {
        let fim = pos.checked_add(buf.len() as u64);
        if fim.is_none_or(|f| f > self.len) {
            return corrompido("leitura além do fim da imagem");
        }
        self.r.seek(SeekFrom::Start(pos))?;
        self.r.read_exact(buf)?;
        Ok(())
    }
}

pub(crate) type Dev = Rc<RefCell<Box<dyn Disco>>>;

/// Janela `[base, base+len)` sobre um disco.
#[derive(Clone)]
pub(crate) struct Vista {
    dev: Dev,
    base: u64,
    len: u64,
}

impl std::fmt::Debug for Vista {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Vista({}+{})", self.base, self.len)
    }
}

impl Vista {
    pub(crate) fn nova(d: Box<dyn Disco>) -> Self {
        let len = d.tamanho();
        Self {
            dev: Rc::new(RefCell::new(d)),
            base: 0,
            len,
        }
    }
    pub(crate) fn len(&self) -> u64 {
        self.len
    }
    pub(crate) fn sub(&self, off: u64, len: u64) -> Result<Vista> {
        let fim = off.checked_add(len);
        if fim.is_none_or(|f| f > self.len) {
            return corrompido("região fora da imagem");
        }
        Ok(Vista {
            dev: self.dev.clone(),
            base: self.base + off,
            len,
        })
    }
    pub(crate) fn ler(&self, pos: u64, buf: &mut [u8]) -> Result<()> {
        let fim = pos.checked_add(buf.len() as u64);
        if fim.is_none_or(|f| f > self.len) {
            return corrompido("leitura fora da região");
        }
        self.dev.borrow_mut().ler(self.base + pos, buf)
    }
    pub(crate) fn ler_vec(&self, pos: u64, n: u64) -> Result<Vec<u8>> {
        if n > MAX_METADADO {
            return corrompido("estrutura de metadados grande demais");
        }
        let mut v = vec![0u8; n as usize];
        self.ler(pos, &mut v)?;
        Ok(v)
    }
    /// Leitura tolerante ao fim: devolve o que existe (zeros além do fim).
    pub(crate) fn ler_ate(&self, pos: u64, n: usize) -> Result<Vec<u8>> {
        let disp = self.len.saturating_sub(pos).min(n as u64) as usize;
        let mut v = vec![0u8; n];
        if disp > 0 {
            self.ler(pos, &mut v[..disp])?;
        }
        Ok(v)
    }
}

/// Trecho contíguo de um arquivo: `desloc` na vista, ou `None` para zeros (esparso).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Run {
    pub desloc: Option<u64>,
    pub len: u64,
}

/// Construtor de listas de runs que funde trechos adjacentes.
#[derive(Default)]
pub(crate) struct Runs {
    pub v: Vec<Run>,
}

impl Runs {
    pub(crate) fn push(&mut self, desloc: Option<u64>, len: u64) -> Result<()> {
        if len == 0 {
            return Ok(());
        }
        if let Some(u) = self.v.last_mut() {
            match (u.desloc, desloc) {
                (None, None) => {
                    u.len = u.len.saturating_add(len);
                    return Ok(());
                }
                (Some(a), Some(b)) if a.checked_add(u.len) == Some(b) => {
                    u.len = u.len.saturating_add(len);
                    return Ok(());
                }
                _ => {}
            }
        }
        if self.v.len() >= MAX_RUNS {
            return corrompido("arquivo fragmentado demais");
        }
        self.v.push(Run { desloc, len });
        Ok(())
    }
    pub(crate) fn total(&self) -> u64 {
        self.v.iter().fold(0u64, |a, r| a.saturating_add(r.len))
    }
}

/// Fluxo de leitura do conteúdo de um arquivo, montado a partir de runs.
pub(crate) struct Corrente {
    v: Vista,
    runs: Vec<Run>,
    idx: usize,
    dentro: u64,
    restante: u64,
}

impl Corrente {
    pub(crate) fn novo(v: Vista, runs: Vec<Run>, tamanho: u64) -> Self {
        Self {
            v,
            runs,
            idx: 0,
            dentro: 0,
            restante: tamanho,
        }
    }
}

impl Read for Corrente {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.restante == 0 || buf.is_empty() {
            return Ok(0);
        }
        loop {
            let Some(r) = self.runs.get(self.idx) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "dados do arquivo ausentes na imagem",
                ));
            };
            if self.dentro >= r.len {
                self.idx += 1;
                self.dentro = 0;
                continue;
            }
            let n = (buf.len() as u64)
                .min(r.len - self.dentro)
                .min(self.restante) as usize;
            match r.desloc {
                Some(d) => {
                    let pos = d.checked_add(self.dentro).ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "deslocamento inválido")
                    })?;
                    self.v.ler(pos, &mut buf[..n]).map_err(io::Error::from)?;
                }
                None => buf[..n].fill(0),
            }
            self.dentro += n as u64;
            self.restante -= n as u64;
            return Ok(n);
        }
    }
}

/// Fluxo sobre bytes em memória (dados residentes/inline).
pub(crate) struct Memoria(pub io::Cursor<Vec<u8>>);

impl Read for Memoria {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

/// Lê todo o conteúdo descrito por `runs` (metadados em memória, com limite).
pub(crate) fn ler_runs(v: &Vista, runs: Runs) -> Result<Vec<u8>> {
    let total = runs.total();
    if total > MAX_DIR {
        return corrompido("estrutura de diretório grande demais");
    }
    let mut c = Corrente::novo(v.clone(), runs.v, total);
    let mut out = Vec::with_capacity(total as usize);
    c.read_to_end(&mut out)?;
    Ok(out)
}

/// Limite de diretório lido em memória.
pub(crate) const MAX_DIR: u64 = 64 * 1024 * 1024;
