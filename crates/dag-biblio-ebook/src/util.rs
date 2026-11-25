//! Utilidades comuns: nomes, texto, HTML, codificações, paginação e montagem dos resultados.

use std::cmp::Ordering;

use dag_biblio::{
    Campo, EstadoLeitura, Evidencia, FormatoLivro, OrigemEvidencia, PaginaTexto, ResultadoExtracao,
    ValorSugerido,
};

// ---------------------------------------------------------------------------------------------
// Resultado
// ---------------------------------------------------------------------------------------------

/// Resultado sem nada extraído ainda.
pub(crate) fn vazio(formato: FormatoLivro, estado: EstadoLeitura) -> ResultadoExtracao {
    ResultadoExtracao {
        formato,
        estado,
        metadados: Vec::new(),
        isbns: Vec::new(),
        sumario: Vec::new(),
        paginas: Vec::new(),
        total_paginas: None,
        ficha_catalografica: None,
        capa: None,
        capa_mime: None,
        paginas_ocr: Vec::new(),
        avisos: Vec::new(),
    }
}

pub(crate) fn evidencia(detalhe: &str, confianca: f32) -> Evidencia {
    Evidencia {
        origem: OrigemEvidencia::MetadadoInterno,
        pagina: None,
        trecho: None,
        detalhe: Some(detalhe.to_owned()),
        confianca,
    }
}

/// Valor simples de um metadado interno.
pub(crate) fn valor(campo: Campo, v: &str, confianca: f32, detalhe: &str) -> ValorSugerido {
    ValorSugerido {
        campo,
        valor: Some(v.to_owned()),
        lista: Vec::new(),
        confianca,
        evidencias: vec![evidencia(detalhe, confianca)],
    }
}

/// Lista (autores, tags): `valor` junta com ` & ` como no Calibre; `lista` é a fonte da verdade.
pub(crate) fn lista(
    campo: Campo,
    itens: Vec<String>,
    confianca: f32,
    detalhe: &str,
) -> ValorSugerido {
    ValorSugerido {
        campo,
        valor: Some(itens.join(" & ")),
        lista: itens,
        confianca,
        evidencias: vec![evidencia(detalhe, confianca)],
    }
}

/// Acrescenta se o texto não é vazio.
pub(crate) fn adicionar(
    r: &mut ResultadoExtracao,
    campo: Campo,
    v: Option<&str>,
    confianca: f32,
    detalhe: &str,
) {
    if let Some(t) = v.map(str::trim).filter(|t| !t.is_empty()) {
        r.metadados.push(valor(campo, t, confianca, detalhe));
    }
}

pub(crate) fn adicionar_lista(
    r: &mut ResultadoExtracao,
    campo: Campo,
    itens: Vec<String>,
    confianca: f32,
    detalhe: &str,
) {
    let itens: Vec<String> = itens
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if !itens.is_empty() {
        r.metadados.push(lista(campo, itens, confianca, detalhe));
    }
}

// ---------------------------------------------------------------------------------------------
// Nomes de membros
// ---------------------------------------------------------------------------------------------

/// Nome de membro em UTF-8 (com perda se preciso), barras `/`, sem `./` inicial.
pub(crate) fn normalizar_nome(bruto: &[u8]) -> String {
    let s = String::from_utf8_lossy(bruto).replace('\\', "/");
    let mut s = s.as_str();
    while let Some(r) = s.strip_prefix("./") {
        s = r;
    }
    s.to_owned()
}

/// `false` para caminho absoluto, com `..`, unidade (`C:`), NUL ou vazio.
pub(crate) fn caminho_seguro(nome: &str) -> bool {
    if nome.is_empty() || nome.contains('\0') || nome.starts_with('/') {
        return false;
    }
    let b = nome.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return false;
    }
    !nome.split('/').any(|c| c == "..")
}

pub(crate) fn extensao(nome: &str) -> String {
    let base = nome.rsplit('/').next().unwrap_or(nome);
    match base.rsplit_once('.') {
        Some((_, e)) => e.to_ascii_lowercase(),
        None => String::new(),
    }
}

pub(crate) fn nome_base(nome: &str) -> &str {
    nome.rsplit('/').next().unwrap_or(nome)
}

