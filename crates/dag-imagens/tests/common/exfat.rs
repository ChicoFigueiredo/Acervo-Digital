//! Formatador exFAT mínimo para testes.

use super::No;

const SPC_SHIFT: u32 = 3; // 8 setores por cluster = 4096 bytes
const BC: usize = 512 << SPC_SHIFT;
const FAT_OFF: usize = 128;
const N_CLUSTERS: usize = 2000;
const FAT_SET: usize = 16; // 2000*4/512 -> 16 setores
const HEAP_OFF: usize = FAT_OFF + FAT_SET + 8;
const DATA: u32 = ((2024 - 1980) << 9) | (3 << 5) | 15;
const HORA: u32 = (12 << 11) | (34 << 5) | (56 / 2);

struct B {
    img: Vec<u8>,
    fat: Vec<u32>,
    prox: u32,
    n_entrada: u32,
}

impl B {
    fn off(c: u32) -> usize {
        HEAP_OFF * 512 + (c as usize - 2) * BC
    }
    fn alloc(&mut self, n: usize, encadeado: bool) -> u32 {
        let n = n.max(1) as u32;
        let ini = self.prox;
        self.prox += n;
        assert!(self.prox as usize <= N_CLUSTERS + 2);
        for c in ini..ini + n {
            self.fat[c as usize] = if c + 1 < ini + n { c + 1 } else { 0xFFFF_FFFF };
            if !encadeado {
                self.fat[c as usize] = 0;
            }
        }
        ini
    }
    fn escrever(&mut self, c: u32, d: &[u8]) {
        let o = Self::off(c);
        self.img[o..o + d.len()].copy_from_slice(d);
    }
    fn dir(&mut self, entradas: &[(String, No)], eh_raiz: bool) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        if eh_raiz {
            let mut e = [0u8; 32];
            e[0] = 0x81; // bitmap de alocação (ignorado pelo leitor)
            out.extend_from_slice(&e);
            let mut e = [0u8; 32];
            e[0] = 0x83; // rótulo
            out.extend_from_slice(&e);
        }
        for (nome, no) in entradas {
            self.n_entrada += 1;
            let contiguo = self.n_entrada % 2 == 0;
            let (attr, cl, tam, sem_cadeia) = match no {
                No::Arq(d) => {
                    if d.is_empty() {
                        (0x20u16, 0u32, 0u64, false)
                    } else {
                        let c = self.alloc(d.len().div_ceil(BC), !contiguo);
                        self.escrever(c, d);
                        (0x20, c, d.len() as u64, contiguo)
                    }
                }
                No::Dir(f) => {
                    let bytes = self.dir(f, false);
                    let c = self.alloc(bytes.len().div_ceil(BC), !contiguo);
                    self.escrever(c, &bytes);
                    (
                        0x10,
                        c,
                        bytes.len().div_ceil(BC).max(1) as u64 * BC as u64,
                        contiguo,
                    )
                }
                No::Link(_) => panic!("exFAT não tem links"),
            };
            let u: Vec<u16> = nome.encode_utf16().collect();
            let n_nome = u.len().div_ceil(15);
            let mut e = [0u8; 32];
            e[0] = 0x85;
            e[1] = 1 + n_nome as u8;
            e[4..6].copy_from_slice(&attr.to_le_bytes());
            let t = (DATA << 16) | HORA;
            e[8..12].copy_from_slice(&t.to_le_bytes());
            e[12..16].copy_from_slice(&t.to_le_bytes());
            out.extend_from_slice(&e);
            let mut s = [0u8; 32];
            s[0] = 0xC0;
            s[1] = if sem_cadeia { 3 } else { 1 };
            s[3] = u.len() as u8;
            s[8..16].copy_from_slice(&tam.to_le_bytes());
            s[20..24].copy_from_slice(&cl.to_le_bytes());
            s[24..32].copy_from_slice(&tam.to_le_bytes());
            out.extend_from_slice(&s);
            for k in 0..n_nome {
                let mut n = [0u8; 32];
                n[0] = 0xC1;
                for i in 0..15 {
                    if let Some(c) = u.get(k * 15 + i) {
                        n[2 + i * 2..4 + i * 2].copy_from_slice(&c.to_le_bytes());
                    }
                }
                out.extend_from_slice(&n);
            }
        }
        out
    }
}

/// Monta uma imagem exFAT.
pub fn exfat_img(arv: &[(String, No)]) -> Vec<u8> {
    let total_set = HEAP_OFF + N_CLUSTERS * 8;
    let mut img = vec![0u8; total_set * 512];
    img[3..11].copy_from_slice(b"EXFAT   ");
    img[72..80].copy_from_slice(&(total_set as u64).to_le_bytes());
    img[80..84].copy_from_slice(&(FAT_OFF as u32).to_le_bytes());
    img[84..88].copy_from_slice(&(FAT_SET as u32).to_le_bytes());
    img[88..92].copy_from_slice(&(HEAP_OFF as u32).to_le_bytes());
    img[92..96].copy_from_slice(&(N_CLUSTERS as u32).to_le_bytes());
    img[108] = 9;
    img[109] = SPC_SHIFT as u8;
    img[510] = 0x55;
    img[511] = 0xAA;
    let mut b = B {
        img,
        fat: vec![0; N_CLUSTERS + 2],
        prox: 2,
        n_entrada: 0,
    };
    b.fat[0] = 0xFFFF_FFF8;
    b.fat[1] = 0xFFFF_FFFF;
    let bytes = b.dir(arv, true);
    let c = b.alloc(bytes.len().div_ceil(BC), true);
    b.escrever(c, &bytes);
    b.img[96..100].copy_from_slice(&c.to_le_bytes());
    for (i, v) in b.fat.iter().enumerate() {
        let o = FAT_OFF * 512 + i * 4;
        b.img[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    b.img
}
