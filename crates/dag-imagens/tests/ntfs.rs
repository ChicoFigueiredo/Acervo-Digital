#![allow(clippy::unwrap_used, clippy::manual_is_multiple_of)]
mod common;

use common::ntfs::{Opc, ntfs_img};
use common::*;
use dag_imagens::{Controle, SistemaArquivos, TipoMembro, abrir};
use std::io::Cursor;

fn arvore() -> Vec<(String, No)> {
    let mut a = exemplo();
    // diretório grande: força $INDEX_ALLOCATION com vários blocos INDX
    let muitos: Vec<(String, No)> = (0..150)
        .map(|i| {
            (
                format!("arquivo-numero-{i:04}.txt"),
                No::Arq(format!("conteudo {i}").into_bytes()),
            )
        })
        .collect();
    a.push(("muitos".into(), No::Dir(muitos)));
    // arquivos especiais
    let mut esparso = dados(30, 4096);
    esparso.extend(vec![0u8; 8192]);
    esparso.extend(dados(31, 4096 + 100));
    a.push(("esparso.bin".into(), No::Arq(esparso)));
    let mut init = dados(32, 20_000);
    init.extend(vec![0u8; 20_000]);
    a.push(("init.bin".into(), No::Arq(init)));
    a
}

#[test]
fn ntfs_arvore_completa() {
    for frag in [false, true] {
        let arv = arvore();
        let bytes = ntfs_img(
            &arv,
            &Opc {
                frag,
                links: vec![],
            },
        );
        let img = abrir(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(img.particoes()[0].sistema, Some(SistemaArquivos::Ntfs));
        let (itens, r) = coletar(bytes);
        assert!(r.avisos.is_empty(), "{:?}", r.avisos);
        assert!(r.nao_lidas.is_empty(), "{:?}", r.nao_lidas);
        conferir(&itens, 0, &arv);
        assert!(!itens.iter().any(|i| i.caminho.contains("fantasma")));
        let f = itens.iter().find(|i| i.caminho == "leia-me.txt").unwrap();
        assert_eq!(f.mtime, Some(1_710_506_096));
    }
}

#[test]
fn ntfs_hard_link() {
    let arv = exemplo();
    let bytes = ntfs_img(
        &arv,
        &Opc {
            frag: false,
            links: vec![("docs/sub/dados.bin".into(), "link-duro.bin".into())],
        },
    );
    let (itens, _) = coletar(bytes);
    let a = itens
        .iter()
        .find(|i| i.caminho == "docs/sub/dados.bin")
        .unwrap();
    let b = itens.iter().find(|i| i.caminho == "link-duro.bin").unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(a.hash, b.hash);
    assert_ne!(a.repetido, b.repetido);
}

#[test]
fn ntfs_comprimido_e_sem_suporte_sem_derrubar() {
    let arv = vec![
        ("normal.txt".to_string(), No::Arq(dados(1, 5000))),
        ("comprimido.dat".to_string(), No::Arq(dados(2, 9000))),
    ];
    let bytes = ntfs_img(
        &arv,
        &Opc {
            frag: false,
            links: vec![],
        },
    );
    let mut img = abrir(Cursor::new(bytes)).unwrap();
    let mut lidos = Vec::new();
    let r = img
        .percorrer(&mut |m, l| -> Result<Controle, std::io::Error> {
            let mut v = Vec::new();
            let ok = l.read_to_end(&mut v).is_ok();
            lidos.push((String::from_utf8_lossy(&m.caminho).into_owned(), ok, m.tipo));
            Ok(Controle::Continuar)
        })
        .unwrap();
    assert!(lidos.contains(&("normal.txt".into(), true, TipoMembro::Arquivo)));
    assert!(lidos.contains(&("comprimido.dat".into(), false, TipoMembro::Arquivo)));
    assert!(
        r.avisos.iter().any(|a| a.contains("comprimido")),
        "{:?}",
        r.avisos
    );
}

#[test]
fn ntfs_em_gpt() {
    let arv = exemplo();
    let disco = part::gpt(&[(
        "Windows",
        ntfs_img(
            &arv,
            &Opc {
                frag: true,
                links: vec![],
            },
        ),
    )]);
    let (itens, r) = coletar(disco);
    assert!(r.nao_lidas.is_empty(), "{:?}", r.nao_lidas);
    conferir(&itens, 0, &arv);
}

#[test]
fn fuzz_ntfs() {
    let base = ntfs_img(
        &arvore(),
        &Opc {
            frag: true,
            links: vec![],
        },
    );
    common::fuzz::fuzz(&base, 500, 2024);
    // concentra a corrupção na área do MFT e dos índices
    let mut r = 99u64;
    for _ in 0..300 {
        let mut v = base.clone();
        for _ in 0..6 {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            let p = 16 * 4096 + (r as usize % (256 * 1024));
            v[p] = (r >> 20) as u8;
        }
        common::fuzz::exercitar(v);
    }
}
