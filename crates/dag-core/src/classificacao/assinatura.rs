//! Verificação leve por assinatura (magic numbers) e divergência (A-07, CA-A06).
//!
//! Recebe apenas um trecho inicial do conteúdo; nenhuma leitura de arquivo
//! ocorre aqui. Recomenda-se fornecer ao menos [`BYTES_RECOMENDADOS`] bytes.

use serde::Serialize;

use super::formato::{CategoriaFormato, TipoFormato, tipo_por_extensao};

/// Tamanho de cabeçalho que cobre todas as assinaturas, inclusive ISO 9660
/// (`CD001` no deslocamento 0x8001, exige pelo menos 0x8006 = 32 774 bytes;
/// recomenda-se 32 KiB + 16) e `ustar` do tar (deslocamento 257).
pub const BYTES_RECOMENDADOS: usize = 0x8000 + 16;

/// Deslocamento do identificador `CD001` de ISO 9660.
const DESLOCAMENTO_ISO: usize = 0x8001;

/// Contêiner OLE2 (formatos legados do Office, MSI, MSG).
pub const TIPO_OLE: TipoFormato = TipoFormato {
    extensao: "ole",
    mime: "application/x-ole-storage",
    descricao: "Contêiner OLE2 (Office legado)",
    categoria: CategoriaFormato::Documento,
};

/// Origem da evidência de classificação do formato.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenciaFormato {
    Extensao,
    Assinatura,
}

/// Resultado da comparação entre tipo por extensão e tipo por assinatura.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Divergencia {
    /// Mesmo tipo (MIME idêntico).
    Nenhuma,
    /// Tipos diferentes, porém compatíveis (ex.: `.docx` é um ZIP; `.so` é ELF).
    Compativel,
    /// Extensão enganosa: o conteúdo é de outro formato (ex.: `.jpg` que é PNG).
    Divergente,
    /// Sem base para comparar (extensão desconhecida/genérica ou sem assinatura).
    Indeterminado,
}

impl Divergencia {
    /// `true` apenas para [`Divergencia::Divergente`].
    #[must_use]
    pub fn eh_divergente(self) -> bool {
        self == Self::Divergente
    }
}

fn tipo(chave: &str) -> Option<TipoFormato> {
    tipo_por_extensao(chave)
}

fn comeca(d: &[u8], prefixo: &[u8]) -> bool {
    d.starts_with(prefixo)
}

fn em(d: &[u8], deslocamento: usize, esperado: &[u8]) -> bool {
    d.get(deslocamento..deslocamento + esperado.len()) == Some(esperado)
}

fn contem(d: &[u8], agulha: &[u8]) -> bool {
    d.windows(agulha.len()).any(|j| j == agulha)
}

