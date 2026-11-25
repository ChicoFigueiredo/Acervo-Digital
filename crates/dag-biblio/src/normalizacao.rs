//! Normalização de títulos e autores para comparação, `author_sort` no estilo do Calibre e
//! similaridade de strings. Tudo puro e determinístico, sem I/O.
//!
//! A normalização serve para COMPARAR; nunca substitui a grafia homologada que o usuário vê.

use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Texto básico
// ---------------------------------------------------------------------------

/// Dobra um caractere minúsculo latino para ASCII (remove acento). `None` = manter como está.
fn dobrar_char(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ţ' | 'ť' | 'ŧ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        'æ' => "ae",
        'œ' => "oe",
        'ß' => "ss",
        'þ' => "th",
        'ª' => "a",
        'º' => "o",
        _ => return None,
    })
}

fn passo_normalizar(s: &str) -> String {
    let mut saida = String::with_capacity(s.len());
    let mut ultimo_espaco = true;
    for c in s.chars() {
        // Marcas combinantes (texto decomposto): somem, o acento já foi "dobrado".
        if ('\u{300}'..='\u{36f}').contains(&c) {
            continue;
        }
        for m in c.to_lowercase() {
            if let Some(d) = dobrar_char(m) {
                saida.push_str(d);
                ultimo_espaco = false;
            } else if m.is_alphanumeric() {
                saida.push(m);
                ultimo_espaco = false;
            } else if !ultimo_espaco {
                saida.push(' ');
                ultimo_espaco = true;
            }
        }
    }
    if saida.ends_with(' ') {
        saida.pop();
    }
    saida
}

/// Minúsculas, sem acentos, sem pontuação, espaços colapsados. Idempotente.
#[must_use]
pub fn normalizar_texto(s: &str) -> String {
    let mut atual = passo_normalizar(s);
    // Ponto fixo: garante idempotência mesmo para letras exóticas cuja caixa muda duas vezes.
    for _ in 0..4 {
        let proximo = passo_normalizar(&atual);
        if proximo == atual {
            break;
        }
        atual = proximo;
    }
    atual
}

/// Tokens de [`normalizar_texto`].
#[must_use]
pub fn tokens(s: &str) -> Vec<String> {
    normalizar_texto(s)
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

// ---------------------------------------------------------------------------
// Título
// ---------------------------------------------------------------------------

/// Artigos iniciais (pt, en, es, fr) já normalizados. `l` cobre `l'homme`.
const ARTIGOS: &[&str] = &[
    "o", "a", "os", "as", "um", "uma", "uns", "umas", "the", "an", "el", "la", "los", "las", "un",
    "una", "unos", "unas", "le", "les", "l", "une", "des",
];

/// Palavras que antecedem o número do volume.
const MARCADORES_VOLUME: &[&str] = &[
    "vol", "volume", "v", "tomo", "t", "livro", "parte", "part", "pt", "book", "n", "no", "num",
    "numero", "number", "issue", "cap", "capitulo",
];

/// Título decomposto para comparação.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TituloNormalizado {
    /// Título principal, sem artigo inicial nem marcador de volume.
    pub principal: String,
    /// Subtítulo (depois de `:` ou ` - `), se houver.
    pub subtitulo: Option<String>,
    /// Volume/número ("1", "2", "12"), em algarismos arábicos.
    pub volume: Option<String>,
}

impl TituloNormalizado {
    /// Principal + subtítulo, para comparação "inteira".
    #[must_use]
    pub fn completo(&self) -> String {
        match &self.subtitulo {
            Some(s) => format!("{} {}", self.principal, s),
            None => self.principal.clone(),
        }
    }
}

fn valor_romano(s: &str) -> Option<u32> {
    if s.is_empty() || !s.chars().all(|c| matches!(c, 'i' | 'v' | 'x' | 'l')) {
        return None;
    }
    let val = |c: char| match c {
        'i' => 1u32,
        'v' => 5,
        'x' => 10,
        _ => 50,
    };
    let cs: Vec<u32> = s.chars().map(val).collect();
    let mut total = 0u32;
    for (i, v) in cs.iter().enumerate() {
        if cs.get(i + 1).is_some_and(|prox| prox > v) {
            total = total.checked_sub(*v)?;
        } else {
            total += v;
        }
    }
    (1..=49).contains(&total).then_some(total)
}

/// Número (algarismos ou romano) em algarismos sem zeros à esquerda. `permitir_i`: aceita o
/// romano `i` (=1) isolado, só seguro depois de um marcador.
fn numeral(t: &str, permitir_i: bool) -> Option<String> {
    if !t.is_empty() && t.len() <= 3 && t.chars().all(|c| c.is_ascii_digit()) {
        let n: u32 = t.parse().ok()?;
        return (n > 0).then(|| n.to_string());
    }
    let r = valor_romano(t)?;
    if r == 1 && !permitir_i {
        return None;
    }
    Some(r.to_string())
}

