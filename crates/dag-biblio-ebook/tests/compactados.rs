#![allow(clippy::unwrap_used, clippy::expect_used)]
mod comum;

use std::io::{Cursor, Write};

use comum::*;
use dag_biblio::{Campo, EstadoLeitura, FormatoLivro, OrigemEvidencia, ResultadoExtracao};
use dag_biblio_ebook::{
    Extraido, LimitesConteiner, OpcoesExtracao, ResultadoCompactado, extrair_bytes,
};

fn livro(d: &[u8], nome: Option<&str>) -> ResultadoExtracao {
    match extrair_bytes(d, nome, None, &OpcoesExtracao::default()) {
        Extraido::Livro(r) => r,
        Extraido::Compactado(_) => panic!("esperava livro"),
    }
}

fn compactado(d: &[u8], opc: &OpcoesExtracao) -> ResultadoCompactado {
    match extrair_bytes(d, None, None, opc) {
        Extraido::Compactado(c) => c,
        Extraido::Livro(r) => panic!("esperava compactado, veio {:?}", r.estado),
    }
}

fn estado(d: &[u8], opc: &OpcoesExtracao) -> EstadoLeitura {
    extrair_bytes(d, None, None, opc).estado().clone()
}

fn rar(nomes: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut b = rars::Builder::new(rars::ArchiveVersion::Rar50);
    for (n, d) in nomes {
        b.add_bytes(n.as_bytes().to_vec(), d.clone(), None, None)
            .unwrap();
    }
    let mut out = Vec::new();
    b.write_to(&mut out, &rars::WriterResources::default(), None)
        .unwrap();
    out
}

fn sete_z(nomes: &[(&str, Vec<u8>)]) -> Vec<u8> {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
    let mut w = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    for (n, d) in nomes {
        w.push_archive_entry(ArchiveEntry::new_file(n), Some(Cursor::new(d.clone())))
            .unwrap();
    }
    w.finish().unwrap().into_inner()
}

// ----------------------------------------------------------------------------- quadrinhos

#[test]
fn cbz_com_comicinfo() {
    let r = livro(&cbz(true), Some("Tex 999.cbz"));
    assert_eq!(r.formato, FormatoLivro::Cbz);
    assert_eq!(r.estado, EstadoLeitura::Lido);
    assert_eq!(r.total_paginas, Some(3));
    assert_eq!(valor(&r, Campo::Serie).as_deref(), Some("Tex"));
    assert_eq!(valor(&r, Campo::IndiceSerie).as_deref(), Some("123"));
    assert_eq!(valor(&r, Campo::Volume).as_deref(), Some("2"));
    assert_eq!(valor(&r, Campo::Titulo).as_deref(), Some("O Retorno"));
    assert_eq!(
        lista(&r, Campo::Autores),
        vec!["Gianluigi Bonelli".to_owned(), "Mauro Boselli".to_owned()]
    );
    assert_eq!(valor(&r, Campo::Editora).as_deref(), Some("Mythos"));
    assert_eq!(valor(&r, Campo::Data).as_deref(), Some("1999-03"));
    assert_eq!(valor(&r, Campo::Idioma).as_deref(), Some("pt"));
    assert_eq!(valor(&r, Campo::Resumo).as_deref(), Some("Resumo do gibi."));
    // Ordem natural: a capa é p1.jpg (não p10.jpg).
    assert_eq!(r.capa.as_deref(), Some(&[&JPEG[..], b"1"].concat()[..]));
    // O número do nome do arquivo (999) discorda do ComicInfo: aviso, e vale o ComicInfo.
    assert!(r.avisos.iter().any(|a| a.contains("999")));
    assert!(!r.metadados.iter().any(|m| {
        m.evidencias
            .iter()
            .any(|e| e.origem == OrigemEvidencia::NomeArquivo)
    }));
}

#[test]
fn cbz_sem_comicinfo_usa_nome_do_arquivo_com_baixa_confianca() {
    let r = livro(&cbz(false), Some("Tex #045 (1990).cbz"));
    let n = r
        .metadados
        .iter()
        .find(|m| m.campo == Campo::IndiceSerie)
        .unwrap();
    assert_eq!(n.valor.as_deref(), Some("45"));
    assert_eq!(n.evidencias[0].origem, OrigemEvidencia::NomeArquivo);
    assert!(n.confianca < 0.5);
    assert_eq!(valor(&r, Campo::Serie).as_deref(), Some("Tex"));
    // Volume não é inventado a partir do número.
    assert_eq!(valor(&r, Campo::Volume), None);
    let r = livro(&cbz(false), Some("Sandman Vol. 2 #15.cbz"));
    assert_eq!(valor(&r, Campo::Volume).as_deref(), Some("2"));
    assert_eq!(valor(&r, Campo::IndiceSerie).as_deref(), Some("15"));
}

