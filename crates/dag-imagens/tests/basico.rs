mod common;

use common::*;
use dag_imagens::{Controle, FormatoImagem, SistemaArquivos, TipoMembro, abrir, detectar};
use std::io::{Cursor, Read};

fn sistema_unico(bytes: Vec<u8>) -> (SistemaArquivos, Vec<Item>) {
    let img = abrir(Cursor::new(bytes.clone())).unwrap();
    let sa = img.particoes()[0].sistema.expect("sistema reconhecido");
    let (it, r) = coletar(bytes);
    assert!(r.avisos.is_empty(), "avisos: {:?}", r.avisos);
    assert!(r.nao_lidas.is_empty(), "{:?}", r.nao_lidas);
    (sa, it)
}

#[test]
fn fat12_fat16_fat32() {
    for (bits, esperado) in [
        (12, SistemaArquivos::Fat12),
        (16, SistemaArquivos::Fat16),
        (32, SistemaArquivos::Fat32),
    ] {
        for frag in [false, true] {
            let arv = exemplo();
            let (sa, itens) = sistema_unico(fat::fat_img(bits, &arv, frag));
            assert_eq!(sa, esperado);
            conferir(&itens, 0, &arv);
            let f = itens.iter().find(|i| i.caminho == "leia-me.txt").unwrap();
            assert_eq!(f.mtime, Some(1_710_506_096));
        }
    }
}

#[test]
fn exfat() {
    let arv = exemplo();
    let (sa, itens) = sistema_unico(exfat::exfat_img(&arv));
    assert_eq!(sa, SistemaArquivos::ExFat);
    conferir(&itens, 0, &arv);
    let f = itens.iter().find(|i| i.caminho == "leia-me.txt").unwrap();
    assert_eq!(f.mtime, Some(1_710_506_096));
}

fn arv_links() -> Vec<(String, No)> {
    let mut a = exemplo();
    a.push(("atalho".into(), No::Link("docs/sub/dados.bin".into())));
    a
}

#[test]
fn iso9660_variantes() {
    for (joliet, rr) in [(false, false), (true, false), (false, true), (true, true)] {
        let arv = if rr || joliet { exemplo() } else { exemplo() };
        let bytes = iso::iso_img(&arv, joliet, rr);
        assert_eq!(detectar(&mut Cursor::new(&bytes)), Some(FormatoImagem::Iso));
        let (sa, itens) = sistema_unico(bytes);
        assert_eq!(sa, SistemaArquivos::Iso9660);
        if joliet || rr {
            conferir(&itens, 0, &arv);
        } else {
            // sem extensões, os nomes viram maiúsculas ISO (8.3 simplificado)
            assert_eq!(itens.len(), common::achatar(&arv).len());
            let f = itens.iter().find(|i| i.caminho == "LEIA_ME.TXT").unwrap();
            assert_eq!(f.hash.as_deref(), Some(hash(b"ola mundo\n").as_str()));
        }
        let f = itens
            .iter()
            .find(|i| eh_pasta(i) && i.caminho.eq_ignore_ascii_case("docs"));
        assert!(f.is_some());
        assert!(itens.iter().any(|i| i.mtime == Some(1_710_506_096)));
    }
}

#[test]
fn iso_rock_ridge_link() {
    let arv = arv_links();
    let (_, itens) = sistema_unico(iso::iso_img(&arv, false, true));
    let l = itens.iter().find(|i| i.caminho == "atalho").unwrap();
    assert_eq!(l.tipo, TipoMembro::Link);
    assert_eq!(
        l.hash.as_deref(),
        Some(hash(b"docs/sub/dados.bin").as_str())
    );
}

#[test]
fn mbr_primarias_e_estendida() {
    let a = exemplo();
    let b = vec![("x.txt".to_string(), No::Arq(dados(9, 3000)))];
    let c = vec![("y.txt".to_string(), No::Arq(dados(8, 70_000)))];
    let disco = part::mbr(&[
        (0x0B, fat::fat_img(32, &a, false)),
        (0x06, fat::fat_img(16, &b, true)),
    ]);
    let (itens, r) = coletar(disco);
    assert!(r.avisos.is_empty(), "{:?}", r.avisos);
    assert_eq!(r.particoes_lidas, 2);
    conferir(&itens, 0, &a);
    conferir(&itens, 1, &b);

    let disco = part::mbr_estendida(
        (0x0C, fat::fat_img(32, &a, false)),
        &[
            (0x06, fat::fat_img(16, &b, false)),
            (0x01, fat::fat_img(12, &c, false)),
        ],
    );
    let img = abrir(Cursor::new(disco.clone())).unwrap();
    assert_eq!(img.particoes().len(), 3);
    let (itens, r) = coletar(disco);
    assert!(r.nao_lidas.is_empty(), "{:?}", r.nao_lidas);
    conferir(&itens, 0, &a);
    conferir(&itens, 1, &b);
    conferir(&itens, 2, &c);
}

