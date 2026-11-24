//! Conversões de data sem dependência externa.

/// Dias desde 1970-01-01 para uma data civil (algoritmo de Howard Hinnant).
#[must_use]
pub(crate) fn dias_desde_epoca(ano: i64, mes: i64, dia: i64) -> i64 {
    let a = ano - i64::from(mes <= 2);
    let era = a.div_euclid(400);
    let yoe = a.rem_euclid(400);
    let mp = (mes + 9) % 12;
    let doy = (153 * mp + 2) / 5 + dia - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Data e hora civis (UTC, o formato não guarda fuso) em nanossegundos Unix.
#[must_use]
pub(crate) fn civil_para_ns(ano: i64, mes: i64, dia: i64, h: i64, m: i64, s: i64) -> i64 {
    (dias_desde_epoca(ano, mes, dia) * 86_400 + h * 3600 + m * 60 + s).saturating_mul(1_000_000_000)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn datas_conhecidas() {
        assert_eq!(dias_desde_epoca(1970, 1, 1), 0);
        assert_eq!(dias_desde_epoca(2000, 3, 1), 11_017);
        assert_eq!(civil_para_ns(1970, 1, 2, 0, 0, 1), 86_401_000_000_000);
    }
}
