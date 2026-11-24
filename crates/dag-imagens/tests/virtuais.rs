#![allow(
    clippy::unwrap_used,
    clippy::manual_is_multiple_of,
    clippy::type_complexity
)]
//! VHDX e qcow2: discos virtuais fixos, dinâmicos, esparsos e comprimidos.

mod common;

use common::virt::{Esparso, qcow2, vhdx};
use common::*;
use dag_imagens::{Erro, FormatoImagem, abrir, detectar};
use std::io::Cursor;

fn disco_base() -> (Vec<u8>, Vec<(String, No)>, Vec<(String, No)>) {
    let a = exemplo();
    let b = vec![("b.txt".to_string(), No::Arq(dados(11, 123_456)))];
    let d = part::mbr(&[
        (0x0B, fat::fat_img(32, &a, false)),
        (0x06, fat::fat_img(16, &b, true)),
    ]);
    (d, a, b)
}

fn conferir_base(bytes: Vec<u8>, a: &[(String, No)], b: &[(String, No)]) {
    let (itens, r) = coletar(bytes);
    assert!(r.avisos.is_empty(), "{:?}", r.avisos);
    assert!(r.nao_lidas.is_empty(), "{:?}", r.nao_lidas);
    conferir(&itens, 0, a);
    conferir(&itens, 1, b);
}

/// MBR com duas partições FAT em posições dadas (em bytes).
fn mbr_duas(pos: [u64; 2], tams: [usize; 2]) -> Vec<u8> {
    let mut mbr = vec![0u8; 512];
    for i in 0..2 {
        let mut e = [0u8; 16];
        e[4] = 0x01;
        e[8..12].copy_from_slice(&((pos[i] / 512) as u32).to_le_bytes());
        e[12..16].copy_from_slice(&((tams[i] / 512) as u32).to_le_bytes());
        mbr[446 + i * 16..462 + i * 16].copy_from_slice(&e);
    }
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    mbr
}

#[test]
fn vhdx_dinamico_e_fixo() {
    let (d, a, b) = disco_base();
    for bloco in [1u64 << 20, 4 << 20] {
        for fixo in [false, true] {
            let v = vhdx(&Esparso::denso(d.clone()), bloco, fixo, false);
            assert_eq!(detectar(&mut Cursor::new(&v)), Some(FormatoImagem::Vhdx));
            conferir_base(v, &a, &b);
        }
    }
}

#[test]
fn vhdx_grande_com_bitmap_de_setores_intercalado() {
    // 5 GiB virtuais, blocos de 1 MiB: a BAT passa de 4096 entradas de payload.
    let a = exemplo();
    let b = vec![("longe.bin".to_string(), No::Arq(dados(21, 300_000)))];
    let pa = fat::fat_img(12, &a, false);
    let pb = fat::fat_img(16, &b, false);
    let pos_b: u64 = (4 << 30) + (512 << 20);
    let mbr = mbr_duas([1 << 20, pos_b], [pa.len(), pb.len()]);
    let disco = Esparso {
        tam: 5 << 30,
        segs: vec![(0, mbr), (1 << 20, pa), (pos_b, pb)],
    };
    let v = vhdx(&disco, 1 << 20, false, false);
    assert!(v.len() < 8 << 20, "arquivo deve ser pequeno: {}", v.len());
    conferir_base(v, &a, &b);
}

#[test]
fn vhdx_diferencial_e_sem_suporte() {
    let (d, _, _) = disco_base();
    let v = vhdx(&Esparso::denso(d), 1 << 20, false, true);
    match abrir(Cursor::new(v)) {
        Err(Erro::SemSuporte(m)) => assert!(m.contains("diferencial"), "{m}"),
        outro => panic!("esperava SemSuporte, veio {outro:?}"),
    }
}

#[test]
fn qcow2_simples_e_comprimido() {
    let (d, a, b) = disco_base();
    for bits in [12u32, 16] {
        for comp in [false, true] {
            let q = qcow2(&Esparso::denso(d.clone()), bits, comp);
            assert_eq!(detectar(&mut Cursor::new(&q)), Some(FormatoImagem::Qcow2));
            conferir_base(q, &a, &b);
        }
    }
}

#[test]
fn qcow2_grande_esparso() {
    let a = exemplo();
    let pa = fat::fat_img(12, &a, false);
    let b = vec![("longe.bin".to_string(), No::Arq(dados(22, 200_000)))];
    let pb = fat::fat_img(16, &b, false);
    let pos_b: u64 = 3 << 30;
    let mbr = mbr_duas([1 << 20, pos_b], [pa.len(), pb.len()]);
    let disco = Esparso {
        tam: 4 << 30,
        segs: vec![(0, mbr), (1 << 20, pa), (pos_b, pb)],
    };
    let q = qcow2(&disco, 16, true);
    conferir_base(q, &a, &b);
}

#[test]
fn qcow2_apoio_cifra_e_zstd_sem_suporte() {
    let (d, _, _) = disco_base();
    let q = qcow2(&Esparso::denso(d), 16, false);
    let mut com_apoio = q.clone();
    com_apoio[8..16].copy_from_slice(&1000u64.to_be_bytes());
    com_apoio[16..20].copy_from_slice(&5u32.to_be_bytes());
    let mut cifrado = q.clone();
    cifrado[32..36].copy_from_slice(&1u32.to_be_bytes());
    let mut zstd = q;
    zstd[72..80].copy_from_slice(&8u64.to_be_bytes());
    zstd[104] = 1;
    for (nome, img) in [("apoio", com_apoio), ("cifrado", cifrado), ("zstd", zstd)] {
        assert!(
            matches!(abrir(Cursor::new(img)), Err(Erro::SemSuporte(_))),
            "{nome}"
        );
    }
}

#[test]
fn fuzz_vhdx_qcow2() {
    let a = exemplo();
    let b = vec![("x".to_string(), No::Arq(dados(1, 20_000)))];
    let d = part::mbr(&[
        (0x01, fat::fat_img(12, &a, false)),
        (0x01, fat::fat_img(12, &b, false)),
    ]);
    let v = vhdx(&Esparso::denso(d.clone()), 1 << 20, false, false);
    common::fuzz::fuzz(&v, 300, 31);
    let q = qcow2(&Esparso::denso(d), 12, true);
    common::fuzz::fuzz(&q, 500, 32);
}
