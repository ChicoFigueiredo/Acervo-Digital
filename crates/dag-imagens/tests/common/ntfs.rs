//! Formatador NTFS mínimo para testes (MFT, $I30 com INDX, runs fragmentados/esparsos).

use super::No;
use std::collections::HashMap;

const CL: usize = 4096;
const REC: usize = 1024;
const N_REC: usize = 256;
const MFT_LCN: usize = 16;
const DATA_LCN: usize = 96;
const TOTAL_CL: usize = 4096;
const FT: u64 = (1_710_506_096 + 11_644_473_600) * 10_000_000;

pub struct Opc {
    pub frag: bool,
    /// (caminho existente, novo nome na raiz): cria um hard link.
    pub links: Vec<(String, String)>,
}

fn nome_utf16(n: &str) -> Vec<u8> {
    n.encode_utf16().flat_map(|c| c.to_le_bytes()).collect()
}

fn fname(pai: u64, nome: &str, dir: bool, tam: u64) -> Vec<u8> {
    let mut v = vec![0u8; 66];
    v[..8].copy_from_slice(&(pai | (1 << 48)).to_le_bytes());
    for i in 0..4 {
        v[8 + i * 8..16 + i * 8].copy_from_slice(&FT.to_le_bytes());
    }
    v[40..48].copy_from_slice(&tam.to_le_bytes());
    v[48..56].copy_from_slice(&tam.to_le_bytes());
    let fl: u32 = if dir { 0x1000_0000 } else { 0x20 };
    v[56..60].copy_from_slice(&fl.to_le_bytes());
    v[64] = nome.encode_utf16().count() as u8;
    v[65] = 1;
    v.extend_from_slice(&nome_utf16(nome));
    v
}

fn alinha8(v: &mut Vec<u8>) {
    while v.len() % 8 != 0 {
        v.push(0);
    }
}

fn attr_res(tipo: u32, nome: &str, valor: &[u8], id: u16) -> Vec<u8> {
    let nm = nome_utf16(nome);
    let vo = (24 + nm.len()).div_ceil(8) * 8;
    let len = (vo + valor.len()).div_ceil(8) * 8;
    let mut a = vec![0u8; len];
    a[..4].copy_from_slice(&tipo.to_le_bytes());
    a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    a[9] = nome.encode_utf16().count() as u8;
    a[10..12].copy_from_slice(&24u16.to_le_bytes());
    a[14..16].copy_from_slice(&id.to_le_bytes());
    a[16..20].copy_from_slice(&(valor.len() as u32).to_le_bytes());
    a[20..22].copy_from_slice(&(vo as u16).to_le_bytes());
    a[24..24 + nm.len()].copy_from_slice(&nm);
    a[vo..vo + valor.len()].copy_from_slice(valor);
    a
}

