//! Microsoft VHDX (fixo e dinâmico). Somente leitura.
//!
//! Discos diferenciais (com pai) e logs pendentes não são reexecutados: o
//! diferencial é recusado como sem suporte. O log de escrita do VHDX não é
//! reproduzido (nada é gravado), então um VHDX com log pendente é lido como está.

use crate::disco::{Disco, MAX_METADADO};
use crate::erro::{Result, corrompido, sem_suporte};
use crate::util::{le16, le32, le64};

const GUID_BAT: [u8; 16] = guid(
    0x2DC2_7766,
    0xF623,
    0x4200,
    [0x9D, 0x64, 0x11, 0x5E, 0x9B, 0xFD, 0x4A, 0x08],
);
const GUID_METADADOS: [u8; 16] = guid(
    0x8B7C_A206,
    0x4790,
    0x4B9A,
    [0xB8, 0xFE, 0x57, 0x5F, 0x05, 0x0F, 0x88, 0x6E],
);
const GUID_PARAMETROS: [u8; 16] = guid(
    0xCAA1_6737,
    0xFA36,
    0x4D43,
    [0xB3, 0xB6, 0x33, 0xF0, 0xAA, 0x44, 0xE7, 0x6B],
);
const GUID_TAMANHO: [u8; 16] = guid(
    0x2FA5_4224,
    0xCD1B,
    0x4876,
    [0xB2, 0x11, 0x5D, 0xBE, 0xD8, 0x3B, 0xF4, 0xB8],
);
const GUID_SETOR_LOGICO: [u8; 16] = guid(
    0x8141_BF1D,
    0xA96F,
    0x4709,
    [0xBA, 0x47, 0xF2, 0x33, 0xA8, 0xFA, 0xAB, 0x5F],
);

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = c.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6],
        d[7],
    ]
}

struct Vhdx {
    f: Box<dyn Disco>,
    bat: Vec<u64>,
    bloco: u64,
    razao: u64,
    tamanho: u64,
}

pub(crate) fn abrir(mut f: Box<dyn Disco>) -> Result<Box<dyn Disco>> {
    let flen = f.tamanho();
    let mut ler = |pos: u64, n: usize| -> Result<Vec<u8>> {
        let mut v = vec![0u8; n];
        f.ler(pos, &mut v)?;
        Ok(v)
    };
    if flen < 1024 * 1024 {
        return corrompido("VHDX menor que a seção de cabeçalhos");
    }
    if &ler(0, 8)?[..] != b"vhdxfile" {
        return corrompido("VHDX sem identificador");
    }
    // Cabeçalho mais recente.
    let mut melhor: Option<u64> = None;
    for off in [64 * 1024u64, 128 * 1024] {
        let h = ler(off, 72)?;
        if &h[..4] == b"head" {
            let seq = le64(&h, 8);
            if melhor.is_none_or(|m| seq > m) {
                melhor = Some(seq);
            }
            if le16(&h, 66) != 1 {
                return sem_suporte("VHDX: versão de formato desconhecida");
            }
        }
    }
    if melhor.is_none() {
        return corrompido("VHDX sem cabeçalho válido");
    }
    // Tabela de regiões.
    let mut rt = None;
    for off in [192 * 1024u64, 256 * 1024] {
        let t = ler(off, 64 * 1024)?;
        if &t[..4] == b"regi" {
            rt = Some(t);
            break;
        }
    }
    let Some(rt) = rt else {
        return corrompido("VHDX sem tabela de regiões");
    };
    let n = (le32(&rt, 8) as usize).min(2047);
    let (mut bat_reg, mut meta_reg) = (None, None);
    for i in 0..n {
        let e = &rt[16 + i * 32..16 + (i + 1) * 32];
        let reg = (le64(e, 16), u64::from(le32(e, 24)));
        if e[..16] == GUID_BAT {
            bat_reg = Some(reg);
        } else if e[..16] == GUID_METADADOS {
            meta_reg = Some(reg);
        } else if le32(e, 28) & 1 != 0 {
            return sem_suporte("VHDX: região obrigatória desconhecida");
        }
    }
    let (Some((bat_off, bat_len)), Some((meta_off, meta_len))) = (bat_reg, meta_reg) else {
        return corrompido("VHDX sem região BAT ou de metadados");
    };
    if meta_off.saturating_add(meta_len.min(1 << 20)) > flen || meta_len < 64 * 1024 {
        return corrompido("VHDX: região de metadados fora do arquivo");
    }
    let meta = ler(meta_off, meta_len.min(2 << 20) as usize)?;
    if &meta[..8] != b"metadata" {
        return corrompido("VHDX: tabela de metadados inválida");
    }
    let (mut bloco, mut flags, mut tam, mut setor) = (0u64, 0u32, 0u64, 512u64);
    let ne = usize::from(le16(&meta, 10)).min(2047);
    for i in 0..ne {
        let e = &meta[32 + i * 32..32 + (i + 1) * 32];
        let (off, len) = (le32(e, 16) as usize, le32(e, 20) as usize);
        let Some(item) = meta.get(off..off.saturating_add(len)) else {
            return corrompido("VHDX: item de metadados fora da região");
        };
        if e[..16] == GUID_PARAMETROS && len >= 8 {
            bloco = u64::from(le32(item, 0));
            flags = le32(item, 4);
        } else if e[..16] == GUID_TAMANHO && len >= 8 {
            tam = le64(item, 0);
        } else if e[..16] == GUID_SETOR_LOGICO && len >= 4 {
            setor = u64::from(le32(item, 0));
        }
    }
    if flags & 2 != 0 {
        return sem_suporte("VHDX diferencial: disco pai não suportado");
    }
    if !bloco.is_power_of_two() || !(1 << 20..=256 << 20).contains(&bloco) {
        return corrompido("VHDX: tamanho de bloco inválido");
    }
    if !matches!(setor, 512 | 4096) || tam == 0 || tam > 1 << 50 {
        return corrompido("VHDX: tamanho do disco ou do setor inválido");
    }
    let razao = ((1u64 << 23) * setor) / bloco;
    let payload = tam.div_ceil(bloco);
    let total = payload + (payload.saturating_sub(1)) / razao.max(1);
    if total * 8 > bat_len || total * 8 > MAX_METADADO || bat_off.saturating_add(total * 8) > flen {
        return corrompido("VHDX: BAT inconsistente com o tamanho do disco");
    }
    let bytes = ler(bat_off, (total * 8) as usize)?;
    let bat: Vec<u64> = bytes
        .chunks_exact(8)
        .map(|c| u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
        .collect();
    Ok(Box::new(Vhdx {
        f,
        bat,
        bloco,
        razao,
        tamanho: tam,
    }))
}

impl Disco for Vhdx {
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
            let bi = p / self.bloco;
            let dentro = p % self.bloco;
            let n = ((self.bloco - dentro) as usize).min(buf.len() - feito);
            let idx = bi + bi / self.razao;
            let Some(&e) = self.bat.get(idx as usize) else {
                return corrompido("VHDX: bloco fora da BAT");
            };
            let dest = &mut buf[feito..feito + n];
            match e & 7 {
                6 => {
                    let base = (e >> 20) << 20;
                    self.f.ler(base + dentro, dest)?;
                }
                0..=3 => dest.fill(0),
                7 => return sem_suporte("VHDX: bloco parcialmente presente (diferencial)"),
                _ => return corrompido("VHDX: estado de bloco inválido"),
            }
            feito += n;
        }
        Ok(())
    }
}
