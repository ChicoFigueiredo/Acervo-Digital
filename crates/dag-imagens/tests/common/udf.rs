//! Gerador UDF 1.02 mínimo para testes (File Entry/EFE, short/long/embedded ADs).

use super::No;

const S: usize = 2048;
const PART_INI: usize = 272;

struct B {
    img: Vec<u8>,
    prox: u32,
    frag: bool,
    n: usize,
}

fn tag(id: u16, loc: u32) -> Vec<u8> {
    let mut t = vec![0u8; 16];
    t[..2].copy_from_slice(&id.to_le_bytes());
    t[2..4].copy_from_slice(&3u16.to_le_bytes());
    t[12..16].copy_from_slice(&loc.to_le_bytes());
    t
}

fn ts() -> [u8; 12] {
    let mut t = [0u8; 12];
    t[..2].copy_from_slice(&0x1000u16.to_le_bytes());
    t[2..4].copy_from_slice(&2024u16.to_le_bytes());
    t[4] = 3;
    t[5] = 15;
    t[6] = 12;
    t[7] = 34;
    t[8] = 56;
    t
}

impl B {
    fn off(&self, lbn: u32) -> usize {
        (PART_INI + lbn as usize) * S
    }
    fn alloc(&mut self, n: usize) -> u32 {
        let l = self.prox;
        self.prox += n as u32;
        let fim = self.off(self.prox);
        if self.img.len() < fim {
            self.img.resize(fim, 0);
        }
        l
    }
    fn put(&mut self, lbn: u32, d: &[u8]) {
        let o = self.off(lbn);
        if self.img.len() < o + d.len() {
            self.img.resize(o + d.len(), 0);
        }
        self.img[o..o + d.len()].copy_from_slice(d);
    }

    /// Cria extents para `d`; devolve (lista de (len, lbn)).
    fn dados(&mut self, d: &[u8]) -> Vec<(u32, u32)> {
        let n = d.len().div_ceil(S);
        let mut ads = Vec::new();
        let mut i = 0usize;
        while i < n {
            let k = if self.frag { (n - i).min(2) } else { n - i };
            let l = self.alloc(k + usize::from(self.frag));
            let ini = i * S;
            let fim = ((i + k) * S).min(d.len());
            self.put(l, &d[ini..fim]);
            ads.push(((fim - ini) as u32, l));
            i += k;
        }
        ads
    }

    fn fe(&mut self, tipo: u8, d: &[u8], eh_dir: bool) -> u32 {
        self.n += 1;
        let efe = self.n % 2 == 0;
        let (id, ad_len_off, ea_ini, mt_off) = if efe {
            (266u16, 212usize, 216usize, 92usize)
        } else {
            (261, 172, 176, 84)
        };
        let lbn = self.alloc(1);
        let mut b = vec![0u8; S];
        b[..16].copy_from_slice(&tag(id, lbn));
        b[20..22].copy_from_slice(&4u16.to_le_bytes());
        b[27] = tipo;
        let embutir = d.len() <= 100 && (eh_dir || self.n % 3 == 0) && tipo != 12;
        let mut ads: Vec<u8> = Vec::new();
        let flags: u16;
        if embutir {
            flags = 3;
            ads.extend_from_slice(d);
        } else {
            let lista = self.dados(d);
            if self.n % 3 == 1 {
                flags = 1; // long_ad
                for (len, l) in lista {
                    let mut a = [0u8; 16];
                    a[..4].copy_from_slice(&len.to_le_bytes());
                    a[4..8].copy_from_slice(&l.to_le_bytes());
                    ads.extend_from_slice(&a);
                }
            } else {
                flags = 0;
                for (len, l) in lista {
                    ads.extend_from_slice(&len.to_le_bytes());
                    ads.extend_from_slice(&l.to_le_bytes());
                }
            }
        }
        b[34..36].copy_from_slice(&flags.to_le_bytes());
        b[56..64].copy_from_slice(&(d.len() as u64).to_le_bytes());
        b[mt_off..mt_off + 12].copy_from_slice(&ts());
        b[ad_len_off..ad_len_off + 4].copy_from_slice(&(ads.len() as u32).to_le_bytes());
        b[ea_ini..ea_ini + ads.len()].copy_from_slice(&ads);
        self.put(lbn, &b);
        lbn
    }