pub(crate) fn diretorio(nome: &str) -> &str {
    nome.rsplit_once('/').map_or("", |(d, _)| d)
}

pub(crate) fn e_imagem(nome: &str) -> bool {
    matches!(
        extensao(nome).as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "avif" | "tif" | "tiff" | "jp2" | "jxl"
    )
}

/// Resolve `href` (de um OPF/HTML) relativo ao diretório `base`. `None` se sai da raiz.
pub(crate) fn resolver_href(base: &str, href: &str) -> Option<String> {
    let sem = href.split(['#', '?']).next().unwrap_or("");
    if sem.is_empty() || sem.contains("://") || sem.starts_with("data:") {
        return None;
    }
    let dec = percent_decodificar(sem);
    let completo = if let Some(r) = dec.strip_prefix('/') {
        r.to_owned()
    } else if base.is_empty() {
        dec
    } else {
        format!("{base}/{dec}")
    };
    let mut partes: Vec<&str> = Vec::new();
    for c in completo.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                partes.pop()?;
            }
            outro => partes.push(outro),
        }
    }
    if partes.is_empty() {
        None
    } else {
        Some(partes.join("/"))
    }
}

fn percent_decodificar(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Ordem natural (`pagina2` < `pagina10`), sem distinguir caixa.
pub(crate) fn comparar_natural(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(&c) = x.peek() {
                    if !c.is_ascii_digit() {
                        break;
                    }
                    na.push(c);
                    x.next();
                }
                let mut nb = String::new();
                while let Some(&c) = y.peek() {
                    if !c.is_ascii_digit() {
                        break;
                    }
                    nb.push(c);
                    y.next();
                }
                let ta = na.trim_start_matches('0');
                let tb = nb.trim_start_matches('0');
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(ca), Some(cb)) => {
                let la = ca.to_lowercase().next().unwrap_or(ca);
                let lb = cb.to_lowercase().next().unwrap_or(cb);
                if la != lb {
                    return la.cmp(&lb);
                }
                x.next();
                y.next();
            }
        }
    }
}

