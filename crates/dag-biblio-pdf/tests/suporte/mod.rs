//! Fixtures dos testes: gerador mínimo de PDF (texto, imagem, marcadores, cifra falsa) e fonte
//! bitmap 5x7 para criar páginas-imagem que o tesseract consegue ler. Tudo em memória.
#![allow(dead_code)]

use std::io::Write;

use dag_biblio_pdf::{ConfigExtracaoPdf, Extrator};
use flate2::Compression;
use flate2::write::ZlibEncoder;

/// Extrator com as ferramentas do sistema, ou `None` (teste pulado, com aviso) se faltarem.
pub fn extrator(config: ConfigExtracaoPdf) -> Option<Extrator> {
    match Extrator::detectar(config) {
        Ok(e) => Some(e),
        Err(e) => {
            eprintln!("AVISO: teste pulado, ferramenta ausente: {e}");
            None
        }
    }
}

/// Idem, exigindo também o tesseract com `eng` (OCR).
pub fn extrator_com_ocr(config: ConfigExtracaoPdf) -> Option<Extrator> {
    let e = extrator(config)?;
    if e.ferramentas().tesseract.is_none()
        || !e.ferramentas().idiomas_tesseract.iter().any(|i| i == "eng")
    {
        eprintln!("AVISO: teste pulado, tesseract (eng) ausente");
        return None;
    }
    Some(e)
}

