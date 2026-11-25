//! Normalização de texto para comparar evidências: minúsculas, sem acentos nem ligaduras,
//! pontuação e espaços colapsados. É a única flexibilização permitida ao validador.

fn dobrar(c: char, saida: &mut String) {
    let r: &str = match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'ç' | 'ć' | 'č' | 'ĉ' => "c",
        'ď' | 'đ' => "d",
        'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'ğ' => "g",
        'í' | 'ì' | 'î' | 'ï' | 'ī' | 'ı' => "i",
        'ł' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'ř' => "r",
        'š' | 'ś' | 'ş' => "s",
        'ť' => "t",
        'ú' | 'ù' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
        'ý' | 'ÿ' => "y",
        'ž' | 'ź' | 'ż' => "z",
        'æ' => "ae",
        'œ' => "oe",
        'ß' => "ss",
        '\u{fb00}' => "ff",
        '\u{fb01}' => "fi",
        '\u{fb02}' => "fl",
        '\u{fb03}' => "ffi",
        '\u{fb04}' => "ffl",
        // Acentos combinantes soltos (NFD), hífen suave e largura zero somem.
        '\u{0300}'..='\u{036f}'
        | '\u{00ad}'
        | '\u{200b}'
        | '\u{200c}'
        | '\u{200d}'
        | '\u{feff}' => "",
        _ => {
            saida.push(c);
            return;
        }
    };
    saida.push_str(r);
}

/// Minúsculas, sem acentos/ligaduras; qualquer sequência de não alfanuméricos vira um espaço.
#[must_use]
pub fn normalizar(texto: &str) -> String {
    let mut dobrado = String::with_capacity(texto.len());
    for c in texto.chars() {
        for m in c.to_lowercase() {
            dobrar(m, &mut dobrado);
        }
    }
    let mut saida = String::with_capacity(dobrado.len());
    let mut pendente = false;
    for c in dobrado.chars() {
        if c.is_alphanumeric() {
            if pendente && !saida.is_empty() {
                saida.push(' ');
            }
            pendente = false;
            saida.push(c);
        } else {
            pendente = true;
        }
    }
    saida
}

/// Só os dígitos (e `X`) para comparar ISBN/DOI sem hífens e espaços.
#[must_use]
pub fn so_digitos(texto: &str) -> String {
    texto
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == 'x' || *c == 'X')
        .collect()
}

/// Tokens do texto normalizado.
#[must_use]
pub fn tokens(texto: &str) -> Vec<String> {
    normalizar(texto)
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Remove controles (exceto `\n` e `\t`), para texto de livro entrar no prompt.
#[must_use]
pub fn limpar_controles(texto: &str) -> String {
    texto
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn dobra_acentos_ligaduras_e_espacos() {
        assert_eq!(
            normalizar("  Introdução   à  Álgebra "),
            "introducao a algebra"
        );
        assert_eq!(normalizar("e\u{fb01}ciente"), "eficiente");
        assert_eq!(normalizar("Cálculo\n\tVolume-1"), "calculo volume 1");
        assert_eq!(normalizar("Cafe\u{0301}"), "cafe");
    }

    #[test]
    fn digitos() {
        assert_eq!(so_digitos("ISBN 978-85-359-0277-8"), "9788535902778");
    }
}