#[test]
fn zip_sem_imagens_nao_e_quadrinho() {
    let d = zip(&[
        ("a.txt", b"x".to_vec()),
        ("b.txt", b"y".to_vec()),
        ("c.jpg", JPEG.to_vec()),
    ]);
    let r = match extrair_bytes(
        &d,
        Some("fake.cbz"),
        Some(FormatoLivro::Cbz),
        &OpcoesExtracao::default(),
    ) {
        Extraido::Livro(r) => r,
        Extraido::Compactado(c) => panic!("{c:?}"),
    };
    assert!(
        matches!(r.estado, EstadoLeitura::NaoSuportado { ref motivo } if motivo.contains("quadrinho")),
        "{:?}",
        r.estado
    );
}

#[test]
fn cbr_e_cb7_com_imagens_e_comicinfo() {
    let entradas = [
        ("pag 02.png", [&PNG[..], b"2"].concat()),
        ("pag 01.png", [&PNG[..], b"1"].concat()),
        ("ComicInfo.xml", COMICINFO.as_bytes().to_vec()),
    ];
    let r = match extrair_bytes(
        &rar(&entradas),
        Some("x.cbr"),
        None,
        &OpcoesExtracao::default(),
    ) {
        Extraido::Livro(r) => r,
        Extraido::Compactado(c) => panic!("{c:?}"),
    };
    assert_eq!(r.formato, FormatoLivro::Cbr);
    assert_eq!(r.estado, EstadoLeitura::Lido, "{:?}", r.avisos);
    assert_eq!(r.total_paginas, Some(2));
    assert_eq!(valor(&r, Campo::Serie).as_deref(), Some("Tex"));
    assert_eq!(r.capa.as_deref(), Some(&[&PNG[..], b"1"].concat()[..]));
    assert_eq!(r.capa_mime.as_deref(), Some("image/png"));

    let r = match extrair_bytes(
        &sete_z(&entradas),
        Some("x.cb7"),
        None,
        &OpcoesExtracao::default(),
    ) {
        Extraido::Livro(r) => r,
        Extraido::Compactado(c) => panic!("{c:?}"),
    };
    assert_eq!(r.formato, FormatoLivro::Cb7);
    assert_eq!(r.total_paginas, Some(2));
    assert_eq!(valor(&r, Campo::IndiceSerie).as_deref(), Some("123"));
}

#[test]
fn comicinfo_com_capa_declarada_fora_da_primeira() {
    let info = r#"<ComicInfo><Series>S</Series><Pages><Page Image="1" Type="FrontCover"/></Pages></ComicInfo>"#;
    let d = zip(&[
        ("1.jpg", [&JPEG[..], b"um"].concat()),
        ("2.jpg", [&JPEG[..], b"dois"].concat()),
        ("ComicInfo.xml", info.as_bytes().to_vec()),
    ]);
    let r = livro(&d, None);
    assert_eq!(r.capa.as_deref(), Some(&[&JPEG[..], b"dois"].concat()[..]));
}

// ----------------------------------------------------------------------------- compactados

fn pdf() -> Vec<u8> {
    b"%PDF-1.4\n%fake".to_vec()
}

#[test]
fn livros_dentro_de_zip_rar_7z() {
    let membros = [
        ("pasta/livro.epub", epub3()),
        ("pasta/gibi.cbz", cbz(true)),
        ("manual.pdf", pdf()),
        ("leia-me.txt", b"nada".to_vec()),
        ("foto.jpg", JPEG.to_vec()),
    ];
    for (nome, bytes) in [
        ("zip", zip(&membros)),
        ("rar", rar(&membros)),
        ("7z", sete_z(&membros)),
    ] {
        let c = compactado(&bytes, &OpcoesExtracao::default());
        assert_eq!(c.conteiner, nome);
        assert_eq!(c.estado, EstadoLeitura::Lido, "{nome}: {:?}", c.avisos);
        let mut caminhos: Vec<&str> = c
            .membros
            .iter()
            .map(|m| m.caminho_interno.as_str())
            .collect();
        caminhos.sort_unstable();
        assert_eq!(
            caminhos,
            vec!["manual.pdf", "pasta/gibi.cbz", "pasta/livro.epub"],
            "{nome}"
        );
        let epub = c
            .membros
            .iter()
            .find(|m| m.formato == FormatoLivro::Epub)
            .unwrap();
        assert_eq!(
            valor(&epub.resultado, Campo::Titulo).as_deref(),
            Some("O Livro de Teste")
        );
        let gibi = c
            .membros
            .iter()
            .find(|m| m.formato == FormatoLivro::Cbz)
            .unwrap();
        assert_eq!(gibi.resultado.total_paginas, Some(3));
        assert_eq!(valor(&gibi.resultado, Campo::Serie).as_deref(), Some("Tex"));
        let pdf = c
            .membros
            .iter()
            .find(|m| m.formato == FormatoLivro::Pdf)
            .unwrap();
        assert!(matches!(
            pdf.resultado.estado,
            EstadoLeitura::NaoSuportado { .. }
        ));
    }
}