/// Identifica o formato pelos primeiros bytes do conteúdo (mais de 40 assinaturas).
///
/// ISO 9660 só é reconhecido com pelo menos 0x8006 bytes; `tar` com pelo menos 262.
#[must_use]
pub fn identificar_por_assinatura(d: &[u8]) -> Option<TipoFormato> {
    if d.len() < 2 {
        return None;
    }
    // Contêineres RIFF
    if comeca(d, b"RIFF") && d.len() >= 12 {
        return match &d[8..12] {
            b"WAVE" => tipo("wav"),
            b"WEBP" => tipo("webp"),
            b"AVI " => tipo("avi"),
            _ => None,
        };
    }
    if comeca(d, b"FORM") && em(d, 8, b"AIFF") {
        return tipo("aiff");
    }
    // ISO BMFF (MP4 e derivados): "ftyp" no deslocamento 4.
    if em(d, 4, b"ftyp") {
        return Some(iso_bmff(d.get(8..12).unwrap_or_default()));
    }
    if comeca(d, b"PK\x03\x04") || comeca(d, b"PK\x05\x06") || comeca(d, b"PK\x07\x08") {
        return zip_ou_derivado(d);
    }
    if comeca(d, &[0xCA, 0xFE, 0xBA, 0xBE]) {
        // Mach-O universal (poucas arquiteturas) vs. classe Java (versão >= 45).
        let n = d
            .get(4..8)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        return match n {
            Some(n) if n < 45 => tipo("dylib"),
            Some(_) => tipo("class"),
            None => None,
        };
    }
    if comeca(d, &[0x1A, 0x45, 0xDF, 0xA3]) {
        let janela = &d[..d.len().min(64)];
        return if contem(janela, b"webm") {
            tipo("webm")
        } else {
            tipo("mkv")
        };
    }
    if comeca(d, b"II*\0") || comeca(d, b"MM\0*") {
        return Some(tiff_ou_raw(d));
    }
    if comeca(d, b"II\x52\x4F") || comeca(d, b"II\x52\x53") {
        return tipo("orf");
    }
    if comeca(d, b"IIU\0") {
        return tipo("rw2");
    }
    if comeca(d, b"!<arch>\n") {
        return if em(d, 8, b"debian-binary") {
            tipo("deb")
        } else {
            tipo("ar")
        };
    }
    let tabela: &[(&[u8], &str)] = &[
        (b"%PDF-", "pdf"),
        (b"\x89PNG\r\n\x1a\n", "png"),
        (b"\xFF\xD8\xFF", "jpg"),
        (b"GIF87a", "gif"),
        (b"GIF89a", "gif"),
        (b"\x1F\x8B", "gz"),
        (b"\x28\xB5\x2F\xFD", "zst"),
        (b"\xFD7zXZ\x00", "xz"),
        (b"BZh", "bz2"),
        (b"7z\xBC\xAF\x27\x1C", "7z"),
        (b"Rar!\x1A\x07\x00", "rar"),
        (b"Rar!\x1A\x07\x01\x00", "rar"),
        (b"\x7FELF", "elf"),
        (b"\xFE\xED\xFA\xCE", "dylib"),
        (b"\xFE\xED\xFA\xCF", "dylib"),
        (b"\xCE\xFA\xED\xFE", "dylib"),
        (b"\xCF\xFA\xED\xFE", "dylib"),
        (b"ID3", "mp3"),
        (b"fLaC", "flac"),
        (b"OggS", "ogg"),
        (b"SQLite format 3\0", "sqlite"),
        (b"MSCF", "cab"),
        (b"8BPS", "psd"),
        (b"{\\rtf", "rtf"),
        (b"%!PS", "ps"),
        (b"\x04\x22\x4D\x18", "lz4"),
        (b"LZIP", "lz"),
        (b"\xED\xAB\xEE\xDB", "rpm"),
        (b"MThd", "mid"),
        (b"\0asm", "wasm"),
        (b"\xD4\xC3\xB2\xA1", "pcap"),
        (b"\xA1\xB2\xC3\xD4", "pcap"),
        (b"FLV\x01", "flv"),
        (b"\x30\x26\xB2\x75\x8E\x66\xCF\x11", "wmv"),
        (b"dex\n", "dex"),
        (b"conectix", "vhd"),
        (b"QFI\xFB", "qcow2"),
        (b"KDMV", "vmdk"),
        (b"\x76\x2F\x31\x01", "exr"),
        (b"AT&TFORM", "djvu"),
        (b"FUJIFILMCCD-RAW", "raf"),
        (b"\xFF\x0A", "jxl"),
        (b"\0\0\0\x0CJXL \r\n\x87\n", "jxl"),
        (b"\0\0\0\x0C\x6A\x50\x20\x20\r\n\x87\n", "jp2"),
        (b"wOFF", "woff"),
        (b"wOF2", "woff2"),
        (b"OTTO", "otf"),
        (b"\0\x01\0\0\0", "ttf"),
    ];
    for (magico, chave) in tabela {
        if comeca(d, magico) {
            return tipo(chave);
        }
    }
    if comeca(d, b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") {
        return Some(TIPO_OLE);
    }
    if comeca(d, b"MZ") && d.len() >= 64 {
        return tipo("exe");
    }
    if comeca(d, b"BM") && d.len() >= 14 && d[6..10] == [0, 0, 0, 0] {
        return tipo("bmp");
    }
    if comeca(d, &[0, 0, 1, 0]) && d.len() >= 10 && d[4] > 0 && d[9] == 0 {
        return tipo("ico");
    }
    // MP3 sem ID3: sincronismo de quadro MPEG (heurística fraca, exige 0xFF 0xFB/0xF3/0xF2).
    if d[0] == 0xFF && matches!(d[1], 0xFB | 0xF3 | 0xF2) {
        return tipo("mp3");
    }
    if em(d, 257, b"ustar") {
        return tipo("tar");
    }
    if em(d, DESLOCAMENTO_ISO, b"CD001") {
        return tipo("iso");
    }
    None
}

