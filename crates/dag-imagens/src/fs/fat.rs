//! FAT12/16/32 com nomes longos (LFN). Somente leitura.

use super::{Ctx, MAX_PROFUNDIDADE};
use crate::SistemaArquivos;
use crate::TipoMembro;
use crate::disco::{Corrente, Run, Runs, Vista, ler_runs};
use crate::erro::{Result, corrompido};
use crate::util::{byte, dos_para_unix, juntar, le16, le32, limpar_nome};
use std::collections::HashSet;
use std::io::empty;

struct Bpb {
    bps: u64,
    spc: u64,
    reservados: u64,
    nfats: u64,
    raiz_entradas: u64,
    fat_setores: u64,
    total_setores: u64,
    raiz_cluster: u32,
    fs: SistemaArquivos,
}

fn ler_bpb(b: &[u8]) -> Option<Bpb> {
    if b.len() < 512 || b[510] != 0x55 || b[511] != 0xAA {
        return None;
    }
    if !matches!(b[0], 0xEB | 0xE9) {
        return None;
    }
    let bps = u64::from(le16(b, 11));
    let spc = u64::from(byte(b, 13));
    let reservados = u64::from(le16(b, 14));
    let nfats = u64::from(byte(b, 16));
    let raiz_entradas = u64::from(le16(b, 17));
    let t16 = u64::from(le16(b, 19));
    let f16 = u64::from(le16(b, 22));
    let t32 = u64::from(le32(b, 32));
    let f32_ = u64::from(le32(b, 36));
    if !matches!(bps, 512 | 1024 | 2048 | 4096)
        || spc == 0
        || !spc.is_power_of_two()
        || spc > 128
        || reservados == 0
        || !(1..=4).contains(&nfats)
    {
        return None;
    }
    let fat_setores = if f16 != 0 { f16 } else { f32_ };
    let total_setores = if t16 != 0 { t16 } else { t32 };
    if fat_setores == 0 || total_setores == 0 {
        return None;
    }
    let raiz_set = (raiz_entradas * 32).div_ceil(bps);
    let inicio_dados = reservados + nfats * fat_setores + raiz_set;
    if total_setores <= inicio_dados {
        return None;
    }
    let clusters = (total_setores - inicio_dados) / spc;
    let fs = if clusters < 4085 {
        SistemaArquivos::Fat12
    } else if clusters < 65525 {
        SistemaArquivos::Fat16
    } else {
        SistemaArquivos::Fat32
    };
    // FAT32 não tem diretório raiz fixo; FAT12/16 têm.
    if (fs == SistemaArquivos::Fat32) == (raiz_entradas != 0) {
        return None;
    }
    Some(Bpb {
        bps,
        spc,
        reservados,
        nfats,
        raiz_entradas,
        fat_setores,
        total_setores,
        raiz_cluster: le32(b, 44),
        fs,
    })
}

pub(crate) fn detectar(b: &[u8]) -> Option<SistemaArquivos> {
    ler_bpb(b).map(|p| p.fs)
}

/// Resultado da consulta à FAT.
pub(crate) enum Prox {
    Fim,
    Cluster(u32),
    Ruim,
}

/// Acesso preguiçoso à tabela FAT com cache de um setor.
pub(crate) struct TabelaFat {
    v: Vista,
    inicio: u64,
    tam: u64,
    bits: u8,
    cache_pos: u64,
    cache: Vec<u8>,
    max_cluster: u32,
}

impl TabelaFat {
    pub(crate) fn nova(v: Vista, inicio: u64, tam: u64, bits: u8, max_cluster: u32) -> Self {
        Self {
            v,
            inicio,
            tam,
            bits,
            cache_pos: u64::MAX,
            cache: vec![0; 512],
            max_cluster,
        }
    }
    fn byte_em(&mut self, off: u64) -> Result<u8> {
        if off >= self.tam {
            return corrompido("FAT: cluster fora da tabela");
        }
        let base = off & !511;
        if base != self.cache_pos {
            let n = (self.tam - base).min(512) as usize;
            self.cache = vec![0; 512];
            self.v.ler(self.inicio + base, &mut self.cache[..n])?;
            self.cache_pos = base;
        }
        Ok(self.cache[(off & 511) as usize])
    }
    pub(crate) fn proximo(&mut self, c: u32) -> Result<Prox> {
        let v = match self.bits {
            12 => {
                let off = u64::from(c) + u64::from(c) / 2;
                let lo = u32::from(self.byte_em(off)?);
                let hi = u32::from(self.byte_em(off + 1)?);
                let w = lo | (hi << 8);
                let e = if c & 1 == 0 { w & 0xFFF } else { w >> 4 };
                return Ok(classifica(e, 0xFF8, 0xFF7, self.max_cluster));
            }
            16 => {
                let off = u64::from(c) * 2;
                u32::from(self.byte_em(off)?) | (u32::from(self.byte_em(off + 1)?) << 8)
            }
            _ => {
                let off = u64::from(c) * 4;
                let mut x = 0u32;
                for i in 0..4 {
                    x |= u32::from(self.byte_em(off + i)?) << (8 * i);
                }
                if self.bits == 32 { x & 0x0FFF_FFFF } else { x }
            }
        };
        Ok(match self.bits {
            16 => classifica(v, 0xFFF8, 0xFFF7, self.max_cluster),
            32 => classifica(v, 0x0FFF_FFF8, 0x0FFF_FFF7, self.max_cluster),
            _ => classifica(v, 0xFFFF_FFF8, 0xFFFF_FFF7, self.max_cluster),
        })
    }
}

