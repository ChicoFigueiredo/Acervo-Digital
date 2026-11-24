//! Visita de um diretório (lado worker). Só lê nomes e metadados: `read_dir` e `lstat` por
//! entrada, mais `readlink` nos symlinks. Nunca abre o conteúdo de arquivos, nunca segue links e
//! nunca escreve no filesystem varrido.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;
use std::sync::mpsc::SyncSender;
use std::time::UNIX_EPOCH;

use dag_store::inventario::{Severidade, TipoEntrada};

use crate::classificador::{Classificador, TipoDetectado};
use crate::config::Exclusao;
use crate::controle::ControleScan;
use crate::elevado::{EntradaElevada, LeitorElevado, TipoElevado};

/// Diretório a visitar.
#[derive(Debug)]
pub(crate) struct Job {
    pub fila_id: i64,
    pub caminho: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Natureza {
    Pasta,
    Outra(TipoEntrada),
}

/// Metadados de uma entrada, já em tipos do Catálogo.
#[derive(Debug)]
pub(crate) struct Entrada {
    pub caminho: Vec<u8>,
    pub off_nome: usize,
    pub natureza: Natureza,
    pub dev: u64,
    pub inode: u64,
    pub nlink: u64,
    pub tamanho: u64,
    pub alocado: Option<u64>,
    pub mtime_ns: Option<i64>,
    pub ctime_ns: Option<i64>,
    pub birth_ns: Option<i64>,
    pub attrs: String,
    pub alvo: Option<Vec<u8>>,
    pub tipo: Option<TipoDetectado>,
}

impl Entrada {
    pub(crate) fn nome(&self) -> &[u8] {
        &self.caminho[self.off_nome..]
    }
}

#[derive(Debug)]
pub(crate) struct Ignorada {
    pub caminho: Vec<u8>,
    pub motivo: String,
}

#[derive(Debug)]
pub(crate) struct Problema {
    pub caminho: Vec<u8>,
    pub codigo: &'static str,
    pub motivo: String,
    pub severidade: Severidade,
}

#[derive(Debug)]
pub(crate) enum Fim {
    Ok,
    Falha(Problema),
    /// Cancelado no meio: o diretório volta para a fila.
    Abortado,
}

#[derive(Debug)]
pub(crate) enum Msg {
    Lote {
        fila_id: i64,
        entradas: Vec<Entrada>,
        ignoradas: Vec<Ignorada>,
        problemas: Vec<Problema>,
    },
    Fim {
        fila_id: i64,
        fim: Fim,
    },
}

impl Msg {
    pub(crate) fn linhas(&self) -> usize {
        match self {
            Self::Lote {
                entradas,
                ignoradas,
                problemas,
                ..
            } => entradas.len() + ignoradas.len() + problemas.len(),
            Self::Fim { .. } => 1,
        }
    }
}

/// Mapeia um erro de E/S para (código estável, severidade).
pub(crate) fn classificar_io(e: &io::Error) -> (&'static str, Severidade) {
    match e.kind() {
        io::ErrorKind::PermissionDenied => ("permissao_negada", Severidade::Aviso),
        io::ErrorKind::NotFound => ("nao_encontrado", Severidade::Aviso),
        io::ErrorKind::NotADirectory => ("nao_e_diretorio", Severidade::Aviso),
        _ => ("erro_es", Severidade::Erro),
    }
}

fn ns_desde_epoca(s: i64, ns: i64) -> Option<i64> {
    s.checked_mul(1_000_000_000)?.checked_add(ns)
}

fn birth_ns(m: &fs::Metadata) -> Option<i64> {
    let t = m.created().ok()?;
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).ok(),
        Err(e) => i64::try_from(e.duration().as_nanos()).ok().map(|n| -n),
    }
}

fn natureza_de(m: &fs::Metadata) -> Natureza {
    let t = m.file_type();
    if t.is_dir() {
        Natureza::Pasta
    } else if t.is_file() {
        Natureza::Outra(TipoEntrada::Arquivo)
    } else if t.is_symlink() {
        Natureza::Outra(TipoEntrada::Symlink)
    } else if t.is_fifo() {
        Natureza::Outra(TipoEntrada::Fifo)
    } else if t.is_socket() {
        Natureza::Outra(TipoEntrada::Socket)
    } else if t.is_block_device() {
        Natureza::Outra(TipoEntrada::DispositivoBloco)
    } else if t.is_char_device() {
        Natureza::Outra(TipoEntrada::DispositivoChar)
    } else {
        Natureza::Outra(TipoEntrada::Outro)
    }
}