/// MIME de uma imagem pelos bytes mágicos.
pub(crate) fn mime_imagem(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else if b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.starts_with(b"BM") && b.len() > 26 {
        Some("image/bmp")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Codificações
// ---------------------------------------------------------------------------------------------

const CP1252_ALTO: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

const CP1251_ALTO: [u16; 64] = [
    0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021, 0x20AC, 0x2030, 0x0409, 0x2039,
    0x040A, 0x040C, 0x040B, 0x040F, 0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0098, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F, 0x00A0, 0x040E, 0x045E, 0x0408,
    0x00A4, 0x0490, 0x00A6, 0x00A7, 0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407,
    0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7, 0x0451, 0x2116, 0x0454, 0x00BB,
    0x0458, 0x0405, 0x0455, 0x0457,
];

fn de_u16(u: u16) -> char {
    char::from_u32(u32::from(u)).unwrap_or('\u{FFFD}')
}

pub(crate) fn decodificar_cp1252(b: &[u8]) -> String {
    b.iter()
        .map(|&c| match c {
            0x80..=0x9F => de_u16(CP1252_ALTO[usize::from(c - 0x80)]),
            _ => char::from(c),
        })
        .collect()
}

fn decodificar_cp1251(b: &[u8]) -> String {
    b.iter()
        .map(|&c| match c {
            0x80..=0xBF => de_u16(CP1251_ALTO[usize::from(c - 0x80)]),
            0xC0..=0xFF => de_u16(0x0410 + u16::from(c - 0xC0)),
            _ => char::from(c),
        })
        .collect()
}

fn decodificar_utf16(b: &[u8], big: bool) -> String {
    let unidades = b.as_chunks::<2>().0.iter().map(|p| {
        if big {
            u16::from_be_bytes(*p)
        } else {
            u16::from_le_bytes(*p)
        }
    });
    char::decode_utf16(unidades)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect()
}

/// Codificação declarada na declaração XML (`<?xml ... encoding="..."?>`), em minúsculas.
pub(crate) fn codificacao_xml(b: &[u8]) -> Option<String> {
    let cabeca = &b[..b.len().min(200)];
    let txt = String::from_utf8_lossy(cabeca).to_ascii_lowercase();
    let i = txt.find("encoding")?;
    let resto = &txt[i + 8..];
    let resto = resto.trim_start().strip_prefix('=')?.trim_start();
    let aspas = resto.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let resto = &resto[1..];
    let fim = resto.find(aspas)?;
    Some(resto[..fim].trim().to_owned())
}

/// Bytes para texto: BOM, UTF-16, cp1251, cp1252/latin1; o resto é UTF-8 (com perda).
pub(crate) fn decodificar(b: &[u8], codificacao: Option<&str>) -> String {
    if let Some(r) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(r).into_owned();
    }
    if let Some(r) = b.strip_prefix(&[0xFF, 0xFE]) {
        return decodificar_utf16(r, false);
    }
    if let Some(r) = b.strip_prefix(&[0xFE, 0xFF]) {
        return decodificar_utf16(r, true);
    }
    match codificacao.map(str::to_ascii_lowercase).as_deref() {
        Some("windows-1251" | "cp1251" | "win-1251") => decodificar_cp1251(b),
        Some(
            "windows-1252" | "cp1252" | "iso-8859-1" | "latin1" | "latin-1" | "iso-8859-15"
            | "us-ascii",
        ) => decodificar_cp1252(b),
        Some("utf-16le") => decodificar_utf16(b, false),
        Some("utf-16be" | "utf-16") => decodificar_utf16(b, true),
        _ => String::from_utf8_lossy(b).into_owned(),
    }
}

/// Texto de um documento XML/HTML, olhando a declaração de codificação.
pub(crate) fn decodificar_documento(b: &[u8]) -> String {
    let enc = codificacao_xml(b);
    decodificar(b, enc.as_deref())
}

// ---------------------------------------------------------------------------------------------
// Entidades e XML seguro
// ---------------------------------------------------------------------------------------------

const LATIN1: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf",
    "laquo", "not", "shy", "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro",
    "para", "middot", "cedil", "sup1", "ordm", "raquo", "frac14", "frac12", "frac34", "iquest",
    "Agrave", "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig", "Ccedil", "Egrave", "Eacute",
    "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH", "Ntilde", "Ograve", "Oacute",
    "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc", "Uuml", "Yacute",
    "THORN", "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig", "ccedil",
    "egrave", "eacute", "ecirc", "euml", "igrave", "iacute", "icirc", "iuml", "eth", "ntilde",
    "ograve", "oacute", "ocirc", "otilde", "ouml", "divide", "oslash", "ugrave", "uacute", "ucirc",
    "uuml", "yacute", "thorn", "yuml",
];

fn entidade_nomeada(nome: &str) -> Option<char> {
    if let Some(i) = LATIN1.iter().position(|n| *n == nome) {
        return char::from_u32(160 + i as u32);
    }
    Some(match nome {
        "ndash" => '\u{2013}',
        "mdash" => '\u{2014}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "sbquo" => '\u{201A}',
        "ldquo" => '\u{201C}',
        "rdquo" => '\u{201D}',
        "bdquo" => '\u{201E}',
        "hellip" => '\u{2026}',
        "bull" => '\u{2022}',
        "euro" => '\u{20AC}',
        "trade" => '\u{2122}',
        "dagger" => '\u{2020}',
        "prime" => '\u{2032}',
        "ensp" | "emsp" | "thinsp" => ' ',
        "zwj" | "zwnj" => '\u{200D}',
        _ => return None,
    })
}

/// Decodifica uma referência `&...;` (sem o `&` e o `;`) de texto HTML.
fn entidade_texto(nome: &str) -> Option<char> {
    if let Some(n) = nome.strip_prefix('#') {
        let cod = if let Some(h) = n.strip_prefix(['x', 'X']) {
            u32::from_str_radix(h, 16).ok()?
        } else {
            n.parse::<u32>().ok()?
        };
        return char::from_u32(cod);
    }
    match nome {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => entidade_nomeada(nome),
    }
}

fn controle_invalido_xml(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}')
}