#[test]
fn caminhos_perigosos_sao_ignorados_com_aviso() {
    let d = zip(&[
        ("../../fora.epub", epub3()),
        ("/abs/outro.epub", epub3()),
        ("ok/..\\x/../legit.epub", epub3()),
        ("bom.epub", epub3()),
    ]);
    let c = compactado(&d, &OpcoesExtracao::default());
    let caminhos: Vec<&str> = c
        .membros
        .iter()
        .map(|m| m.caminho_interno.as_str())
        .collect();
    assert_eq!(caminhos, vec!["bom.epub"]);
    assert!(
        c.avisos.iter().filter(|a| a.contains("suspeito")).count() >= 2,
        "{:?}",
        c.avisos
    );
}

#[test]
fn aninhados_respeitam_profundidade() {
    let nivel3 = zip(&[("livro.epub", epub3())]);
    let nivel2 = zip(&[("n3.zip", nivel3)]);
    let nivel1 = zip(&[("n2.zip", nivel2)]);
    let c = compactado(&nivel1, &OpcoesExtracao::default());
    assert_eq!(c.membros.len(), 1);
    assert_eq!(c.membros[0].caminho_interno, "n2.zip!/n3.zip!/livro.epub");
    // Com profundidade 1, o terceiro nível é rejeitado com motivo, sem erro.
    let opc = OpcoesExtracao {
        limites: LimitesConteiner {
            profundidade_max: 1,
            ..LimitesConteiner::default()
        },
        ..OpcoesExtracao::default()
    };
    let c = compactado(&nivel1, &opc);
    assert!(c.membros.is_empty());
    assert!(
        c.rejeitados.iter().any(|(_, m)| m.contains("profundidade")),
        "{:?}",
        c.rejeitados
    );
}

/// Zip com `n` MiB de zeros (deflate comprime ~1000:1).
fn bomba(mib: usize, nome: &str) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    z.start_file(
        nome,
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
    )
    .unwrap();
    let bloco = vec![0u8; 1 << 20];
    for _ in 0..mib {
        z.write_all(&bloco).unwrap();
    }
    z.finish().unwrap().into_inner()
}

fn opc_bomba() -> OpcoesExtracao {
    OpcoesExtracao {
        limites: LimitesConteiner {
            piso_razao: 1 << 20,
            ..LimitesConteiner::default()
        },
        ..OpcoesExtracao::default()
    }
}

#[test]
fn zip_bomb_e_rejeitada_pela_razao_de_expansao_sem_descompactar() {
    let b = bomba(40, "zeros.epub");
    assert!(
        b.len() < 100_000,
        "a bomba deveria ser minúscula: {}",
        b.len()
    );
    let e = extrair_bytes(&b, None, None, &opc_bomba());
    assert!(
        matches!(e.estado(), EstadoLeitura::LimiteExcedido { limite } if limite.contains("expansão")),
        "{:?}",
        e.estado()
    );
    assert!(
        e.livro()
            .is_some_and(|r| r.paginas.is_empty() && r.capa.is_none())
    );
}

#[test]
fn bomba_como_epub_e_como_quadrinho_tambem_e_rejeitada() {
    let b = bomba(40, "livro.xhtml");
    let r = match extrair_bytes(&b, Some("x.epub"), Some(FormatoLivro::Epub), &opc_bomba()) {
        Extraido::Livro(r) => r,
        Extraido::Compactado(c) => panic!("{c:?}"),
    };
    assert!(
        matches!(r.estado, EstadoLeitura::LimiteExcedido { .. }),
        "{:?}",
        r.estado
    );
}

