#![allow(clippy::unwrap_used, clippy::manual_is_multiple_of)]
//! Testes estilo fuzz: imagens corrompidas nunca podem causar pânico nem laço infinito.

mod common;

use common::fuzz::fuzz;
use common::*;

#[test]
fn fuzz_fat() {
    let a = exemplo();
    for (bits, frag) in [(12, false), (16, true), (32, false), (32, true)] {
        fuzz(&fat::fat_img(bits, &a, frag), 150, 0x1234 + u64::from(bits));
    }
}

#[test]
fn fuzz_exfat() {
    fuzz(&exfat::exfat_img(&exemplo()), 300, 77);
}

#[test]
fn fuzz_iso() {
    let a = exemplo();
    fuzz(&iso::iso_img(&a, true, true), 300, 91);
    fuzz(&iso::iso_img(&a, false, false), 200, 92);
}

#[test]
fn fuzz_tabelas() {
    let a = exemplo();
    let b = vec![("x".to_string(), No::Arq(dados(1, 2000)))];
    fuzz(
        &part::gpt(&[
            ("a", fat::fat_img(16, &b, false)),
            ("b", fat::fat_img(12, &a, false)),
        ]),
        300,
        5,
    );
    fuzz(
        &part::mbr_estendida(
            (0x0B, fat::fat_img(12, &b, false)),
            &[
                (0x06, fat::fat_img(12, &a, false)),
                (0x01, fat::fat_img(12, &b, false)),
            ],
        ),
        300,
        6,
    );
}