    fn fid(nome: &str, lbn: u32, dir: bool, parent: bool) -> Vec<u8> {
        let ident: Vec<u8> = if parent {
            vec![]
        } else if nome.chars().all(|c| (c as u32) < 256) {
            let mut v = vec![8u8];
            v.extend(nome.chars().map(|c| c as u8));
            v
        } else {
            let mut v = vec![16u8];
            for c in nome.encode_utf16() {
                v.extend_from_slice(&c.to_be_bytes());
            }
            v
        };
        let len = (38 + ident.len()).div_ceil(4) * 4;
        let mut f = vec![0u8; len];
        f[..16].copy_from_slice(&tag(257, 0));
        f[16..18].copy_from_slice(&1u16.to_le_bytes());
        f[18] = if parent {
            0x0A
        } else if dir {
            0x02
        } else {
            0
        };
        f[19] = ident.len() as u8;
        f[20..24].copy_from_slice(&(S as u32).to_le_bytes());
        f[24..28].copy_from_slice(&lbn.to_le_bytes());
        f[38..38 + ident.len()].copy_from_slice(&ident);
        f
    }

    fn dir(&mut self, entradas: &[(String, No)], pai: u32) -> u32 {
        // reserva o ICB do diretório depois dos filhos: precisamos dos LBNs dos filhos
        let mut fids: Vec<u8> = Self::fid("", pai, true, true);
        for (nome, no) in entradas {
            match no {
                No::Arq(d) => {
                    let l = self.fe(5, d, false);
                    fids.extend(Self::fid(nome, l, false, false));
                }
                No::Link(t) => {
                    let mut comp = Vec::new();
                    for p in t.split('/') {
                        comp.extend_from_slice(&[5, (p.len() + 1) as u8, 0, 0, 8]);
                        comp.extend_from_slice(p.as_bytes());
                    }
                    let l = self.fe(12, &comp, false);
                    fids.extend(Self::fid(nome, l, false, false));
                }
                No::Dir(f) => {
                    // o ICB do filho precisa saber o pai: usamos um LBN provisório 0
                    let l = self.dir(f, 0);
                    fids.extend(Self::fid(nome, l, true, false));
                }
            }
        }
        self.fe(4, &fids, true)
    }
}

/// Gera uma imagem UDF (sem ISO 9660). `frag` fragmenta os extents.
pub fn udf_img(arv: &[(String, No)], frag: bool) -> Vec<u8> {
    let mut b = B {
        img: vec![0u8; (PART_INI + 2) * S],
        prox: 0,
        frag,
        n: 0,
    };
    b.alloc(1); // lbn 0: FSD
    let raiz = b.dir(arv, 0);
    // FSD
    let mut fsd = vec![0u8; S];
    fsd[..16].copy_from_slice(&tag(256, 0));
    fsd[400..404].copy_from_slice(&(S as u32).to_le_bytes());
    fsd[404..408].copy_from_slice(&raiz.to_le_bytes());
    b.put(0, &fsd);
    let total = b.img.len() / S;
    // reconhecimento de volume
    for (i, id) in [(16usize, b"BEA01"), (17, b"NSR02"), (18, b"TEA01")] {
        b.img[i * S + 1..i * S + 6].copy_from_slice(id);
        b.img[i * S + 6] = 1;
    }
    // AVDP
    let mut avdp = tag(2, 256);
    avdp.resize(S, 0);
    avdp[16..20].copy_from_slice(&((6 * S) as u32).to_le_bytes());
    avdp[20..24].copy_from_slice(&32u32.to_le_bytes());
    b.img[256 * S..257 * S].copy_from_slice(&avdp);
    // PD
    let mut pd = tag(5, 32);
    pd.resize(S, 0);
    pd[22..24].copy_from_slice(&0u16.to_le_bytes());
    pd[188..192].copy_from_slice(&(PART_INI as u32).to_le_bytes());
    pd[192..196].copy_from_slice(&((total - PART_INI) as u32).to_le_bytes());
    b.img[32 * S..33 * S].copy_from_slice(&pd);
    // LVD
    let mut lvd = tag(6, 33);
    lvd.resize(S, 0);
    lvd[212..216].copy_from_slice(&(S as u32).to_le_bytes());
    lvd[248..252].copy_from_slice(&(S as u32).to_le_bytes());
    lvd[252..256].copy_from_slice(&0u32.to_le_bytes());
    lvd[264..268].copy_from_slice(&6u32.to_le_bytes());
    lvd[268..272].copy_from_slice(&1u32.to_le_bytes());
    lvd[440..446].copy_from_slice(&[1, 6, 1, 0, 0, 0]);
    b.img[33 * S..34 * S].copy_from_slice(&lvd);
    // TD
    let td = tag(8, 34);
    b.img[34 * S..34 * S + 16].copy_from_slice(&td);
    b.img
}