fn iso_bmff(marca: &[u8]) -> TipoFormato {
    let chave = match marca {
        b"heic" | b"heix" | b"hevc" | b"hevx" | b"mif1" | b"msf1" => "heic",
        b"avif" | b"avis" => "avif",
        b"crx " => "cr3",
        b"M4A " => "m4a",
        b"M4B " => "m4b",
        b"M4V " => "m4v",
        b"qt  " => "mov",
        b"3gp4" | b"3gp5" | b"3gp6" | b"3g2a" => "3gp",
        _ => "mp4",
    };
    tipo(chave).unwrap_or(super::formato::TIPO_DESCONHECIDO)
}

fn tiff_ou_raw(d: &[u8]) -> TipoFormato {
    // CR2: assinatura "CR" no deslocamento 8.
    let chave = if em(d, 8, b"CR\x02\x00") || em(d, 8, b"CR\x03\x00") {
        "cr2"
    } else {
        "tif"
    };
    tipo(chave).unwrap_or(super::formato::TIPO_DESCONHECIDO)
}

fn zip_ou_derivado(d: &[u8]) -> Option<TipoFormato> {
    // Entrada "mimetype" armazenada (sem compressão) logo no primeiro registro.
    if em(d, 30, b"mimetype") {
        let conteudo = &d[38..];
        for (mime, chave) in [
            (&b"application/epub+zip"[..], "epub"),
            (b"application/vnd.oasis.opendocument.text", "odt"),
            (b"application/vnd.oasis.opendocument.spreadsheet", "ods"),
            (b"application/vnd.oasis.opendocument.presentation", "odp"),
            (b"application/vnd.oasis.opendocument.graphics", "odg"),
        ] {
            if comeca(conteudo, mime) {
                return tipo(chave);
            }
        }
    }
    tipo("zip")
}

fn em_familia(chave: &str, familia: &[&str]) -> bool {
    familia.contains(&chave)
}

const FAMILIA_ZIP: &[&str] = &[
    "zip", "docx", "dotx", "xlsx", "xlsm", "pptx", "ppsx", "odt", "ods", "odp", "odg", "epub",
    "jar", "war", "apk", "whl", "cbz", "xpi", "crx", "xps", "vsdx", "pages", "numbers", "key",
    "snap",
];
const FAMILIA_BMFF: &[&str] = &[
    "mp4", "m4v", "m4a", "m4b", "mov", "3gp", "heic", "heif", "avif", "cr3",
];
const FAMILIA_MATROSKA: &[&str] = &["mkv", "webm", "mka"];
const FAMILIA_OLE: &[&str] = &["ole", "doc", "dot", "xls", "ppt", "pps", "msi", "msg"];
const FAMILIA_ELF: &[&str] = &["elf", "so", "o", "ko", "a"];
const FAMILIA_PE: &[&str] = &["exe", "dll", "sys", "ocx", "scr", "efi", "com"];
const FAMILIA_MACHO: &[&str] = &["dylib", "o"];
const FAMILIA_OGG: &[&str] = &["ogg", "oga", "ogv", "opus"];
const FAMILIA_TIFF: &[&str] = &[
    "tif", "tiff", "cr2", "nef", "nrw", "arw", "sr2", "dng", "pef", "srw", "3fr", "rw2", "orf",
    "raw",
];
const FAMILIA_JPEG: &[&str] = &["jpg", "jpeg", "jpe", "jfif"];
const FAMILIA_GZ: &[&str] = &["gz", "tgz", "tar.gz", "svgz"];
const FAMILIA_AR: &[&str] = &["ar", "a", "deb"];
const FAMILIA_SQLITE: &[&str] = &["sqlite", "sqlite3", "db"];
const FAMILIA_PS: &[&str] = &["ps", "eps"];
const FAMILIA_AI: &[&str] = &["ai", "pdf"];