/// Monta a [`Entrada`] a partir de metadados `lstat`.
pub(crate) fn entrada_de(caminho: Vec<u8>, off_nome: usize, m: &fs::Metadata) -> Entrada {
    let natureza = natureza_de(m);
    let alocado = m.blocks().checked_mul(512);
    Entrada {
        caminho,
        off_nome,
        natureza,
        dev: m.dev(),
        inode: m.ino(),
        nlink: m.nlink(),
        tamanho: m.size(),
        alocado,
        mtime_ns: ns_desde_epoca(m.mtime(), m.mtime_nsec()),
        ctime_ns: ns_desde_epoca(m.ctime(), m.ctime_nsec()),
        birth_ns: birth_ns(m),
        attrs: format!(
            "{{\"mode\":{},\"uid\":{},\"gid\":{}}}",
            m.mode(),
            m.uid(),
            m.gid()
        ),
        alvo: None,
        tipo: None,
    }
}

pub(crate) struct Contexto<'a> {
    pub exclusoes: &'a [Exclusao],
    pub classificador: &'a dyn Classificador,
    pub controle: &'a ControleScan,
    pub entradas_por_mensagem: usize,
    /// Leitor com privilégio (opt-in) para pastas que dão `EACCES`.
    pub elevado: Option<&'a dyn LeitorElevado>,
}

fn juntar(dir: &[u8], nome: &[u8]) -> (Vec<u8>, usize) {
    let mut c = Vec::with_capacity(dir.len() + 1 + nome.len());
    c.extend_from_slice(dir);
    if dir.last() != Some(&b'/') {
        c.push(b'/');
    }
    let off = c.len();
    c.extend_from_slice(nome);
    (c, off)
}

/// Lê um diretório e envia lotes pelo canal. Devolve `false` se o escritor já saiu.
pub(crate) fn visitar(job: &Job, ctx: &Contexto<'_>, saida: &SyncSender<Msg>) -> bool {
    let dir = Path::new(OsStr::from_bytes(&job.caminho));
    let leitura = match fs::read_dir(dir) {
        Ok(l) => l,
        Err(e) => {
            let (codigo, severidade) = classificar_io(&e);
            let mut motivo = format!("não foi possível listar o diretório: {e}");
            if e.kind() == io::ErrorKind::PermissionDenied
                && let Some(leitor) = ctx.elevado
            {
                match leitor.listar(&job.caminho) {
                    Ok(filhos) => return visitar_elevado(job, ctx, saida, &e, filhos),
                    Err(e2) => motivo.push_str(&format!("; leitura elevada falhou: {e2}")),
                }
            }
            let fim = Fim::Falha(Problema {
                caminho: job.caminho.clone(),
                codigo,
                motivo,
                severidade,
            });
            return saida
                .send(Msg::Fim {
                    fila_id: job.fila_id,
                    fim,
                })
                .is_ok();
        }
    };
    let mut entradas = Vec::new();
    let mut ignoradas = Vec::new();
    let mut problemas = Vec::new();
    macro_rules! enviar {
        () => {
            if !(entradas.is_empty() && ignoradas.is_empty() && problemas.is_empty()) {
                let msg = Msg::Lote {
                    fila_id: job.fila_id,
                    entradas: std::mem::take(&mut entradas),
                    ignoradas: std::mem::take(&mut ignoradas),
                    problemas: std::mem::take(&mut problemas),
                };
                if saida.send(msg).is_err() {
                    return false;
                }
            }
        };
    }
    for item in leitura {
        if ctx.controle.cancelado() {
            enviar!();
            return saida
                .send(Msg::Fim {
                    fila_id: job.fila_id,
                    fim: Fim::Abortado,
                })
                .is_ok();
        }
        let item = match item {
            Ok(i) => i,
            Err(e) => {
                let (codigo, severidade) = classificar_io(&e);
                problemas.push(Problema {
                    caminho: job.caminho.clone(),
                    codigo,
                    motivo: format!("falha ao ler uma entrada do diretório: {e}"),
                    severidade,
                });
                continue;
            }
        };
        let nome = item.file_name();
        let (caminho, off) = juntar(&job.caminho, nome.as_bytes());
        if let Some(ex) = ctx
            .exclusoes
            .iter()
            .find(|x| x.casa(&caminho, &caminho[off..]))
        {
            ignoradas.push(Ignorada {
                caminho,
                motivo: ex.motivo.clone(),
            });
        } else {
            // DirEntry::metadata não segue symlink (equivale a lstat).
            match item.metadata() {
                Ok(m) => {
                    let mut entrada = entrada_de(caminho, off, &m);
                    match entrada.natureza {
                        Natureza::Outra(TipoEntrada::Symlink) => {
                            match fs::read_link(OsStr::from_bytes(&entrada.caminho)) {
                                Ok(alvo) => {
                                    entrada.alvo = Some(alvo.as_os_str().as_bytes().to_vec())
                                }
                                Err(e) => {
                                    let (codigo, severidade) = classificar_io(&e);
                                    problemas.push(Problema {
                                        caminho: entrada.caminho.clone(),
                                        codigo,
                                        motivo: format!(
                                            "não foi possível ler o alvo do symlink: {e}"
                                        ),
                                        severidade,
                                    });
                                }
                            }
                        }
                        Natureza::Outra(TipoEntrada::Arquivo) => {
                            entrada.tipo = ctx.classificador.tipo_do_arquivo(entrada.nome());
                        }
                        _ => {}
                    }
                    entradas.push(entrada);
                }
                Err(e) => {
                    let (codigo, severidade) = classificar_io(&e);
                    problemas.push(Problema {
                        caminho,
                        codigo,
                        motivo: format!("não foi possível ler os metadados: {e}"),
                        severidade,
                    });
                }
            }
        }
        if entradas.len() + ignoradas.len() + problemas.len() >= ctx.entradas_por_mensagem {
            enviar!();
        }
    }
    enviar!();
    saida
        .send(Msg::Fim {
            fila_id: job.fila_id,
            fim: Fim::Ok,
        })
        .is_ok()
}