/// Extrai o volume de uma lista de tokens já normalizados. Devolve o volume e os tokens
/// restantes (sem o marcador/número).
fn extrair_volume_tokens(toks: Vec<String>) -> (Option<String>, Vec<String>) {
    for i in 0..toks.len() {
        let t = toks[i].as_str();
        if MARCADORES_VOLUME.contains(&t)
            && let Some(prox) = toks.get(i + 1)
            && let Some(n) = numeral(prox, true)
        {
            let mut resto = toks.clone();
            resto.drain(i..=i + 1);
            return (Some(n), resto);
        }
        // "v2", "vol2", "t3": letras + dígitos colados.
        let letras: String = t.chars().take_while(char::is_ascii_alphabetic).collect();
        let digitos = &t[letras.len()..];
        if !letras.is_empty()
            && !digitos.is_empty()
            && MARCADORES_VOLUME.contains(&letras.as_str())
            && let Some(n) = numeral(digitos, true)
        {
            let mut resto = toks.clone();
            resto.remove(i);
            return (Some(n), resto);
        }
    }
    // Número solto no fim ("Cálculo 2", "Cálculo II"), com pelo menos outra palavra antes.
    if toks.len() >= 2
        && let Some(ultimo) = toks.last()
        && let Some(n) = numeral(ultimo, false)
    {
        let mut resto = toks.clone();
        resto.pop();
        return (Some(n), resto);
    }
    (None, toks)
}

/// Extrai o volume de um texto livre (`"Vol. 2"`, `"Tomo II"`, `"#5"`, `"Cálculo 2"`).
#[must_use]
pub fn extrair_volume(texto: &str) -> Option<String> {
    extrair_volume_tokens(tokens(&texto.replace('#', " n ")))
        .0
        .or_else(|| numeral(&normalizar_texto(texto), true))
}

fn separar_subtitulo(s: &str) -> (String, Option<String>) {
    let mut corte: Option<(usize, usize)> = s.find(':').map(|i| (i, i + 1));
    for sep in [" - ", " – ", " — ", " -- "] {
        if let Some(i) = s.find(sep)
            && corte.is_none_or(|(c, _)| i < c)
        {
            corte = Some((i, i + sep.len()));
        }
    }
    match corte {
        Some((a, b)) => (s[..a].to_owned(), Some(s[b..].to_owned())),
        None => (s.to_owned(), None),
    }
}

/// Remove artigo da forma de ordenação "Título, O" / "Odyssey, The".
fn sem_artigo_final(s: &str) -> &str {
    if let Some((cabeca, cauda)) = s.rsplit_once(',') {
        let c = normalizar_texto(cauda);
        if ARTIGOS.contains(&c.as_str()) && !cabeca.trim().is_empty() {
            return cabeca;
        }
    }
    s
}

fn tirar_artigo_inicial(toks: &mut Vec<String>) {
    if toks.len() > 1 && ARTIGOS.contains(&toks[0].as_str()) {
        toks.remove(0);
    }
}

/// Normaliza um título para comparação: minúsculas, sem acento, sem artigo inicial
/// (pt/en/es/fr), sem pontuação, subtítulo separado (`:` ou ` - `) e volume extraído.
#[must_use]
pub fn normalizar_titulo(titulo: &str) -> TituloNormalizado {
    let bruto = titulo.replace(['_', '#'], " ");
    let (p, sub) = separar_subtitulo(&bruto);
    let p = sem_artigo_final(&p);
    let ptoks = tokens(p);
    let mut stoks = sub.as_deref().map(tokens).unwrap_or_default();
    let (mut vol, mut ptoks) = extrair_volume_tokens(ptoks);
    if vol.is_none() && !stoks.is_empty() {
        // "Cálculo - Volume 2": o volume veio no "subtítulo".
        let (v, s2) = extrair_volume_tokens(stoks);
        vol = v;
        stoks = s2;
    }
    tirar_artigo_inicial(&mut ptoks);
    TituloNormalizado {
        principal: ptoks.join(" "),
        subtitulo: (!stoks.is_empty()).then(|| stoks.join(" ")),
        volume: vol,
    }
}

const ORDINAIS: &[(&str, u32)] = &[
    ("primeira", 1),
    ("primeiro", 1),
    ("segunda", 2),
    ("segundo", 2),
    ("terceira", 3),
    ("terceiro", 3),
    ("quarta", 4),
    ("quinta", 5),
    ("first", 1),
    ("second", 2),
    ("third", 3),
    ("fourth", 4),
    ("fifth", 5),
];