fn runlist(runs: &[(u64, Option<u64>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut ant: i64 = 0;
    for &(len, lcn) in runs {
        let lb = len.to_le_bytes();
        let nl = 8 - lb.iter().rev().take_while(|&&b| b == 0).count();
        let nl = nl.max(1);
        match lcn {
            None => {
                out.push(nl as u8);
                out.extend_from_slice(&lb[..nl]);
            }
            Some(l) => {
                let d = l as i64 - ant;
                ant = l as i64;
                let db = d.to_le_bytes();
                let mut no = 8;
                while no > 1
                    && ((db[no - 1] == 0 && db[no - 2] & 0x80 == 0)
                        || (db[no - 1] == 0xFF && db[no - 2] & 0x80 != 0))
                {
                    no -= 1;
                }
                out.push((no as u8) << 4 | nl as u8);
                out.extend_from_slice(&lb[..nl]);
                out.extend_from_slice(&db[..no]);
            }
        }
    }
    out.push(0);
    out
}

#[allow(clippy::too_many_arguments)]
fn attr_nres(
    tipo: u32,
    nome: &str,
    runs: &[(u64, Option<u64>)],
    aloc: u64,
    tam: u64,
    init: u64,
    flags: u16,
    id: u16,
) -> Vec<u8> {
    let nm = nome_utf16(nome);
    let ro = (64 + nm.len()).div_ceil(8) * 8;
    let rl = runlist(runs);
    let len = (ro + rl.len()).div_ceil(8) * 8;
    let mut a = vec![0u8; len];
    a[..4].copy_from_slice(&tipo.to_le_bytes());
    a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    a[8] = 1;
    a[9] = nome.encode_utf16().count() as u8;
    a[10..12].copy_from_slice(&64u16.to_le_bytes());
    a[12..14].copy_from_slice(&flags.to_le_bytes());
    a[14..16].copy_from_slice(&id.to_le_bytes());
    let total_cl: u64 = runs.iter().map(|r| r.0).sum();
    a[24..32].copy_from_slice(&total_cl.saturating_sub(1).to_le_bytes());
    a[32..34].copy_from_slice(&(ro as u16).to_le_bytes());
    a[40..48].copy_from_slice(&aloc.to_le_bytes());
    a[48..56].copy_from_slice(&tam.to_le_bytes());
    a[56..64].copy_from_slice(&init.to_le_bytes());
    a[64..64 + nm.len()].copy_from_slice(&nm);
    a[ro..ro + rl.len()].copy_from_slice(&rl);
    a
}

fn registro(n: usize, flags: u16, attrs: &[Vec<u8>]) -> Vec<u8> {
    let mut r = vec![0u8; REC];
    r[..4].copy_from_slice(b"FILE");
    r[4..6].copy_from_slice(&48u16.to_le_bytes());
    r[6..8].copy_from_slice(&3u16.to_le_bytes());
    r[16..18].copy_from_slice(&1u16.to_le_bytes());
    r[20..22].copy_from_slice(&56u16.to_le_bytes());
    r[22..24].copy_from_slice(&flags.to_le_bytes());
    r[28..32].copy_from_slice(&(REC as u32).to_le_bytes());
    r[44..48].copy_from_slice(&(n as u32).to_le_bytes());
    let mut p = 56;
    for a in attrs {
        assert!(p + a.len() + 8 <= REC, "registro {n} cheio");
        r[p..p + a.len()].copy_from_slice(a);
        p += a.len();
    }
    r[p..p + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    r[24..28].copy_from_slice(&((p + 8) as u32).to_le_bytes());
    fixups(&mut r, 48);
    r
}

fn fixups(b: &mut [u8], off: usize) {
    let usn: u16 = 0x0007;
    b[off..off + 2].copy_from_slice(&usn.to_le_bytes());
    for i in 1..=b.len() / 512 {
        let fim = i * 512;
        let (x, y) = (b[fim - 2], b[fim - 1]);
        b[off + i * 2] = x;
        b[off + i * 2 + 1] = y;
        b[fim - 2..fim].copy_from_slice(&usn.to_le_bytes());
    }
}

fn entrada(rec: u64, nome: &str, dir: bool, tam: u64, pai: u64) -> Vec<u8> {
    let key = fname(pai, nome, dir, tam);
    let len = (16 + key.len()).div_ceil(8) * 8;
    let mut e = vec![0u8; len];
    e[..8].copy_from_slice(&(rec | (1 << 48)).to_le_bytes());
    e[8..10].copy_from_slice(&(len as u16).to_le_bytes());
    e[10..12].copy_from_slice(&(key.len() as u16).to_le_bytes());
    e[16..16 + key.len()].copy_from_slice(&key);
    e
}

fn entrada_final(com_filhos: bool) -> Vec<u8> {
    let len = if com_filhos { 24 } else { 16 };
    let mut e = vec![0u8; len];
    e[8..10].copy_from_slice(&(len as u16).to_le_bytes());
    e[12..14].copy_from_slice(&(if com_filhos { 3u16 } else { 2 }).to_le_bytes());
    e
}

fn bloco_indx(vcn: u64, entradas: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; CL];
    b[..4].copy_from_slice(b"INDX");
    b[4..6].copy_from_slice(&40u16.to_le_bytes());
    b[6..8].copy_from_slice(&9u16.to_le_bytes());
    b[16..24].copy_from_slice(&vcn.to_le_bytes());
    let mut p = 64;
    for e in entradas
        .iter()
        .chain(std::iter::once(&entrada_final(false)))
    {
        b[p..p + e.len()].copy_from_slice(e);
        p += e.len();
    }
    assert!(p <= CL);
    b[24..28].copy_from_slice(&40u32.to_le_bytes());
    b[28..32].copy_from_slice(&((p - 24) as u32).to_le_bytes());
    b[32..36].copy_from_slice(&((CL - 24) as u32).to_le_bytes());
    fixups(&mut b, 40);
    b
}

struct B {
    img: Vec<u8>,
    prox: usize,
    frag: bool,
    regs: Vec<Vec<u8>>,
    rec_de: HashMap<String, usize>,
    tam_de: HashMap<usize, u64>,
    n_rec: usize,
    links: Vec<(String, String)>,
}

impl B {
    fn clusters(&mut self, n: usize) -> Vec<(u64, Option<u64>)> {
        let mut runs = Vec::new();
        let mut falta = n;
        while falta > 0 {
            let k = if self.frag { falta.min(2) } else { falta };
            runs.push((k as u64, Some(self.prox as u64)));
            self.prox += k + usize::from(self.frag);
            falta -= k;
        }
        assert!(self.prox < TOTAL_CL);
        runs
    }

    fn escrever(&mut self, runs: &[(u64, Option<u64>)], d: &[u8], fora: u8) {
        let mut pos = 0usize;
        for &(len, lcn) in runs {
            let Some(l) = lcn else {
                pos += len as usize * CL;
                continue;
            };
            let off = l as usize * CL;
            for i in 0..len as usize * CL {
                self.img[off + i] = d.get(pos + i).copied().unwrap_or(fora);
            }
            pos += len as usize * CL;
        }
    }

    fn atribs_arquivo(&mut self, nome: &str, rec: usize, pai: usize, d: &[u8]) -> Vec<Vec<u8>> {
        let mut attrs = vec![
            attr_res(0x10, "", &[0u8; 72].map(|_| 0), 0),
            attr_res(0x30, "", &fname(pai as u64, nome, false, d.len() as u64), 1),
        ];
        let mut std = vec![0u8; 72];
        for i in 0..4 {
            std[i * 8..i * 8 + 8].copy_from_slice(&FT.to_le_bytes());
        }
        attrs[0] = attr_res(0x10, "", &std, 0);
        let especial = nome.starts_with("esparso")
            || nome.starts_with("init")
            || nome.starts_with("comprimido");
        if d.len() <= 600 && !especial {
            attrs.push(attr_res(0x80, "", d, 2));
        } else {
            let n = d.len().div_ceil(CL).max(1);
            let (mut runs, init, flags);
            if nome.starts_with("esparso") {
                // clusters totalmente zerados viram runs esparsos
                let mut lista: Vec<(u64, Option<u64>)> = Vec::new();
                let mut dados_concat = Vec::new();
                for c in 0..n {
                    let ch = &d[c * CL..((c + 1) * CL).min(d.len())];
                    if ch.iter().all(|&x| x == 0) {
                        lista.push((1, None));
                    } else {
                        let r = self.clusters(1);
                        lista.push((1, r[0].1));
                        dados_concat.extend_from_slice(ch);
                        dados_concat.resize(dados_concat.len().div_ceil(CL) * CL, 0);
                    }
                }
                // escreve cada cluster alocado
                let mut k = 0usize;
                for (i, &(_, l)) in lista.iter().enumerate() {
                    if let Some(l) = l {
                        let ch = &d[i * CL..((i + 1) * CL).min(d.len())];
                        let off = l as usize * CL;
                        self.img[off..off + ch.len()].copy_from_slice(ch);
                        k += 1;
                    }
                }
                let _ = k;
                // funde runs adjacentes iguais
                runs = Vec::new();
                for r in lista {
                    if let Some(u) = runs.last_mut() {
                        let u: &mut (u64, Option<u64>) = u;
                        match (u.1, r.1) {
                            (None, None) => {
                                u.0 += 1;
                                continue;
                            }
                            (Some(a), Some(b)) if a + u.0 == b => {
                                u.0 += 1;
                                continue;
                            }
                            _ => {}
                        }
                    }
                    runs.push(r);
                }
                init = d.len() as u64;
                flags = 0x8000u16;
            } else {
                runs = self.clusters(n);
                if nome.starts_with("init") {
                    init = (d.len() / 2) as u64;
                    self.escrever(&runs, &d[..d.len() / 2], 0xAA);
                    flags = 0;
                } else {
                    init = d.len() as u64;
                    self.escrever(&runs, d, 0);
                    flags = if nome.starts_with("comprimido") { 1 } else { 0 };
                }
            }
            let _ = &mut runs;
            attrs.push(attr_nres(
                0x80,
                "",
                &runs,
                (n * CL) as u64,
                d.len() as u64,
                init,
                flags,
                2,
            ));
        }
        let _ = rec;
        attrs
    }

    fn dir(&mut self, caminho: &str, entradas: &[(String, No)], rec: usize, pai: usize) {
        // atribui números
        let mut filhos: Vec<(String, usize, bool, u64)> = Vec::new();
        for (nome, no) in entradas {
            let p = if caminho.is_empty() {
                nome.clone()
            } else {
                format!("{caminho}/{nome}")
            };
            let r = self.n_rec;
            self.n_rec += 1;
            self.rec_de.insert(p.clone(), r);
            let (dir, tam) = match no {
                No::Arq(d) => (false, d.len() as u64),
                No::Dir(_) => (true, 0),
                No::Link(_) => panic!("sem links no NTFS de teste"),
            };
            self.tam_de.insert(r, tam);
            filhos.push((nome.clone(), r, dir, tam));
        }
        // registros dos filhos
        for ((nome, no), (_, r, _, _)) in entradas.iter().zip(filhos.clone()) {
            let p = if caminho.is_empty() {
                nome.clone()
            } else {
                format!("{caminho}/{nome}")
            };
            match no {
                No::Arq(d) => {
                    let attrs = self.atribs_arquivo(nome, r, rec, d);
                    self.regs[r] = registro(r, 1, &attrs);
                }
                No::Dir(f) => self.dir(&p, f, r, rec),
                No::Link(_) => unreachable!(),
            }
        }
        // links extras (somente na raiz)
        if caminho.is_empty() {
            for (de, novo) in self.links.clone() {
                let r = self.rec_de[&de];
                filhos.push((novo, r, false, self.tam_de[&r]));
            }
        }
        filhos.sort_by_key(|f| f.0.to_uppercase());
        let ents: Vec<Vec<u8>> = filhos
            .iter()
            .map(|(n, r, d, t)| entrada(*r as u64, n, *d, *t, rec as u64))
            .collect();
        let total: usize = ents.iter().map(Vec::len).sum();
        let mut attrs = vec![attr_res(
            0x10,
            "",
            &{
                let mut s = vec![0u8; 72];
                for i in 0..4 {
                    s[i * 8..i * 8 + 8].copy_from_slice(&FT.to_le_bytes());
                }
                s
            },
            0,
        )];
        let nome_dir = caminho.rsplit('/').next().unwrap_or("");
        attrs.push(attr_res(
            0x30,
            "",
            &fname(pai as u64, if rec == 5 { "." } else { nome_dir }, true, 0),
            1,
        ));
        let mut raiz = vec![0u8; 16];
        raiz[..4].copy_from_slice(&0x30u32.to_le_bytes());
        raiz[4..8].copy_from_slice(&1u32.to_le_bytes());
        raiz[8..12].copy_from_slice(&(CL as u32).to_le_bytes());
        raiz[12] = 1;
        if total <= 400 {
            let mut ent = ents.concat();
            ent.extend_from_slice(&entrada_final(false));
            let mut nh = vec![0u8; 16];
            nh[..4].copy_from_slice(&16u32.to_le_bytes());
            nh[4..8].copy_from_slice(&((16 + ent.len()) as u32).to_le_bytes());
            nh[8..12].copy_from_slice(&((16 + ent.len()) as u32).to_le_bytes());
            raiz.extend_from_slice(&nh);
            raiz.extend_from_slice(&ent);
            attrs.push(attr_res(0x90, "$I30", &raiz, 2));
        } else {
            let ent = entrada_final(true);
            let mut nh = vec![0u8; 16];
            nh[..4].copy_from_slice(&16u32.to_le_bytes());
            nh[4..8].copy_from_slice(&((16 + ent.len()) as u32).to_le_bytes());
            nh[8..12].copy_from_slice(&((16 + ent.len()) as u32).to_le_bytes());
            nh[12] = 1;
            raiz.extend_from_slice(&nh);
            raiz.extend_from_slice(&ent);
            attrs.push(attr_res(0x90, "$I30", &raiz, 2));
            // blocos INDX
            let mut blocos: Vec<Vec<u8>> = Vec::new();
            let mut atual: Vec<Vec<u8>> = Vec::new();
            let mut usado = 0usize;
            for e in ents {
                if usado + e.len() > 3800 {
                    blocos.push(bloco_indx(blocos.len() as u64, &atual));
                    atual.clear();
                    usado = 0;
                }
                usado += e.len();
                atual.push(e);
            }
            blocos.push(bloco_indx(blocos.len() as u64, &atual));
            let n_usados = blocos.len();
            // bloco obsoleto, fora do bitmap, com entrada falsa
            blocos.push(bloco_indx(
                n_usados as u64,
                &[entrada(9999, "fantasma.txt", false, 1, rec as u64)],
            ));
            let dados = blocos.concat();
            let runs = self.clusters(blocos.len());
            self.escrever(&runs, &dados, 0);
            attrs.push(attr_nres(
                0xA0,
                "$I30",
                &runs,
                dados.len() as u64,
                dados.len() as u64,
                dados.len() as u64,
                0,
                3,
            ));
            let mut bm = vec![0u8; 8];
            for i in 0..n_usados {
                bm[i / 8] |= 1 << (i % 8);
            }
            attrs.push(attr_res(0xB0, "$I30", &bm, 4));
        }
        self.regs[rec] = registro(rec, 3, &attrs);
    }
}

/// Monta uma imagem NTFS de 16 MiB com a árvore dada.
pub fn ntfs_img(arv: &[(String, No)], opc: &Opc) -> Vec<u8> {
    let mut b = B {
        img: vec![0u8; TOTAL_CL * CL],
        prox: DATA_LCN,
        frag: opc.frag,
        regs: vec![vec![0u8; REC]; N_REC],
        rec_de: HashMap::new(),
        tam_de: HashMap::new(),
        n_rec: 24,
        links: opc.links.clone(),
    };
    b.dir("", arv, 5, 5);
    // $MFT
    let mft_cl = N_REC * REC / CL;
    let attrs = vec![
        attr_res(0x30, "", &fname(5, "$MFT", false, (N_REC * REC) as u64), 0),
        attr_nres(
            0x80,
            "",
            &[(mft_cl as u64, Some(MFT_LCN as u64))],
            (mft_cl * CL) as u64,
            (N_REC * REC) as u64,
            (N_REC * REC) as u64,
            0,
            1,
        ),
    ];
    b.regs[0] = registro(0, 1, &attrs);
    for (i, r) in b.regs.iter().enumerate() {
        let off = MFT_LCN * CL + i * REC;
        b.img[off..off + REC].copy_from_slice(r);
    }
    let img = &mut b.img;
    img[..3].copy_from_slice(&[0xEB, 0x52, 0x90]);
    img[3..11].copy_from_slice(b"NTFS    ");
    img[11..13].copy_from_slice(&512u16.to_le_bytes());
    img[13] = (CL / 512) as u8;
    img[21] = 0xF8;
    img[40..48].copy_from_slice(&((TOTAL_CL * CL / 512 - 1) as u64).to_le_bytes());
    img[48..56].copy_from_slice(&(MFT_LCN as u64).to_le_bytes());
    img[64] = 0xF6;
    img[68] = 1;
    img[510] = 0x55;
    img[511] = 0xAA;
    b.img
}
