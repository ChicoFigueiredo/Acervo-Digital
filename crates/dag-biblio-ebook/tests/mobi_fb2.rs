#![allow(clippy::unwrap_used, clippy::expect_used)]
mod comum;

use comum::*;
use dag_biblio::{Campo, EstadoLeitura, FormatoLivro, OrigemEvidencia, ResultadoExtracao};
use dag_biblio_ebook::{Extraido, OpcoesExtracao, extrair_arquivo, extrair_bytes};

fn livro(d: &[u8], nome: Option<&str>) -> ResultadoExtracao {
    match extrair_bytes(d, nome, None, &OpcoesExtracao::default()) {
        Extraido::Livro(r) => r,
        Extraido::Compactado(_) => panic!("esperava livro"),
    }
}

#[test]
fn mobi_palmdoc_metadados_capa_texto_e_isbn() {
    let r = livro(&mobi(&MobiSpec::default()), Some("x.mobi"));
    assert_eq!(r.formato, FormatoLivro::Mobi);
    assert_eq!(r.estado, EstadoLeitura::Lido, "{:?}", r.avisos);
    assert_eq!(valor(&r, Campo::Titulo).as_deref(), Some("Livro Móbi"));
    assert_eq!(lista(&r, Campo::Autores), vec!["Carlos Silva".to_owned()]);
    assert_eq!(valor(&r, Campo::Editora).as_deref(), Some("Editora Móbi"));
    assert_eq!(valor(&r, Campo::Resumo).as_deref(), Some("Descrição"));
    assert_eq!(lista(&r, Campo::Tags), vec!["Ficção", "Teste"]);
    assert_eq!(valor(&r, Campo::Data).as_deref(), Some("2010-02-03"));
    assert_eq!(valor(&r, Campo::Idioma).as_deref(), Some("pt-BR"));
    assert_eq!(valor(&r, Campo::Isbn).as_deref(), Some("9788535902778"));
    assert_eq!(r.capa.as_deref(), Some(&[&JPEG[..], b"capa"].concat()[..]));
    assert_eq!(r.capa_mime.as_deref(), Some("image/jpeg"));
    assert_eq!(
        r.paginas[0].texto,
        "Primeiro parágrafo.\nISBN 978-85-359-0277-8"
    );
    assert!(
        r.isbns
            .iter()
            .any(|(i, e)| i == "9788535902778" && e.origem == OrigemEvidencia::Conteudo)
    );
}

#[test]
fn mobi_sem_compressao_e_azw3_por_versao() {
    let s = MobiSpec {
        compressao: 1,
        versao: 8,
        ..MobiSpec::default()
    };
    let r = livro(&mobi(&s), None);
    assert_eq!(r.formato, FormatoLivro::Azw3);
    assert_eq!(
        r.paginas[0].texto.lines().next(),
        Some("Primeiro parágrafo.")
    );
    // A dica de extensão manda entre MOBI/AZW/AZW3.
    let r = match extrair_bytes(
        &mobi(&MobiSpec::default()),
        Some("a.azw"),
        None,
        &OpcoesExtracao::default(),
    ) {
        Extraido::Livro(r) => r,
        _ => panic!(),
    };
    assert_eq!(r.formato, FormatoLivro::Azw);
}

#[test]
fn mobi_com_drm_e_marcado_e_nao_extrai_texto() {
    let r = livro(
        &mobi(&MobiSpec {
            cripto: 2,
            ..MobiSpec::default()
        }),
        None,
    );
    assert_eq!(r.estado, EstadoLeitura::Drm);
    assert!(r.paginas.is_empty());
    assert_eq!(valor(&r, Campo::Titulo).as_deref(), Some("Livro Móbi"));
}

#[test]
fn mobi_huff_cdic_so_metadados() {
    let r = livro(
        &mobi(&MobiSpec {
            compressao: 17480,
            ..MobiSpec::default()
        }),
        None,
    );
    assert_eq!(r.estado, EstadoLeitura::Lido);
    assert!(r.paginas.is_empty());
    assert!(r.avisos.iter().any(|a| a.contains("HUFF/CDIC")));
    assert_eq!(valor(&r, Campo::Titulo).as_deref(), Some("Livro Móbi"));
}