fn natureza_elevada(t: TipoElevado) -> Natureza {
    match t {
        TipoElevado::Pasta => Natureza::Pasta,
        TipoElevado::Arquivo => Natureza::Outra(TipoEntrada::Arquivo),
        TipoElevado::Symlink => Natureza::Outra(TipoEntrada::Symlink),
        TipoElevado::Fifo => Natureza::Outra(TipoEntrada::Fifo),
        TipoElevado::Socket => Natureza::Outra(TipoEntrada::Socket),
        TipoElevado::DispositivoBloco => Natureza::Outra(TipoEntrada::DispositivoBloco),
        TipoElevado::DispositivoChar => Natureza::Outra(TipoEntrada::DispositivoChar),
        TipoElevado::Outro => Natureza::Outra(TipoEntrada::Outro),
    }
}

fn entrada_elevada(dir: &[u8], f: EntradaElevada, ctx: &Contexto<'_>) -> Entrada {
    let (caminho, off) = juntar(dir, &f.nome);
    let natureza = natureza_elevada(f.tipo);
    let mut e = Entrada {
        caminho,
        off_nome: off,
        natureza,
        dev: f.dev,
        inode: f.ino,
        nlink: f.nlink,
        tamanho: f.tamanho,
        alocado: f.blocos.checked_mul(512),
        mtime_ns: Some(f.mtime_ns),
        ctime_ns: Some(f.ctime_ns),
        birth_ns: None,
        attrs: format!(
            "{{\"mode\":{},\"uid\":{},\"gid\":{}}}",
            f.mode, f.uid, f.gid
        ),
        alvo: f.alvo_symlink,
        tipo: None,
    };
    if natureza == Natureza::Outra(TipoEntrada::Arquivo) {
        e.tipo = ctx.classificador.tipo_do_arquivo(e.nome());
    }
    e
}

