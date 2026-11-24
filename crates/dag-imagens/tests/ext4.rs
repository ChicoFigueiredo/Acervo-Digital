//! Testes de ext2/3/4 com imagens geradas por `mke2fs -d` (sem root, sem montar).

mod common;

use common::*;
use dag_imagens::{SistemaArquivos, TipoMembro, abrir};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;

fn tem_mke2fs() -> bool {
    ["/usr/sbin/mke2fs", "/sbin/mke2fs", "/usr/bin/mke2fs"]
        .iter()
        .any(|p| Path::new(p).exists())
        || Command::new("mke2fs").arg("-V").output().is_ok()
}

fn mke2fs() -> Command {
    for p in ["/usr/sbin/mke2fs", "/sbin/mke2fs", "/usr/bin/mke2fs"] {
        if Path::new(p).exists() {
            return Command::new(p);
        }
    }
    Command::new("mke2fs")
}

/// Esperado: caminho -> (tipo, hash do conteúdo)
fn esperado(raiz: &Path) -> BTreeMap<String, (TipoMembro, Option<String>)> {
    fn rec(base: &Path, rel: &str, out: &mut BTreeMap<String, (TipoMembro, Option<String>)>) {
        let mut ents: Vec<_> = std::fs::read_dir(base)
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        ents.sort_by_key(|e| e.file_name());
        for e in ents {
            let nome = e.file_name().to_string_lossy().into_owned();
            let p = if rel.is_empty() {
                nome
            } else {
                format!("{rel}/{nome}")
            };
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                out.insert(p.clone(), (TipoMembro::Pasta, None));
                rec(&e.path(), &p, out);
            } else if ft.is_symlink() {
                let t = std::fs::read_link(e.path()).unwrap();
                out.insert(
                    p,
                    (TipoMembro::Link, Some(hash(t.to_string_lossy().as_bytes()))),
                );
            } else {
                let d = std::fs::read(e.path()).unwrap();
                out.insert(p, (TipoMembro::Arquivo, Some(hash(&d))));
            }
        }
    }
    let mut m = BTreeMap::new();
    rec(raiz, "", &mut m);
    m
}

fn popular(raiz: &Path) {
    use std::os::unix::fs::symlink;
    let w = |p: &str, d: &[u8]| {
        let c = raiz.join(p);
        std::fs::create_dir_all(c.parent().unwrap()).unwrap();
        std::fs::write(c, d).unwrap();
    };
    w("leia-me.txt", b"ola mundo\n");
    w("vazio", b"");
    w("pequeno", &[7u8; 50]); // cabe inline
    w("medio.bin", &dados(1, 70_000));
    w("grande.bin", &dados(2, 1_500_000)); // indireto duplo em ext2/3
    w("docs/Relatório final.pdf", &dados(3, 5000));
    w("docs/sub/dados.bin", &dados(4, 100_000));
    w("a/b/c/d/profundo.txt", &dados(5, 513));
    std::fs::create_dir_all(raiz.join("pasta vazia")).unwrap();
    std::fs::hard_link(raiz.join("medio.bin"), raiz.join("docs/medio-link.bin")).unwrap();
    symlink("docs/sub/dados.bin", raiz.join("atalho")).unwrap();
    symlink(
        "um/destino/bem/longo/que/passa/de/sessenta/bytes/para/nao/ser/rapido/nem/inline/ok",
        raiz.join("atalho-longo"),
    )
    .unwrap();
    // esparso: buraco no meio
    let f = std::fs::File::create(raiz.join("esparso.bin")).unwrap();
    f.set_len(300_000).unwrap();
    use std::os::unix::fs::FileExt;
    f.write_all_at(b"inicio", 0).unwrap();
    f.write_all_at(b"fim!", 299_996).unwrap();
    // diretório grande (htree)
    for i in 0..400 {
        w(
            &format!("muitos/arquivo-com-nome-bem-comprido-numero-{i:05}.txt"),
            format!("conteudo {i}").as_bytes(),
        );
    }
}

fn gerar(args: &[&str], tamanho: &str) -> Option<(tempfile::TempDir, Vec<u8>)> {
    if !tem_mke2fs() {
        eprintln!("PULADO: mke2fs não encontrado neste ambiente");
        return None;
    }
    let tmp = tempfile::tempdir().unwrap();
    let fonte = tmp.path().join("fonte");
    std::fs::create_dir(&fonte).unwrap();
    popular(&fonte);
    let img = tmp.path().join("fs.img");
    let out = mke2fs()
        .args(args)
        .arg("-q")
        .arg("-d")
        .arg(&fonte)
        .arg(&img)
        .arg(tamanho)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "mke2fs falhou: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(&img).unwrap();
    Some((tmp, bytes))
}

fn verificar(args: &[&str], tamanho: &str, tem_inline: bool) {
    let Some((tmp, bytes)) = gerar(args, tamanho) else {
        return;
    };
    let img = abrir(Cursor::new(bytes.clone())).unwrap();
    assert_eq!(img.particoes()[0].sistema, Some(SistemaArquivos::Ext));
    let (itens, r) = coletar(bytes);
    assert!(r.avisos.is_empty(), "{:?}", r.avisos);
    assert!(r.nao_lidas.is_empty(), "{:?}", r.nao_lidas);
    let esp = esperado(&tmp.path().join("fonte"));
    let obt: BTreeMap<String, &Item> = itens
        .iter()
        .filter(|i| !i.caminho.starts_with("lost+found"))
        .map(|i| (i.caminho.clone(), i))
        .collect();
    assert_eq!(
        esp.keys().collect::<Vec<_>>(),
        obt.keys().collect::<Vec<_>>(),
        "caminhos"
    );
    for (p, (tipo, h)) in &esp {
        let i = obt[p];
        assert_eq!(i.tipo, *tipo, "{p}");
        assert_eq!(i.hash, *h, "hash de {p}");
        if *tipo == TipoMembro::Arquivo {
            assert_eq!(
                i.tamanho,
                std::fs::metadata(tmp.path().join("fonte").join(p))
                    .unwrap()
                    .len(),
                "{p}"
            );
        }
    }
    // hard link: mesmo id, um deles marcado como repetido
    let a = obt["medio.bin"];
    let b = obt["docs/medio-link.bin"];
    assert_eq!(a.id, b.id);
    assert_ne!(a.repetido, b.repetido);
    assert!(a.mtime.is_some());
    let _ = tem_inline;
}

#[test]
fn ext4_padrao() {
    verificar(&["-t", "ext4"], "32M", false);
}

#[test]
fn ext4_bloco_1k_sem_journal() {
    verificar(
        &["-t", "ext4", "-b", "1024", "-O", "^has_journal"],
        "32M",
        false,
    );
}

#[test]
fn ext4_64bit_inline_data() {
    verificar(
        &["-t", "ext4", "-O", "64bit,inline_data", "-I", "256"],
        "32M",
        true,
    );
}

#[test]
fn ext3_mapa_de_blocos() {
    verificar(&["-t", "ext3", "-b", "1024"], "32M", false);
}

#[test]
fn ext2_inodes_128() {
    verificar(&["-t", "ext2", "-I", "128"], "32M", false);
}

#[test]
fn fuzz_ext4() {
    for args in [&["-t", "ext4"][..], &["-t", "ext3", "-b", "1024"][..]] {
        let Some((_tmp, bytes)) = gerar(args, "16M") else {
            return;
        };
        common::fuzz::fuzz(&bytes, 400, 4242);
    }
}
