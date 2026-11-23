//! Formatação para humanos (pt-BR). Tamanhos e inteiros vêm de `dag_core::formato`.

use std::time::Duration;

pub use dag_core::formato::{bytes_humanos, data_hora_utc, inteiro_pt_br};

/// `12,3%`; abaixo de 0,1% vira `<0,1%`; `None` vira `—`.
#[must_use]
pub fn percentual(p: Option<f64>) -> String {
    match p {
        None => "—".into(),
        Some(p) if p > 0.0 && p < 0.1 => "<0,1%".into(),
        Some(p) => format!("{p:.1}%").replace('.', ","),
    }
}

/// `5s`, `3min 04s`, `1h 02min 03s`.
#[must_use]
pub fn duracao(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}min {:02}s", s / 60, s % 60)
    } else {
        format!("{}h {:02}min {:02}s", s / 3600, s % 3600 / 60, s % 60)
    }
}

/// `12,3 MiB/s` (taxa em bytes por segundo).
#[must_use]
pub fn taxa_bytes(bytes_por_s: f64) -> String {
    format!("{}/s", bytes_humanos(bytes_por_s.max(0.0) as u64))
}

/// `1.234 itens/s`.
#[must_use]
pub fn taxa_itens(por_s: f64, unidade: &str) -> String {
    format!("{} {unidade}/s", inteiro_pt_br(por_s.max(0.0) as u64))
}

/// Nanossegundos Unix como data e hora UTC; `—` se ausente.
#[must_use]
pub fn data_ns(ns: Option<i64>) -> String {
    ns.map_or_else(
        || "—".into(),
        |n| data_hora_utc(n.div_euclid(1_000_000_000)),
    )
}

/// Segundos Unix como data e hora UTC; `—` se ausente.
#[must_use]
pub fn data_s(s: Option<i64>) -> String {
    s.map_or_else(|| "—".into(), data_hora_utc)
}

/// Idade em dias inteiros de `mtime_ns` em relação a `agora_s`; `None` sem data. Datas no
/// futuro contam como 0.
#[must_use]
pub fn idade_em_dias(mtime_ns: Option<i64>, agora_s: i64) -> Option<u64> {
    let m = mtime_ns?.div_euclid(1_000_000_000);
    Some(u64::try_from((agora_s - m).max(0)).unwrap_or(0) / 86_400)
}

/// Os 8 primeiros dígitos hexadecimais de um hash (para tabelas).
#[must_use]
pub fn hash_curto(hex: &str) -> String {
    hex.chars().take(8).collect()
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn percentual_formata_com_virgula() {
        assert_eq!(percentual(Some(12.34)), "12,3%");
        assert_eq!(percentual(Some(0.04)), "<0,1%");
        assert_eq!(percentual(Some(0.0)), "0,0%");
        assert_eq!(percentual(None), "—");
    }

    #[test]
    fn duracao_e_taxa() {
        assert_eq!(duracao(Duration::from_secs(5)), "5s");
        assert_eq!(duracao(Duration::from_secs(184)), "3min 04s");
        assert_eq!(duracao(Duration::from_secs(3723)), "1h 02min 03s");
        assert_eq!(taxa_bytes(1024.0 * 1024.0), "1,0 MiB/s");
        assert_eq!(taxa_itens(12_345.6, "itens"), "12.345 itens/s");
    }

    #[test]
    fn tamanhos_humanos_vem_do_core() {
        assert_eq!(bytes_humanos(512), "512 B");
        assert_eq!(bytes_humanos(1536), "1,5 KiB");
        assert_eq!(bytes_humanos(3 * 1024 * 1024 * 1024), "3,0 GiB");
        assert_eq!(inteiro_pt_br(1_234_567), "1.234.567");
    }

    #[test]
    fn datas_e_idade() {
        assert_eq!(data_ns(None), "—");
        assert_eq!(data_ns(Some(86_400 * 1_000_000_000)), "1970-01-02 00:00:00");
        assert_eq!(idade_em_dias(Some(0), 86_400 * 10), Some(10));
        assert_eq!(idade_em_dias(Some(5_000_000_000_000_000_000), 0), Some(0));
        assert_eq!(idade_em_dias(None, 0), None);
        assert_eq!(hash_curto("abcdef0123456789"), "abcdef01");
    }
}
