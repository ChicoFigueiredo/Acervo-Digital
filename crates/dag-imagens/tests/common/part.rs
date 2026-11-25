//! Montagem de discos com MBR (primárias e estendida) e GPT.

const ALINHA: usize = 2048 * 512;

fn alinhar(n: usize) -> usize {
    n.div_ceil(ALINHA) * ALINHA
}

fn entrada_mbr(tipo: u8, ini: u32, len: u32) -> [u8; 16] {
    let mut e = [0u8; 16];
    e[4] = tipo;
    e[8..12].copy_from_slice(&ini.to_le_bytes());
    e[12..16].copy_from_slice(&len.to_le_bytes());
    e
}

/// Disco MBR com até 4 partições primárias: `(tipo, imagem)`.
pub fn mbr(parts: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut disco = vec![0u8; ALINHA];
    for (i, (tipo, img)) in parts.iter().enumerate() {
        let ini = disco.len();
        disco.extend_from_slice(img);
        disco.resize(alinhar(disco.len()), 0);
        let e = entrada_mbr(*tipo, (ini / 512) as u32, (img.len().div_ceil(512)) as u32);
        disco[446 + i * 16..446 + (i + 1) * 16].copy_from_slice(&e);
    }
    disco[510] = 0x55;
    disco[511] = 0xAA;
    disco
}

/// MBR com uma primária e uma estendida contendo as lógicas.
pub fn mbr_estendida(primaria: (u8, Vec<u8>), logicas: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut disco = vec![0u8; ALINHA];
    let ini = disco.len();
    disco.extend_from_slice(&primaria.1);
    disco.resize(alinhar(disco.len()), 0);
    let e = entrada_mbr(
        primaria.0,
        (ini / 512) as u32,
        primaria.1.len().div_ceil(512) as u32,
    );
    disco[446..462].copy_from_slice(&e);
    let ext_ini = disco.len();
    // cada lógica: [EBR (1 trecho alinhado)][partição]
    let mut rel = 0usize;
    let total_ext: usize = logicas.iter().map(|(_, d)| ALINHA + alinhar(d.len())).sum();
    for (k, (tipo, img)) in logicas.iter().enumerate() {
        let ebr_abs = ext_ini + rel;
        let part_abs = ebr_abs + ALINHA;
        let tam = alinhar(img.len());
        disco.resize(part_abs + tam, 0);
        disco[part_abs..part_abs + img.len()].copy_from_slice(img);
        let mut ebr = vec![0u8; 512];
        let e0 = entrada_mbr(*tipo, (ALINHA / 512) as u32, img.len().div_ceil(512) as u32);
        ebr[446..462].copy_from_slice(&e0);
        if k + 1 < logicas.len() {
            let prox_rel = rel + ALINHA + tam;
            let prox_len = ALINHA + alinhar(logicas[k + 1].1.len());
            let e1 = entrada_mbr(0x05, (prox_rel / 512) as u32, (prox_len / 512) as u32);
            ebr[462..478].copy_from_slice(&e1);
        }
        ebr[510] = 0x55;
        ebr[511] = 0xAA;
        disco[ebr_abs..ebr_abs + 512].copy_from_slice(&ebr);
        rel += ALINHA + tam;
    }
    let e = entrada_mbr(0x0F, (ext_ini / 512) as u32, (total_ext / 512) as u32);
    disco[462..478].copy_from_slice(&e);
    disco[510] = 0x55;
    disco[511] = 0xAA;
    disco
}

fn guid(n: u8) -> [u8; 16] {
    let mut g = [0u8; 16];
    for (i, b) in g.iter_mut().enumerate() {
        *b = n.wrapping_mul(17).wrapping_add(i as u8);
    }
    g
}

/// Disco GPT: `(nome, imagem)`.
pub fn gpt(parts: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut disco = vec![0u8; ALINHA];
    let mut ents = vec![0u8; 128 * 128];
    let mut ultimo = 0u64;
    for (i, (nome, img)) in parts.iter().enumerate() {
        let ini = disco.len();
        disco.extend_from_slice(img);
        disco.resize(alinhar(disco.len()), 0);
        let e = &mut ents[i * 128..(i + 1) * 128];
        e[..16].copy_from_slice(&guid(1));
        e[16..32].copy_from_slice(&guid(10 + i as u8));
        let l0 = (ini / 512) as u64;
        let l1 = l0 + img.len().div_ceil(512) as u64 - 1;
        e[32..40].copy_from_slice(&l0.to_le_bytes());
        e[40..48].copy_from_slice(&l1.to_le_bytes());
        for (k, c) in nome.encode_utf16().enumerate() {
            e[56 + k * 2..58 + k * 2].copy_from_slice(&c.to_le_bytes());
        }
        ultimo = l1;
    }
    // MBR protetor
    let e = entrada_mbr(0xEE, 1, 0xFFFF_FFFF);
    disco[446..462].copy_from_slice(&e);
    disco[510] = 0x55;
    disco[511] = 0xAA;
    // cabeçalho
    let mut h = vec![0u8; 512];
    h[..8].copy_from_slice(b"EFI PART");
    h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    h[12..16].copy_from_slice(&92u32.to_le_bytes());
    h[24..32].copy_from_slice(&1u64.to_le_bytes());
    h[40..48].copy_from_slice(&34u64.to_le_bytes());
    h[48..56].copy_from_slice(&ultimo.to_le_bytes());
    h[56..72].copy_from_slice(&guid(99));
    h[72..80].copy_from_slice(&2u64.to_le_bytes());
    h[80..84].copy_from_slice(&128u32.to_le_bytes());
    h[84..88].copy_from_slice(&128u32.to_le_bytes());
    disco[512..1024].copy_from_slice(&h);
    disco[1024..1024 + ents.len()].copy_from_slice(&ents);
    disco
}
