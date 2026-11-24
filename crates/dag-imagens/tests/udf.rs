#![allow(clippy::unwrap_used, clippy::manual_is_multiple_of)]
mod common;

use common::*;
use dag_imagens::{FormatoImagem, SistemaArquivos, TipoMembro, abrir, detectar};
use std::io::Cursor;

#[test]
fn udf_basico() {
    for frag in [false, true] {
        let mut arv = exemplo();
        arv.push(("atalho".into(), No::Link("docs/sub/dados.bin".into())));
        let bytes = udf::udf_img(&arv, frag);
        assert_eq!(detectar(&mut Cursor::new(&bytes)), Some(FormatoImagem::Iso));
        let img = abrir(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(img.particoes()[0].sistema, Some(SistemaArquivos::Udf));
        let (itens, r) = coletar(bytes);
        assert!(r.avisos.is_empty(), "{:?}", r.avisos);
        assert!(r.nao_lidas.is_empty(), "{:?}", r.nao_lidas);
        conferir(&itens, 0, &arv);
        let l = itens.iter().find(|i| i.caminho == "atalho").unwrap();
        assert_eq!(l.tipo, TipoMembro::Link);
        assert!(itens.iter().any(|i| i.mtime == Some(1_710_506_096)));
    }
}

#[test]
fn udf_sem_particao_virtual() {
    // Mapa de partição tipo 2 "virtual" é recusado como sem suporte.
    let arv = exemplo();
    let mut bytes = udf::udf_img(&arv, false);
    let lvd = 33 * 2048;
    bytes[lvd + 440] = 2;
    bytes[lvd + 441] = 64;
    bytes[lvd + 445..lvd + 445 + 17].copy_from_slice(b"*UDF Virtual Part");
    let (_, r) = coletar(bytes);
    assert_eq!(r.nao_lidas.len(), 1);
    assert!(r.nao_lidas[0].1.contains("VAT"), "{:?}", r.nao_lidas);
}

#[test]
fn fuzz_udf() {
    let arv = exemplo();
    common::fuzz::fuzz(&udf::udf_img(&arv, true), 600, 8080);
}
