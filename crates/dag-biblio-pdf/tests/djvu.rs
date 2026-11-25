//! DjVu: fixtures geradas com `c44` (djvulibre) a partir de PPM montado em código. Pulam (com
//! aviso) se `c44`/`djvused` não existirem.
#![allow(clippy::unwrap_used)]

mod suporte;

use std::path::{Path, PathBuf};
use std::process::Command;

use dag_biblio::{Campo, EstadoLeitura, FormatoLivro};
use dag_biblio_pdf::{ConfigExtracaoPdf, Fonte};
use suporte::{extrator_com_ocr, ppm_de_texto};

fn tem(programa: &str) -> bool {
    Command::new(programa).arg("--help").output().is_ok()
}

/// Cria um DjVu de uma página com a imagem dada; devolve o caminho do arquivo no tempdir.
fn criar_djvu(dir: &Path, nome: &str, linhas: &[&str]) -> PathBuf {
    let ppm = dir.join(format!("{nome}.ppm"));
    let djvu = dir.join(format!("{nome}.djvu"));
    std::fs::write(&ppm, ppm_de_texto(linhas)).unwrap();
    let st = Command::new("c44")
        .arg("-dpi")
        .arg("300")
        .arg(&ppm)
        .arg(&djvu)
        .status()
        .unwrap();
    assert!(st.success());
    djvu
}

fn djvused(arquivo: &Path, script: &str) {
    let st = Command::new("djvused")
        .arg(arquivo)
        .arg("-e")
        .arg(script)
        .arg("-s")
        .status()
        .unwrap();
    assert!(st.success(), "djvused {script}");
}

#[test]
fn djvu_sem_camada_de_texto_usa_ocr_e_gera_capa_png() {
    if !tem("c44") || !tem("djvused") {
        eprintln!("AVISO: teste pulado, djvulibre (c44/djvused) ausente");
        return;
    }
    let Some(ex) = extrator_com_ocr(ConfigExtracaoPdf {
        idiomas_ocr: vec!["eng".into()],
        ..Default::default()
    }) else {
        return;
    };
    if ex.ferramentas().djvu.is_none() {
        eprintln!("AVISO: teste pulado, extrator sem djvulibre");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let arquivo = criar_djvu(dir.path(), "scan", &["DOM CASMURRO", "MACHADO DE ASSIS"]);
    let bytes = std::fs::read(&arquivo).unwrap();

    // Do disco e da memória (ADR 0006): mesmo resultado.
    for fonte in [Fonte::Caminho(&arquivo), Fonte::Bytes(&bytes)] {
        let r = ex.extrair_djvu(fonte);
        let res = &r.resultado;
        assert_eq!(res.formato, FormatoLivro::Djvu);
        assert_eq!(res.estado, EstadoLeitura::Lido, "{:?}", res.avisos);
        assert_eq!(res.total_paginas, Some(1));
        assert_eq!(res.paginas_ocr, vec![1]);
        let t = res.paginas[0].texto.to_uppercase();
        assert!(t.contains("CASMURRO") && t.contains("ASSIS"), "{t}");
        assert!(res.capa.as_ref().unwrap().starts_with(b"\x89PNG"));
        assert_eq!(res.capa_mime.as_deref(), Some("image/png"));
    }
    assert_eq!(std::fs::read(&arquivo).unwrap(), bytes, "original intacto");
}

#[test]
fn djvu_com_texto_meta_e_marcadores() {
    if !tem("c44") || !tem("djvused") {
        eprintln!("AVISO: teste pulado, djvulibre (c44/djvused) ausente");
        return;
    }
    let Some(ex) = suporte::extrator(ConfigExtracaoPdf::default()) else {
        return;
    };
    if ex.ferramentas().djvu.is_none() {
        eprintln!("AVISO: teste pulado, extrator sem djvulibre");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let arquivo = criar_djvu(dir.path(), "livro", &["LIVRO"]);
    let txt = dir.path().join("p1.txt");
    std::fs::write(
        &txt,
        "(page 0 0 1800 800 \"Dados Internacionais de Cataloga\\303\\247\\303\\243o na Publica\\303\\247\\303\\243o (CIP)\\nISBN 978-85-359-0277-8\\nCDD 869.3\")\n",
    )
    .unwrap();
    let meta = dir.path().join("meta.txt");
    std::fs::write(
        &meta,
        "title\t\"Dom Casmurro\"\nauthor\t\"Machado de Assis\"\n",
    )
    .unwrap();
    let outline = dir.path().join("outline.txt");
    std::fs::write(&outline, "(bookmarks (\"Inicio\" \"#1\"))\n").unwrap();
    djvused(&arquivo, &format!("select 1; set-txt {}", txt.display()));
    djvused(&arquivo, &format!("set-meta {}", meta.display()));
    djvused(&arquivo, &format!("set-outline {}", outline.display()));

    let r = ex.extrair_djvu(Fonte::Caminho(&arquivo));
    let res = &r.resultado;
    assert_eq!(res.estado, EstadoLeitura::Lido, "{:?}", res.avisos);
    assert!(res.paginas_ocr.is_empty(), "tem texto: sem OCR");
    assert_eq!(res.ficha_catalografica.as_ref().unwrap().0, 1);
    assert_eq!(res.isbns[0].0, "9788535902778");
    assert_eq!(res.sumario.len(), 1);
    assert_eq!(res.sumario[0].titulo, "Inicio");
    let autores: Vec<_> = res
        .metadados
        .iter()
        .filter(|v| v.campo == Campo::Autores)
        .collect();
    assert_eq!(autores[0].valor.as_deref(), Some("Machado de Assis"));
}

#[test]
fn djvu_corrompido_e_arquivo_de_outro_formato() {
    let Some(ex) = suporte::extrator(ConfigExtracaoPdf::default()) else {
        return;
    };
    if ex.ferramentas().djvu.is_none() {
        eprintln!("AVISO: teste pulado, extrator sem djvulibre");
        return;
    }
    let r = ex.extrair_djvu(Fonte::Bytes(b"isto nao e djvu"));
    assert!(matches!(
        r.resultado.estado,
        EstadoLeitura::Corrompido { .. }
    ));
    let r = ex.extrair_djvu(Fonte::Bytes(b"AT&TFORM\0\0\0\x10DJVUINFOlixo lixo lixo"));
    assert!(
        matches!(r.resultado.estado, EstadoLeitura::Corrompido { .. }),
        "{:?}",
        r.resultado.estado
    );
}