fn classifica(e: u32, eoc: u32, ruim: u32, max: u32) -> Prox {
    if e >= eoc {
        Prox::Fim
    } else if e == ruim || e < 2 || e > max {
        Prox::Ruim
    } else {
        Prox::Cluster(e)
    }
}

pub(crate) struct Fat {
    v: Vista,
    tab: TabelaFat,
    bytes_cluster: u64,
    dados_ini: u64,
    max_cluster: u32,
    raiz_fixa: Option<(u64, u64)>,
    raiz_cluster: u32,
}

impl Fat {
    fn abrir(v: Vista) -> Result<Fat> {
        let b = v.ler_ate(0, 512)?;
        let Some(p) = ler_bpb(&b) else {
            return corrompido("setor de boot FAT inválido");
        };
        let raiz_set = (p.raiz_entradas * 32).div_ceil(p.bps);
        let inicio_dados_set = p.reservados + p.nfats * p.fat_setores + raiz_set;
        let clusters = (p.total_setores - inicio_dados_set) / p.spc;
        let max_cluster = (clusters + 1).min(u64::from(u32::MAX - 16)) as u32;
        let bits = match p.fs {
            SistemaArquivos::Fat12 => 12,
            SistemaArquivos::Fat16 => 16,
            _ => 32,
        };
        let tab = TabelaFat::nova(
            v.clone(),
            p.reservados * p.bps,
            p.fat_setores * p.bps,
            bits,
            max_cluster,
        );
        let raiz_fixa = (p.raiz_entradas > 0).then(|| {
            (
                (p.reservados + p.nfats * p.fat_setores) * p.bps,
                p.raiz_entradas * 32,
            )
        });
        Ok(Fat {
            v,
            tab,
            bytes_cluster: p.spc * p.bps,
            dados_ini: inicio_dados_set * p.bps,
            max_cluster,
            raiz_fixa,
            raiz_cluster: p.raiz_cluster,
        })
    }

    fn off_cluster(&self, c: u32) -> u64 {
        self.dados_ini + (u64::from(c) - 2) * self.bytes_cluster
    }

    /// Runs da cadeia; se `limite` for `Some(n)`, para quando cobrir `n` bytes.
    fn cadeia(&mut self, inicio: u32, limite: Option<u64>) -> Result<Runs> {
        let mut runs = Runs::default();
        let mut c = inicio;
        let mut passos = 0u64;
        let mut coberto = 0u64;
        loop {
            if c < 2 || c > self.max_cluster {
                return corrompido("cadeia FAT com cluster inválido");
            }
            runs.push(Some(self.off_cluster(c)), self.bytes_cluster)?;
            coberto += self.bytes_cluster;
            if limite.is_some_and(|l| coberto >= l) {
                return Ok(runs);
            }
            passos += 1;
            if passos > u64::from(self.max_cluster) {
                return corrompido("laço na cadeia FAT");
            }
            match self.tab.proximo(c)? {
                Prox::Fim => return Ok(runs),
                Prox::Cluster(n) => c = n,
                Prox::Ruim => return corrompido("cadeia FAT interrompida"),
            }
        }
    }

    fn ler_dir_bytes(&mut self, cluster: u32) -> Result<Vec<u8>> {
        if cluster == 0 {
            let Some((off, len)) = self.raiz_fixa else {
                return corrompido("raiz FAT sem localização");
            };
            return self.v.ler_vec(off, len);
        }
        let runs = self.cadeia(cluster, None)?;
        ler_runs(&self.v, runs)
    }
}

pub(crate) fn percorrer(v: Vista, ctx: &mut Ctx<'_>) -> Result<()> {
    let mut fat = Fat::abrir(v)?;
    let raiz = if fat.raiz_fixa.is_some() {
        0
    } else {
        fat.raiz_cluster
    };
    let mut vistos = HashSet::new();
    dir(&mut fat, ctx, raiz, &[], 0, &mut vistos)
}

