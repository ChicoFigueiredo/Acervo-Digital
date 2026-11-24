//! Thread leitora do inotify: mantém os watches recursivos, converte eventos em dicas de
//! diretório sujo e detecta perda de eventos. Só chama `inotify_add_watch`, `read_dir` e
//! `lstat` no acervo; nunca escreve nele.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use dag_scanner::{Exclusao, RaizMonitorada};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::OwnedFd;
use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
use rustix::io::Errno;

use crate::fila::{Fila, Lacuna, TipoLacuna, agora};

/// `fs.inotify.max_user_watches` do sistema (8192 se ilegível).
pub(crate) fn limite_do_sistema() -> usize {
    std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(8192)
}

fn juntar(dir: &[u8], nome: &[u8]) -> Vec<u8> {
    let mut c = Vec::with_capacity(dir.len() + 1 + nome.len());
    c.extend_from_slice(dir);
    if dir.last() != Some(&b'/') {
        c.push(b'/');
    }
    c.extend_from_slice(nome);
    c
}

fn pai(caminho: &[u8]) -> Option<&[u8]> {
    match caminho.iter().rposition(|b| *b == b'/') {
        Some(0) if caminho.len() > 1 => Some(&caminho[..1]),
        Some(i) if i > 0 => Some(&caminho[..i]),
        _ => None,
    }
}

fn excluida(ex: &[Exclusao], caminho: &[u8], nome: &[u8]) -> bool {
    ex.iter().any(|x| x.casa(caminho, nome))
}

pub(crate) struct Observador {
    fila: Arc<Fila>,
    raizes: Vec<RaizMonitorada>,
    exclusoes: Vec<Exclusao>,
    teto: usize,
    fd: OwnedFd,
    wds: HashMap<i32, (Vec<u8>, usize)>,
    por_caminho: BTreeMap<Vec<u8>, i32>,
}

fn mascara() -> WatchFlags {
    WatchFlags::CREATE
        | WatchFlags::DELETE
        | WatchFlags::MODIFY
        | WatchFlags::ATTRIB
        | WatchFlags::CLOSE_WRITE
        | WatchFlags::MOVED_FROM
        | WatchFlags::MOVED_TO
        | WatchFlags::DELETE_SELF
        | WatchFlags::MOVE_SELF
        | WatchFlags::ONLYDIR
        | WatchFlags::DONT_FOLLOW
        | WatchFlags::EXCL_UNLINK
}

/// Corpo da thread: arma os watches, avisa `pronto` e lê eventos até a fila pedir o fim.
pub(crate) fn rodar(
    fila: Arc<Fila>,
    raizes: Vec<RaizMonitorada>,
    exclusoes: Vec<Exclusao>,
    teto: usize,
    pronto: &Sender<Result<(), String>>,
) {
    let fd = match inotify::init(CreateFlags::NONBLOCK | CreateFlags::CLOEXEC) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = pronto.send(Err(format!("inotify_init1: {e}")));
            return;
        }
    };
    let mut o = Observador {
        fila,
        raizes,
        exclusoes,
        teto,
        fd,
        wds: HashMap::new(),
        por_caminho: BTreeMap::new(),
    };
    for i in 0..o.raizes.len() {
        let caminho = o.raizes[i].caminho.clone();
        o.adicionar_arvore(i, caminho);
    }
    o.publicar();
    let _ = pronto.send(Ok(()));
    o.laco();
}

impl Observador {
    fn publicar(&self) {
        let mut e = self.fila.travar();
        e.watches = self.wds.len();
        e.watches_limite = self.teto;
    }

    fn limite_atingido(&self, raiz: usize, onde: &[u8]) {
        let mut e = self.fila.travar();
        if !e.parciais.insert(raiz) {
            return;
        }
        let detalhe = format!(
            "limite de {} watches atingido em {}: o restante da raiz é coberto só pela reconciliação periódica",
            self.teto,
            dag_store::inventario::exibir(onde)
        );
        e.lacunas.push(Lacuna {
            tipo: TipoLacuna::LimiteDeWatches,
            raiz: Some(raiz),
            detalhe,
            observado_em: agora(),
        });
        drop(e);
        self.fila.acordar.notify_all();
    }

