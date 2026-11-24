//! rar (1.3 a 7, pela crate `rars`, Rust puro, Apache-2.0).
//!
//! A `rars` entrega o conteúdo por empurrão (um `Write` por Membro), e o contrato desta crate
//! é por puxão (`&mut dyn Read`). Uma thread da própria travessia decodifica e empurra por um
//! canal limitado; a thread chamadora lê. Nada vai a disco.
//!
//! Atenção: um erro no meio de um Membro faz a travessia inteira devolver `Err`, e quem chama
//! descarta o que já viu daquele Contêiner (política "tudo ou nada" do Catálogo), porque o
//! leitor do Membro truncado enxerga só o fim do fluxo.

use std::io::{self, Read, Write};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use rars::{
    Archive, ArchiveMember, ArchiveReadOptions, ArchiveReader, ErrorKind, ExtractionDecision,
};

use crate::{
    Controle, ErroConteiner, FabricaLeitor, Fonte, FonteBox, LeitorConteiner, MembroInfo, Resumo,
    TipoMembro, Visita,
};

const MAGICA: &[u8] = b"Rar!\x1a\x07";

#[derive(Debug)]
pub(crate) struct FabricaRar;

impl FabricaLeitor for FabricaRar {
    fn nome(&self) -> &'static str {
        "rar"
    }

    fn reconhece(&self, fonte: &mut dyn Fonte) -> io::Result<bool> {
        let mut m = [0u8; 6];
        let n = super::ler_ate_encher(fonte, &mut m)?;
        Ok(n == 6 && m == *MAGICA)
    }

    fn prefixo_promissor(&self, p: &[u8]) -> bool {
        p.starts_with(MAGICA)
    }

    fn abrir(
        &self,
        fonte: FonteBox,
        senha: Option<&str>,
    ) -> Result<Box<dyn LeitorConteiner>, ErroConteiner> {
        let opcoes = ArchiveReadOptions::with_optional_password(senha.map(str::as_bytes));
        let arq = ArchiveReader::read_reader_with_options(fonte, opcoes).map_err(erro_rar)?;
        Ok(Box::new(LeitorRar {
            arq,
            senha: senha.map(str::to_owned),
        }))
    }
}

fn erro_rar(e: rars::Error) -> ErroConteiner {
    match e.kind() {
        ErrorKind::PasswordRequired => ErroConteiner::SenhaNecessaria,
        ErrorKind::BadPassword => ErroConteiner::SenhaIncorreta,
        ErrorKind::UnsupportedFormat | ErrorKind::UnsupportedFeature => {
            ErroConteiner::NaoSuportado(e.to_string())
        }
        ErrorKind::Cancelled => ErroConteiner::Cancelado,
        _ => ErroConteiner::Corrompido(e.to_string()),
    }
}

struct LeitorRar {
    arq: Archive,
    senha: Option<String>,
}

type Dados = Receiver<Vec<u8>>;

enum Evento {
    Membro(MembroInfo, Dados),
}

/// Ponta de escrita que a `rars` usa; empacota em pedaços e manda pelo canal.
struct Escritor {
    tx: SyncSender<Vec<u8>>,
}

impl Write for Escritor {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.tx
            .send(b.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "leitor encerrou"))?;
        Ok(b.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Ponta de leitura entregue à visita.
struct Cano {
    rx: Dados,
    atual: io::Cursor<Vec<u8>>,
}

impl Cano {
    fn novo(rx: Dados) -> Self {
        Self {
            rx,
            atual: io::Cursor::new(Vec::new()),
        }
    }
}

impl Read for Cano {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = self.atual.read(buf)?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            match self.rx.recv() {
                Ok(v) => self.atual = io::Cursor::new(v),
                Err(_) => return Ok(0),
            }
        }
    }
}

fn info_de(m: &ArchiveMember) -> MembroInfo {
    MembroInfo {
        caminho: m.meta.name_bytes().to_vec(),
        tamanho: Some(m.meta.unpacked_size),
        mtime_ns: m
            .meta
            .modification_time()
            .and_then(|t| i64::try_from(t.unix_nanoseconds()).ok()),
        tipo: if m.meta.is_directory {
            TipoMembro::Pasta
        } else if m.meta.is_redirection {
            TipoMembro::Link
        } else {
            TipoMembro::Arquivo
        },
        cifrado: m.meta.is_encrypted,
    }
}

impl LeitorConteiner for LeitorRar {
    fn formato(&self) -> &'static str {
        "rar"
    }

    fn percorrer(&mut self, visita: &mut Visita<'_>) -> Result<Resumo, ErroConteiner> {
        let tem_cifrado = self.arq.members().any(|m| m.meta.is_encrypted);
        if tem_cifrado && self.senha.is_none() {
            return Err(ErroConteiner::SenhaNecessaria);
        }
        let arq = &self.arq;
        let senha = self.senha.as_deref().map(str::as_bytes);
        let mut resumo = Resumo::default();
        let mut falha: Option<ErroConteiner> = None;
        let mut interrompido = false;
        let resultado = std::thread::scope(|s| {
            let (tx, rx) = sync_channel::<Evento>(1);
            let h = s.spawn(move || {
                let opcoes = ArchiveReadOptions::with_optional_password(senha);
                arq.extract_with_control(opcoes, |m| {
                    let info = info_de(m);
                    let (dtx, drx) = sync_channel::<Vec<u8>>(4);
                    let sem_conteudo = info.tipo != TipoMembro::Arquivo;
                    tx.send(Evento::Membro(info, drx))
                        .map_err(|_| rars::Error::InvalidArgument("leitor encerrou"))?;
                    Ok(if sem_conteudo {
                        ExtractionDecision::Skip
                    } else {
                        ExtractionDecision::Extract(Box::new(Escritor { tx: dtx }))
                    })
                })
            });
            while let Ok(Evento::Membro(info, drx)) = rx.recv() {
                resumo.membros += 1;
                resumo.bytes += info.tamanho.unwrap_or(0);
                let mut cano = Cano::novo(drx);
                let r = visita(info, &mut cano);
                // Esvazia o que a visita não leu, para a decodificação poder seguir.
                let _ = io::copy(&mut cano, &mut io::sink());
                match r {
                    Ok(Controle::Continuar) => {}
                    Ok(Controle::Parar) => {
                        interrompido = true;
                        break;
                    }
                    Err(e) => {
                        falha = Some(e);
                        break;
                    }
                }
            }
            drop(rx);
            h.join()
        });
        if let Some(e) = falha {
            return Err(e);
        }
        match resultado {
            Ok(Ok(_)) => Ok(resumo),
            // Parada pedida pela visita: o canal fechou e a decodificação terminou em erro.
            Ok(Err(_)) if interrompido => Ok(resumo),
            Ok(Err(e)) => Err(erro_rar(e)),
            Err(_) => Err(ErroConteiner::Corrompido(
                "a decodificação do rar encerrou de forma inesperada".into(),
            )),
        }
    }
}