fn sfn(e: &[u8]) -> Vec<u8> {
    let mut nome = e[..8].to_vec();
    let mut ext = e[8..11].to_vec();
    if nome[0] == 0x05 {
        nome[0] = 0xE5;
    }
    let flags = e[12];
    let fim = |v: &mut Vec<u8>| {
        while v.last() == Some(&b' ') {
            v.pop();
        }
    };
    fim(&mut nome);
    fim(&mut ext);
    if flags & 0x08 != 0 {
        nome.make_ascii_lowercase();
    }
    if flags & 0x10 != 0 {
        ext.make_ascii_lowercase();
    }
    let mut s: String = nome.iter().map(|&c| c as char).collect();
    if !ext.is_empty() {
        s.push('.');
        s.extend(ext.iter().map(|&c| c as char));
    }
    s.into_bytes()
}

fn checksum(e: &[u8]) -> u8 {
    e[..11]
        .iter()
        .fold(0u8, |s, &c| s.rotate_right(1).wrapping_add(c))
}

fn dir(
    fat: &mut Fat,
    ctx: &mut Ctx<'_>,
    cluster: u32,
    pai: &[u8],
    prof: usize,
    vistos: &mut HashSet<u32>,
) -> Result<()> {
    if prof >= MAX_PROFUNDIDADE {
        ctx.aviso("profundidade máxima de diretórios excedida");
        return Ok(());
    }
    if cluster != 0 && !vistos.insert(cluster) {
        ctx.aviso("diretório FAT repetido (laço)");
        return Ok(());
    }
    let dados = fat.ler_dir_bytes(cluster)?;
    let mut lfn: Vec<(u8, [u16; 13], u8)> = Vec::new();
    for e in dados.chunks_exact(32) {
        if ctx.parar {
            break;
        }
        match e[0] {
            0x00 => break,
            0xE5 => {
                lfn.clear();
                continue;
            }
            _ => {}
        }
        let attr = e[11];
        if attr & 0x3F == 0x0F {
            let mut ch = [0u16; 13];
            let pos = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
            for (i, &p) in pos.iter().enumerate() {
                ch[i] = le16(e, p);
            }
            lfn.push((e[0], ch, e[13]));
            continue;
        }
        if attr & 0x08 != 0 {
            lfn.clear();
            continue; // rótulo do volume
        }
        let nome = nome_final(e, &lfn);
        lfn.clear();
        if nome == b"." || nome == b".." {
            continue;
        }
        let nome = limpar_nome(nome);
        let caminho = juntar(pai, &nome);
        let cl = (u32::from(le16(e, 20)) << 16) | u32::from(le16(e, 26));
        let tam = u64::from(le32(e, 28));
        let mtime = dos_para_unix(le16(e, 24), le16(e, 22));
        let eh_dir = attr & 0x10 != 0;
        if eh_dir {
            let c = ctx.emitir(
                caminho.clone(),
                TipoMembro::Pasta,
                0,
                mtime,
                0,
                &mut empty(),
            )?;
            if c == crate::Controle::Continuar && cl >= 2 {
                let r = dir(fat, ctx, cl, &caminho, prof + 1, vistos);
                ctx.tolerar(r, &caminho)?;
            }
        } else {
            let runs = if tam == 0 || cl < 2 {
                Ok(Runs::default())
            } else {
                fat.cadeia(cl, Some(tam))
            };
            match runs {
                Ok(r) if r.total() >= tam => {
                    let mut leitor = Corrente::novo(fat.v.clone(), r.v, tam);
                    ctx.emitir(
                        caminho,
                        TipoMembro::Arquivo,
                        tam,
                        mtime,
                        u64::from(cl),
                        &mut leitor,
                    )?;
                }
                Ok(_) | Err(_) => {
                    // Cadeia truncada/corrompida: o arquivo é listado, mas a leitura falha.
                    ctx.aviso(format!(
                        "cadeia FAT inválida: {}",
                        String::from_utf8_lossy(&caminho)
                    ));
                    let mut leitor = Corrente::novo(fat.v.clone(), Vec::<Run>::new(), tam);
                    ctx.emitir(
                        caminho,
                        TipoMembro::Arquivo,
                        tam,
                        mtime,
                        u64::from(cl),
                        &mut leitor,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn nome_final(e: &[u8], lfn: &[(u8, [u16; 13], u8)]) -> Vec<u8> {
    if !lfn.is_empty() {
        let ck = checksum(e);
        let ok = lfn.iter().all(|(_, _, c)| *c == ck);
        if ok {
            let mut partes: Vec<&(u8, [u16; 13], u8)> = lfn.iter().collect();
            partes.sort_by_key(|p| p.0 & 0x1F);
            let mut u: Vec<u16> = Vec::new();
            for p in partes {
                u.extend_from_slice(&p.1);
            }
            if let Some(z) = u.iter().position(|&c| c == 0) {
                u.truncate(z);
            }
            let s = String::from_utf16_lossy(&u);
            if !s.is_empty() {
                return s.into_bytes();
            }
        }
    }
    sfn(e)
}