    /// Arma watches em `inicio` e em todos os diretórios abaixo dele (sem seguir symlinks, sem
    /// cruzar para outro filesystem).
    fn adicionar_arvore(&mut self, raiz: usize, inicio: Vec<u8>) {
        let dev = self.raizes[raiz].dev;
        let mut pilha = vec![inicio];
        while let Some(dir) = pilha.pop() {
            if self.por_caminho.contains_key(&dir) {
                continue;
            }
            if self.wds.len() >= self.teto {
                self.limite_atingido(raiz, &dir);
                return;
            }
            match inotify::add_watch(&self.fd, OsStr::from_bytes(&dir), mascara()) {
                Ok(wd) => {
                    // O mesmo inode por dois caminhos (bind mount) devolve o mesmo wd: o
                    // último vence; o outro caminho fica coberto pela reconciliação.
                    self.wds.insert(wd, (dir.clone(), raiz));
                    self.por_caminho.insert(dir.clone(), wd);
                }
                Err(Errno::NOSPC) => {
                    self.limite_atingido(raiz, &dir);
                    return;
                }
                Err(Errno::NOENT | Errno::NOTDIR) => continue,
                Err(e) => {
                    let mut est = self.fila.travar();
                    est.erro = Some(format!(
                        "sem watch em {}: {e}",
                        dag_store::inventario::exibir(&dir)
                    ));
                    est.parciais.insert(raiz);
                    continue;
                }
            }
            let Ok(leitura) = std::fs::read_dir(OsStr::from_bytes(&dir)) else {
                continue;
            };
            for item in leitura.flatten() {
                let Ok(m) = item.metadata() else { continue };
                if !m.is_dir() || m.dev() != dev {
                    continue;
                }
                let nome = item.file_name();
                let caminho = juntar(&dir, nome.as_bytes());
                if excluida(&self.exclusoes, &caminho, nome.as_bytes()) {
                    continue;
                }
                pilha.push(caminho);
            }
        }
    }

    /// Remove os watches de `caminho` e de tudo abaixo dele.
    fn remover_arvore(&mut self, caminho: &[u8]) {
        let mut prefixo = caminho.to_vec();
        prefixo.push(b'/');
        let alvo: Vec<Vec<u8>> = self
            .por_caminho
            .range(caminho.to_vec()..)
            .take_while(|(c, _)| c.as_slice() == caminho || c.starts_with(&prefixo))
            .map(|(c, _)| c.clone())
            .collect();
        for c in alvo {
            if let Some(wd) = self.por_caminho.remove(&c) {
                self.wds.remove(&wd);
                let _ = inotify::remove_watch(&self.fd, wd);
            }
        }
    }

    fn laco(&mut self) {
        let mut buf = [MaybeUninit::<u8>::uninit(); 32 * 1024];
        loop {
            if self.fila.encerrando() {
                return;
            }
            self.rearmar();
            let mut fds = [PollFd::new(&self.fd, PollFlags::IN)];
            let espera = Timespec {
                tv_sec: 0,
                tv_nsec: 100_000_000,
            };
            match poll(&mut fds, Some(&espera)) {
                Ok(0) | Err(Errno::INTR) => continue,
                Ok(_) => {}
                Err(e) => {
                    self.fila.travar().erro = Some(format!("poll do inotify: {e}"));
                    return;
                }
            }
            self.drenar(&mut buf);
            self.publicar();
        }
    }