fn mesma_familia(a: &str, b: &str) -> bool {
    [
        FAMILIA_ZIP,
        FAMILIA_BMFF,
        FAMILIA_MATROSKA,
        FAMILIA_OLE,
        FAMILIA_ELF,
        FAMILIA_PE,
        FAMILIA_MACHO,
        FAMILIA_OGG,
        FAMILIA_TIFF,
        FAMILIA_JPEG,
        FAMILIA_GZ,
        FAMILIA_AR,
        FAMILIA_SQLITE,
        FAMILIA_PS,
        FAMILIA_AI,
    ]
    .iter()
    .any(|f| em_familia(a, f) && em_familia(b, f))
}

/// Compara o tipo inferido pela extensão com o tipo validado por assinatura.
///
/// `.docx` cuja assinatura é um ZIP genérico é [`Divergencia::Compativel`], não
/// divergência. Extensões desconhecidas ou genéricas (`application/octet-stream`)
/// e ausência de assinatura resultam em [`Divergencia::Indeterminado`].
#[must_use]
pub fn verificar_divergencia(
    ext_tipo: Option<TipoFormato>,
    assinatura_tipo: Option<TipoFormato>,
) -> Divergencia {
    let (Some(e), Some(a)) = (ext_tipo, assinatura_tipo) else {
        return Divergencia::Indeterminado;
    };
    if e.categoria == CategoriaFormato::Desconhecido || e.mime == "application/octet-stream" {
        return Divergencia::Indeterminado;
    }
    if e.mime == a.mime {
        return Divergencia::Nenhuma;
    }
    if mesma_familia(e.extensao, a.extensao) {
        return Divergencia::Compativel;
    }
    Divergencia::Divergente
}

#[cfg(test)]
mod testes {
    use super::*;

