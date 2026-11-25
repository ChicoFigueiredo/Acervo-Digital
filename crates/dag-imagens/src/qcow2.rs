//! QEMU qcow2 v2/v3, incluindo clusters comprimidos (deflate). Somente leitura.
//!
//! Arquivo de apoio (backing file), cifra, arquivo de dados externo, zstd e L2
//! estendida ficam sem suporte.

use crate::disco::{Disco, MAX_METADADO};
use crate::erro::{Result, corrompido, sem_suporte};
use crate::util::{be32, be64, byte};
use flate2::read::DeflateDecoder;
use std::io::Read;

struct Qcow2 {
    f: Box<dyn Disco>,
    cluster: u64,
    bits: u32,
    tamanho: u64,
    l1: Vec<u64>,
    l2_por_tabela: u64,
    cache_l2: Option<(u64, Vec<u8>)>,
    cache_cluster: Option<(u64, Vec<u8>)>,
}

const MASCARA_OFFSET: u64 = 0x00ff_ffff_ffff_fe00;

pub(crate) fn abrir(mut f: Box<dyn Disco>) -> Result<Box<dyn Disco>> {
    let flen = f.tamanho();
    if flen < 72 {
        return corrompido("qcow2 truncado");
    }
    let mut h = vec![0u8; 112.min(flen as usize)];
    f.ler(0, &mut h)?;
    if &h[..4] != b"QFI\xfb" {
        return corrompido("qcow2 sem assinatura");
    }
    let versao = be32(&h, 4);
    if !(2..=3).contains(&versao) {
        return sem_suporte(format!("qcow2 versão {versao}"));
    }
    if be64(&h, 8) != 0 || be32(&h, 16) != 0 {
        return sem_suporte("qcow2 com arquivo de apoio (backing file)");
    }
    let bits = be32(&h, 20);
    if !(9..=22).contains(&bits) {
        return corrompido("qcow2: cluster_bits inválido");
    }
    if be32(&h, 32) != 0 {
        return sem_suporte("qcow2 cifrado");
    }
    if versao == 3 {
        let incompat = be64(&h, 72);
        if incompat & 4 != 0 {
            return sem_suporte("qcow2 com arquivo de dados externo");
        }
        if incompat & 8 != 0 && byte(&h, 104) != 0 {
            return sem_suporte("qcow2 com compressão zstd");
        }
        if incompat & 16 != 0 {
            return sem_suporte("qcow2 com L2 estendida");
        }
        if incompat & !0x1F != 0 {
            return sem_suporte("qcow2 com recurso incompatível desconhecido");
        }
    }
    let tamanho = be64(&h, 24);
    let cluster = 1u64 << bits;
    let l2_por_tabela = cluster / 8;
    if tamanho == 0 || tamanho > 1 << 50 {
        return corrompido("qcow2: tamanho virtual inválido");
    }
    let l1_tam = u64::from(be32(&h, 36));
    let l1_off = be64(&h, 40);
    let necessario = tamanho.div_ceil(cluster * l2_por_tabela);
    if l1_tam < necessario || l1_tam * 8 > MAX_METADADO || l1_off.saturating_add(l1_tam * 8) > flen
    {
        return corrompido("qcow2: tabela L1 inconsistente");
    }
    let mut bytes = vec![0u8; (l1_tam * 8) as usize];
    f.ler(l1_off, &mut bytes)?;
    let l1 = bytes
        .chunks_exact(8)
        .map(|c| be64(c, 0))
        .collect::<Vec<_>>();
    Ok(Box::new(Qcow2 {
        f,
        cluster,
        bits,
        tamanho,
        l1,
        l2_por_tabela,
        cache_l2: None,
        cache_cluster: None,
    }))
}

impl Qcow2 {
    /// Entrada L2 do cluster virtual `ci`.
    fn entrada(&mut self, ci: u64) -> Result<u64> {
        let l1i = ci / self.l2_por_tabela;
        let l2i = (ci % self.l2_por_tabela) as usize;
        let Some(&e1) = self.l1.get(l1i as usize) else {
            return corrompido("qcow2: índice L1 fora da tabela");
        };
        let off = e1 & MASCARA_OFFSET;
        if off == 0 {
            return Ok(0);
        }
        if off.saturating_add(self.cluster) > self.f.tamanho() {
            return corrompido("qcow2: tabela L2 fora do arquivo");
        }
        if self.cache_l2.as_ref().is_none_or(|(o, _)| *o != off) {
            let mut t = vec![0u8; self.cluster as usize];
            self.f.ler(off, &mut t)?;
            self.cache_l2 = Some((off, t));
        }
        let Some((_, t)) = &self.cache_l2 else {
            return corrompido("qcow2: cache L2");
        };
        Ok(be64(t, l2i * 8))
    }

    fn descomprimir(&mut self, e: u64) -> Result<Vec<u8>> {
        let shift = 62 - (self.bits - 8);
        let off = e & ((1u64 << shift) - 1);
        let setores = ((e >> shift) & ((1u64 << (self.bits - 8)) - 1)) + 1;
        let tam = setores * 512 - (off & 511);
        if off.saturating_add(tam) > self.f.tamanho() {
            // O último cluster comprimido pode ficar colado ao fim do arquivo.
            if off >= self.f.tamanho() {
                return corrompido("qcow2: cluster comprimido fora do arquivo");
            }
        }
        let disp = (self.f.tamanho() - off).min(tam);
        let mut comp = vec![0u8; disp as usize];
        self.f.ler(off, &mut comp)?;
        let mut out = Vec::with_capacity(self.cluster as usize);
        DeflateDecoder::new(&comp[..])
            .take(self.cluster)
            .read_to_end(&mut out)
            .map_err(|e| crate::Erro::Corrompido(format!("qcow2: deflate inválido: {e}")))?;
        out.resize(self.cluster as usize, 0);
        Ok(out)
    }
}

impl Disco for Qcow2 {
    fn tamanho(&self) -> u64 {
        self.tamanho
    }

    fn ler(&mut self, pos: u64, buf: &mut [u8]) -> Result<()> {
        if pos
            .checked_add(buf.len() as u64)
            .is_none_or(|f| f > self.tamanho)
        {
            return corrompido("leitura além do fim do disco virtual");
        }
        let mut feito = 0usize;
        while feito < buf.len() {
            let p = pos + feito as u64;
            let ci = p / self.cluster;
            let dentro = p % self.cluster;
            let n = ((self.cluster - dentro) as usize).min(buf.len() - feito);
            let e = self.entrada(ci)?;
            let dest = &mut buf[feito..feito + n];
            if e & (1 << 62) != 0 {
                if self.cache_cluster.as_ref().is_none_or(|(c, _)| *c != ci) {
                    let d = self.descomprimir(e)?;
                    self.cache_cluster = Some((ci, d));
                }
                if let Some((_, d)) = &self.cache_cluster {
                    dest.copy_from_slice(&d[dentro as usize..dentro as usize + n]);
                }
            } else if e & 1 != 0 || e & MASCARA_OFFSET == 0 {
                dest.fill(0);
            } else {
                self.f.ler((e & MASCARA_OFFSET) + dentro, dest)?;
            }
            feito += n;
        }
        Ok(())
    }
}