/// Número da edição em textos como "2ª edição", "3rd ed.", "Segunda edição".
#[must_use]
pub fn numero_edicao(texto: &str) -> Option<u32> {
    let toks = tokens(texto);
    let marca = ["ed", "edicao", "edition", "edicion", "edizione", "revised"];
    let ler = |t: &str| -> Option<u32> {
        if let Some((_, n)) = ORDINAIS.iter().find(|(p, _)| *p == t) {
            return Some(*n);
        }
        let digitos: String = t.chars().take_while(char::is_ascii_digit).collect();
        let sufixo = &t[digitos.len()..];
        if digitos.is_empty() || !matches!(sufixo, "" | "a" | "o" | "st" | "nd" | "rd" | "th" | "e")
        {
            return None;
        }
        digitos.parse().ok().filter(|n| (1..100).contains(n))
    };
    for (i, t) in toks.iter().enumerate() {
        if marca.contains(&t.as_str()) {
            if i > 0
                && let Some(n) = ler(&toks[i - 1])
            {
                return Some(n);
            }
            if let Some(n) = toks.get(i + 1).and_then(|x| ler(x)) {
                return Some(n);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Autor
// ---------------------------------------------------------------------------

/// Partículas de sobrenome (já normalizadas).
const PARTICULAS: &[&str] = &[
    "de", "da", "do", "dos", "das", "di", "du", "del", "della", "des", "van", "von", "der", "den",
    "ter", "la", "le", "el", "al", "bin", "ibn", "y",
];

const HONORIFICOS: &[&str] = &["dr", "dra", "prof", "profa", "mr", "mrs", "ms", "sir"];
const TITULOS_FINAIS: &[&str] = &["phd", "md"];
const SUFIXOS: &[&str] = &[
    "jr", "junior", "filho", "sr", "senior", "neto", "sobrinho", "ii", "iii", "iv",
];

fn sufixo_canonico(s: &str) -> &str {
    match s {
        "junior" | "filho" => "jr",
        "senior" => "sr",
        outro => outro,
    }
}

/// Nome de autor decomposto para comparação.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AutorNormalizado {
    /// Prenomes (tokens normalizados; iniciais viram um token de 1 letra).
    pub prenomes: Vec<String>,
    /// Partículas ("de", "van"), fora da chave.
    pub particulas: Vec<String>,
    /// Sobrenome (um ou mais tokens; o último é o "sobrenome final").
    pub sobrenome: Vec<String>,
    /// Sufixo canônico ("jr", "neto", "iii").
    pub sufixo: Option<String>,
}

impl AutorNormalizado {
    /// Último token do sobrenome, base de comparação e de blocos.
    #[must_use]
    pub fn sobrenome_final(&self) -> &str {
        self.sobrenome.last().map_or("", String::as_str)
    }

    /// Chave estável "sobrenome|inicial-do-1º-prenome" (blocos e confirmações).
    #[must_use]
    pub fn chave(&self) -> String {
        let ini: String = self
            .prenomes
            .first()
            .and_then(|p| p.chars().next())
            .map(String::from)
            .unwrap_or_default();
        format!("{}|{}", self.sobrenome_final(), ini)
    }

    /// Forma completa canônica: "prenomes particulas sobrenome sufixo".
    #[must_use]
    pub fn canonico(&self) -> String {
        let mut p: Vec<&str> = self.prenomes.iter().map(String::as_str).collect();
        p.extend(self.particulas.iter().map(String::as_str));
        p.extend(self.sobrenome.iter().map(String::as_str));
        if let Some(s) = &self.sufixo {
            p.push(s);
        }
        p.join(" ")
    }
}

/// Resultado da comparação de dois nomes de autor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompatAutor {
    /// Nomes diferentes.
    Diferente,
    /// Sufixo diverge ou só um lado tem ("João Silva" x "João Silva Filho"): pessoas distintas
    /// prováveis, nunca unir sozinho.
    SufixoAmbiguo,
    /// Compatíveis por iniciais ou nomes do meio ausentes ("J. R. R. Tolkien" x "John Tolkien").
    Compativel,
    /// Mesmos tokens.
    Igual,
}

fn remover_colchetes(s: &str) -> String {
    let mut abertos = [0u32; 3];
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '(' => abertos[0] += 1,
            '[' => abertos[1] += 1,
            '{' => abertos[2] += 1,
            ')' => abertos[0] = abertos[0].saturating_sub(1),
            ']' => abertos[1] = abertos[1].saturating_sub(1),
            '}' => abertos[2] = abertos[2].saturating_sub(1),
            _ if abertos.iter().sum::<u32>() < 1 => out.push(c),
            _ => {}
        }
    }
    out
}