#[test]
fn gpt_com_particoes_mistas() {
    let a = exemplo();
    let disco = part::gpt(&[
        ("EFI", fat::fat_img(32, &a, false)),
        ("dados", exfat::exfat_img(&a)),
        ("iso", iso::iso_img(&a, true, false)),
    ]);
    let img = abrir(Cursor::new(disco.clone())).unwrap();
    assert_eq!(img.formato(), FormatoImagem::Raw);
    let ps = img.particoes();
    assert_eq!(ps.len(), 3);
    assert_eq!(ps[1].nome.as_deref(), Some("dados"));
    assert!(ps[0].tipo.starts_with("gpt:"));
    let (itens, r) = coletar(disco);
    assert!(r.avisos.is_empty(), "{:?}", r.avisos);
    for i in 0..3 {
        conferir(&itens, i, &a);
    }
}

#[test]
fn particao_desconhecida_vira_nao_lida() {
    let a = exemplo();
    let mut lixo = dados(5, 1 << 20);
    lixo[0] = 1;
    let disco = part::mbr(&[(0x83, lixo), (0x0B, fat::fat_img(32, &a, false))]);
    let (itens, r) = coletar(disco);
    assert_eq!(r.nao_lidas.len(), 1);
    assert_eq!(r.nao_lidas[0].0, 0);
    conferir(&itens, 1, &a);
}

#[test]
fn raw_sem_tabela_e_formato_desconhecido() {
    let a = exemplo();
    let bytes = fat::fat_img(16, &a, false);
    assert_eq!(detectar(&mut Cursor::new(&bytes)), Some(FormatoImagem::Raw));
    assert_eq!(detectar(&mut Cursor::new(dados(1, 100_000))), None);
    assert!(abrir(Cursor::new(dados(1, 100_000))).is_err());
}

#[test]
fn pular_e_parar() {
    let a = exemplo();
    let bytes = fat::fat_img(32, &a, false);
    let mut img = abrir(Cursor::new(bytes.clone())).unwrap();
    let mut vistos = Vec::new();
    img.percorrer(&mut |m, _| -> Result<Controle, std::io::Error> {
        let c = String::from_utf8_lossy(&m.caminho).into_owned();
        let pular = c == "docs";
        vistos.push(c);
        Ok(if pular {
            Controle::Pular
        } else {
            Controle::Continuar
        })
    })
    .unwrap();
    assert!(vistos.contains(&"docs".to_string()));
    assert!(!vistos.iter().any(|c| c.starts_with("docs/")));
    assert!(vistos.iter().any(|c| c.starts_with("a/b")));

    let mut img = abrir(Cursor::new(bytes)).unwrap();
    let mut n = 0;
    let r = img
        .percorrer(&mut |_, _| -> Result<Controle, std::io::Error> {
            n += 1;
            Ok(if n == 3 {
                Controle::Parar
            } else {
                Controle::Continuar
            })
        })
        .unwrap();
    assert_eq!(n, 3);
    assert!(r.interrompido);
}

#[test]
fn erro_do_visitante_propaga() {
    let bytes = fat::fat_img(16, &exemplo(), false);
    let mut img = abrir(Cursor::new(bytes)).unwrap();
    let r = img.percorrer(&mut |_, _| -> Result<Controle, std::io::Error> {
        Err(std::io::Error::other("falhou"))
    });
    assert!(matches!(r, Err(dag_imagens::Erro::Visita(_))));
}

#[test]
fn leitura_parcial_do_conteudo_e_permitida() {
    let bytes = fat::fat_img(32, &exemplo(), false);
    let mut img = abrir(Cursor::new(bytes)).unwrap();
    let r = img
        .percorrer(&mut |_, l| -> Result<Controle, std::io::Error> {
            let mut b = [0u8; 4];
            let _ = l.read(&mut b)?;
            Ok(Controle::Continuar)
        })
        .unwrap();
    assert!(r.membros > 5);
}
