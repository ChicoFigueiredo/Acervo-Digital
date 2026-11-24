//! Formatador FAT12/16/32 mínimo (com LFN) para testes.

use super::No;

const DATA: u16 = ((2024 - 1980) << 9) | (3 << 5) | 15;
const HORA: u16 = (12 << 11) | (34 << 5) | (56 / 2);

struct B {
    img: Vec<u8>,
    fat: Vec<u32>,
    pool: Vec<u32>,
    prox: usize,
    dados_ini: usize,
    bytes_cluster: usize,
    n_sfn: u32,
}

fn sfn_valido(n: &str) -> Option<[u8; 11]> {
    let (base, ext) = match n.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (n, ""),
    };
    let ok = |s: &str| {
        s.bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 || !ok(base) || !ok(ext) {
        return None;
    }
    let mut r = [b' '; 11];
    r[..base.len()].copy_from_slice(base.as_bytes());
    r[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Some(r)
}

fn checksum(s: &[u8; 11]) -> u8 {
    s.iter()
        .fold(0u8, |a, &c| a.rotate_right(1).wrapping_add(c))
}

impl B {
    fn alloc(&mut self, n: usize) -> Vec<u32> {
        let n = n.max(1);
        let chain: Vec<u32> = self.pool[self.prox..self.prox + n].to_vec();
        self.prox += n;
        for w in chain.windows(2) {
            self.fat[w[0] as usize] = w[1];
        }
        self.fat[*chain.last().unwrap() as usize] = 0x0FFF_FFFF;
        chain
    }
    fn escrever(&mut self, chain: &[u32], d: &[u8]) {
        for (i, &c) in chain.iter().enumerate() {
            let off = self.dados_ini + (c as usize - 2) * self.bytes_cluster;
            let ini = i * self.bytes_cluster;
            if ini < d.len() {
                let fim = (ini + self.bytes_cluster).min(d.len());
                self.img[off..off + (fim - ini)].copy_from_slice(&d[ini..fim]);
            }
        }
    }
    fn tamanho_dir(entradas: &[(String, No)]) -> usize {
        let mut n = 2; // . e ..
        for (nome, _) in entradas {
            n += 1;
            if sfn_valido(nome).is_none() {
                n += nome.encode_utf16().count().div_ceil(13);
            }
        }
        n * 32
    }
    fn dir(&mut self, entradas: &[(String, No)], chain: &[u32], pai: u32, raiz: bool) {
        let mut out: Vec<u8> = Vec::new();
        let mut ent = |nome: &[u8; 11], attr: u8, cl: u32, tam: u32| {
            let mut e = [0u8; 32];
            e[..11].copy_from_slice(nome);
            e[11] = attr;
            e[22..24].copy_from_slice(&HORA.to_le_bytes());
            e[24..26].copy_from_slice(&DATA.to_le_bytes());
            e[20..22].copy_from_slice(&((cl >> 16) as u16).to_le_bytes());
            e[26..28].copy_from_slice(&(cl as u16).to_le_bytes());
            e[28..32].copy_from_slice(&tam.to_le_bytes());
            e
        };
        if !raiz {
            out.extend_from_slice(&ent(b".          ", 0x10, chain[0], 0));
            out.extend_from_slice(&ent(b"..         ", 0x10, pai, 0));
        }
        for (nome, no) in entradas {
            let (attr, cl, tam) = match no {
                No::Arq(d) => {
                    let n = d.len().div_ceil(self.bytes_cluster);
                    if d.is_empty() {
                        (0x20u8, 0u32, 0u32)
                    } else {
                        let ch = self.alloc(n);
                        self.escrever(&ch, d);
                        (0x20, ch[0], d.len() as u32)
                    }
                }
                No::Dir(f) => {
                    let n = Self::tamanho_dir(f).div_ceil(self.bytes_cluster);
                    let ch = self.alloc(n);
                    let c0 = ch[0];
                    self.dir(f, &ch, if raiz { 0 } else { chain[0] }, false);
                    (0x10, c0, 0)
                }
                No::Link(_) => panic!("FAT não tem links"),
            };
            let (curto, lfn) = match sfn_valido(nome) {
                Some(s) => (s, false),
                None => {
                    self.n_sfn += 1;
                    let mut s = [b' '; 11];
                    let b = format!("LF{:06}", self.n_sfn);
                    s[..8].copy_from_slice(b.as_bytes());
                    (s, true)
                }
            };
            if lfn {
                let mut u: Vec<u16> = nome.encode_utf16().collect();
                u.push(0);
                while u.len() % 13 != 0 {
                    u.push(0xFFFF);
                }
                let total = u.len() / 13;
                let ck = checksum(&curto);
                for k in (0..total).rev() {
                    let mut e = [0u8; 32];
                    e[0] = (k as u8 + 1) | if k == total - 1 { 0x40 } else { 0 };
                    e[11] = 0x0F;
                    e[13] = ck;
                    let pos = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
                    for (i, p) in pos.iter().enumerate() {
                        e[*p..*p + 2].copy_from_slice(&u[k * 13 + i].to_le_bytes());
                    }
                    out.extend_from_slice(&e);
                }
            }
            out.extend_from_slice(&ent(&curto, attr, cl, tam));
        }
        self.escrever(chain, &out);
    }
}

/// Monta uma imagem FAT. `bits` = 12, 16 ou 32; `frag` espalha os clusters.
pub fn fat_img(bits: u8, arv: &[(String, No)], frag: bool) -> Vec<u8> {
    let (spc, total, reservados, fatsz, raiz_ent): (usize, usize, usize, usize, usize) = match bits
    {
        12 => (1, 2880, 1, 9, 224),
        16 => (4, 40000, 1, 40, 512),
        _ => (1, 70000, 32, 548, 0),
    };
    let raiz_set = raiz_ent * 32 / 512;
    let dados_set = reservados + 2 * fatsz + raiz_set;
    let n_clusters = (total - dados_set) / spc;
    let mut img = vec![0u8; total * 512];
    img[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    img[3..11].copy_from_slice(b"MSDOS5.0");
    img[11..13].copy_from_slice(&512u16.to_le_bytes());
    img[13] = spc as u8;
    img[14..16].copy_from_slice(&(reservados as u16).to_le_bytes());
    img[16] = 2;
    img[17..19].copy_from_slice(&(raiz_ent as u16).to_le_bytes());
    if total < 65536 && bits != 32 {
        img[19..21].copy_from_slice(&(total as u16).to_le_bytes());
    } else {
        img[32..36].copy_from_slice(&(total as u32).to_le_bytes());
    }
    img[21] = 0xF8;
    if bits == 32 {
        img[36..40].copy_from_slice(&(fatsz as u32).to_le_bytes());
        img[44..48].copy_from_slice(&2u32.to_le_bytes());
        img[82..90].copy_from_slice(b"FAT32   ");
    } else {
        img[22..24].copy_from_slice(&(fatsz as u16).to_le_bytes());
    }
    img[510] = 0x55;
    img[511] = 0xAA;
    let mut pool: Vec<u32> = (2..2 + n_clusters as u32).collect();
    if frag {
        let (pares, impares): (Vec<u32>, Vec<u32>) = pool.iter().partition(|c| **c % 2 == 0);
        pool = pares.into_iter().chain(impares).collect();
    }
    let mut b = B {
        img,
        fat: vec![0; n_clusters + 2],
        pool,
        prox: 0,
        dados_ini: dados_set * 512,
        bytes_cluster: spc * 512,
        n_sfn: 0,
    };
    b.fat[0] = 0x0FFF_FFF8;
    b.fat[1] = 0x0FFF_FFFF;
    if bits == 32 {
        // raiz no cluster 2 (primeiro do pool, também sem frag)
        let ch = b.alloc(B::tamanho_dir(arv).div_ceil(b.bytes_cluster));
        assert_eq!(ch[0], 2);
        let raiz = ch[0];
        b.img[44..48].copy_from_slice(&raiz.to_le_bytes());
        b.dir(arv, &ch, 0, true);
    } else {
        let tam_raiz = B::tamanho_dir(arv);
        assert!(tam_raiz <= raiz_ent * 32, "raiz grande demais");
        raiz_fixa(&mut b, arv, reservados + 2 * fatsz, raiz_ent);
    }
    // grava as FATs
    let ini = reservados * 512;
    let mut tabela = vec![0u8; fatsz * 512];
    for (c, &v) in b.fat.iter().enumerate() {
        match bits {
            12 => {
                let off = c + c / 2;
                let v = v & 0xFFF;
                if c % 2 == 0 {
                    tabela[off] = v as u8;
                    tabela[off + 1] = (tabela[off + 1] & 0xF0) | (v >> 8) as u8;
                } else {
                    tabela[off] = (tabela[off] & 0x0F) | ((v << 4) as u8);
                    tabela[off + 1] = (v >> 4) as u8;
                }
            }
            16 => {
                let v = if v >= 0x0FFF_FFF8 { 0xFFFF } else { v };
                tabela[c * 2..c * 2 + 2].copy_from_slice(&(v as u16).to_le_bytes());
            }
            _ => tabela[c * 4..c * 4 + 4].copy_from_slice(&v.to_le_bytes()),
        }
    }
    b.img[ini..ini + tabela.len()].copy_from_slice(&tabela);
    let ini2 = ini + tabela.len();
    b.img[ini2..ini2 + tabela.len()].copy_from_slice(&tabela);
    b.img
}

/// Raiz fixa FAT12/16: constrói as entradas como um diretório comum e copia.
fn raiz_fixa(b: &mut B, arv: &[(String, No)], setor: usize, ent: usize) {
    // Usa um buffer de "cluster" virtual: capturamos o que `dir` escreveria.
    let bytes = ent * 32;
    let n = bytes.div_ceil(b.bytes_cluster);
    // Reserva clusters ao fim do pool só para ter onde escrever, depois copia.
    let ch = {
        let salvo = b.prox;
        let ch = b.alloc(n);
        // desfaz a marcação na FAT
        for &c in &ch {
            b.fat[c as usize] = 0;
        }
        b.prox = salvo;
        ch
    };
    // Reserva de novo para que arquivos não reutilizem esses clusters temporários.
    let mut tmp = B {
        img: std::mem::take(&mut b.img),
        fat: std::mem::take(&mut b.fat),
        pool: std::mem::take(&mut b.pool),
        prox: b.prox + n,
        dados_ini: b.dados_ini,
        bytes_cluster: b.bytes_cluster,
        n_sfn: 0,
    };
    tmp.dir(arv, &ch, 0, true);
    // copia do cluster temporário para a raiz fixa
    let mut buf = vec![0u8; bytes];
    for (i, &c) in ch.iter().enumerate() {
        let off = tmp.dados_ini + (c as usize - 2) * tmp.bytes_cluster;
        let ini = i * tmp.bytes_cluster;
        let fim = (ini + tmp.bytes_cluster).min(bytes);
        if ini < fim {
            buf[ini..fim].copy_from_slice(&tmp.img[off..off + (fim - ini)]);
        }
        for x in &mut tmp.img[off..off + tmp.bytes_cluster] {
            *x = 0;
        }
    }
    tmp.img[setor * 512..setor * 512 + bytes].copy_from_slice(&buf);
    b.img = tmp.img;
    b.fat = tmp.fat;
    b.pool = tmp.pool;
    b.prox = tmp.prox;
}