/// Remove `<!DOCTYPE ...>` (com subconjunto interno). Sem DTD não há entidades definidas pelo
/// documento: "billion laughs" e entidades externas deixam de existir.
fn remover_doctype(s: &str) -> String {
    let maiusc = s.to_ascii_uppercase();
    let Some(ini) = maiusc.find("<!DOCTYPE") else {
        return s.to_owned();
    };
    let b = s.as_bytes();
    let mut i = ini + 9;
    let mut profundidade = 0i32;
    let mut aspas: Option<u8> = None;
    while i < b.len() {
        let c = b[i];
        match aspas {
            Some(q) => {
                if c == q {
                    aspas = None;
                }
            }
            None => match c {
                b'"' | b'\'' => aspas = Some(c),
                b'[' => profundidade += 1,
                b']' => profundidade -= 1,
                b'>' if profundidade <= 0 => {
                    return format!("{}{}", &s[..ini], &s[i + 1..]);
                }
                _ => {}
            },
        }
        i += 1;
    }
    s[..ini].to_owned()
}

/// Deixa um texto XML/XHTML/HTML-ish digerível pelo `roxmltree`: sem DOCTYPE, entidades HTML
/// nomeadas viram caracteres, entidades desconhecidas viram espaço, `&` solto vira `&amp;`.
pub(crate) fn sanear_xml(s: &str) -> String {
    let sem_dtd = remover_doctype(s);
    let mut out = String::with_capacity(sem_dtd.len());
    let mut resto = sem_dtd.as_str();
    while let Some(i) = resto.find('&') {
        out.push_str(&resto[..i]);
        resto = &resto[i..];
        let fim = resto
            .char_indices()
            .take(34)
            .find(|(_, c)| *c == ';')
            .map(|(k, _)| k);
        match fim {
            Some(k) => {
                let nome = &resto[1..k];
                let nome_ok = !nome.is_empty()
                    && nome
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '#' || c == '_');
                if !nome_ok {
                    out.push_str("&amp;");
                    resto = &resto[1..];
                    continue;
                }
                if nome.starts_with('#') || matches!(nome, "amp" | "lt" | "gt" | "quot" | "apos") {
                    out.push_str(&resto[..=k]);
                } else if let Some(c) = entidade_nomeada(nome) {
                    out.push(c);
                } else {
                    out.push(' ');
                }
                resto = &resto[k + 1..];
            }
            None => {
                out.push_str("&amp;");
                resto = &resto[1..];
            }
        }
    }
    out.push_str(resto);
    out.chars()
        .map(|c| if controle_invalido_xml(c) { ' ' } else { c })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// HTML para texto (nada é executado: só se varre a marcação)
// ---------------------------------------------------------------------------------------------

fn procurar_ci(h: &str, agulha: &str) -> Option<usize> {
    let (hb, ab) = (h.as_bytes(), agulha.as_bytes());
    if ab.is_empty() || hb.len() < ab.len() {
        return None;
    }
    (0..=hb.len() - ab.len()).find(|&i| hb[i..i + ab.len()].eq_ignore_ascii_case(ab))
}

/// Posição do `>` que fecha a tag que começa em `s[0] == '<'`, respeitando aspas.
fn fim_da_tag(s: &str) -> Option<usize> {
    let mut aspas: Option<u8> = None;
    for (i, &c) in s.as_bytes().iter().enumerate().skip(1) {
        match aspas {
            Some(q) if c == q => aspas = None,
            Some(_) => {}
            None => match c {
                b'"' | b'\'' => aspas = Some(c),
                b'>' => return Some(i),
                _ => {}
            },
        }
    }
    None
}

fn tag_de_bloco(nome: &str) -> bool {
    matches!(
        nome,
        "p" | "div"
            | "br"
            | "hr"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "li"
            | "ul"
            | "ol"
            | "tr"
            | "table"
            | "section"
            | "article"
            | "blockquote"
            | "pre"
            | "dt"
            | "dd"
            | "title"
            | "body"
            | "mbp:pagebreak"
            | "image"
            | "figure"
            | "figcaption"
            | "aside"
            | "nav"
            | "header"
            | "footer"
    )
}