#[test]
fn bomba_aninhada_e_limite_de_bytes_lidos() {
    // Interno minúsculo (bomba) dentro de zip externo: a razão considera o arquivo raiz.
    let interno = bomba(40, "zeros.bin");
    let externo = zip(&[("bomba.zip", interno)]);
    let st = estado(&externo, &opc_bomba());
    assert!(matches!(st, EstadoLeitura::LimiteExcedido { .. }), "{st:?}");

    // Limite absoluto de bytes lidos.
    let opc = OpcoesExtracao {
        limites: LimitesConteiner {
            bytes_lidos_max: 10_000,
            ..LimitesConteiner::default()
        },
        ..OpcoesExtracao::default()
    };
    let st = estado(&zip(&[("a.epub", epub3()), ("b.epub", epub3())]), &opc);
    assert!(
        matches!(&st, EstadoLeitura::LimiteExcedido { limite } if limite.contains("bytes")),
        "{st:?}"
    );
}

#[test]
fn limite_de_membros_e_de_tempo() {
    let entradas: Vec<(String, Vec<u8>)> = (0..50)
        .map(|i| (format!("f{i}.txt"), vec![b'x'; 10]))
        .collect();
    let refs: Vec<(&str, Vec<u8>)> = entradas
        .iter()
        .map(|(n, d)| (n.as_str(), d.clone()))
        .collect();
    let d = zip(&refs);
    let opc = OpcoesExtracao {
        limites: LimitesConteiner {
            membros_max: 10,
            ..LimitesConteiner::default()
        },
        ..OpcoesExtracao::default()
    };
    let st = estado(&d, &opc);
    assert!(
        matches!(&st, EstadoLeitura::LimiteExcedido { limite } if limite.contains("membros")),
        "{st:?}"
    );

    let opc = OpcoesExtracao {
        limites: LimitesConteiner {
            tempo_max: std::time::Duration::ZERO,
            ..LimitesConteiner::default()
        },
        ..OpcoesExtracao::default()
    };
    std::thread::sleep(std::time::Duration::from_millis(2));
    let st = estado(&zip(&[("a.epub", epub3())]), &opc);
    assert!(
        matches!(&st, EstadoLeitura::LimiteExcedido { limite } if limite.contains("tempo")),
        "{st:?}"
    );
}

/// A classificação do zip (EPUB? CBZ? compactado?) percorre os Membros uma vez e a extração
/// percorre de novo: cada Membro conta uma vez só no limite, não duas.
#[test]
fn membros_nao_contam_duas_vezes_entre_classificar_e_extrair() {
    let entradas: Vec<(String, Vec<u8>)> = (0..7)
        .map(|i| (format!("f{i}.txt"), vec![b'x'; 10]))
        .collect();
    let refs: Vec<(&str, Vec<u8>)> = entradas
        .iter()
        .map(|(n, d)| (n.as_str(), d.clone()))
        .collect();
    let opc = OpcoesExtracao {
        limites: LimitesConteiner {
            membros_max: 10,
            ..LimitesConteiner::default()
        },
        ..OpcoesExtracao::default()
    };
    let c = compactado(&zip(&refs), &opc);
    assert_eq!(c.estado, EstadoLeitura::Lido, "{:?}", c.estado);
    assert_eq!(c.membros_vistos, 7);
}

#[test]
fn zip_com_senha_e_corrompido() {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default()
        .with_aes_encryption(zip::AesMode::Aes256, "segredo");
    z.start_file("livro.epub", o).unwrap();
    z.write_all(&epub3()).unwrap();
    let d = z.finish().unwrap().into_inner();
    assert_eq!(estado(&d, &OpcoesExtracao::default()), EstadoLeitura::Senha);

    let mut ruim = zip(&[("livro.epub", epub3())]);
    ruim.truncate(ruim.len() - 30);
    let r = extrair_bytes(&ruim, None, None, &OpcoesExtracao::default());
    match r {
        Extraido::Livro(l) => assert!(
            matches!(l.estado, EstadoLeitura::Corrompido { .. }),
            "{:?}",
            l.estado
        ),
        Extraido::Compactado(c) => assert!(
            matches!(c.estado, EstadoLeitura::Corrompido { .. }),
            "{:?}",
            c.estado
        ),
    }
}

#[test]
fn retem_dados_dos_formatos_nao_tratados_se_pedido() {
    let d = zip(&[("manual.pdf", pdf()), ("a.epub", epub3())]);
    let opc = OpcoesExtracao {
        reter_dados_nao_tratados: true,
        ..OpcoesExtracao::default()
    };
    let c = compactado(&d, &opc);
    let p = c
        .membros
        .iter()
        .find(|m| m.formato == FormatoLivro::Pdf)
        .unwrap();
    assert_eq!(p.dados.as_deref(), Some(&pdf()[..]));
    let e = c
        .membros
        .iter()
        .find(|m| m.formato == FormatoLivro::Epub)
        .unwrap();
    assert!(e.dados.is_none());
}
