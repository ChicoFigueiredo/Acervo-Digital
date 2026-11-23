//! Identificação de formato por extensão (A-07): tabela estática, normalização
//! de nomes (multi-ponto, ocultos, maiúsculas) e tipos de formato.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Serialize;

/// Categoria geral do formato.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CategoriaFormato {
    Documento,
    Imagem,
    Audio,
    Video,
    Compactado,
    Executavel,
    Codigo,
    Fonte,
    Livro,
    ImagemDeDisco,
    BancoDeDados,
    Sistema,
    Outro,
    Desconhecido,
}

/// Tipo de formato identificado: chave canônica, MIME IANA, descrição e categoria.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct TipoFormato {
    /// Chave canônica na tabela (extensão normalizada, ex.: `pdf`, `tar.gz`).
    pub extensao: &'static str,
    pub mime: &'static str,
    pub descricao: &'static str,
    pub categoria: CategoriaFormato,
}

/// Tipo usado quando a extensão é desconhecida ou ausente.
pub const TIPO_DESCONHECIDO: TipoFormato = TipoFormato {
    extensao: "",
    mime: "application/octet-stream",
    descricao: "Formato desconhecido",
    categoria: CategoriaFormato::Desconhecido,
};

type Entrada = (&'static str, &'static str, &'static str, CategoriaFormato);

use CategoriaFormato::{
    Audio as A, BancoDeDados as B, Codigo as K, Compactado as Z, Documento as D, Executavel as E,
    Fonte as F, Imagem as I, ImagemDeDisco as G, Livro as L, Outro as O, Sistema as S, Video as V,
};

#[rustfmt::skip]
static TABELA: &[Entrada] = &[
    // Documentos
    ("pdf", "application/pdf", "Portable Document Format", D),
    ("doc", "application/msword", "Documento do Microsoft Word 97-2003", D),
    ("docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "Documento do Microsoft Word", D),
    ("dot", "application/msword", "Modelo do Microsoft Word 97-2003", D),
    ("dotx", "application/vnd.openxmlformats-officedocument.wordprocessingml.template", "Modelo do Microsoft Word", D),
    ("xls", "application/vnd.ms-excel", "Planilha do Microsoft Excel 97-2003", D),
    ("xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "Planilha do Microsoft Excel", D),
    ("xlsm", "application/vnd.ms-excel.sheet.macroenabled.12", "Planilha do Excel com macros", D),
    ("ppt", "application/vnd.ms-powerpoint", "Apresentação do PowerPoint 97-2003", D),
    ("pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation", "Apresentação do Microsoft PowerPoint", D),
    ("pps", "application/vnd.ms-powerpoint", "Apresentação de slides do PowerPoint", D),
    ("ppsx", "application/vnd.openxmlformats-officedocument.presentationml.slideshow", "Apresentação de slides do PowerPoint", D),
    ("odt", "application/vnd.oasis.opendocument.text", "Documento de texto OpenDocument", D),
    ("ods", "application/vnd.oasis.opendocument.spreadsheet", "Planilha OpenDocument", D),
    ("odp", "application/vnd.oasis.opendocument.presentation", "Apresentação OpenDocument", D),
    ("odg", "application/vnd.oasis.opendocument.graphics", "Desenho OpenDocument", D),
    ("rtf", "application/rtf", "Rich Text Format", D),
    ("txt", "text/plain", "Texto simples", D),
    ("md", "text/markdown", "Documento Markdown", D),
    ("markdown", "text/markdown", "Documento Markdown", D),
    ("rst", "text/x-rst", "reStructuredText", D),
    ("tex", "application/x-tex", "Documento LaTeX", D),
    ("log", "text/plain", "Arquivo de registro (log)", D),
    ("csv", "text/csv", "Valores separados por vírgula", D),
    ("tsv", "text/tab-separated-values", "Valores separados por tabulação", D),
    ("ps", "application/postscript", "PostScript", D),
    ("djvu", "image/vnd.djvu", "Documento DjVu", D),
    ("xps", "application/vnd.ms-xpsdocument", "XML Paper Specification", D),
    ("pages", "application/vnd.apple.pages", "Documento do Apple Pages", D),
    ("numbers", "application/vnd.apple.numbers", "Planilha do Apple Numbers", D),
    ("key", "application/vnd.apple.keynote", "Apresentação do Apple Keynote", D),
    ("msg", "application/vnd.ms-outlook", "Mensagem do Microsoft Outlook", D),
    ("eml", "message/rfc822", "Mensagem de e-mail", D),
    ("vsdx", "application/vnd.ms-visio.drawing", "Diagrama do Microsoft Visio", D),
    // Imagens
    ("jpg", "image/jpeg", "Imagem JPEG", I),
    ("jpeg", "image/jpeg", "Imagem JPEG", I),
    ("jpe", "image/jpeg", "Imagem JPEG", I),
    ("jfif", "image/jpeg", "Imagem JPEG (JFIF)", I),
    ("png", "image/png", "Portable Network Graphics", I),
    ("gif", "image/gif", "Graphics Interchange Format", I),
    ("bmp", "image/bmp", "Bitmap do Windows", I),
    ("webp", "image/webp", "Imagem WebP", I),
    ("tif", "image/tiff", "Tagged Image File Format", I),
    ("tiff", "image/tiff", "Tagged Image File Format", I),
    ("svg", "image/svg+xml", "Gráfico vetorial escalável (SVG)", I),
    ("svgz", "image/svg+xml", "SVG compactado", I),
    ("ico", "image/vnd.microsoft.icon", "Ícone do Windows", I),
    ("icns", "image/icns", "Ícone do macOS", I),
    ("heic", "image/heic", "High Efficiency Image Container", I),
    ("heif", "image/heif", "High Efficiency Image Format", I),
    ("avif", "image/avif", "AV1 Image File Format", I),
    ("jxl", "image/jxl", "JPEG XL", I),
    ("jp2", "image/jp2", "JPEG 2000", I),
    ("psd", "image/vnd.adobe.photoshop", "Documento do Adobe Photoshop", I),
    ("ai", "application/illustrator", "Documento do Adobe Illustrator", I),
    ("eps", "application/postscript", "Encapsulated PostScript", I),
    ("xcf", "image/x-xcf", "Imagem do GIMP", I),
    ("exr", "image/aces", "OpenEXR", I),
    ("tga", "image/x-tga", "Imagem Targa", I),
    ("pbm", "image/x-portable-bitmap", "Portable Bitmap", I),
    ("pgm", "image/x-portable-graymap", "Portable Graymap", I),
    ("ppm", "image/x-portable-pixmap", "Portable Pixmap", I),
    // Imagens RAW de câmeras
    ("cr2", "image/x-canon-cr2", "RAW da Canon (CR2)", I),
    ("cr3", "image/x-canon-cr3", "RAW da Canon (CR3)", I),
    ("crw", "image/x-canon-crw", "RAW da Canon (CRW)", I),
    ("nef", "image/x-nikon-nef", "RAW da Nikon (NEF)", I),
    ("nrw", "image/x-nikon-nrw", "RAW da Nikon (NRW)", I),
    ("arw", "image/x-sony-arw", "RAW da Sony (ARW)", I),
    ("sr2", "image/x-sony-sr2", "RAW da Sony (SR2)", I),
    ("dng", "image/x-adobe-dng", "Digital Negative da Adobe (RAW)", I),
    ("orf", "image/x-olympus-orf", "RAW da Olympus (ORF)", I),
    ("rw2", "image/x-panasonic-rw2", "RAW da Panasonic (RW2)", I),
    ("raf", "image/x-fuji-raf", "RAW da Fujifilm (RAF)", I),
    ("pef", "image/x-pentax-pef", "RAW da Pentax (PEF)", I),
    ("srw", "image/x-samsung-srw", "RAW da Samsung (SRW)", I),
    ("x3f", "image/x-sigma-x3f", "RAW da Sigma (X3F)", I),
    ("raw", "image/x-raw", "Imagem RAW genérica", I),
    ("3fr", "image/x-hasselblad-3fr", "RAW da Hasselblad (3FR)", I),
    ("rwl", "image/x-leica-rwl", "RAW da Leica (RWL)", I),
    // Áudio
    ("mp3", "audio/mpeg", "Áudio MPEG Layer 3", A),
    ("flac", "audio/flac", "Free Lossless Audio Codec", A),
    ("wav", "audio/vnd.wave", "Áudio WAVE", A),
    ("ogg", "audio/ogg", "Áudio Ogg Vorbis", A),
    ("oga", "audio/ogg", "Áudio Ogg", A),
    ("opus", "audio/opus", "Áudio Opus", A),
    ("m4a", "audio/mp4", "Áudio MPEG-4 (AAC)", A),
    ("m4b", "audio/mp4", "Audiolivro MPEG-4", A),
    ("aac", "audio/aac", "Advanced Audio Coding", A),
    ("wma", "audio/x-ms-wma", "Windows Media Audio", A),
    ("aiff", "audio/aiff", "Audio Interchange File Format", A),
    ("aif", "audio/aiff", "Audio Interchange File Format", A),
    ("mid", "audio/midi", "Arquivo MIDI", A),
    ("midi", "audio/midi", "Arquivo MIDI", A),
    ("ape", "audio/x-ape", "Monkey's Audio", A),
    ("amr", "audio/amr", "Adaptive Multi-Rate", A),
    ("mka", "audio/x-matroska", "Áudio Matroska", A),
    ("m3u", "audio/x-mpegurl", "Lista de reprodução M3U", A),
    // Vídeo
    ("mp4", "video/mp4", "Vídeo MPEG-4", V),
    ("m4v", "video/x-m4v", "Vídeo MPEG-4 (iTunes)", V),
    ("mkv", "video/x-matroska", "Vídeo Matroska", V),
    ("webm", "video/webm", "Vídeo WebM", V),
    ("avi", "video/x-msvideo", "Audio Video Interleave", V),
    ("mov", "video/quicktime", "Vídeo QuickTime", V),
    ("wmv", "video/x-ms-wmv", "Windows Media Video", V),
    ("flv", "video/x-flv", "Flash Video", V),
    ("mpg", "video/mpeg", "Vídeo MPEG", V),
    ("mpeg", "video/mpeg", "Vídeo MPEG", V),
    ("3gp", "video/3gpp", "Vídeo 3GPP", V),
    ("ts", "video/mp2t", "Fluxo de transporte MPEG-2", V),
    ("m2ts", "video/mp2t", "Vídeo Blu-ray (M2TS)", V),
    ("vob", "video/dvd", "Vídeo DVD (VOB)", V),
    ("ogv", "video/ogg", "Vídeo Ogg Theora", V),
    ("srt", "application/x-subrip", "Legendas SubRip", V),
    // Compactados e pacotes
    ("zip", "application/zip", "Arquivo ZIP", Z),
    ("gz", "application/gzip", "Arquivo compactado com gzip", Z),
    ("tgz", "application/gzip", "Tarball compactado com gzip", Z),
    ("tar.gz", "application/gzip", "Tarball compactado com gzip", Z),
    ("bz2", "application/x-bzip2", "Arquivo compactado com bzip2", Z),
    ("tbz2", "application/x-bzip2", "Tarball compactado com bzip2", Z),
    ("tar.bz2", "application/x-bzip2", "Tarball compactado com bzip2", Z),
    ("xz", "application/x-xz", "Arquivo compactado com xz", Z),
    ("txz", "application/x-xz", "Tarball compactado com xz", Z),
    ("tar.xz", "application/x-xz", "Tarball compactado com xz", Z),
    ("zst", "application/zstd", "Arquivo compactado com Zstandard", Z),
    ("tar.zst", "application/zstd", "Tarball compactado com Zstandard", Z),
    ("lz4", "application/x-lz4", "Arquivo compactado com LZ4", Z),
    ("tar.lz4", "application/x-lz4", "Tarball compactado com LZ4", Z),
    ("lz", "application/x-lzip", "Arquivo compactado com lzip", Z),
    ("lzma", "application/x-lzma", "Arquivo compactado com LZMA", Z),
    ("tar", "application/x-tar", "Arquivo tar (tarball)", Z),
    ("7z", "application/x-7z-compressed", "Arquivo 7-Zip", Z),
    ("rar", "application/vnd.rar", "Arquivo RAR", Z),
    ("cab", "application/vnd.ms-cab-compressed", "Gabinete do Windows (CAB)", Z),
    ("ar", "application/x-archive", "Arquivo ar", Z),
    ("a", "application/x-archive", "Biblioteca estática (ar)", Z),
    ("cpio", "application/x-cpio", "Arquivo cpio", Z),
    ("z", "application/x-compress", "Arquivo compactado (compress)", Z),
    ("deb", "application/vnd.debian.binary-package", "Pacote Debian", Z),
    ("rpm", "application/x-rpm", "Pacote RPM", Z),
    ("apk", "application/vnd.android.package-archive", "Pacote Android (APK)", Z),
    ("jar", "application/java-archive", "Arquivo Java (JAR)", Z),
    ("war", "application/java-archive", "Aplicação web Java (WAR)", Z),
    ("whl", "application/zip", "Pacote Python (wheel)", Z),
    ("crx", "application/x-chrome-extension", "Extensão do Chrome", Z),
    ("xpi", "application/x-xpinstall", "Extensão do Firefox", Z),
    ("snap", "application/vnd.snap", "Pacote Snap", Z),
    ("flatpak", "application/vnd.flatpak", "Pacote Flatpak", Z),
    ("appimage", "application/vnd.appimage", "Aplicativo AppImage", E),
    ("msi", "application/x-msi", "Instalador do Windows (MSI)", Z),
    ("pkg", "application/x-newton-compatible-pkg", "Pacote de instalação", Z),
    // Executáveis e bibliotecas
    ("exe", "application/vnd.microsoft.portable-executable", "Executável do Windows", E),
    ("dll", "application/vnd.microsoft.portable-executable", "Biblioteca de vínculo dinâmico do Windows", E),
    ("sys", "application/vnd.microsoft.portable-executable", "Driver do Windows", E),
    ("ocx", "application/vnd.microsoft.portable-executable", "Controle ActiveX", E),
    ("scr", "application/vnd.microsoft.portable-executable", "Protetor de tela do Windows", E),
    ("efi", "application/vnd.microsoft.portable-executable", "Aplicativo EFI", E),
    ("bat", "application/x-bat", "Script em lote do Windows", E),
    ("cmd", "application/x-bat", "Script de comandos do Windows", E),
    ("com", "application/x-msdos-program", "Programa do MS-DOS", E),
    ("elf", "application/x-executable", "Executável ELF", E),
    ("so", "application/x-sharedlib", "Biblioteca compartilhada ELF", E),
    ("o", "application/x-object", "Arquivo objeto compilado", E),
    ("ko", "application/x-object", "Módulo do kernel Linux", E),
    ("dylib", "application/x-mach-binary", "Biblioteca dinâmica Mach-O", E),
    ("class", "application/java-vm", "Classe Java compilada", E),
    ("dex", "application/x-dex", "Executável Dalvik (DEX)", E),
    ("wasm", "application/wasm", "WebAssembly", E),
    ("bin", "application/octet-stream", "Arquivo binário", O),
    // Código-fonte e dados estruturados
    ("c", "text/x-csrc", "Código-fonte C", K),
    ("h", "text/x-chdr", "Cabeçalho C", K),
    ("cpp", "text/x-c++src", "Código-fonte C++", K),
    ("cc", "text/x-c++src", "Código-fonte C++", K),
    ("hpp", "text/x-c++hdr", "Cabeçalho C++", K),
    ("rs", "text/x-rust", "Código-fonte Rust", K),
    ("py", "text/x-python", "Código-fonte Python", K),
    ("pyc", "application/x-python-code", "Bytecode Python", K),
    ("js", "text/javascript", "Código-fonte JavaScript", K),
    ("mjs", "text/javascript", "Módulo JavaScript", K),
    ("jsx", "text/jsx", "Código-fonte JSX", K),
    ("tsx", "text/tsx", "Código-fonte TypeScript JSX", K),
    ("java", "text/x-java-source", "Código-fonte Java", K),
    ("kt", "text/x-kotlin", "Código-fonte Kotlin", K),
    ("go", "text/x-go", "Código-fonte Go", K),
    ("cs", "text/x-csharp", "Código-fonte C#", K),
    ("rb", "text/x-ruby", "Código-fonte Ruby", K),
    ("php", "application/x-httpd-php", "Código-fonte PHP", K),
    ("swift", "text/x-swift", "Código-fonte Swift", K),
    ("lua", "text/x-lua", "Código-fonte Lua", K),
    ("pl", "text/x-perl", "Código-fonte Perl", K),
    ("sh", "application/x-sh", "Script de shell", K),
    ("bash", "application/x-sh", "Script Bash", K),
    ("zsh", "application/x-sh", "Script Zsh", K),
    ("ps1", "text/x-powershell", "Script PowerShell", K),
    ("sql", "application/sql", "Script SQL", K),
    ("html", "text/html", "Documento HTML", K),
    ("htm", "text/html", "Documento HTML", K),
    ("css", "text/css", "Folha de estilo CSS", K),
    ("scss", "text/x-scss", "Folha de estilo Sass", K),
    ("json", "application/json", "JavaScript Object Notation", K),
    ("xml", "application/xml", "Extensible Markup Language", K),
    ("yaml", "application/yaml", "Documento YAML", K),
    ("yml", "application/yaml", "Documento YAML", K),
    ("toml", "application/toml", "Documento TOML", K),
    ("ini", "text/x-ini", "Arquivo de configuração INI", K),
    ("conf", "text/plain", "Arquivo de configuração", K),
    ("cfg", "text/plain", "Arquivo de configuração", K),
    ("lock", "text/plain", "Arquivo de travamento de dependências", K),
    ("diff", "text/x-diff", "Arquivo de diferenças", K),
    ("patch", "text/x-diff", "Patch de código", K),
    ("ipynb", "application/x-ipynb+json", "Notebook Jupyter", K),
    // Fontes tipográficas
    ("ttf", "font/ttf", "Fonte TrueType", F),
    ("otf", "font/otf", "Fonte OpenType", F),
    ("woff", "font/woff", "Web Open Font Format", F),
    ("woff2", "font/woff2", "Web Open Font Format 2", F),
    ("ttc", "font/collection", "Coleção de fontes TrueType", F),
    ("pfb", "application/x-font-type1", "Fonte PostScript Type 1", F),
    ("eot", "application/vnd.ms-fontobject", "Fonte OpenType incorporada", F),
    // Livros eletrônicos
    ("epub", "application/epub+zip", "Livro eletrônico EPUB", L),
    ("mobi", "application/x-mobipocket-ebook", "Livro eletrônico Mobipocket", L),
    ("azw", "application/vnd.amazon.ebook", "Livro eletrônico Kindle", L),
    ("azw3", "application/vnd.amazon.ebook", "Livro eletrônico Kindle (KF8)", L),
    ("kfx", "application/vnd.amazon.ebook", "Livro eletrônico Kindle (KFX)", L),
    ("fb2", "application/x-fictionbook+xml", "Livro eletrônico FictionBook", L),
    ("cbz", "application/vnd.comicbook+zip", "Quadrinhos em ZIP (CBZ)", L),
    ("cbr", "application/vnd.comicbook-rar", "Quadrinhos em RAR (CBR)", L),
    ("cb7", "application/x-cb7", "Quadrinhos em 7z (CB7)", L),
    ("lit", "application/x-ms-reader", "Livro do Microsoft Reader", L),
    // Imagens de disco e virtualização
    ("iso", "application/x-iso9660-image", "Imagem de disco ISO 9660", G),
    ("img", "application/x-raw-disk-image", "Imagem de disco bruta", G),
    ("dmg", "application/x-apple-diskimage", "Imagem de disco do macOS", G),
    ("vhd", "application/x-vhd", "Disco virtual VHD", G),
    ("vhdx", "application/x-vhdx", "Disco virtual VHDX", G),
    ("vmdk", "application/x-vmdk", "Disco virtual VMware", G),
    ("vdi", "application/x-virtualbox-vdi", "Disco virtual VirtualBox", G),
    ("qcow2", "application/x-qemu-disk", "Disco virtual QEMU (qcow2)", G),
    ("wim", "application/x-ms-wim", "Imagem do Windows (WIM)", G),
    ("toast", "application/x-roxio-toast", "Imagem de disco Toast", G),
    // Bancos de dados
    ("sqlite", "application/vnd.sqlite3", "Banco de dados SQLite", B),
    ("sqlite3", "application/vnd.sqlite3", "Banco de dados SQLite", B),
    ("db", "application/vnd.sqlite3", "Arquivo de banco de dados", B),
    ("mdb", "application/vnd.ms-access", "Banco de dados do Microsoft Access", B),
    ("accdb", "application/vnd.ms-access", "Banco de dados do Microsoft Access", B),
    ("dbf", "application/x-dbf", "Banco de dados dBase", B),
    ("ldb", "application/x-leveldb", "Banco de dados LevelDB", B),
    ("mdf", "application/x-sqlserver-mdf", "Banco de dados do SQL Server", B),
    ("parquet", "application/vnd.apache.parquet", "Dados colunares Apache Parquet", B),
    // Sistema, temporários e outros
    ("lnk", "application/x-ms-shortcut", "Atalho do Windows", S),
    ("desktop", "application/x-desktop", "Entrada de aplicativo (desktop entry)", S),
    ("service", "text/x-systemd-unit", "Unidade de serviço systemd", S),
    ("sock", "application/x-socket", "Soquete de rede/IPC", S),
    ("pid", "text/plain", "Arquivo de identificador de processo", S),
    ("tmp", "application/octet-stream", "Arquivo temporário", S),
    ("bak", "application/octet-stream", "Cópia de segurança", S),
    ("swp", "application/octet-stream", "Arquivo de troca do editor", S),
    ("cache", "application/octet-stream", "Arquivo de cache", S),
    ("ics", "text/calendar", "Calendário iCalendar", O),
    ("vcf", "text/vcard", "Cartão de contato vCard", O),
    ("torrent", "application/x-bittorrent", "Arquivo BitTorrent", O),
    ("pcap", "application/vnd.tcpdump.pcap", "Captura de pacotes de rede", O),
    ("pem", "application/x-pem-file", "Certificado/chave PEM", O),
    ("crt", "application/x-x509-ca-cert", "Certificado X.509", O),
    ("gpg", "application/pgp-encrypted", "Arquivo criptografado GPG", O),
    ("kdbx", "application/x-keepass2", "Banco de senhas KeePass", O),
    ("gpx", "application/gpx+xml", "Trilha GPS (GPX)", O),
    ("kml", "application/vnd.google-earth.kml+xml", "Dados geográficos KML", O),
    ("blend", "application/x-blender", "Cena do Blender", O),
    ("stl", "model/stl", "Modelo 3D STL", O),
    ("obj", "model/obj", "Modelo 3D Wavefront OBJ", O),
    ("dwg", "image/vnd.dwg", "Desenho do AutoCAD", O),
    ("dxf", "image/vnd.dxf", "Intercâmbio de desenho AutoCAD", O),
];

static INDICE: LazyLock<HashMap<&'static str, usize>> =
    LazyLock::new(|| TABELA.iter().enumerate().map(|(i, e)| (e.0, i)).collect());

/// Número de entradas da tabela estática.
#[must_use]
pub fn total_extensoes_conhecidas() -> usize {
    TABELA.len()
}

fn tipo_da_entrada(e: &Entrada) -> TipoFormato {
    TipoFormato {
        extensao: e.0,
        mime: e.1,
        descricao: e.2,
        categoria: e.3,
    }
}

/// Busca pela chave canônica (já normalizada em minúsculas, sem ponto inicial).
#[must_use]
pub fn tipo_por_extensao(chave: &str) -> Option<TipoFormato> {
    INDICE.get(chave).map(|&i| tipo_da_entrada(&TABELA[i]))
}

/// Extrai a extensão de um nome de arquivo em bytes (aceita caminho completo).
///
/// Retorna `(original, normalizada)`. Nomes ocultos (`.bashrc`), sem ponto ou
/// terminados em ponto não têm extensão. Extensões compostas conhecidas
/// (`.tar.gz`, `.tar.zst`...) são devolvidas inteiras.
#[must_use]
pub fn extrair_extensao(nome: &[u8]) -> Option<(String, String)> {
    let base = nome.rsplit(|&b| b == b'/').next().unwrap_or(nome);
    let ultimo = base.iter().rposition(|&b| b == b'.')?;
    if ultimo == 0 || ultimo + 1 == base.len() {
        return None;
    }
    let simples = &base[ultimo + 1..];
    if let Some(anterior) = base[..ultimo].iter().rposition(|&b| b == b'.')
        && anterior > 0
    {
        let composta = &base[anterior + 1..];
        let normalizada = normalizar(composta);
        if INDICE.contains_key(normalizada.as_str()) {
            return Some((String::from_utf8_lossy(composta).into_owned(), normalizada));
        }
    }
    Some((
        String::from_utf8_lossy(simples).into_owned(),
        normalizar(simples),
    ))
}

fn normalizar(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_lowercase()
}

/// Resultado da identificação por extensão.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdentificacaoExtensao {
    pub extensao_original: Option<String>,
    pub extensao_normalizada: Option<String>,
    /// [`TIPO_DESCONHECIDO`] quando a extensão é ausente ou desconhecida.
    pub tipo: TipoFormato,
    /// `true` quando a extensão não consta na tabela (inclusive ausente).
    pub desconhecido: bool,
}

/// Identifica o formato apenas pelo nome (A-07, inferência por extensão).
#[must_use]
pub fn identificar_por_extensao(nome: &[u8]) -> IdentificacaoExtensao {
    let (original, normalizada) = match extrair_extensao(nome) {
        Some((o, n)) => (Some(o), Some(n)),
        None => (None, None),
    };
    let tipo = normalizada.as_deref().and_then(tipo_por_extensao);
    IdentificacaoExtensao {
        extensao_original: original,
        extensao_normalizada: normalizada,
        desconhecido: tipo.is_none(),
        tipo: tipo.unwrap_or(TIPO_DESCONHECIDO),
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn tabela_tem_ao_menos_150_entradas_sem_duplicatas() {
        assert!(total_extensoes_conhecidas() >= 150);
        assert_eq!(INDICE.len(), TABELA.len(), "chaves duplicadas na tabela");
        for e in TABELA {
            assert_eq!(e.0, e.0.to_lowercase());
            assert!(!e.1.is_empty() && !e.2.is_empty());
        }
    }

    #[test]
    fn extensao_simples_e_maiusculas() {
        let r = identificar_por_extensao(b"Relatorio.PDF");
        assert_eq!(r.extensao_original.as_deref(), Some("PDF"));
        assert_eq!(r.extensao_normalizada.as_deref(), Some("pdf"));
        assert_eq!(r.tipo.mime, "application/pdf");
        assert_eq!(r.tipo.descricao, "Portable Document Format");
        assert_eq!(r.tipo.categoria, CategoriaFormato::Documento);
        assert!(!r.desconhecido);
    }

    #[test]
    fn extensao_composta() {
        for (nome, chave) in [
            (&b"a.tar.gz"[..], "tar.gz"),
            (b"a.TAR.GZ", "tar.gz"),
            (b"/x/y/backup.tar.zst", "tar.zst"),
            (b"a.tar.xz", "tar.xz"),
            (b"a.tar.bz2", "tar.bz2"),
        ] {
            let r = identificar_por_extensao(nome);
            assert_eq!(r.extensao_normalizada.as_deref(), Some(chave), "{nome:?}");
            assert_eq!(r.tipo.categoria, CategoriaFormato::Compactado);
        }
        let r = identificar_por_extensao(b"a.tar.gz");
        assert_eq!(r.extensao_original.as_deref(), Some("tar.gz"));
    }

    #[test]
    fn multi_ponto_nao_composto_usa_ultimo_segmento() {
        let r = identificar_por_extensao(b"relatorio.v2.final.pdf");
        assert_eq!(r.extensao_normalizada.as_deref(), Some("pdf"));
        let r = identificar_por_extensao(b"foto.2020.01.jpg");
        assert_eq!(r.extensao_normalizada.as_deref(), Some("jpg"));
        let r = identificar_por_extensao(b"x.foo.gz");
        assert_eq!(r.extensao_normalizada.as_deref(), Some("gz"));
    }

    #[test]
    fn sem_extensao_ocultos_e_ponto_final() {
        for nome in [
            &b"Makefile"[..],
            b".bashrc",
            b".gitignore",
            b"arquivo.",
            b"",
        ] {
            let r = identificar_por_extensao(nome);
            assert!(r.extensao_original.is_none(), "{nome:?}");
            assert!(r.desconhecido);
            assert_eq!(r.tipo.categoria, CategoriaFormato::Desconhecido);
        }
        // Oculto com extensão: .config.json tem extensão json.
        let r = identificar_por_extensao(b".config.json");
        assert_eq!(r.extensao_normalizada.as_deref(), Some("json"));
        // Ponto em diretório não conta como extensão do nome.
        let r = identificar_por_extensao(b"/a.b/arquivo");
        assert!(r.extensao_original.is_none());
    }

    #[test]
    fn extensao_desconhecida_sinalizada() {
        let r = identificar_por_extensao(b"coisa.xyzzy");
        assert!(r.desconhecido);
        assert_eq!(r.extensao_normalizada.as_deref(), Some("xyzzy"));
        assert_eq!(r.tipo, TIPO_DESCONHECIDO);
    }

    #[test]
    fn nome_nao_utf8() {
        let r = identificar_por_extensao(b"foto\xff\xfe.JPG");
        assert_eq!(r.extensao_normalizada.as_deref(), Some("jpg"));
        let r = identificar_por_extensao(b"a.\xff\xfe");
        assert!(r.desconhecido);
        assert!(r.extensao_original.is_some());
    }

    #[test]
    fn categorias_representativas() {
        use CategoriaFormato as C;
        for (ext, cat) in [
            ("cr2", C::Imagem),
            ("nef", C::Imagem),
            ("dng", C::Imagem),
            ("arw", C::Imagem),
            ("flac", C::Audio),
            ("mkv", C::Video),
            ("7z", C::Compactado),
            ("exe", C::Executavel),
            ("so", C::Executavel),
            ("rs", C::Codigo),
            ("woff2", C::Fonte),
            ("epub", C::Livro),
            ("mobi", C::Livro),
            ("azw3", C::Livro),
            ("cbz", C::Livro),
            ("cbr", C::Livro),
            ("iso", C::ImagemDeDisco),
            ("vmdk", C::ImagemDeDisco),
            ("sqlite", C::BancoDeDados),
            ("mdb", C::BancoDeDados),
            ("desktop", C::Sistema),
        ] {
            assert_eq!(tipo_por_extensao(ext).unwrap().categoria, cat, "{ext}");
        }
    }
}
