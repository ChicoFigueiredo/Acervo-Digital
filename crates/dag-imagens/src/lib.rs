//! Leitor somente leitura de Imagens de Disco, em Rust puro e sem `unsafe`.
//!
//! Nada é extraído, montado ou gravado: o conteúdo de cada membro é entregue em
//! fluxo, direto da imagem. Veja o `README.md` do crate para os formatos aceitos.
//!
//! ```no_run
//! use dag_imagens::{abrir, Controle};
//! let f = std::fs::File::open("disco.vhdx").unwrap();
//! let mut img = abrir(f).unwrap();
//! let resumo = img
//!     .percorrer(&mut |m, _conteudo| -> Result<Controle, std::io::Error> {
//!         println!("{} ({} bytes)", String::from_utf8_lossy(&m.caminho), m.tamanho);
//!         Ok(Controle::Continuar)
//!     })
//!     .unwrap();
//! println!("{} membros", resumo.membros);
//! ```

mod disco;
mod erro;
mod fs;
mod qcow2;
mod tabela;
mod util;
mod vhdx;

pub use erro::{Erro, Result};

use disco::{Bruto, Vista};
use std::io::{Read, Seek, SeekFrom};

/// Formato do contêiner de disco.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatoImagem {
    /// Imagem bruta (`.img`): setores do disco ou de uma partição.
    Raw,
    /// Microsoft VHDX (fixo, dinâmico ou diferencial sem pai).
    Vhdx,
    /// QEMU qcow2 (v2/v3, sem arquivo de apoio).
    Qcow2,
    /// ISO 9660 / UDF de mídia óptica.
    Iso,
}

/// Sistema de arquivos reconhecido numa partição.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SistemaArquivos {
    /// ext2/ext3/ext4.
    Ext,
    /// NTFS.
    Ntfs,
    /// FAT12.
    Fat12,
    /// FAT16.
    Fat16,
    /// FAT32.
    Fat32,
    /// exFAT.
    ExFat,
    /// ISO 9660 (com Joliet e Rock Ridge).
    Iso9660,
    /// UDF.
    Udf,
}

/// Uma partição (ou o disco inteiro, se não houver tabela de partições).
#[derive(Clone, Debug)]
pub struct Particao {
    /// Posição na ordem de descoberta (0, 1, ...).
    pub indice: usize,
    /// Deslocamento inicial em bytes no disco lógico.
    pub inicio: u64,
    /// Tamanho em bytes.
    pub tamanho: u64,
    /// Tipo segundo a tabela (`mbr:0x83`, `gpt:<guid>`, `sem-tabela`, `iso`).
    pub tipo: String,
    /// Nome da partição (GPT) ou rótulo, se houver.
    pub nome: Option<String>,
    /// Sistema de arquivos legível, se reconhecido.
    pub sistema: Option<SistemaArquivos>,
    /// Motivo de a partição não ser lida (formato sem suporte, vazia...).
    pub nao_lida: Option<String>,
}

/// Natureza de um membro.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipoMembro {
    /// Arquivo comum.
    Arquivo,
    /// Pasta.
    Pasta,
    /// Link simbólico (o conteúdo lido é o destino).
    Link,
}

/// Entrada dentro de uma Imagem de Disco.
#[derive(Clone, Debug)]
pub struct MembroImagem {
    /// Índice da partição (`Imagem::particoes`).
    pub particao: usize,
    /// Caminho dentro da partição, sem `/` inicial, separado por `/` (bytes; UTF-8
    /// salvo em ext4 com nomes não UTF-8).
    pub caminho: Vec<u8>,
    /// Tamanho em bytes (0 para pastas).
    pub tamanho: u64,
    /// Data de modificação em segundos Unix, se válida.
    pub mtime: Option<i64>,
    /// Tipo da entrada.
    pub tipo: TipoMembro,
    /// Identificador interno (inode, registro MFT, cluster inicial, extent...);
    /// 0 quando não existe.
    pub id: u64,
    /// `true` se `id` já apareceu antes nesta partição (hard link): o conteúdo é o
    /// mesmo e o chamador pode deduplicar.
    pub repetido: bool,
}

/// Decisão da função visitante.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Controle {
    /// Segue normalmente (entra nas pastas).
    Continuar,
    /// Numa pasta, não entra nela; nos demais, equivale a `Continuar`.
    Pular,
    /// Encerra o percurso inteiro.
    Parar,
}

/// Resultado de um percurso.
#[derive(Clone, Debug, Default)]
pub struct Resumo {
    /// Membros apresentados ao visitante.
    pub membros: u64,
    /// Soma dos tamanhos dos arquivos únicos (sem contar `repetido`).
    pub bytes: u64,
    /// Partições percorridas.
    pub particoes_lidas: usize,
    /// Partições não lidas: (índice, motivo).
    pub nao_lidas: Vec<(usize, String)>,
    /// Problemas toleráveis (diretório corrompido, arquivo truncado...).
    pub avisos: Vec<String>,
    /// `true` se o visitante pediu `Parar`.
    pub interrompido: bool,
}

/// Imagem de disco aberta.
pub struct Imagem {
    formato: FormatoImagem,
    vista: Vista,
    particoes: Vec<Particao>,
    avisos: Vec<String>,
}

impl std::fmt::Debug for Imagem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Imagem")
            .field("formato", &self.formato)
            .field("particoes", &self.particoes)
            .finish()
    }
}