    /// Refaz os watches das raízes que voltaram (pedido do escritor).
    fn rearmar(&mut self) {
        let raizes: Vec<usize> = std::mem::take(&mut self.fila.travar().rearmar);
        for r in raizes {
            let caminho = self.raizes[r].caminho.clone();
            self.remover_arvore(&caminho);
            self.fila.travar().parciais.remove(&r);
            self.adicionar_arvore(r, caminho.clone());
            // Entre o Fast Scan que pediu o rearme e o watch novo, mudanças não geram evento:
            // registrar a lacuna faz o escritor reconciliar a raiz de novo, já com o watch ativo.
            self.fila.lacuna(Lacuna {
                tipo: TipoLacuna::WatchesRefeitos,
                raiz: Some(r),
                detalhe: format!(
                    "watches refeitos em {}",
                    dag_store::inventario::exibir(&caminho)
                ),
                observado_em: agora(),
            });
            self.publicar();
        }
    }

    fn drenar(&mut self, buf: &mut [MaybeUninit<u8>]) {
        // Eventos coletados primeiro (o leitor empresta o buffer); aplicados depois.
        struct Ev {
            wd: i32,
            flags: ReadFlags,
            nome: Option<Vec<u8>>,
        }
        let mut evs: Vec<Ev> = Vec::new();
        {
            let mut leitor = inotify::Reader::new(&self.fd, buf);
            loop {
                match leitor.next() {
                    Ok(e) => evs.push(Ev {
                        wd: e.wd(),
                        flags: e.events(),
                        nome: e.file_name().map(|c| c.to_bytes().to_vec()),
                    }),
                    Err(Errno::AGAIN) => break,
                    Err(e) => {
                        self.fila.travar().erro = Some(format!("leitura do inotify: {e}"));
                        break;
                    }
                }
            }
        }
        for ev in evs {
            if ev.flags.contains(ReadFlags::QUEUE_OVERFLOW) {
                self.fila.lacuna(Lacuna {
                    tipo: TipoLacuna::Overflow,
                    raiz: None,
                    detalhe: "IN_Q_OVERFLOW: o kernel descartou eventos".into(),
                    observado_em: agora(),
                });
                continue;
            }
            let Some((dir, raiz)) = self.wds.get(&ev.wd).cloned() else {
                continue;
            };
            if ev.flags.contains(ReadFlags::IGNORED) {
                // Watch encerrado pelo kernel (diretório removido ou desmontado).
                self.wds.remove(&ev.wd);
                if self.por_caminho.get(&dir) == Some(&ev.wd) {
                    self.por_caminho.remove(&dir);
                }
                continue;
            }
            if ev.flags.contains(ReadFlags::UNMOUNT) {
                self.fila.lacuna(Lacuna {
                    tipo: TipoLacuna::RaizDesmontada,
                    raiz: Some(raiz),
                    detalhe: format!("IN_UNMOUNT em {}", dag_store::inventario::exibir(&dir)),
                    observado_em: agora(),
                });
                continue;
            }
            let eh_dir = ev.flags.contains(ReadFlags::ISDIR);
            match ev.nome {
                Some(nome) => {
                    let filho = juntar(&dir, &nome);
                    if eh_dir && ev.flags.contains(ReadFlags::MOVED_FROM) {
                        self.remover_arvore(&filho);
                    }
                    if eh_dir
                        && ev.flags.intersects(ReadFlags::CREATE | ReadFlags::MOVED_TO)
                        && !excluida(&self.exclusoes, &filho, &nome)
                    {
                        self.adicionar_arvore(raiz, filho.clone());
                        self.fila.marcar(&filho, raiz);
                    }
                    self.fila.marcar(&dir, raiz);
                }
                None => {
                    if ev.flags.contains(ReadFlags::MOVE_SELF) {
                        self.remover_arvore(&dir);
                    }
                    self.fila.marcar(&dir, raiz);
                    if let Some(p) = pai(&dir)
                        && self.raizes[raiz].caminho.len() <= p.len()
                    {
                        self.fila.marcar(p, raiz);
                    }
                }
            }
        }
    }
}
