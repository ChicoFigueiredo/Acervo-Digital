//! Construtores de VHDX e qcow2 para testes, a partir de discos esparsos.

use flate2::Compression;
use flate2::write::DeflateEncoder;
use std::collections::BTreeMap;
use std::io::Write;

/// Disco virtual esparso: tamanho total e trechos com dados.
pub struct Esparso {
    pub tam: u64,
    pub segs: Vec<(u64, Vec<u8>)>,
}

impl Esparso {
    pub fn denso(d: Vec<u8>) -> Self {
        Esparso {
            tam: d.len().div_ceil(1 << 20) as u64 * (1 << 20),
            segs: vec![(0, d)],
        }
    }

    /// Divide em blocos de `tam` bytes, descartando os totalmente zerados.
    fn blocos(&self, tam: u64) -> BTreeMap<u64, Vec<u8>> {
        let mut m: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
        for (off, d) in &self.segs {
            let mut i = 0usize;
            while i < d.len() {
                let p = off + i as u64;
                let b = p / tam;
                let dentro = (p % tam) as usize;
                let n = (tam as usize - dentro).min(d.len() - i);
                let e = m.entry(b).or_insert_with(|| vec![0u8; tam as usize]);
                e[dentro..dentro + n].copy_from_slice(&d[i..i + n]);
                i += n;
            }
        }
        m.retain(|_, v| v.iter().any(|&x| x != 0));
        m
    }
}

fn guid_bytes(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let mut g = [0u8; 16];
    g[..4].copy_from_slice(&a.to_le_bytes());
    g[4..6].copy_from_slice(&b.to_le_bytes());
    g[6..8].copy_from_slice(&c.to_le_bytes());
    g[8..].copy_from_slice(&d);
    g
}

/// Gera um VHDX. `fixo` aloca todos os blocos; `diferencial` marca HasParent.
pub fn vhdx(disco: &Esparso, bloco: u64, fixo: bool, diferencial: bool) -> Vec<u8> {
    const MB: u64 = 1 << 20;
    let razao = ((1u64 << 23) * 512) / bloco;
    let payload = disco.tam.div_ceil(bloco);
    let total = payload + payload.saturating_sub(1) / razao;
    let bat_len = (total * 8).div_ceil(MB).max(1) * MB;
    let bat_off = 3 * MB;
    let ini_dados = bat_off + bat_len;
    let blocos = disco.blocos(bloco);
    let mut arq = vec![0u8; ini_dados as usize];
    arq[..8].copy_from_slice(b"vhdxfile");
    for (off, seq) in [(64 * 1024usize, 7u64), (128 * 1024, 3)] {
        arq[off..off + 4].copy_from_slice(b"head");
        arq[off + 8..off + 16].copy_from_slice(&seq.to_le_bytes());
        arq[off + 66..off + 68].copy_from_slice(&1u16.to_le_bytes());
    }
    let guid_bat = guid_bytes(
        0x2DC2_7766,
        0xF623,
        0x4200,
        [0x9D, 0x64, 0x11, 0x5E, 0x9B, 0xFD, 0x4A, 0x08],
    );
    let guid_meta = guid_bytes(
        0x8B7C_A206,
        0x4790,
        0x4B9A,
        [0xB8, 0xFE, 0x57, 0x5F, 0x05, 0x0F, 0x88, 0x6E],
    );
    for off in [192 * 1024usize, 256 * 1024] {
        arq[off..off + 4].copy_from_slice(b"regi");
        arq[off + 8..off + 12].copy_from_slice(&2u32.to_le_bytes());
        let e = off + 16;
        arq[e..e + 16].copy_from_slice(&guid_bat);
        arq[e + 16..e + 24].copy_from_slice(&bat_off.to_le_bytes());
        arq[e + 24..e + 28].copy_from_slice(&(bat_len as u32).to_le_bytes());
        arq[e + 28..e + 32].copy_from_slice(&1u32.to_le_bytes());
        let e = off + 48;
        arq[e..e + 16].copy_from_slice(&guid_meta);
        arq[e + 16..e + 24].copy_from_slice(&(2 * MB).to_le_bytes());
        arq[e + 24..e + 28].copy_from_slice(&(MB as u32).to_le_bytes());
        arq[e + 28..e + 32].copy_from_slice(&1u32.to_le_bytes());
    }
    let m = (2 * MB) as usize;
    arq[m..m + 8].copy_from_slice(b"metadata");
    arq[m + 10..m + 12].copy_from_slice(&3u16.to_le_bytes());
    let itens: [([u8; 16], u32, Vec<u8>); 3] = [
        (
            guid_bytes(
                0xCAA1_6737,
                0xFA36,
                0x4D43,
                [0xB3, 0xB6, 0x33, 0xF0, 0xAA, 0x44, 0xE7, 0x6B],
            ),
            0x1_0000,
            {
                let mut v = (bloco as u32).to_le_bytes().to_vec();
                let fl = u32::from(fixo) | (u32::from(diferencial) << 1);
                v.extend_from_slice(&fl.to_le_bytes());
                v
            },
        ),
        (
            guid_bytes(
                0x2FA5_4224,
                0xCD1B,
                0x4876,
                [0xB2, 0x11, 0x5D, 0xBE, 0xD8, 0x3B, 0xF4, 0xB8],
            ),
            0x1_0010,
            disco.tam.to_le_bytes().to_vec(),
        ),
        (
            guid_bytes(
                0x8141_BF1D,
                0xA96F,
                0x4709,
                [0xBA, 0x47, 0xF2, 0x33, 0xA8, 0xFA, 0xAB, 0x5F],
            ),
            0x1_0020,
            512u32.to_le_bytes().to_vec(),
        ),
    ];
    for (i, (g, off, d)) in itens.iter().enumerate() {
        let e = m + 32 + i * 32;
        arq[e..e + 16].copy_from_slice(g);
        arq[e + 16..e + 20].copy_from_slice(&off.to_le_bytes());
        arq[e + 20..e + 24].copy_from_slice(&(d.len() as u32).to_le_bytes());
        arq[m + *off as usize..m + *off as usize + d.len()].copy_from_slice(d);
    }
    // BAT e blocos
    let mut proximo = ini_dados;
    for b in 0..payload {
        let idx = (b + b / razao) as usize;
        if blocos.contains_key(&b) || fixo {
            let dados = blocos
                .get(&b)
                .cloned()
                .unwrap_or_else(|| vec![0u8; bloco as usize]);
            arq.resize((proximo + bloco) as usize, 0);
            arq[proximo as usize..(proximo + bloco) as usize].copy_from_slice(&dados);
            let e = ((proximo / MB) << 20) | 6;
            let o = (bat_off as usize) + idx * 8;
            arq[o..o + 8].copy_from_slice(&e.to_le_bytes());
            proximo += bloco;
        }
    }
    arq
}