#[test]
fn mobi_isbn_invalido_no_exth_e_ignorado() {
    let r = livro(
        &mobi(&MobiSpec {
            isbn: Some("9788535902779"),
            ..MobiSpec::default()
        }),
        None,
    );
    assert!(r.avisos.iter().any(|a| a.contains("inválido")));
    // O ISBN válido do texto continua sendo achado.
    assert_eq!(r.isbns.len(), 1);
}

#[test]
fn mobi_fuzz_leve_nunca_entra_em_panico() {
    let base = mobi(&MobiSpec::default());
    let mut rng = Prng(0x9E37_79B9_7F4A_7C15);
    for i in 0..4000 {
        let mut d = base.clone();
        let n = 1 + (rng.proximo() % 8) as usize;
        for _ in 0..n {
            let pos = (rng.proximo() as usize) % d.len();
            d[pos] = rng.proximo() as u8;
        }
        if i % 5 == 0 {
            let corte = (rng.proximo() as usize) % d.len();
            d.truncate(corte);
        }
        if i % 7 == 0 {
            // Cabeçalho PalmDB/MOBI com valores extremos.
            for pos in [0x54usize, 0x58, 0x6C, 76, 78, 82] {
                if pos + 4 <= d.len() {
                    d[pos..pos + 4].copy_from_slice(&u32::MAX.to_be_bytes());
                }
            }
        }
        let _ = extrair_bytes(
            &d,
            Some("f.mobi"),
            Some(FormatoLivro::Mobi),
            &OpcoesExtracao::default(),
        );
    }
    // Bytes totalmente aleatórios com a assinatura no lugar.
    for _ in 0..500 {
        let mut d: Vec<u8> = (0..2000).map(|_| rng.proximo() as u8).collect();
        d[60..68].copy_from_slice(b"BOOKMOBI");
        let _ = extrair_bytes(&d, None, None, &OpcoesExtracao::default());
    }
}

#[test]
fn zips_e_xmls_corrompidos_nao_entram_em_panico() {
    let mut rng = Prng(42);
    for base in [
        epub3(),
        cbz(true),
        zip(&[("a.fb2", fb2_xml().into_bytes())]),
    ] {
        for _ in 0..150 {
            let mut d = base.clone();
            for _ in 0..4 {
                let pos = (rng.proximo() as usize) % d.len();
                d[pos] = rng.proximo() as u8;
            }
            let _ = extrair_bytes(&d, None, None, &OpcoesExtracao::default());
        }
    }
    let fb2 = fb2_xml().into_bytes();
    for _ in 0..200 {
        let mut d = fb2.clone();
        let corte = (rng.proximo() as usize) % d.len();
        d.truncate(corte);
        let _ = extrair_bytes(
            &d,
            None,
            Some(FormatoLivro::Fb2),
            &OpcoesExtracao::default(),
        );
    }
}

#[test]
fn fb2_metadados_capa_sumario_texto() {
    let r = livro(fb2_xml().as_bytes(), Some("x.fb2"));
    assert_eq!(r.formato, FormatoLivro::Fb2);
    assert_eq!(r.estado, EstadoLeitura::Lido);
    assert_eq!(valor(&r, Campo::Titulo).as_deref(), Some("A Fita de Teste"));
    assert_eq!(lista(&r, Campo::Autores), vec!["Ivan I. Petrov".to_owned()]);
    assert_eq!(lista(&r, Campo::Tags), vec!["sf_fantasy", "adventure"]);
    assert_eq!(
        valor(&r, Campo::Resumo).as_deref(),
        Some("Resumo da fita.\nSegundo parágrafo.")
    );
    assert_eq!(valor(&r, Campo::Idioma).as_deref(), Some("pt"));
    assert_eq!(valor(&r, Campo::Editora).as_deref(), Some("Editora Y"));
    assert_eq!(valor(&r, Campo::Data).as_deref(), Some("2003"));
    assert_eq!(valor(&r, Campo::Serie).as_deref(), Some("Ciclo Teste"));
    assert_eq!(valor(&r, Campo::IndiceSerie).as_deref(), Some("4"));
    assert_eq!(valor(&r, Campo::Isbn).as_deref(), Some("9788535902778"));
    assert_eq!(
        r.capa.as_deref(),
        Some(
            &[
                0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0, 1
            ][..]
        )
    );
    let tit: Vec<(&str, u8)> = r
        .sumario
        .iter()
        .map(|i| (i.titulo.as_str(), i.nivel))
        .collect();
    assert_eq!(tit, vec![("Capítulo Um", 0), ("Subseção", 1)]);
    assert!(r.paginas[0].texto.contains("Primeiro parágrafo do livro."));
}