fn empurrar_texto(out: &mut String, s: &str) {
    let mut resto = s;
    while !resto.is_empty() {
        let Some(i) = resto.find('&') else {
            empurrar_caracteres(out, resto);
            return;
        };
        empurrar_caracteres(out, &resto[..i]);
        resto = &resto[i..];
        let fim = resto
            .char_indices()
            .take(34)
            .find(|(_, c)| *c == ';')
            .map(|(k, _)| k);
        match fim.and_then(|k| entidade_texto(&resto[1..k]).map(|c| (c, k))) {
            Some((c, k)) => {
                let mut b = [0u8; 4];
                empurrar_caracteres(out, c.encode_utf8(&mut b));
                resto = &resto[k + 1..];
            }
            None => {
                out.push('&');
                resto = &resto[1..];
            }
        }
    }
}

fn empurrar_caracteres(out: &mut String, s: &str) {
    for c in s.chars() {
        if c.is_whitespace() || c == '\u{A0}' || c == '\u{200B}' {
            if !out.is_empty() && !out.ends_with([' ', '\n']) {
                out.push(' ');
            }
        } else if c.is_control() {
            // descarta
        } else {
            out.push(c);
        }
    }
}

fn quebra(out: &mut String) {
    while out.ends_with(' ') {
        out.pop();
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

/// Texto visível de um HTML/XHTML/MOBI: tags viram quebras de parágrafo, `script`, `style` e
/// `head` somem, entidades são decodificadas. Para ao passar de `max_bytes` de saída.
pub(crate) fn texto_de_html(html: &str, max_bytes: usize) -> String {
    let mut out = String::new();
    let mut resto = html;
    while out.len() < max_bytes {
        let Some(i) = resto.find('<') else {
            empurrar_texto(&mut out, resto);
            break;
        };
        empurrar_texto(&mut out, &resto[..i]);
        resto = &resto[i..];
        if let Some(r) = resto.strip_prefix("<!--") {
            match r.find("-->") {
                Some(j) => {
                    resto = &r[j + 3..];
                    continue;
                }
                None => break,
            }
        }
        if let Some(r) = resto.strip_prefix("<![CDATA[") {
            let (conteudo, depois) = match r.find("]]>") {
                Some(j) => (&r[..j], &r[j + 3..]),
                None => (r, ""),
            };
            empurrar_caracteres(&mut out, conteudo);
            resto = depois;
            continue;
        }
        let Some(fim) = fim_da_tag(resto) else {
            break;
        };
        let tag = &resto[1..fim];
        resto = &resto[fim + 1..];
        let fechando = tag.starts_with('/');
        let nome: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == ':' || *c == '-')
            .collect::<String>()
            .to_ascii_lowercase();
        if tag.starts_with(['!', '?']) {
            continue;
        }
        if !fechando && !tag.ends_with('/') && matches!(nome.as_str(), "script" | "style" | "head")
        {
            let fecha = format!("</{nome}");
            match procurar_ci(resto, &fecha) {
                Some(j) => {
                    let depois = &resto[j..];
                    resto = depois.find('>').map_or("", |k| &depois[k + 1..]);
                }
                None => break,
            }
            continue;
        }
        if tag_de_bloco(&nome) {
            quebra(&mut out);
        }
    }
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Valor de um atributo de uma tag textual (`src`, `href`...), sem XML. Só para HTML solto.
pub(crate) fn atributo_de_tag(html: &str, tag: &str, atributo: &str) -> Option<String> {
    let abre = format!("<{tag}");
    let mut resto = html;
    while let Some(i) = procurar_ci(resto, &abre) {
        resto = &resto[i..];
        let fim = fim_da_tag(resto)?;
        let corpo = &resto[abre.len()..fim];
        resto = &resto[fim..];
        // o nome da tag precisa terminar aqui (não `<imgx`)
        if !corpo.starts_with(|c: char| c.is_whitespace() || c == '/') {
            continue;
        }
        let mut c = corpo;
        while let Some(j) = procurar_ci(c, atributo) {
            let antes_ok = c[..j].ends_with(|ch: char| ch.is_whitespace() || ch == ':');
            let depois = c[j + atributo.len()..].trim_start();
            if antes_ok {
                if let Some(v) = depois.strip_prefix('=') {
                    let v = v.trim_start();
                    let q = v.chars().next().filter(|q| *q == '"' || *q == '\'')?;
                    let v = &v[1..];
                    return v.find(q).map(|k| v[..k].to_owned());
                }
            }
            c = &c[j + atributo.len()..];
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Páginas, ficha catalográfica e ISBN no texto
// ---------------------------------------------------------------------------------------------

/// Parte `texto` em blocos de aproximadamente `tam` caracteres, em fronteiras de parágrafo (ou
/// de palavra, se o parágrafo sozinho é maior).
pub(crate) fn paginar(texto: &str, tam: usize) -> Vec<String> {
    let tam = tam.max(200);
    let mut blocos: Vec<String> = Vec::new();
    let mut atual = String::new();
    let mut n_atual = 0usize;
    for par in texto.lines() {
        let n = par.chars().count();
        if n > tam {
            if !atual.is_empty() {
                blocos.push(std::mem::take(&mut atual));
                n_atual = 0;
            }
            let mut buf = String::new();
            let mut nb = 0usize;
            for palavra in par.split(' ') {
                let np = palavra.chars().count() + 1;
                if nb + np > tam && !buf.is_empty() {
                    blocos.push(std::mem::take(&mut buf));
                    nb = 0;
                }
                if !buf.is_empty() {
                    buf.push(' ');
                }
                buf.push_str(palavra);
                nb += np;
            }
            if !buf.is_empty() {
                blocos.push(buf);
            }
            continue;
        }
        if n_atual + n + 1 > tam && !atual.is_empty() {
            blocos.push(std::mem::take(&mut atual));
            n_atual = 0;
        }
        if !atual.is_empty() {
            atual.push('\n');
        }
        atual.push_str(par);
        n_atual += n + 1;
    }
    if !atual.is_empty() {
        blocos.push(atual);
    }
    blocos
}

/// Proporção de caracteres "bons" (não de substituição nem de controle).
pub(crate) fn qualidade(texto: &str) -> f32 {
    let mut total = 0u32;
    let mut bons = 0u32;
    for c in texto.chars() {
        total += 1;
        if c != '\u{FFFD}' && (!c.is_control() || c == '\n') {
            bons += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        bons as f32 / total as f32
    }
}

/// Transforma blocos em páginas numeradas a partir de 1.
pub(crate) fn montar_paginas(blocos: Vec<String>, maximo: usize) -> Vec<PaginaTexto> {
    blocos
        .into_iter()
        .take(maximo)
        .enumerate()
        .map(|(i, texto)| PaginaTexto {
            numero: u32::try_from(i + 1).unwrap_or(u32::MAX),
            via_ocr: false,
            qualidade: qualidade(&texto),
            idioma_ocr: None,
            texto,
        })
        .collect()
}

const MARCADORES_FICHA: [&str; 9] = [
    "dados internacionais de cataloga",
    "cataloga\u{e7}\u{e3}o na publica",
    "cip-brasil",
    "ficha catalogr",
    "cataloging-in-publication",
    "cataloguing-in-publication",
    "cataloguing in publication",
    "library of congress cataloging",
    "library of congress control number",
];

fn minusculas(s: &str) -> Vec<char> {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// Procura a ficha catalográfica nas páginas: devolve (número da página, trecho de até 1500
/// caracteres a partir do marcador).
pub(crate) fn achar_ficha(paginas: &[PaginaTexto]) -> Option<(u32, String)> {
    for p in paginas {
        let baixo = minusculas(&p.texto);
        let original: Vec<char> = p.texto.chars().collect();
        for m in MARCADORES_FICHA {
            let agulha: Vec<char> = m.chars().collect();
            if agulha.len() > baixo.len() {
                continue;
            }
            let pos = (0..=baixo.len() - agulha.len())
                .find(|&i| baixo[i..i + agulha.len()] == agulha[..]);
            if let Some(i) = pos {
                let trecho: String = original[i..].iter().take(1500).collect();
                return Some((p.numero, trecho));
            }
        }
    }
    None
}

/// Primeiros `n` caracteres.
pub(crate) fn truncar(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Colapsa espaços em branco.
pub(crate) fn colapsar(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
