//! Leitores de sistemas de arquivos e contexto de percurso.

pub(crate) mod exfat;
pub(crate) mod ext;
pub(crate) mod fat;
pub(crate) mod iso;
pub(crate) mod ntfs;
pub(crate) mod udf;

use crate::disco::Vista;
use crate::erro::{Erro, Result, sem_suporte};
use crate::util::{byte, fatia, le16};
use crate::{Controle, MembroImagem, Resumo, SistemaArquivos, TipoMembro};
use std::collections::HashSet;
use std::io::Read;

/// Profundidade máxima de diretórios.
pub(crate) const MAX_PROFUNDIDADE: usize = 128;
/// Máximo de membros por imagem.
pub(crate) const MAX_MEMBROS: u64 = 200_000_000;

pub(crate) type Visita<'a> = dyn FnMut(MembroImagem, &mut dyn Read) -> Result<Controle> + 'a;

/// Estado de um percurso numa partição.
pub(crate) struct Ctx<'a> {
    pub particao: usize,
    visita: &'a mut Visita<'a>,
    pub resumo: &'a mut Resumo,
    pub parar: bool,
    vistos: HashSet<u64>,
}

impl<'a> Ctx<'a> {
    pub(crate) fn novo(
        particao: usize,
        visita: &'a mut Visita<'a>,
        resumo: &'a mut Resumo,
    ) -> Self {
        Self {
            particao,
            visita,
            resumo,
            parar: false,
            vistos: HashSet::new(),
        }
    }

    /// Apresenta um membro ao visitante.
    pub(crate) fn emitir(
        &mut self,
        caminho: Vec<u8>,
        tipo: TipoMembro,
        tamanho: u64,
        mtime: Option<i64>,
        id: u64,
        leitor: &mut dyn Read,
    ) -> Result<Controle> {
        if self.parar {
            return Ok(Controle::Parar);
        }
        if self.resumo.membros >= MAX_MEMBROS {
            self.aviso("limite de membros atingido");
            self.parar = true;
            return Ok(Controle::Parar);
        }
        let repetido = id != 0 && tipo != TipoMembro::Pasta && !self.vistos.insert(id);
        self.resumo.membros += 1;
        if tipo == TipoMembro::Arquivo && !repetido {
            self.resumo.bytes = self.resumo.bytes.saturating_add(tamanho);
        }
        let m = MembroImagem {
            particao: self.particao,
            caminho,
            tamanho,
            mtime,
            tipo,
            id,
            repetido,
        };
        let c = (self.visita)(m, leitor)?;
        if c == Controle::Parar {
            self.parar = true;
        }
        Ok(c)
    }

    pub(crate) fn aviso(&mut self, msg: impl Into<String>) {
        if self.resumo.avisos.len() < 1000 {
            self.resumo.avisos.push(msg.into());
        }
    }

    /// Converte erros locais em avisos; só erros do visitante propagam.
    pub(crate) fn tolerar(&mut self, r: Result<()>, onde: &[u8]) -> Result<()> {
        match r {
            Err(Erro::Visita(e)) => Err(Erro::Visita(e)),
            Err(e) => {
                self.aviso(format!("{}: {e}", String::from_utf8_lossy(onde)));
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }
}

pub(crate) enum Sonda {
    Fs(SistemaArquivos),
    Outro(String),
}

/// Reconhece o sistema de arquivos pelos bytes iniciais (>= 64 KiB, zero-preenchido).
pub(crate) fn sondar_bytes(b: &[u8]) -> Option<SistemaArquivos> {
    match sondar_completo(b) {
        Sonda::Fs(f) => Some(f),
        Sonda::Outro(_) => None,
    }
}

pub(crate) fn sondar_completo(b: &[u8]) -> Sonda {
    if fatia(b, 3, 8) == b"NTFS    " {
        return Sonda::Fs(SistemaArquivos::Ntfs);
    }
    if fatia(b, 3, 8) == b"EXFAT   " {
        return Sonda::Fs(SistemaArquivos::ExFat);
    }
    if le16(b, 1080) == 0xEF53 {
        return Sonda::Fs(SistemaArquivos::Ext);
    }
    if let Some(f) = fat::detectar(b) {
        return Sonda::Fs(f);
    }
    if fatia(b, 0x8001, 5) == b"CD001" {
        return Sonda::Fs(SistemaArquivos::Iso9660);
    }
    for i in 0..8usize {
        if fatia(b, 0x8000 + i * 2048 + 1, 5) == b"BEA01" {
            return Sonda::Fs(SistemaArquivos::Udf);
        }
    }
    if fatia(b, 0, 4) == b"XFSB" {
        return Sonda::Outro("XFS (sem suporte)".into());
    }
    if fatia(b, 0, 6) == b"LUKS\xba\xbe" {
        return Sonda::Outro("LUKS cifrado (sem suporte)".into());
    }
    if fatia(b, 0x10040, 8) == b"_BHRfS_M" {
        return Sonda::Outro("btrfs (sem suporte)".into());
    }
    if fatia(b, 512, 8) == b"LABELONE" || fatia(b, 536, 8) == b"LVM2 001" {
        return Sonda::Outro("LVM (sem suporte)".into());
    }
    if fatia(b, 4086, 10) == b"SWAPSPACE2" {
        return Sonda::Outro("swap".into());
    }
    if b.iter().take(65536).all(|&x| x == 0) && byte(b, 0) == 0 {
        return Sonda::Outro("vazia".into());
    }
    Sonda::Outro("formato sem suporte".into())
}

pub(crate) fn sondar(v: &Vista) -> Sonda {
    match v.ler_ate(0, 0x10100) {
        Ok(b) => sondar_completo(&b),
        Err(_) => Sonda::Outro("ilegível".into()),
    }
}

pub(crate) fn percorrer(sa: SistemaArquivos, v: Vista, ctx: &mut Ctx<'_>) -> Result<()> {
    match sa {
        SistemaArquivos::Fat12 | SistemaArquivos::Fat16 | SistemaArquivos::Fat32 => {
            fat::percorrer(v, ctx)
        }
        SistemaArquivos::ExFat => exfat::percorrer(v, ctx),
        SistemaArquivos::Iso9660 => iso::percorrer(v, ctx),
        SistemaArquivos::Ext => ext::percorrer(v, ctx),
        SistemaArquivos::Ntfs => ntfs::percorrer(v, ctx),
        SistemaArquivos::Udf => udf::percorrer(v, ctx),
    }
}

pub(crate) fn _nao_implementado<T>(nome: &str) -> Result<T> {
    sem_suporte(format!("{nome} ainda não implementado"))
}
