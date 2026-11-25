//! ISBN: validação por dígito verificador, normalização para ISBN-13 e busca em texto livre.
//!
//! ISBN identifica Edição, mas sozinho não prova nada (spec, regras de classificação): quem
//! chama decide quanto confia.

/// Remove hífens/espaços e põe `X` em maiúscula. Não valida.
#[must_use]
pub fn limpar(bruto: &str) -> String {
    bruto
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == 'x' || *c == 'X')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// ISBN-10 válido (dígito verificador mod 11; `X` só na última posição).
#[must_use]
pub fn valido_10(isbn: &str) -> bool {
    let b = isbn.as_bytes();
    if b.len() != 10 {
        return false;
    }
    let mut soma = 0u32;
    for (i, &c) in b.iter().enumerate() {
        let v = match c {
            b'0'..=b'9' => u32::from(c - b'0'),
            b'X' if i == 9 => 10,
            _ => return false,
        };
        soma += v * (10 - i as u32);
    }
    soma.is_multiple_of(11)
}

/// ISBN-13 válido (prefixo 978/979 e dígito verificador mod 10).
#[must_use]
pub fn valido_13(isbn: &str) -> bool {
    let b = isbn.as_bytes();
    if b.len() != 13 || !b.iter().all(u8::is_ascii_digit) {
        return false;
    }
    if !(isbn.starts_with("978") || isbn.starts_with("979")) {
        return false;
    }
    let soma: u32 = b
        .iter()
        .enumerate()
        .map(|(i, &c)| u32::from(c - b'0') * if i % 2 == 0 { 1 } else { 3 })
        .sum();
    soma.is_multiple_of(10)
}

/// Converte ISBN-10 válido em ISBN-13 (prefixo 978).
#[must_use]
pub fn para_13(isbn10: &str) -> Option<String> {
    if !valido_10(isbn10) {
        return None;
    }
    let base = format!("978{}", &isbn10[..9]);
    let soma: u32 = base
        .bytes()
        .enumerate()
        .map(|(i, c)| u32::from(c - b'0') * if i % 2 == 0 { 1 } else { 3 })
        .sum();
    let dv = (10 - soma % 10) % 10;
    Some(format!("{base}{dv}"))
}

/// Normaliza qualquer ISBN (10 ou 13, com ou sem hífens) para ISBN-13 válido.
#[must_use]
pub fn normalizar(bruto: &str) -> Option<String> {
    let l = limpar(bruto);
    match l.len() {
        13 if valido_13(&l) => Some(l),
        10 => para_13(&l),
        _ => None,
    }
}

/// Encontra ISBNs válidos num texto (ficha catalográfica, verso da folha de rosto, OPF).
///
/// Aceita grupos de dígitos separados por hífen (ou espaço, só logo após o rótulo "ISBN").
/// Sem o rótulo, só vale ISBN-13 (978/979); ISBN-10 sem rótulo colide demais com telefones e
/// códigos. Devolve ISBN-13 normalizados, sem repetição, na ordem em que aparecem.
#[must_use]
pub fn encontrar(texto: &str) -> Vec<String> {
    let chars: Vec<char> = texto.chars().collect();
    let mut achados: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() || (i > 0 && chars[i - 1].is_ascii_alphanumeric()) {
            i += 1;
            continue;
        }
        let antes: String = chars[i.saturating_sub(12)..i].iter().collect();
        let rotulado = antes.to_ascii_lowercase().contains("isbn");
        // Junta dígitos, X final e separadores simples até 17 caracteres.
        let mut j = i;
        let mut digitos = String::new();
        while j < chars.len() && j - i < 17 {
            let c = chars[j];
            if c.is_ascii_digit() {
                digitos.push(c);
            } else if (c == 'X' || c == 'x') && digitos.len() == 9 {
                digitos.push('X');
                j += 1;
                break;
            } else if (c == '-' || (c == ' ' && rotulado) || c == '\u{2010}' || c == '\u{2013}')
                && j + 1 < chars.len()
                && chars[j + 1].is_ascii_digit()
                && !digitos.is_empty()
            {
                // separador
            } else {
                break;
            }
            j += 1;
            if digitos.len() == 13 {
                break;
            }
        }
        let candidato = if digitos.len() >= 13 {
            digitos[..13].to_string()
        } else {
            digitos.clone()
        };
        let norm = match candidato.len() {
            13 if valido_13(&candidato) => Some(candidato),
            10 if rotulado => para_13(&candidato),
            _ => None,
        };
        if let Some(n) = norm {
            if !achados.contains(&n) {
                achados.push(n);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    achados
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn valida_10_e_13() {
        assert!(valido_10("8535902775"));
        assert!(valido_10("080442957X"));
        assert!(!valido_10("8535902776"));
        assert!(valido_13("9788535902778"));
        assert!(!valido_13("9788535902779"));
        assert!(!valido_13("1234567890128"));
    }

    #[test]
    fn converte_10_para_13() {
        assert_eq!(para_13("8535902775").as_deref(), Some("9788535902778"));
        assert_eq!(
            normalizar("978-85-359-0277-8").as_deref(),
            Some("9788535902778")
        );
        assert_eq!(
            normalizar("0-8044-2957-X").as_deref(),
            Some("9780804429573")
        );
        assert_eq!(normalizar("123"), None);
    }

    #[test]
    fn encontra_em_ficha() {
        let ficha = "Dados Internacionais de Catalogação na Publicação (CIP)\n\
                     ISBN 978-85-359-0277-8\nISBN: 85-359-0277-5 (broch.)\nCDD 869.3\n\
                     Telefone 11 3707-3500";
        assert_eq!(encontrar(ficha), vec!["9788535902778".to_string()]);
    }

    #[test]
    fn ignora_numeros_invalidos() {
        assert!(encontrar("Pedido 9788535902779 e CEP 04532-002").is_empty());
        assert!(encontrar("tel. 11 3707-3500 ou 1137073500").is_empty());
        assert_eq!(
            encontrar("código 9788535902778"),
            vec!["9788535902778".to_string()]
        );
        assert_eq!(
            encontrar("isbn:080442957x."),
            vec!["9780804429573".to_string()]
        );
    }
}