/// Segue a visita de uma pasta cujo `read_dir` deu `EACCES`, com os metadados lidos pelo
/// helper elevado. O acesso negado continua registrado (aviso), e as exclusões valem igual.
fn visitar_elevado(
    job: &Job,
    ctx: &Contexto<'_>,
    saida: &SyncSender<Msg>,
    negado: &io::Error,
    filhos: Vec<EntradaElevada>,
) -> bool {
    let mut entradas = Vec::new();
    let mut ignoradas = Vec::new();
    for f in filhos {
        let e = entrada_elevada(&job.caminho, f, ctx);
        if let Some(ex) = ctx
            .exclusoes
            .iter()
            .find(|x| x.casa(&e.caminho, &e.caminho[e.off_nome..]))
        {
            ignoradas.push(Ignorada {
                caminho: e.caminho,
                motivo: ex.motivo.clone(),
            });
        } else {
            entradas.push(e);
        }
    }
    let problemas = vec![Problema {
        caminho: job.caminho.clone(),
        codigo: "permissao_negada_elevada",
        motivo: format!(
            "não foi possível listar o diretório: {negado}; metadados lidos pelo helper elevado (conteúdo dos arquivos continua inacessível)"
        ),
        severidade: Severidade::Aviso,
    }];
    let lote = Msg::Lote {
        fila_id: job.fila_id,
        entradas,
        ignoradas,
        problemas,
    };
    saida.send(lote).is_ok()
        && saida
            .send(Msg::Fim {
                fila_id: job.fila_id,
                fim: Fim::Ok,
            })
            .is_ok()
}

/// Metadados `lstat` de um caminho (usado nas raízes).
pub(crate) fn lstat(caminho: &[u8]) -> io::Result<fs::Metadata> {
    fs::symlink_metadata(OsStr::from_bytes(caminho))
}

/// Listagem completa de um diretório, para a reconciliação direcionada do monitor.
#[derive(Debug, Default)]
pub(crate) struct Listagem {
    pub entradas: Vec<Entrada>,
    /// Caminhos cuja leitura de metadados falhou (inclusive por corrida com remoção): o estado
    /// deles é desconhecido e **não** pode ser tomado como remoção.
    pub incertos: Vec<Vec<u8>>,
    pub problemas: Vec<Problema>,
    /// A iteração do diretório terminou sem erro: só então a ausência de um nome é evidência.
    pub completa: bool,
}

/// Lê um diretório inteiro (nomes, `lstat`, alvo de symlink e tipo por extensão). Só leitura.
pub(crate) fn ler_diretorio(
    dir: &[u8],
    classificador: &dyn Classificador,
    exclusoes: &[Exclusao],
) -> io::Result<Listagem> {
    let leitura = fs::read_dir(OsStr::from_bytes(dir))?;
    let mut l = Listagem {
        completa: true,
        ..Listagem::default()
    };
    for item in leitura {
        let item = match item {
            Ok(i) => i,
            Err(e) => {
                let (codigo, severidade) = classificar_io(&e);
                l.completa = false;
                l.problemas.push(Problema {
                    caminho: dir.to_vec(),
                    codigo,
                    motivo: format!("falha ao ler uma entrada do diretório: {e}"),
                    severidade,
                });
                continue;
            }
        };
        let (caminho, off) = juntar(dir, item.file_name().as_bytes());
        if exclusoes.iter().any(|x| x.casa(&caminho, &caminho[off..])) {
            // Excluída de propósito: fora do escopo, nunca vira remoção.
            l.incertos.push(caminho);
            continue;
        }
        match item.metadata() {
            Ok(m) => {
                let mut entrada = entrada_de(caminho, off, &m);
                match entrada.natureza {
                    Natureza::Outra(TipoEntrada::Symlink) => {
                        entrada.alvo = fs::read_link(OsStr::from_bytes(&entrada.caminho))
                            .ok()
                            .map(|a| a.as_os_str().as_bytes().to_vec());
                    }
                    Natureza::Outra(TipoEntrada::Arquivo) => {
                        entrada.tipo = classificador.tipo_do_arquivo(entrada.nome());
                    }
                    _ => {}
                }
                l.entradas.push(entrada);
            }
            Err(e) => {
                let (codigo, severidade) = classificar_io(&e);
                l.problemas.push(Problema {
                    caminho: caminho.clone(),
                    codigo,
                    motivo: format!("não foi possível ler os metadados: {e}"),
                    severidade,
                });
                l.incertos.push(caminho);
            }
        }
    }
    Ok(l)
}
