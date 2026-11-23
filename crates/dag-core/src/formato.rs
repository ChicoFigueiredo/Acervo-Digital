//! Formatação para humanos (pt-BR): tamanhos em unidades binárias e inteiros com milhar.

const UNIDADES: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

/// Tamanho em unidades binárias com uma casa decimal e vírgula: `1,2 GiB`, `512 B`.
#[must_use]
pub fn bytes_humanos(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut valor = bytes as f64;
    let mut unidade = 0;
    while unidade < UNIDADES.len() - 1 && ((valor * 10.0).round() / 10.0) >= 1024.0 {
        valor /= 1024.0;
        unidade += 1;
    }
    format!("{valor:.1}").replace('.', ",") + " " + UNIDADES[unidade]
}

/// Inteiro com ponto como separador de milhar: `1.234.567`.
#[must_use]
pub fn inteiro_pt_br(n: u64) -> String {
    let digitos = n.to_string();
    let mut saida = String::with_capacity(digitos.len() + digitos.len() / 3);
    for (i, c) in digitos.chars().enumerate() {
        if i > 0 && (digitos.len() - i).is_multiple_of(3) {
            saida.push('.');
        }
        saida.push(c);
    }
    saida
}

/// Segundos Unix como `AAAA-MM-DD HH:MM:SS` em UTC.
#[must_use]
pub fn data_hora_utc(segundos: i64) -> String {
    let dias = segundos.div_euclid(86_400);
    let resto = segundos.rem_euclid(86_400);
    // Dias desde 1970-01-01 -> data civil (algoritmo de Howard Hinnant).
    let z = dias + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dia = doy - (153 * mp + 2) / 5 + 1;
    let mes = if mp < 10 { mp + 3 } else { mp - 9 };
    let ano = yoe + era * 400 + i64::from(mes <= 2);
    format!(
        "{ano:04}-{mes:02}-{dia:02} {:02}:{:02}:{:02}",
        resto / 3600,
        resto % 3600 / 60,
        resto % 60
    )
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn tamanhos() {
        assert_eq!(bytes_humanos(0), "0 B");
        assert_eq!(bytes_humanos(1023), "1023 B");
        assert_eq!(bytes_humanos(1024), "1,0 KiB");
        assert_eq!(bytes_humanos(1536), "1,5 KiB");
        assert_eq!(bytes_humanos(1024 * 1024 - 1), "1,0 MiB");
        assert_eq!(bytes_humanos(1_288_490_189), "1,2 GiB");
        assert!(bytes_humanos(u64::MAX).ends_with(" PiB"));
    }

    #[test]
    fn data_hora() {
        assert_eq!(data_hora_utc(0), "1970-01-01 00:00:00");
        assert_eq!(data_hora_utc(951_782_400 + 3661), "2000-02-29 01:01:01");
        assert_eq!(data_hora_utc(1_795_000_000), "2026-11-18 11:06:40");
    }

    #[test]
    fn milhar() {
        assert_eq!(inteiro_pt_br(0), "0");
        assert_eq!(inteiro_pt_br(999), "999");
        assert_eq!(inteiro_pt_br(1000), "1.000");
        assert_eq!(inteiro_pt_br(1_234_567), "1.234.567");
    }
}