    fn chave(d: &[u8]) -> &'static str {
        identificar_por_assinatura(d).map_or("", |t| t.extensao)
    }

    fn preenchido(inicio: &[u8], tam: usize) -> Vec<u8> {
        let mut v = vec![0u8; tam];
        v[..inicio.len()].copy_from_slice(inicio);
        v
    }

    #[test]
    fn assinaturas_basicas() {
        let casos: &[(&[u8], &str)] = &[
            (b"%PDF-1.7\n", "pdf"),
            (b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR", "png"),
            (b"\xFF\xD8\xFF\xE0\0\x10JFIF", "jpg"),
            (b"GIF87a\x01\0", "gif"),
            (b"GIF89a\x01\0", "gif"),
            (b"RIFF\x24\0\0\0WEBPVP8 ", "webp"),
            (b"RIFF\x24\0\0\0WAVEfmt ", "wav"),
            (b"RIFF\x24\0\0\0AVI LIST", "avi"),
            (b"II*\0\x08\0\0\0", "tif"),
            (b"MM\0*\0\0\0\x08", "tif"),
            (b"II*\0\x10\0\0\0CR\x02\0", "cr2"),
            (b"IIRO\x08\0\0\0", "orf"),
            (b"IIU\0\x18\0\0\0", "rw2"),
            (b"FUJIFILMCCD-RAW 0201", "raf"),
            (b"PK\x03\x04\x14\0\0\0", "zip"),
            (b"PK\x05\x06\0\0\0\0", "zip"),
            (b"\x1F\x8B\x08\0", "gz"),
            (b"\x28\xB5\x2F\xFD\x04", "zst"),
            (b"\xFD7zXZ\0\0\x04", "xz"),
            (b"BZh91AY&SY", "bz2"),
            (b"7z\xBC\xAF\x27\x1C\0\x04", "7z"),
            (b"Rar!\x1A\x07\x00\xCF", "rar"),
            (b"Rar!\x1A\x07\x01\x00", "rar"),
            (b"\x7FELF\x02\x01\x01", "elf"),
            (b"\xCF\xFA\xED\xFE\x07\0\0\x01", "dylib"),
            (b"\xFE\xED\xFA\xCE\0\0\0\x07", "dylib"),
            (b"\xCA\xFE\xBA\xBE\0\0\0\x02", "dylib"),
            (b"\xCA\xFE\xBA\xBE\0\0\0\x34", "class"),
            (b"ID3\x04\0\0\0\0\0\0", "mp3"),
            (b"\xFF\xFB\x90\x64", "mp3"),
            (b"fLaC\0\0\0\"", "flac"),
            (b"OggS\0\x02\0\0", "ogg"),
            (b"\0\0\0\x18ftypmp42", "mp4"),
            (b"\0\0\0\x18ftypM4A \0", "m4a"),
            (b"\0\0\0\x14ftypqt  ", "mov"),
            (b"\0\0\0\x18ftypheic", "heic"),
            (b"\0\0\0\x18ftypavif", "avif"),
            (b"\0\0\0\x18ftypcrx ", "cr3"),
            (b"\x1A\x45\xDF\xA3\x01\0\0\0\0\0\0\x1FB\x86\x81\x01B\xF7\x81\x01B\xF2\x81\x04B\xF3\x81\x08B\x82\x88matroska", "mkv"),
            (b"\x1A\x45\xDF\xA3\x01\0\0\0B\x82\x84webm", "webm"),
            (b"SQLite format 3\0\x10\0", "sqlite"),
            (b"MSCF\0\0\0\0", "cab"),
            (b"8BPS\0\x01", "psd"),
            (b"{\\rtf1\\ansi", "rtf"),
            (b"%!PS-Adobe-3.0", "ps"),
            (b"\x04\x22\x4D\x18\x64", "lz4"),
            (b"!<arch>\ndebian-binary   ", "deb"),
            (b"!<arch>\nlib.o/  ", "ar"),
            (b"\xED\xAB\xEE\xDB\x03\0", "rpm"),
            (b"MThd\0\0\0\x06", "mid"),
            (b"FORM\0\0\0\0AIFF", "aiff"),
            (b"\0asm\x01\0\0\0", "wasm"),
            (b"\xD4\xC3\xB2\xA1\x02\0", "pcap"),
            (b"FLV\x01\x05", "flv"),
            (b"dex\n035\0", "dex"),
            (b"wOF2\0\x01", "woff2"),
            (b"KDMV\x01\0", "vmdk"),
            (b"QFI\xFB\0\0\0\x03", "qcow2"),
        ];
        for (dados, esperado) in casos {
            assert_eq!(
                chave(dados),
                *esperado,
                "{:?}",
                &dados[..dados.len().min(12)]
            );
        }
        assert!(casos.len() >= 30);
    }

    #[test]
    fn pe_ole_bmp_ico_exigem_cabecalho_minimo() {
        let mz = preenchido(b"MZ\x90\0", 64);
        assert_eq!(chave(&mz), "exe");
        assert_eq!(chave(b"MZ\x90\0"), "", "MZ curto não é PE");
        assert_eq!(
            identificar_por_assinatura(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1\0\0").unwrap(),
            TIPO_OLE
        );
        let bmp = preenchido(b"BM\x36\0\0\0\0\0\0\0", 14);
        assert_eq!(chave(&bmp), "bmp");
        let ico = preenchido(b"\0\0\x01\0\x02\0", 10);
        assert_eq!(chave(&ico), "ico");
        assert_eq!(chave(&preenchido(b"\0\0\x01\0\0\0", 10)), "");
    }

    #[test]
    fn epub_e_odf_pelo_mimetype() {
        let mut epub = vec![0u8; 30];
        epub[..4].copy_from_slice(b"PK\x03\x04");
        epub.extend_from_slice(b"mimetypeapplication/epub+zip");
        assert_eq!(chave(&epub), "epub");
        let mut odt = vec![0u8; 30];
        odt[..4].copy_from_slice(b"PK\x03\x04");
        odt.extend_from_slice(b"mimetypeapplication/vnd.oasis.opendocument.text");
        assert_eq!(chave(&odt), "odt");
        // ZIP comum e OOXML (sem mimetype) permanecem zip.
        let mut ooxml = vec![0u8; 30];
        ooxml[..4].copy_from_slice(b"PK\x03\x04");
        ooxml.extend_from_slice(b"[Content_Types].xml");
        assert_eq!(chave(&ooxml), "zip");
    }

    #[test]
    fn iso_exige_32_kib() {
        let mut v = vec![0u8; 0x8006];
        v[0x8001..0x8006].copy_from_slice(b"CD001");
        assert_eq!(chave(&v), "iso");
        assert_eq!(chave(&v[..0x8000]), "", "truncado antes de 0x8001");
        const { assert!(BYTES_RECOMENDADOS > 0x8006) };
    }

    #[test]
    fn tar_ustar_no_deslocamento_257() {
        let mut v = vec![0u8; 512];
        v[257..262].copy_from_slice(b"ustar");
        assert_eq!(chave(&v), "tar");
    }

    #[test]
    fn sem_assinatura_ou_curto() {
        assert!(identificar_por_assinatura(b"").is_none());
        assert!(identificar_por_assinatura(b"x").is_none());
        assert!(identificar_por_assinatura(b"hello world, texto simples").is_none());
        assert!(identificar_por_assinatura(b"RIFF\0\0\0\0XXXX").is_none());
    }

    #[test]
    fn todos_os_tipos_de_assinatura_existem_na_tabela() {
        // Nenhum retorno pode ser o tipo desconhecido.
        for d in [
            &b"%PDF-"[..],
            b"\xCA\xFE\xBA\xBE\0\0\0\x02",
            b"\0\0\0\x18ftypxxxx",
        ] {
            let t = identificar_por_assinatura(d).unwrap();
            assert_ne!(t.categoria, CategoriaFormato::Desconhecido);
        }
    }

    fn ext(chave: &str) -> Option<TipoFormato> {
        tipo_por_extensao(chave)
    }

    #[test]
    fn divergencia_jpg_que_e_png() {
        let d = verificar_divergencia(ext("jpg"), identificar_por_assinatura(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(d, Divergencia::Divergente);
        assert!(d.eh_divergente());
    }

    #[test]
    fn divergencia_nenhuma_e_compativel() {
        let pdf = identificar_por_assinatura(b"%PDF-1.4");
        assert_eq!(verificar_divergencia(ext("pdf"), pdf), Divergencia::Nenhuma);
        let zip = identificar_por_assinatura(b"PK\x03\x04....");
        for e in ["docx", "xlsx", "pptx", "jar", "apk", "epub", "cbz", "odt"] {
            assert_eq!(
                verificar_divergencia(ext(e), zip),
                Divergencia::Compativel,
                "{e}"
            );
        }
        assert_eq!(verificar_divergencia(ext("zip"), zip), Divergencia::Nenhuma);
        let jpg = identificar_por_assinatura(b"\xFF\xD8\xFF\xE1");
        assert_eq!(
            verificar_divergencia(ext("jpeg"), jpg),
            Divergencia::Nenhuma
        );
        let tif = identificar_por_assinatura(b"II*\0\x08\0\0\0");
        assert_eq!(
            verificar_divergencia(ext("nef"), tif),
            Divergencia::Compativel
        );
        let elf = identificar_por_assinatura(b"\x7FELF\x02\x01");
        assert_eq!(
            verificar_divergencia(ext("so"), elf),
            Divergencia::Compativel
        );
        let gz = identificar_por_assinatura(b"\x1F\x8B\x08\0");
        assert_eq!(
            verificar_divergencia(ext("tar.gz"), gz),
            Divergencia::Nenhuma
        );
        let ole = identificar_por_assinatura(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1");
        assert_eq!(
            verificar_divergencia(ext("doc"), ole),
            Divergencia::Compativel
        );
        let mp4 = identificar_por_assinatura(b"\0\0\0\x18ftypisom");
        assert_eq!(
            verificar_divergencia(ext("mov"), mp4),
            Divergencia::Compativel
        );
    }

    #[test]
    fn docx_que_e_pdf_diverge() {
        let pdf = identificar_por_assinatura(b"%PDF-1.4");
        assert_eq!(
            verificar_divergencia(ext("docx"), pdf),
            Divergencia::Divergente
        );
        let png = identificar_por_assinatura(b"\x89PNG\r\n\x1a\n");
        assert_eq!(
            verificar_divergencia(ext("txt"), png),
            Divergencia::Divergente
        );
    }

    #[test]
    fn divergencia_indeterminada() {
        let png = identificar_por_assinatura(b"\x89PNG\r\n\x1a\n");
        assert_eq!(verificar_divergencia(None, png), Divergencia::Indeterminado);
        assert_eq!(
            verificar_divergencia(ext("jpg"), None),
            Divergencia::Indeterminado
        );
        assert_eq!(
            verificar_divergencia(ext("bin"), png),
            Divergencia::Indeterminado
        );
        assert_eq!(
            verificar_divergencia(ext("tmp"), png),
            Divergencia::Indeterminado
        );
    }

    #[test]
    fn evidencias_serializam() {
        assert_eq!(
            serde_json::to_string(&EvidenciaFormato::Assinatura).unwrap(),
            "\"assinatura\""
        );
    }
}