#[test]
fn fb2_zip_e_codificacao_cp1251() {
    let d = zip(&[("livro.fb2", fb2_xml().into_bytes())]);
    let r = livro(&d, Some("livro.fb2.zip"));
    assert_eq!(r.formato, FormatoLivro::Fb2);
    assert_eq!(valor(&r, Campo::Titulo).as_deref(), Some("A Fita de Teste"));

    // "Война" em windows-1251.
    let mut x = b"<?xml version=\"1.0\" encoding=\"windows-1251\"?><FictionBook><description><title-info><book-title>".to_vec();
    x.extend_from_slice(&[0xC2, 0xEE, 0xE9, 0xED, 0xE0]);
    x.extend_from_slice(b"</book-title></title-info></description><body><section><p>texto</p></section></body></FictionBook>");
    let r = livro(&x, None);
    assert_eq!(valor(&r, Campo::Titulo).as_deref(), Some("Война"));
}

#[test]
fn extrair_arquivo_por_caminho() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("livro.epub");
    std::fs::write(&p, epub3()).unwrap();
    let e = extrair_arquivo(&p, None, &OpcoesExtracao::default()).unwrap();
    let r = e.livro().unwrap();
    assert_eq!(r.formato, FormatoLivro::Epub);
    assert_eq!(valor(r, Campo::Titulo).as_deref(), Some("O Livro de Teste"));
    assert!(
        extrair_arquivo(
            &dir.path().join("nao-existe.epub"),
            None,
            &OpcoesExtracao::default()
        )
        .is_err()
    );
    // Nada foi criado ao lado do arquivo.
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

/// Integração com o calibre: converte um EPUB gerado em MOBI/AZW3 e lê os metadados de volta.
#[test]
fn integracao_ebook_convert_mobi_e_azw3() {
    if std::process::Command::new("ebook-convert")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("AVISO: ebook-convert ausente; teste de integração com o calibre pulado");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let origem = dir.path().join("entrada.epub");
    std::fs::write(&origem, epub3()).unwrap();
    for (ext, esperado) in [("mobi", FormatoLivro::Mobi), ("azw3", FormatoLivro::Azw3)] {
        let destino = dir.path().join(format!("saida.{ext}"));
        let saida = std::process::Command::new("ebook-convert")
            .arg(&origem)
            .arg(&destino)
            .env("QT_QPA_PLATFORM", "offscreen")
            .output()
            .unwrap();
        if !saida.status.success() || !destino.exists() {
            eprintln!(
                "AVISO: ebook-convert falhou para {ext}: {}",
                String::from_utf8_lossy(&saida.stderr)
            );
            continue;
        }
        let e = extrair_arquivo(&destino, None, &OpcoesExtracao::default()).unwrap();
        let r = e.livro().unwrap();
        assert_eq!(r.estado, EstadoLeitura::Lido, "{ext}: {:?}", r.avisos);
        assert_eq!(r.formato, esperado, "{ext}");
        assert!(
            valor(r, Campo::Titulo)
                .unwrap()
                .starts_with("O Livro de Teste"),
            "{ext}"
        );
        assert!(
            lista(r, Campo::Autores).contains(&"Maria Silva".to_owned()),
            "{ext}: {:?}",
            lista(r, Campo::Autores)
        );
        assert_eq!(
            valor(r, Campo::Editora).as_deref(),
            Some("Editora X"),
            "{ext}"
        );
        if ext == "mobi" {
            assert!(!r.paginas.is_empty(), "texto PalmDOC real deve ser lido");
            assert!(r.paginas.iter().any(|p| p.texto.contains("IGNORE TODAS")));
        }
    }
}