/// Reconhece o formato do contêiner pelos bytes mágicos. Não altera o conteúdo; a
/// posição do fluxo volta ao início.
pub fn detectar(r: &mut (impl Read + Seek)) -> Option<FormatoImagem> {
    let fmt = detectar_interno(r);
    let _ = r.seek(SeekFrom::Start(0));
    fmt
}

fn detectar_interno(r: &mut (impl Read + Seek)) -> Option<FormatoImagem> {
    let len = r.seek(SeekFrom::End(0)).ok()?;
    let mut ler = |pos: u64, n: usize| -> Vec<u8> {
        let mut v = vec![0u8; n];
        let disp = len.saturating_sub(pos).min(n as u64) as usize;
        if disp > 0 && r.seek(SeekFrom::Start(pos)).is_ok() {
            let _ = r.read_exact(&mut v[..disp]);
        }
        v
    };
    let cab = ler(0, 4096);
    if cab.starts_with(b"vhdxfile") {
        return Some(FormatoImagem::Vhdx);
    }
    if cab.starts_with(b"QFI\xfb") {
        return Some(FormatoImagem::Qcow2);
    }
    let vd = ler(0x8000, 2048 * 8);
    if vd.get(1..6) == Some(b"CD001") || vd.chunks(2048).any(|s| s.get(1..6) == Some(b"BEA01")) {
        return Some(FormatoImagem::Iso);
    }
    let gpt = &ler(512, 8)[..] == b"EFI PART" || &ler(4096, 8)[..] == b"EFI PART";
    let mbr = tabela::mbr_plausivel(&cab);
    let bare = fs::sondar_bytes(&ler(0, 0x10100)).is_some();
    if gpt || mbr || bare {
        return Some(FormatoImagem::Raw);
    }
    None
}

/// Abre uma Imagem de Disco. O fluxo é só lido (nunca escrito).
pub fn abrir<R: Read + Seek + 'static>(r: R) -> Result<Imagem> {
    let mut r = r;
    let formato = detectar(&mut r).ok_or(Erro::Desconhecido)?;
    let bruto = Bruto::novo(r)?;
    let mut avisos = Vec::new();
    let (vista, particoes) = match formato {
        FormatoImagem::Vhdx => {
            let d = vhdx::abrir(Box::new(bruto))?;
            let v = Vista::nova(d);
            let p = tabela::ler(&v, &mut avisos)?;
            (v, p)
        }
        FormatoImagem::Qcow2 => {
            let d = qcow2::abrir(Box::new(bruto))?;
            let v = Vista::nova(d);
            let p = tabela::ler(&v, &mut avisos)?;
            (v, p)
        }
        FormatoImagem::Raw => {
            let v = Vista::nova(Box::new(bruto));
            let p = tabela::ler(&v, &mut avisos)?;
            (v, p)
        }
        FormatoImagem::Iso => {
            let v = Vista::nova(Box::new(bruto));
            let p = tabela::iso_inteira(&v);
            (v, p)
        }
    };
    Ok(Imagem {
        formato,
        vista,
        particoes,
        avisos,
    })
}

impl Imagem {
    /// Formato do contêiner.
    pub fn formato(&self) -> FormatoImagem {
        self.formato
    }

    /// Partições descobertas.
    pub fn particoes(&self) -> &[Particao] {
        &self.particoes
    }

    /// Tamanho do disco lógico em bytes.
    pub fn tamanho(&self) -> u64 {
        self.vista.len()
    }

    /// Percorre todos os membros de todas as partições legíveis, em ordem de
    /// partição e de diretório. Para cada membro chama `visita` com um leitor do
    /// conteúdo (vazio para pastas); o leitor só vale durante a chamada. Problemas
    /// locais viram `Resumo::avisos` e o percurso continua; só falhas do visitante
    /// interrompem com erro.
    pub fn percorrer<E>(
        &mut self,
        visita: &mut dyn FnMut(MembroImagem, &mut dyn Read) -> std::result::Result<Controle, E>,
    ) -> Result<Resumo>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        let mut resumo = Resumo {
            avisos: self.avisos.clone(),
            ..Resumo::default()
        };
        let mut envelope = |m: MembroImagem, r: &mut dyn Read| -> Result<Controle> {
            visita(m, r).map_err(|e| Erro::Visita(Box::new(e)))
        };
        for p in &self.particoes {
            if let Some(motivo) = &p.nao_lida {
                resumo.nao_lidas.push((p.indice, motivo.clone()));
                continue;
            }
            let Some(sa) = p.sistema else {
                resumo
                    .nao_lidas
                    .push((p.indice, "formato sem suporte".to_string()));
                continue;
            };
            let sub = match self.vista.sub(p.inicio, p.tamanho) {
                Ok(s) => s,
                Err(e) => {
                    resumo.nao_lidas.push((p.indice, e.to_string()));
                    continue;
                }
            };
            let mut ctx = fs::Ctx::novo(p.indice, &mut envelope, &mut resumo);
            match fs::percorrer(sa, sub, &mut ctx) {
                Ok(()) => {}
                Err(Erro::Visita(e)) => return Err(Erro::Visita(e)),
                Err(e) => {
                    ctx.resumo.nao_lidas.push((p.indice, e.to_string()));
                    continue;
                }
            }
            let parar = ctx.parar;
            resumo.particoes_lidas += 1;
            if parar {
                resumo.interrompido = true;
                break;
            }
        }
        Ok(resumo)
    }
}