/// Gera um qcow2 v3. `comprimir` usa deflate nos clusters; `bits` = log2 do cluster.
pub fn qcow2(disco: &Esparso, bits: u32, comprimir: bool) -> Vec<u8> {
    let cluster = 1u64 << bits;
    let l2_n = cluster / 8;
    let blocos = disco.blocos(cluster);
    let n_clusters = disco.tam.div_ceil(cluster);
    let l1_n = n_clusters.div_ceil(l2_n).max(1);
    let l1_clusters = (l1_n * 8).div_ceil(cluster);
    let mut arq = vec![0u8; (cluster * (1 + l1_clusters)) as usize];
    arq[..4].copy_from_slice(b"QFI\xfb");
    arq[4..8].copy_from_slice(&3u32.to_be_bytes());
    arq[20..24].copy_from_slice(&bits.to_be_bytes());
    arq[24..32].copy_from_slice(&disco.tam.to_be_bytes());
    arq[36..40].copy_from_slice(&(l1_n as u32).to_be_bytes());
    arq[40..48].copy_from_slice(&cluster.to_be_bytes());
    arq[100..104].copy_from_slice(&104u32.to_be_bytes());
    let mut l2s: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    let mut n_comp = 0usize;
    for (ci, dados) in &blocos {
        let l1i = ci / l2_n;
        let l2i = (ci % l2_n) as usize;
        let entrada;
        let tentar = comprimir && ci % 3 != 1;
        let mut comp = Vec::new();
        if tentar {
            let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
            enc.write_all(dados).unwrap();
            comp = enc.finish().unwrap();
        }
        if tentar && (comp.len() as u64) < cluster {
            // Cluster comprimido, empacotado sem alinhamento.
            let off = arq.len() as u64;
            let setores = (off % 512 + comp.len() as u64).div_ceil(512);
            arq.extend_from_slice(&comp);
            let shift = 62 - (bits - 8);
            entrada = (1u64 << 62) | ((setores - 1) << shift) | off;
            n_comp += 1;
        } else {
            // alinha ao cluster
            let off = (arq.len() as u64).div_ceil(cluster) * cluster;
            arq.resize((off + cluster) as usize, 0);
            arq[off as usize..(off + cluster) as usize].copy_from_slice(dados);
            entrada = off | (1 << 63);
        }
        l2s.entry(l1i).or_insert_with(|| vec![0; l2_n as usize])[l2i] = entrada;
    }
    assert!(!comprimir || n_comp > 0);
    for (l1i, tab) in l2s {
        let off = (arq.len() as u64).div_ceil(cluster) * cluster;
        arq.resize((off + cluster) as usize, 0);
        for (i, e) in tab.iter().enumerate() {
            arq[off as usize + i * 8..off as usize + i * 8 + 8].copy_from_slice(&e.to_be_bytes());
        }
        let o = cluster as usize + l1i as usize * 8;
        arq[o..o + 8].copy_from_slice(&(off | (1 << 63)).to_be_bytes());
    }
    arq
}
