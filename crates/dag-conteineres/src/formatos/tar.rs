//! tar e tar comprimido (gz, bz2, xz, zst). Sem índice: percorrer já lê tudo, em fluxo.

use std::io::{self, BufReader, Read};

use tar::{Archive, EntryType};

use crate::{
    Controle, ErroConteiner, FabricaLeitor, Fonte, FonteBox, LeitorConteiner, MembroInfo, Resumo,
    TipoMembro, Visita,
};

const JANELA: u64 = 2 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Compressao {
    Gzip,
    Bzip2,
    Xz,
    Zstd,
}

impl Compressao {
    fn nome_tar(self) -> &'static str {
        match self {
            Self::Gzip => "tar.gz",
            Self::Bzip2 => "tar.bz2",
            Self::Xz => "tar.xz",
            Self::Zstd => "tar.zst",
        }
    }

    fn magica(self, p: &[u8]) -> bool {
        match self {
            Self::Gzip => p.starts_with(&[0x1F, 0x8B]),
            Self::Bzip2 => p.starts_with(b"BZh"),
            Self::Xz => p.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0x00]),
            Self::Zstd => p.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]),
        }
    }

    fn decodificador(self, fonte: FonteBox) -> io::Result<Box<dyn Read + Send>> {
        let buf = BufReader::with_capacity(1 << 20, fonte);
        Ok(match self {
            Self::Gzip => Box::new(flate2::bufread::MultiGzDecoder::new(buf)),
            Self::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(buf)),
            Self::Xz => Box::new(lzma_rust2::XzReader::new(buf, true)),
            Self::Zstd => Box::new(
                ruzstd::decoding::StreamingDecoder::new(buf)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?,
            ),
        })
    }
}

#[derive(Debug)]
pub(crate) struct FabricaTar {
    compressao: Option<Compressao>,
}

impl FabricaTar {
    pub(crate) fn simples() -> Self {
        Self { compressao: None }
    }

    pub(crate) fn comprimida(c: Compressao) -> Self {
        Self {
            compressao: Some(c),
        }
    }
}

/// Cabeçalho ustar/GNU: `ustar` no deslocamento 257 do primeiro bloco.
fn bloco_tar(b: &[u8]) -> bool {
    b.len() >= 263 && &b[257..262] == b"ustar"
}

impl FabricaLeitor for FabricaTar {
    fn nome(&self) -> &'static str {
        self.compressao.map_or("tar", Compressao::nome_tar)
    }

    fn reconhece(&self, fonte: &mut dyn Fonte) -> io::Result<bool> {
        let mut bloco = [0u8; 512];
        match self.compressao {
            None => {
                let n = super::ler_ate_encher(fonte, &mut bloco)?;
                Ok(bloco_tar(&bloco[..n]))
            }
            Some(c) => {
                let mut m = [0u8; 8];
                let n = super::ler_ate_encher(fonte, &mut m)?;
                if !c.magica(&m[..n]) {
                    return Ok(false);
                }
                // Só é "tar.X" se o conteúdo descomprimido começa com um cabeçalho tar. Um
                // `.gz` qualquer (log, por exemplo) não é Contêiner.
                fonte.seek(io::SeekFrom::Start(0))?;
                // Janela generosa: o bzip2 só entrega dados com um bloco inteiro (até ~900 KiB).
                let mut v = Vec::new();
                Read::take(&mut *fonte, JANELA).read_to_end(&mut v)?;
                let origem: FonteBox = Box::new(io::Cursor::new(v));
                let Ok(mut dec) = c.decodificador(origem) else {
                    return Ok(false);
                };
                let Ok(n) = super::ler_ate_encher(&mut dec, &mut bloco) else {
                    return Ok(false);
                };
                Ok(bloco_tar(&bloco[..n]))
            }
        }
    }

    fn prefixo_promissor(&self, p: &[u8]) -> bool {
        self.compressao
            .map_or_else(|| bloco_tar(p), |c| c.magica(p))
    }

    fn abrir(
        &self,
        fonte: FonteBox,
        _senha: Option<&str>,
    ) -> Result<Box<dyn LeitorConteiner>, ErroConteiner> {
        let leitor: Box<dyn Read + Send> = match self.compressao {
            None => Box::new(BufReader::with_capacity(1 << 20, fonte)),
            Some(c) => c.decodificador(fonte).map_err(ErroConteiner::de_leitura)?,
        };
        Ok(Box::new(LeitorTar {
            nome: self.nome(),
            leitor: Some(leitor),
        }))
    }
}

struct LeitorTar {
    nome: &'static str,
    leitor: Option<Box<dyn Read + Send>>,
}

fn erro_tar(e: io::Error) -> ErroConteiner {
    ErroConteiner::de_leitura(e)
}

impl LeitorConteiner for LeitorTar {
    fn formato(&self) -> &'static str {
        self.nome
    }

    fn percorrer(&mut self, visita: &mut Visita<'_>) -> Result<Resumo, ErroConteiner> {
        let leitor = self
            .leitor
            .take()
            .ok_or_else(|| ErroConteiner::Corrompido("leitor já consumido".into()))?;
        let mut arq = Archive::new(leitor);
        let mut resumo = Resumo::default();
        for e in arq.entries().map_err(erro_tar)? {
            let mut e = e.map_err(erro_tar)?;
            let tipo = match e.header().entry_type() {
                EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse => {
                    TipoMembro::Arquivo
                }
                EntryType::Directory => TipoMembro::Pasta,
                EntryType::Symlink | EntryType::Link => TipoMembro::Link,
                EntryType::XGlobalHeader | EntryType::XHeader => continue,
                _ => TipoMembro::Outro,
            };
            let tamanho = e.header().size().ok();
            let info = MembroInfo {
                caminho: e.path_bytes().into_owned(),
                tamanho,
                mtime_ns: e
                    .header()
                    .mtime()
                    .ok()
                    .and_then(|s| i64::try_from(s).ok())
                    .map(|s| s.saturating_mul(1_000_000_000)),
                tipo,
                cifrado: false,
            };
            resumo.membros += 1;
            resumo.bytes += tamanho.unwrap_or(0);
            let c = if tipo == TipoMembro::Arquivo {
                visita(info, &mut e)?
            } else {
                visita(info, &mut io::empty())?
            };
            if c == Controle::Parar {
                break;
            }
        }
        Ok(resumo)
    }
}