/// "JRR" (maiúsculas, sem vogal) vira as iniciais "J R R".
fn expandir_siglas(s: &str) -> String {
    s.split_whitespace()
        .map(|t| {
            let letras: Vec<char> = t.chars().collect();
            let sigla = (2..=4).contains(&letras.len())
                && letras.iter().all(char::is_ascii_uppercase)
                && !letras.iter().any(|c| "AEIOUY".contains(*c));
            if sigla {
                letras
                    .iter()
                    .map(char::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                t.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn extrair_sufixo(toks: &mut Vec<String>) -> Option<String> {
    while toks.len() > 2
        && toks
            .last()
            .is_some_and(|t| TITULOS_FINAIS.contains(&t.as_str()))
    {
        toks.pop();
    }
    if toks.len() > 2
        && let Some(ultimo) = toks.last()
        && SUFIXOS.contains(&ultimo.as_str())
    {
        let s = sufixo_canonico(ultimo).to_owned();
        toks.pop();
        return Some(s);
    }
    None
}

fn tirar_honorificos(toks: &mut Vec<String>) {
    while toks.len() > 1 && HONORIFICOS.contains(&toks[0].as_str()) {
        toks.remove(0);
    }
}

/// Decompõe um nome de autor ("Sobrenome, Nome" ou "Nome Sobrenome").
#[must_use]
pub fn normalizar_autor(nome: &str) -> AutorNormalizado {
    let limpo = remover_colchetes(nome);
    let limpo = limpo.trim();
    let mut a = AutorNormalizado::default();
    if let Some((esq, dir)) = limpo.split_once(',')
        && !dir.trim().is_empty()
        && !esq.trim().is_empty()
    {
        let mut partes = dir.split(',');
        let dado = partes.next().unwrap_or("");
        let mut sufixo: Option<String> = None;
        for extra in partes {
            let t = tokens(extra);
            if let Some(s) = t.first().filter(|s| SUFIXOS.contains(&s.as_str())) {
                sufixo = Some(sufixo_canonico(s).to_owned());
            }
        }
        let mut dtoks = tokens(&expandir_siglas(dado));
        tirar_honorificos(&mut dtoks);
        while dtoks.len() > 1
            && dtoks
                .last()
                .is_some_and(|t| TITULOS_FINAIS.contains(&t.as_str()))
        {
            dtoks.pop();
        }
        if sufixo.is_none()
            && dtoks.len() > 1
            && let Some(u) = dtoks.last().filter(|s| SUFIXOS.contains(&s.as_str()))
        {
            sufixo = Some(sufixo_canonico(u).to_owned());
            dtoks.pop();
        }
        let mut stoks = tokens(esq);
        // Partículas no fim do prenome ("Beethoven, Ludwig van") pertencem ao sobrenome.
        while dtoks.len() > 1
            && dtoks
                .last()
                .is_some_and(|t| PARTICULAS.contains(&t.as_str()))
        {
            let p = dtoks.pop().unwrap_or_default();
            a.particulas.insert(0, p);
        }
        // Partículas no começo do sobrenome ("von Neumann, John").
        while stoks.len() > 1 && PARTICULAS.contains(&stoks[0].as_str()) {
            a.particulas.push(stoks.remove(0));
        }
        a.prenomes = dtoks;
        a.sobrenome = stoks;
        a.sufixo = sufixo;
        return a;
    }
    let mut toks = tokens(&expandir_siglas(limpo));
    tirar_honorificos(&mut toks);
    a.sufixo = extrair_sufixo(&mut toks);
    match toks.len() {
        0 => {}
        1 => a.sobrenome = toks,
        _ => {
            let ultimo = toks.pop().unwrap_or_default();
            a.sobrenome = vec![ultimo];
            while toks.len() > 1
                && toks
                    .last()
                    .is_some_and(|t| PARTICULAS.contains(&t.as_str()))
            {
                let p = toks.pop().unwrap_or_default();
                a.particulas.insert(0, p);
            }
            a.prenomes = toks;
        }
    }
    a
}

fn token_compativel(a: &str, b: &str) -> bool {
    a == b
        || (a.chars().count() == 1 && b.starts_with(a))
        || (b.chars().count() == 1 && a.starts_with(b))
}

/// Compara dois nomes de autor sem nunca afirmar identidade: só compatibilidade de grafia.
#[must_use]
pub fn comparar_autores(x: &str, y: &str) -> CompatAutor {
    comparar_autores_norm(&normalizar_autor(x), &normalizar_autor(y))
}

/// Versão de [`comparar_autores`] para nomes já decompostos.
#[must_use]
pub fn comparar_autores_norm(a: &AutorNormalizado, b: &AutorNormalizado) -> CompatAutor {
    if a.sobrenome.is_empty() || b.sobrenome.is_empty() {
        return CompatAutor::Diferente;
    }
    // Sobrenomes compostos podem cair nos prenomes ("Gabriel García Márquez"): comparo o
    // último token do sobrenome e exijo prenomes compatíveis.
    if a.sobrenome_final() != b.sobrenome_final() {
        return CompatAutor::Diferente;
    }
    let (pa, pb) = (&a.prenomes, &b.prenomes);
    let menor = pa.len().min(pb.len());
    if menor > 0 {
        if !token_compativel(&pa[0], &pb[0]) {
            return CompatAutor::Diferente;
        }
        let composto = a.sobrenome.len() != b.sobrenome.len();
        for i in 1..menor {
            if !composto && !token_compativel(&pa[i], &pb[i]) {
                return CompatAutor::Diferente;
            }
        }
    }
    match (&a.sufixo, &b.sufixo) {
        (None, None) => {}
        (Some(x), Some(y)) if x == y => {}
        _ => return CompatAutor::SufixoAmbiguo,
    }
    if a.canonico() == b.canonico() {
        CompatAutor::Igual
    } else {
        CompatAutor::Compativel
    }
}

/// "Sobrenome, Nome" para "Nome Sobrenome", preservando acentos e caixa. Sem vírgula: devolve igual.
#[must_use]
pub fn para_nome_sobrenome(nome: &str) -> String {
    let n = nome.trim();
    match n.split_once(',') {
        Some((s, resto)) if !s.trim().is_empty() && !resto.trim().is_empty() => {
            let mut partes = resto.split(',').map(str::trim).filter(|p| !p.is_empty());
            let dado = partes.next().unwrap_or("");
            let mut r = format!("{dado} {}", s.trim());
            for e in partes {
                r.push(' ');
                r.push_str(e);
            }
            r.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        _ => n.split_whitespace().collect::<Vec<_>>().join(" "),
    }
}

// ---------------------------------------------------------------------------
// author_sort (Calibre)
// ---------------------------------------------------------------------------

/// Método de cópia do `author_sort`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetodoAuthorSort {
    Comma,
    NoComma,
    Copy,
}

/// Opções de `author_to_author_sort`, com os padrões do Calibre (`default_tweaks.py`).
#[derive(Debug, Clone)]
pub struct OpcoesAuthorSort {
    pub metodo: MetodoAuthorSort,
    pub copywords: Vec<String>,
    pub usar_prefixos_de_sobrenome: bool,
    pub prefixos_de_sobrenome: Vec<String>,
    pub prefixos_de_nome: Vec<String>,
    pub sufixos_de_nome: Vec<String>,
}

impl Default for OpcoesAuthorSort {
    fn default() -> Self {
        let v = |xs: &[&str]| xs.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        Self {
            metodo: MetodoAuthorSort::Comma,
            copywords: v(&[
                "Agency",
                "Corporation",
                "Company",
                "Co.",
                "Council",
                "Committee",
                "Inc.",
                "Institute",
                "National",
                "Society",
                "Club",
                "Team",
                "Software",
                "Games",
                "Entertainment",
                "Media",
                "Studios",
            ]),
            usar_prefixos_de_sobrenome: false,
            prefixos_de_sobrenome: v(&["da", "de", "di", "la", "le", "van", "von"]),
            prefixos_de_nome: v(&["Mr", "Mrs", "Ms", "Dr", "Prof"]),
            sufixos_de_nome: v(&[
                "Jr", "Sr", "Inc", "Ph.D", "Phd", "MD", "M.D", "I", "II", "III", "IV", "Junior",
                "Senior",
            ]),
        }
    }
}

/// `author_sort` com as regras padrão do Calibre ("Nome Sobrenome" para "Sobrenome, Nome").
#[must_use]
pub fn author_sort(autor: &str) -> String {
    author_sort_com(autor, &OpcoesAuthorSort::default())
}

/// Lista de autores para o `author_sort` do livro (Calibre junta com " & ").
#[must_use]
pub fn author_sort_lista(autores: &[String]) -> String {
    autores
        .iter()
        .map(|a| author_sort(a))
        .collect::<Vec<_>>()
        .join(" & ")
}

fn conjunto_minusculo(xs: &[String], com_ponto: bool) -> HashSet<String> {
    let mut s: HashSet<String> = xs.iter().map(|x| x.to_lowercase()).collect();
    if com_ponto {
        let extra: Vec<String> = s.iter().map(|p| format!("{p}.")).collect();
        s.extend(extra);
    }
    s
}

/// Port fiel de `calibre.ebooks.metadata.author_to_author_sort`.
#[must_use]
pub fn author_sort_com(autor: &str, op: &OpcoesAuthorSort) -> String {
    if autor.is_empty() {
        return String::new();
    }
    if op.metodo == MetodoAuthorSort::Copy {
        return autor.to_owned();
    }
    let sauthor = remover_colchetes(autor);
    let sauthor = sauthor.trim();
    if op.metodo == MetodoAuthorSort::Comma && sauthor.contains(',') {
        return autor.to_owned();
    }
    let mut toks: Vec<String> = sauthor.split_whitespace().map(str::to_owned).collect();
    if toks.len() < 2 {
        return autor.to_owned();
    }
    let copy = conjunto_minusculo(&op.copywords, false);
    if toks.iter().any(|t| copy.contains(&t.to_lowercase())) {
        return autor.to_owned();
    }
    let sp = conjunto_minusculo(&op.prefixos_de_sobrenome, false);
    if op.usar_prefixos_de_sobrenome && toks.len() == 2 && sp.contains(&toks[0].to_lowercase()) {
        return autor.to_owned();
    }
    let prefixos = conjunto_minusculo(&op.prefixos_de_nome, true);
    let Some(primeiro) = toks
        .iter()
        .position(|t| !prefixos.contains(&t.to_lowercase()))
    else {
        return autor.to_owned();
    };
    let sufixos = conjunto_minusculo(&op.sufixos_de_nome, true);
    // Laço do Python: de trás para frente até achar um não-sufixo; se todos forem sufixo,
    // a variável termina em `primeiro`.
    let mut ultimo = primeiro;
    for i in (primeiro..toks.len()).rev() {
        ultimo = i;
        if !sufixos.contains(&toks[i].to_lowercase()) {
            break;
        }
    }
    let sufixo = toks[ultimo + 1..].join(" ");
    if op.usar_prefixos_de_sobrenome
        && ultimo > primeiro
        && sp.contains(&toks[ultimo - 1].to_lowercase())
    {
        let fim = toks[ultimo].clone();
        toks[ultimo - 1].push(' ');
        toks[ultimo - 1].push_str(&fim);
        ultimo -= 1;
    }
    let mut atokens: Vec<String> = vec![toks[ultimo].clone()];
    atokens.extend(toks[primeiro..ultimo].iter().cloned());
    let num = atokens.len();
    if !sufixo.is_empty() {
        atokens.push(sufixo);
    }
    if op.metodo != MetodoAuthorSort::NoComma && num > 1 {
        atokens[0].push(',');
    }
    atokens.join(" ")
}

// ---------------------------------------------------------------------------
// Similaridade
// ---------------------------------------------------------------------------

/// Similaridade de Jaro (0..=1).
#[must_use]
pub fn jaro(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let janela = (a.len().max(b.len()) / 2).saturating_sub(1);
    let mut ma = vec![false; a.len()];
    let mut mb = vec![false; b.len()];
    let mut casados = 0usize;
    for (i, ca) in a.iter().enumerate() {
        let ini = i.saturating_sub(janela);
        let fim = (i + janela + 1).min(b.len());
        for j in ini..fim {
            if !mb[j] && b[j] == *ca {
                ma[i] = true;
                mb[j] = true;
                casados += 1;
                break;
            }
        }
    }
    if casados == 0 {
        return 0.0;
    }
    let mut k = 0;
    let mut trocas = 0usize;
    for (i, ca) in a.iter().enumerate() {
        if ma[i] {
            while !mb[k] {
                k += 1;
            }
            if *ca != b[k] {
                trocas += 1;
            }
            k += 1;
        }
    }
    let m = casados as f64;
    (m / a.len() as f64 + m / b.len() as f64 + (m - (trocas / 2) as f64) / m) / 3.0
}

/// Jaro-Winkler (prefixo comum até 4, fator 0,1).
#[must_use]
pub fn jaro_winkler(a: &str, b: &str) -> f64 {
    let j = jaro(a, b);
    let prefixo = a
        .chars()
        .zip(b.chars())
        .take(4)
        .take_while(|(x, y)| x == y)
        .count();
    j + prefixo as f64 * 0.1 * (1.0 - j)
}

/// Distância de Levenshtein em caracteres.
#[must_use]
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut anterior: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut atual = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let custo = usize::from(ca != cb);
            atual.push(
                (anterior[j] + custo)
                    .min(anterior[j + 1] + 1)
                    .min(atual[j] + 1),
            );
        }
        anterior = atual;
    }
    anterior[b.len()]
}

/// Similaridade de Dice por trigramas de caracteres (com bordas), 0..=1.
#[must_use]
pub fn trigramas(a: &str, b: &str) -> f64 {
    fn conjunto(s: &str) -> HashSet<String> {
        let cs: Vec<char> = format!("  {s} ").chars().collect();
        cs.windows(3).map(|w| w.iter().collect()).collect()
    }
    if a == b {
        return 1.0;
    }
    let (ca, cb) = (conjunto(a), conjunto(b));
    let comum = ca.intersection(&cb).count();
    2.0 * comum as f64 / (ca.len() + cb.len()) as f64
}

fn sim_texto(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    // Ordem das palavras importa pouco em títulos: mistura Jaro-Winkler, trigramas e Jaccard.
    let ta: HashSet<&str> = a.split(' ').collect();
    let tb: HashSet<&str> = b.split(' ').collect();
    let jac = ta.intersection(&tb).count() as f64 / ta.union(&tb).count().max(1) as f64;
    (jaro_winkler(a, b) * 0.4 + trigramas(a, b) * 0.4 + jac * 0.2).min(1.0)
}

/// Similaridade (0..=1) entre dois títulos quaisquer: normaliza, compara principais e, se
/// ambos têm, subtítulos. Volumes diferentes NÃO são tratados aqui (ver `identidade`).
#[must_use]
pub fn similaridade_titulos(a: &str, b: &str) -> f64 {
    similaridade_titulos_norm(&normalizar_titulo(a), &normalizar_titulo(b))
}

/// Versão de [`similaridade_titulos`] para títulos já normalizados.
#[must_use]
pub fn similaridade_titulos_norm(a: &TituloNormalizado, b: &TituloNormalizado) -> f64 {
    let base = sim_texto(&a.principal, &b.principal);
    match (&a.subtitulo, &b.subtitulo) {
        (Some(x), Some(y)) => base * 0.7 + sim_texto(x, y) * 0.3,
        _ => base,
    }
}

/// Distância de títulos = 1 - similaridade.
#[must_use]
pub fn distancia_titulos(a: &str, b: &str) -> f64 {
    1.0 - similaridade_titulos(a, b)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn texto_sem_acento_e_pontuacao() {
        assert_eq!(
            normalizar_texto("  Cálculo, Vol. 1: Ação!  "),
            "calculo vol 1 acao"
        );
        assert_eq!(normalizar_texto("Ærø Straße Œuvre"), "aero strasse oeuvre");
        assert_eq!(normalizar_texto("a\u{301}gua"), "agua");
    }

    #[test]
    fn titulo_artigos_pt_en_es_fr() {
        for (entrada, esperado) in [
            ("O Guarani", "guarani"),
            ("As Aventuras de Tom Sawyer", "aventuras de tom sawyer"),
            ("The Great Gatsby", "great gatsby"),
            ("A Tale of Two Cities", "tale of two cities"),
            ("El Aleph", "aleph"),
            ("Los miserables", "miserables"),
            ("Le Petit Prince", "petit prince"),
            ("L'Étranger", "etranger"),
            ("Une saison en enfer", "saison en enfer"),
            ("Odyssey, The", "odyssey"),
        ] {
            assert_eq!(normalizar_titulo(entrada).principal, esperado, "{entrada}");
        }
        assert_eq!(normalizar_titulo("A").principal, "a");
    }

    #[test]
    fn titulo_subtitulo() {
        let t = normalizar_titulo("Fundamentos de Física: Mecânica");
        assert_eq!(t.principal, "fundamentos de fisica");
        assert_eq!(t.subtitulo.as_deref(), Some("mecanica"));
        let t = normalizar_titulo("Dom Casmurro - Edição Comentada");
        assert_eq!(t.principal, "dom casmurro");
        assert_eq!(t.subtitulo.as_deref(), Some("edicao comentada"));
        assert_eq!(normalizar_titulo("Guarda-chuva").subtitulo, None);
    }

    #[test]
    fn titulo_volume() {
        assert_eq!(
            normalizar_titulo("Cálculo Vol. 2").volume.as_deref(),
            Some("2")
        );
        assert_eq!(normalizar_titulo("Cálculo II").volume.as_deref(), Some("2"));
        assert_eq!(
            normalizar_titulo("Cálculo, Volume 1").volume.as_deref(),
            Some("1")
        );
        assert_eq!(
            normalizar_titulo("Cálculo - Volume 3").volume.as_deref(),
            Some("3")
        );
        assert_eq!(
            normalizar_titulo("Batman #12").volume.as_deref(),
            Some("12")
        );
        assert_eq!(normalizar_titulo("Calculo v2").volume.as_deref(), Some("2"));
        assert_eq!(normalizar_titulo("Dom Casmurro").volume, None);
        assert_eq!(normalizar_titulo("Cálculo Vol. 2").principal, "calculo");
        assert_eq!(extrair_volume("Vol. 7"), Some("7".into()));
        assert_eq!(extrair_volume("II"), Some("2".into()));
    }

    #[test]
    fn numero_da_edicao() {
        assert_eq!(numero_edicao("2ª edição"), Some(2));
        assert_eq!(numero_edicao("3rd ed."), Some(3));
        assert_eq!(numero_edicao("Segunda edição"), Some(2));
        assert_eq!(numero_edicao("Edition 5"), Some(5));
        assert_eq!(numero_edicao("Companhia das Letras"), None);
    }

    #[test]
    fn autor_formas_equivalentes() {
        assert_eq!(
            comparar_autores("Assis, Machado de", "Machado de Assis"),
            CompatAutor::Igual
        );
        assert_eq!(
            comparar_autores("Silva, João", "João Silva"),
            CompatAutor::Igual
        );
        assert_eq!(
            comparar_autores("SILVA, JOÃO", "joao silva"),
            CompatAutor::Igual
        );
        assert_eq!(
            comparar_autores("Beethoven, Ludwig van", "Ludwig van Beethoven"),
            CompatAutor::Igual
        );
        assert_eq!(
            comparar_autores("García Márquez, Gabriel", "Gabriel García Márquez"),
            CompatAutor::Igual
        );
    }

    #[test]
    fn autor_iniciais() {
        assert_eq!(
            comparar_autores("J. R. R. Tolkien", "John Ronald Reuel Tolkien"),
            CompatAutor::Compativel
        );
        assert_eq!(
            comparar_autores("JRR Tolkien", "J. R. R. Tolkien"),
            CompatAutor::Igual
        );
        assert_eq!(
            comparar_autores("Tolkien, J.R.R.", "Tolkien, John Ronald"),
            CompatAutor::Compativel
        );
        assert_eq!(
            comparar_autores("J. Silva", "João Silva"),
            CompatAutor::Compativel
        );
        assert_eq!(
            comparar_autores("M. Silva", "João Silva"),
            CompatAutor::Diferente
        );
    }

    #[test]
    fn autor_sufixos_nunca_unem() {
        assert_eq!(
            comparar_autores("João Silva", "João Silva Filho"),
            CompatAutor::SufixoAmbiguo
        );
        assert_eq!(
            comparar_autores("Silva, João, Jr.", "João Silva Júnior"),
            CompatAutor::Igual
        );
        assert_eq!(
            comparar_autores("João Silva Filho", "João Silva Neto"),
            CompatAutor::SufixoAmbiguo
        );
        assert_eq!(
            comparar_autores("Martin Luther King Jr.", "Martin Luther King Sr."),
            CompatAutor::SufixoAmbiguo
        );
        let n = normalizar_autor("Pedro Neto");
        assert_eq!(n.sobrenome_final(), "neto");
        assert_eq!(n.sufixo, None);
    }

    #[test]
    fn autor_diferente() {
        assert_eq!(
            comparar_autores("João Silva", "João Santos"),
            CompatAutor::Diferente
        );
        assert_eq!(
            comparar_autores("Ana Silva", "João Silva"),
            CompatAutor::Diferente
        );
    }

    #[test]
    fn autor_particulas() {
        let a = normalizar_autor("Ludwig van Beethoven");
        assert_eq!(a.prenomes, ["ludwig"]);
        assert_eq!(a.particulas, ["van"]);
        assert_eq!(a.sobrenome, ["beethoven"]);
        let b = normalizar_autor("von Neumann, John");
        assert_eq!(b.sobrenome_final(), "neumann");
        assert_eq!(b.particulas, ["von"]);
        assert_eq!(a.chave(), "beethoven|l");
    }

    #[test]
    fn nome_sobrenome_para_exibicao() {
        assert_eq!(para_nome_sobrenome("Assis, Machado de"), "Machado de Assis");
        assert_eq!(para_nome_sobrenome("Silva, João, Jr."), "João Silva Jr.");
        assert_eq!(para_nome_sobrenome("Machado  de Assis"), "Machado de Assis");
    }

    #[test]
    fn author_sort_regras_do_calibre() {
        for (entrada, esperado) in [
            ("Dr. John Smith Jr.", "Smith, John Jr."),
            ("Mr Smith", "Smith"),
            ("Jean-Paul Sartre", "Sartre, Jean-Paul"),
            ("Madonna", "Madonna"),
            ("Johann Sebastian Bach III", "Bach, Johann Sebastian III"),
            ("Smith, John", "Smith, John"),
            ("José de Alencar", "Alencar, José de"),
            ("The Beatles Team", "The Beatles Team"),
            ("Carl Friedrich Gauss", "Gauss, Carl Friedrich"),
            ("J. R. R. Tolkien", "Tolkien, J. R. R."),
            ("Foo Bar (ed.)", "Bar, Foo"),
            ("", ""),
        ] {
            assert_eq!(author_sort(entrada), esperado, "{entrada}");
        }
        assert_eq!(
            author_sort_lista(&["Isaac Asimov".into(), "Arthur C. Clarke".into()]),
            "Asimov, Isaac & Clarke, Arthur C."
        );
    }

    /// Compara com o Calibre real quando `calibre-debug` existe; senão avisa e passa.
    #[test]
    fn author_sort_igual_ao_calibre() {
        let nomes = [
            "Dr. John Smith Jr.",
            "Mr Smith",
            "Jean-Paul Sartre",
            "Madonna",
            "Johann Sebastian Bach III",
            "Smith, John",
            "José de Alencar",
            "The Beatles Team",
            "Maria da Silva Filho",
            "Carl Friedrich Gauss",
            "J. R. R. Tolkien",
            "Ph.D. Foo",
            "Foo Bar (ed.)",
            "Foo Bar Inc",
            "Machado de Assis",
            "Gabriel García Márquez",
            "Ludwig van Beethoven",
            "Prof. Ana Maria Souza",
            "Fulano de Tal Junior",
            "Dr. Dr. Who",
            "Sr. Jr.",
            "Clarice Lispector",
            "Isaac Asimov Jr.",
            "Anônimo",
            "Platão",
            "John  Smith",
            "Ana (organizadora) Costa",
            "Jr Smith",
        ];
        let lista = nomes
            .iter()
            .map(|n| format!("{n:?}"))
            .collect::<Vec<_>>()
            .join(",");
        let codigo = format!(
            "import json\nfrom calibre.ebooks.metadata import author_to_author_sort as a\nprint('@@' + json.dumps([a(n) for n in [{lista}]]))"
        );
        let saida = std::process::Command::new("calibre-debug")
            .arg("-c")
            .arg(&codigo)
            .output();
        let Ok(saida) = saida else {
            eprintln!("AVISO: calibre-debug ausente; paridade do author_sort ignorada");
            return;
        };
        let texto = String::from_utf8_lossy(&saida.stdout);
        let Some(json) = texto.lines().find_map(|l| l.strip_prefix("@@")) else {
            eprintln!("AVISO: calibre-debug não respondeu; paridade ignorada");
            return;
        };
        let esperados: Vec<String> = serde_json::from_str(json).unwrap();
        for (n, e) in nomes.iter().zip(esperados) {
            assert_eq!(author_sort(n), e, "divergência do Calibre em {n:?}");
        }
    }

    #[test]
    fn similaridade_basica() {
        assert!((jaro_winkler("martha", "marhta") - 0.9611).abs() < 0.001);
        assert!((jaro_winkler("abc", "abc") - 1.0).abs() < 1e-9);
        assert!(jaro_winkler("abc", "xyz") < 0.1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert!(trigramas("calculo", "calculo") > 0.999);
        assert!(trigramas("calculo", "algebra") < 0.2);
    }

    #[test]
    fn similaridade_de_titulos() {
        assert!(similaridade_titulos("O Guarani", "Guarani") > 0.99);
        assert!(similaridade_titulos("Cálculo: Volume 1", "Calculo 1") > 0.9);
        assert!(similaridade_titulos("Dom Casmurro", "Memórias Póstumas de Brás Cubas") < 0.5);
        assert!(distancia_titulos("Dom Casmurro", "Dom Casmurro") < 1e-9);
    }

    /// Propriedade: normalizar duas vezes = normalizar uma vez (gerador determinístico).
    #[test]
    fn normalizacao_idempotente_por_propriedade() {
        let alfabeto: Vec<char> =
            "abcXYZ àáâãçéêíóôõúüñÅÉÖİßæœ ĲǅΣσς 0123 .,:;-_'\"!?()[]#&/\u{301}\u{307}ǆ"
                .chars()
                .collect();
        let mut estado: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut proximo = || {
            estado ^= estado << 13;
            estado ^= estado >> 7;
            estado ^= estado << 17;
            estado
        };
        for _ in 0..3000 {
            let n = (proximo() % 24) as usize;
            let s: String = (0..n)
                .map(|_| alfabeto[(proximo() % alfabeto.len() as u64) as usize])
                .collect();
            let uma = normalizar_texto(&s);
            assert_eq!(normalizar_texto(&uma), uma, "texto {s:?}");
            let t = normalizar_titulo(&s);
            assert_eq!(normalizar_texto(&t.principal), t.principal, "titulo {s:?}");
            let a = normalizar_autor(&s).canonico();
            assert_eq!(normalizar_texto(&a), a, "autor {s:?}");
        }
    }
}