const FONTE: [(char, [u8; 7]); 41] = [
    ('A', [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    ('B', [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E]),
    ('C', [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E]),
    ('D', [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E]),
    ('E', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F]),
    ('F', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10]),
    ('G', [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F]),
    ('H', [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    ('I', [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ('J', [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C]),
    ('K', [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
    ('L', [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F]),
    ('M', [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11]),
    ('N', [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11]),
    ('O', [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ('P', [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10]),
    ('Q', [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D]),
    ('R', [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11]),
    ('S', [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E]),
    ('T', [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
    ('U', [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ('V', [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04]),
    ('W', [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A]),
    ('X', [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11]),
    ('Y', [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04]),
    ('Z', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F]),
    ('0', [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E]),
    ('1', [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ('2', [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F]),
    ('3', [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E]),
    ('4', [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02]),
    ('5', [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E]),
    ('6', [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E]),
    ('7', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
    ('8', [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E]),
    ('9', [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C]),
    ('.', [0, 0, 0, 0, 0, 0x0C, 0x0C]),
    ('-', [0, 0, 0, 0x1F, 0, 0, 0]),
    (':', [0, 0x0C, 0x0C, 0, 0x0C, 0x0C, 0]),
    (',', [0, 0, 0, 0, 0x0C, 0x04, 0x08]),
    (' ', [0; 7]),
];

const ESCALA: usize = 7;
pub const LARGURA_IMG: usize = 1800;
const DPI_IMG: f64 = 300.0;

/// Rasteriza linhas (maiúsculas, dígitos e `.-:,`) em cinza de 8 bits, levemente suavizado.
pub fn rasterizar(linhas: &[&str]) -> (usize, usize, Vec<u8>) {
    let alt = linhas.len() * 100 + 100;
    let mut px = vec![255u8; LARGURA_IMG * alt];
    for (n, linha) in linhas.iter().enumerate() {
        let y0 = 50 + n * 100;
        for (i, ch) in linha.to_uppercase().chars().enumerate() {
            let Some((_, g)) = FONTE.iter().find(|(c, _)| *c == ch) else {
                continue;
            };
            let x0 = 50 + i * 6 * ESCALA;
            if x0 + 5 * ESCALA >= LARGURA_IMG {
                break;
            }
            for (r, bits) in g.iter().enumerate() {
                for c in 0..5 {
                    if bits & (0x10 >> c) != 0 {
                        for dy in 0..ESCALA {
                            for dx in 0..ESCALA {
                                px[(y0 + r * ESCALA + dy) * LARGURA_IMG + x0 + c * ESCALA + dx] = 0;
                            }
                        }
                    }
                }
            }
        }
    }
    // Suavização 3x3 (simula antialiasing de um scanner).
    let mut suave = px.clone();
    for y in 1..alt - 1 {
        for x in 1..LARGURA_IMG - 1 {
            let mut s = 0u32;
            for dy in 0..3 {
                for dx in 0..3 {
                    s += u32::from(px[(y + dy - 1) * LARGURA_IMG + x + dx - 1]);
                }
            }
            suave[y * LARGURA_IMG + x] = (s / 9) as u8;
        }
    }
    (LARGURA_IMG, alt, suave)
}

/// PPM (P6) de uma página-imagem, para o `c44` do djvulibre.
pub fn ppm_de_texto(linhas: &[&str]) -> Vec<u8> {
    let (w, h, gray) = rasterizar(linhas);
    let mut v = format!("P6\n{w} {h}\n255\n").into_bytes();
    for g in gray {
        v.extend_from_slice(&[g, g, g]);
    }
    v
}

/// Uma página do PDF de teste.
#[derive(Default, Clone)]
pub struct PaginaPdf {
    /// Linhas de texto nativo (Latin-1/WinAnsi).
    pub linhas: Vec<String>,
    /// Linhas rasterizadas numa imagem de página inteira (sem texto nativo).
    pub imagem: Vec<String>,
}

impl PaginaPdf {
    pub fn texto(linhas: &[&str]) -> Self {
        Self {
            linhas: linhas.iter().map(|s| (*s).to_string()).collect(),
            imagem: Vec::new(),
        }
    }

    pub fn imagem(linhas: &[&str]) -> Self {
        Self {
            linhas: Vec::new(),
            imagem: linhas.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cifra {
    Nenhuma,
    /// `/Encrypt` Standard com O/U inválidos: exige senha (a vazia não abre).
    Senha,
    /// `/Encrypt` com `/Filter /EBX_HANDLER` (Adobe ADEPT).
    Adept,
}

pub struct DocPdf {
    pub paginas: Vec<PaginaPdf>,
    pub info: Vec<(String, String)>,
    /// (título, página 1-based).
    pub marcadores: Vec<(String, usize)>,
    pub cifra: Cifra,
}

impl DocPdf {
    pub fn novo(paginas: Vec<PaginaPdf>) -> Self {
        Self {
            paginas,
            info: Vec::new(),
            marcadores: Vec::new(),
            cifra: Cifra::Nenhuma,
        }
    }
}

fn latin1(s: &str) -> Vec<u8> {
    let mut v = Vec::new();
    for c in s.chars() {
        let b = if (c as u32) < 256 { c as u8 } else { b'?' };
        if matches!(b, b'(' | b')' | b'\\') {
            v.push(b'\\');
        }
        v.push(b);
    }
    v
}

fn zlib(dados: &[u8]) -> Vec<u8> {
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(dados).unwrap();
    z.finish().unwrap()
}

/// Gera os bytes do PDF.
pub fn gerar(doc: &DocPdf) -> Vec<u8> {
    let n = doc.paginas.len();
    // Numeração: 1 catálogo, 2 páginas, 3 fonte, 4 info, 5 encrypt, 6 outlines; depois 3 por página.
    let obj_pagina = |i: usize| 7 + i * 3;
    let mut objetos: Vec<(usize, Vec<u8>)> = Vec::new();
    let kids: String = (0..n)
        .map(|i| format!("{} 0 R", obj_pagina(i)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut catalogo = "<< /Type /Catalog /Pages 2 0 R".to_string();
    if !doc.marcadores.is_empty() {
        catalogo.push_str(" /Outlines 6 0 R");
    }
    catalogo.push_str(" >>");
    objetos.push((1, catalogo.into_bytes()));
    objetos.push((
        2,
        format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>").into_bytes(),
    ));
    objetos.push((
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    ));
    let mut info = b"<< ".to_vec();
    for (k, v) in &doc.info {
        info.extend_from_slice(format!("/{k} (").as_bytes());
        info.extend_from_slice(&latin1(v));
        info.extend_from_slice(b") ");
    }
    info.extend_from_slice(b">>");
    objetos.push((4, info));
    match doc.cifra {
        Cifra::Nenhuma => {}
        Cifra::Senha => objetos.push((
            5,
            format!(
                "<< /Filter /Standard /V 1 /R 2 /O <{0}> /U <{1}> /P -4 >>",
                "AB".repeat(32),
                "CD".repeat(32)
            )
            .into_bytes(),
        )),
        Cifra::Adept => objetos.push((
            5,
            b"<< /Filter /EBX_HANDLER /V 4 /SubFilter /ADEPT.4 /Length 128 >>".to_vec(),
        )),
    }
    if !doc.marcadores.is_empty() {
        let m = doc.marcadores.len();
        let primeiro = 7 + n * 3;
        objetos.push((
            6,
            format!(
                "<< /Type /Outlines /First {primeiro} 0 R /Last {} 0 R /Count {m} >>",
                primeiro + m - 1
            )
            .into_bytes(),
        ));
        for (i, (titulo, pag)) in doc.marcadores.iter().enumerate() {
            let mut s = b"<< /Parent 6 0 R /Title (".to_vec();
            s.extend_from_slice(&latin1(titulo));
            s.extend_from_slice(format!(") /Dest [{} 0 R /Fit]", obj_pagina(pag - 1)).as_bytes());
            if i > 0 {
                s.extend_from_slice(format!(" /Prev {} 0 R", primeiro + i - 1).as_bytes());
            }
            if i + 1 < m {
                s.extend_from_slice(format!(" /Next {} 0 R", primeiro + i + 1).as_bytes());
            }
            s.extend_from_slice(b" >>");
            objetos.push((primeiro + i, s));
        }
    }
    for (i, p) in doc.paginas.iter().enumerate() {
        let (po, co, io) = (obj_pagina(i), obj_pagina(i) + 1, obj_pagina(i) + 2);
        if p.imagem.is_empty() {
            let mut cont = b"BT /F1 11 Tf 14 TL 50 780 Td\n".to_vec();
            for l in &p.linhas {
                cont.push(b'(');
                cont.extend_from_slice(&latin1(l));
                cont.extend_from_slice(b") Tj T*\n");
            }
            cont.extend_from_slice(b"ET");
            objetos.push((
                po,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> >> /Contents {co} 0 R >>"
                )
                .into_bytes(),
            ));
            let mut s = format!("<< /Length {} >>\nstream\n", cont.len()).into_bytes();
            s.extend_from_slice(&cont);
            s.extend_from_slice(b"\nendstream");
            objetos.push((co, s));
        } else {
            let linhas: Vec<&str> = p.imagem.iter().map(String::as_str).collect();
            let (w, h, gray) = rasterizar(&linhas);
            let (wp, hp) = (w as f64 * 72.0 / DPI_IMG, h as f64 * 72.0 / DPI_IMG);
            objetos.push((
                po,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {wp:.2} {hp:.2}] /Resources << /XObject << /Im1 {io} 0 R >> >> /Contents {co} 0 R >>"
                )
                .into_bytes(),
            ));
            let cont = format!("q {wp:.2} 0 0 {hp:.2} 0 0 cm /Im1 Do Q").into_bytes();
            let mut s = format!("<< /Length {} >>\nstream\n", cont.len()).into_bytes();
            s.extend_from_slice(&cont);
            s.extend_from_slice(b"\nendstream");
            objetos.push((co, s));
            let z = zlib(&gray);
            let mut s = format!(
                "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
                z.len()
            )
            .into_bytes();
            s.extend_from_slice(&z);
            s.extend_from_slice(b"\nendstream");
            objetos.push((io, s));
        }
        if p.imagem.is_empty() {
            // Objeto de imagem vazio para manter a numeração contínua.
            objetos.push((io, b"null".to_vec()));
        }
    }
    objetos.sort_by_key(|(n, _)| *n);
    let mut pdf = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = vec![0usize; objetos.last().map_or(0, |o| o.0) + 1];
    for (num, corpo) in &objetos {
        offsets[*num] = pdf.len();
        pdf.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
        pdf.extend_from_slice(corpo);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    let tam = offsets.len();
    pdf.extend_from_slice(format!("xref\n0 {tam}\n0000000000 65535 f \n").as_bytes());
    for (num, off) in offsets.iter().enumerate().skip(1) {
        if objetos.iter().any(|(n, _)| *n == num) {
            pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        } else {
            pdf.extend_from_slice(b"0000000000 65535 f \n");
        }
    }
    let mut trailer = format!("trailer\n<< /Size {tam} /Root 1 0 R /Info 4 0 R");
    if doc.cifra != Cifra::Nenhuma {
        trailer.push_str(" /Encrypt 5 0 R /ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]");
    }
    trailer.push_str(&format!(" >>\nstartxref\n{xref}\n%%EOF\n"));
    pdf.extend_from_slice(trailer.as_bytes());
    pdf
}

/// Páginas de livro "normal": capa, folha de rosto, ficha CIP (ISBN válido e um inválido) e miolo.
pub fn livro_textual() -> DocPdf {
    let mut paginas = vec![
        PaginaPdf::texto(&["DOM CASMURRO", "", "Machado de Assis"]),
        PaginaPdf::texto(&[
            "Dom Casmurro",
            "romance",
            "",
            "Machado de Assis",
            "",
            "Companhia das Letras",
        ]),
        PaginaPdf::texto(&[
            "Dados Internacionais de Catalogação na Publicação (CIP)",
            "(Câmara Brasileira do Livro, SP, Brasil)",
            "",
            "Assis, Machado de, 1839-1908.",
            "Dom Casmurro : romance / Machado de Assis. -- 2. ed. --",
            "São Paulo : Companhia das Letras, 2015.",
            "",
            "ISBN 978-85-359-0277-8",
            "ISBN 978-85-359-0277-5 (cópia com erro de digitação)",
            "",
            "1. Romance brasileiro I. Título.",
            "",
            "CDD 869.3",
            "Bibliotecária responsável: Maria da Silva CRB-8/0000",
        ]),
        PaginaPdf::texto(&[
            "Sumário",
            "",
            "Capítulo I - Do título ........ 5",
            "Capítulo II - Do livro ........ 9",
            "Capítulo III - A denúncia ........ 14",
            "Capítulo IV - Um dever amaríssimo ........ 20",
        ]),
    ];
    for i in 5..=8 {
        paginas.push(PaginaPdf::texto(&[
            &format!("Capítulo {i}"),
            "",
            "Uma tarde, quando a casa estava em silêncio, ele voltou para dizer que tudo seria diferente",
            "daquele dia em diante, porque não havia mais o que esperar e uma decisão precisava ser tomada",
            "com calma, para que nada fosse perdido no caminho entre o desejo e a vontade de seguir adiante.",
        ]));
    }
    let mut doc = DocPdf::novo(paginas);
    doc.info = vec![
        ("Title".into(), "Microsoft Word - doc1.docx".into()),
        ("Author".into(), "Machado de Assis".into()),
        ("Producer".into(), "Gerador de teste".into()),
    ];
    doc
}
