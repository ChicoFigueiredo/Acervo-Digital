//! Impressão digital textual (B-14): MinHash de shingles de palavras normalizadas.
//!
//! Serve para levantar a *hipótese* de mesma Edição/Obra entre arquivos diferentes (por exemplo,
//! duas digitalizações do mesmo livro, cujo Hash Completo difere). É puro, sem I/O.
//!
//! Regra do plano: fingerprint semântico nunca é autorização de descarte. Por isso a hipótese
//! devolvida por [`hipotese`] nunca é [`TipoDuplicidade::Exata`] e nunca gera espaço recuperável.
//!
//! Robustez a OCR ruim: acentos e pontuação saem, hifenização de fim de linha é desfeita,
//! palavras curtas e tokens com dígitos (números de página, notas) são ignorados e confusões
//! típicas de OCR (`l`/`1`/`|` por `i`, `rn` por `m`, `vv` por `w`) são dobradas. Os shingles
//! formam um conjunto, então cabeçalhos repetidos a cada página contam uma vez.

use crate::identidade::Risco;
use crate::normalizacao::normalizar_texto;
use crate::tipos::TipoDuplicidade;

/// Tamanho da assinatura MinHash (k fixo).
pub const K_ASSINATURA: usize = 128;
/// Palavras por shingle.
pub const PALAVRAS_POR_SHINGLE: usize = 3;
/// Bandas do LSH: a assinatura é cortada em `BANDAS` faixas de `LINHAS_POR_BANDA` valores.
pub const BANDAS: usize = 64;
/// Valores por banda (`BANDAS * LINHAS_POR_BANDA == K_ASSINATURA`).
pub const LINHAS_POR_BANDA: usize = 2;
/// Menos shingles que isso: texto curto demais para uma impressão confiável.
pub const MIN_SHINGLES: usize = 50;
/// Teto de palavras consideradas (custo limitado em livros enormes).
pub const MAX_PALAVRAS: usize = 400_000;
/// Jaccard estimado a partir do qual a hipótese é Mesma Edição.
pub const LIMIAR_MESMA_EDICAO: f64 = 0.5;
/// Jaccard estimado a partir do qual vale levantar uma hipótese Incerta.
pub const LIMIAR_INCERTA: f64 = 0.25;

/// Assinatura MinHash de um texto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Impressao {
    pub assinatura: Vec<u32>,
    /// Quantos shingles distintos entraram.
    pub shingles: u32,
}

fn mistura(mut x: u64) -> u64 {
    // splitmix64
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn fnv1a(texto: &str) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for b in texto.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

fn dobrar_ocr(p: &str) -> String {
    p.replace("rn", "m")
        .replace("vv", "w")
        .chars()
        .map(|c| if c == 'l' || c == '|' { 'i' } else { c })
        .collect()
}

/// Palavras normalizadas de um trecho: sem acento/pontuação, sem hifenização de fim de linha,
/// sem tokens curtos (menos de 3 letras) nem com dígitos.
#[must_use]
pub fn palavras_normalizadas(texto: &str) -> Vec<String> {
    let sem_hifen = texto.replace("-\r\n", "").replace("-\n", "");
    normalizar_texto(&sem_hifen)
        .split(' ')
        .filter(|p| p.chars().count() >= 3 && !p.chars().any(|c| c.is_ascii_digit()))
        .map(dobrar_ocr)
        .collect()
}

/// Calcula a impressão do texto das páginas. `None` se o texto é curto demais.
#[must_use]
pub fn calcular<'a>(paginas: impl IntoIterator<Item = &'a str>) -> Option<Impressao> {
    let mut palavras: Vec<String> = Vec::new();
    for p in paginas {
        palavras.extend(palavras_normalizadas(p));
        if palavras.len() >= MAX_PALAVRAS {
            palavras.truncate(MAX_PALAVRAS);
            break;
        }
    }
    if palavras.len() < PALAVRAS_POR_SHINGLE {
        return None;
    }
    let mut minimos = vec![u32::MAX; K_ASSINATURA];
    let mut vistos = std::collections::HashSet::new();
    for janela in palavras.windows(PALAVRAS_POR_SHINGLE) {
        let base = fnv1a(&janela.join(" "));
        if !vistos.insert(base) {
            continue;
        }
        for (i, m) in minimos.iter_mut().enumerate() {
            let h = (mistura(base ^ mistura(i as u64 + 1)) >> 32) as u32;
            if h < *m {
                *m = h;
            }
        }
    }
    if vistos.len() < MIN_SHINGLES {
        return None;
    }
    Some(Impressao {
        assinatura: minimos,
        shingles: u32::try_from(vistos.len()).unwrap_or(u32::MAX),
    })
}

/// Jaccard estimado: fração das posições da assinatura que coincidem.
#[must_use]
pub fn jaccard_estimado(a: &Impressao, b: &Impressao) -> f64 {
    let n = a.assinatura.len().min(b.assinatura.len());
    if n == 0 {
        return 0.0;
    }
    let iguais = a
        .assinatura
        .iter()
        .zip(&b.assinatura)
        .filter(|(x, y)| x == y)
        .count();
    iguais as f64 / n as f64
}

impl Impressao {
    /// Chaves LSH (uma por banda), já misturadas com o número da banda: duas impressões
    /// parecidas compartilham várias. Servem de índice para pré-filtrar candidatos.
    #[must_use]
    pub fn chaves_lsh(&self) -> Vec<i64> {
        self.assinatura
            .chunks(LINHAS_POR_BANDA)
            .enumerate()
            .map(|(banda, c)| {
                let mut h = mistura(banda as u64);
                for v in c {
                    h = mistura(h ^ u64::from(*v));
                }
                h as i64
            })
            .collect()
    }

    /// Serialização estável: shingles (4 bytes) + assinatura (4 bytes por valor), little-endian.
    #[must_use]
    pub fn para_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(4 + 4 * self.assinatura.len());
        v.extend_from_slice(&self.shingles.to_le_bytes());
        for x in &self.assinatura {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v
    }

    /// Inverso de [`Impressao::para_bytes`]; `None` se o tamanho não bate.
    #[must_use]
    pub fn de_bytes(b: &[u8]) -> Option<Self> {
        if b.len() != 4 + 4 * K_ASSINATURA {
            return None;
        }
        let shingles = u32::from_le_bytes(b[..4].try_into().ok()?);
        let assinatura = b[4..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        Some(Self {
            assinatura,
            shingles,
        })
    }
}

/// Hipótese bibliográfica a partir do conteúdo. Nunca é `Exata`; é só evidência.
#[derive(Debug, Clone, PartialEq)]
pub struct Hipotese {
    pub tipo: TipoDuplicidade,
    pub risco: Risco,
    pub jaccard: f64,
    pub evidencia: String,
}

/// Transforma o Jaccard estimado em hipótese: `MesmaEdicao` (risco médio) ou `Incerta`
/// (risco alto) ou nada. Nunca autoriza descarte nem produz espaço recuperável.
#[must_use]
pub fn hipotese(jaccard: f64) -> Option<Hipotese> {
    let (tipo, risco) = if jaccard >= LIMIAR_MESMA_EDICAO {
        (TipoDuplicidade::MesmaEdicao, Risco::Medio)
    } else if jaccard >= LIMIAR_INCERTA {
        (TipoDuplicidade::Incerta, Risco::Alto)
    } else {
        return None;
    };
    Some(Hipotese {
        tipo,
        risco,
        jaccard,
        evidencia: format!(
            "semelhança de conteúdo (Jaccard estimado {:.0}%); hipótese, não autoriza descarte",
            jaccard * 100.0
        ),
    })
}

#[cfg(test)]
mod testes {
    use super::*;

    /// Texto pseudo-aleatório determinístico com `n` palavras distintas o bastante.
    fn texto(semente: u64, n: usize) -> String {
        let silabas = [
            "ca", "mo", "ri", "tu", "be", "lo", "sa", "ne", "du", "fi", "ga", "po", "re", "vu",
            "xo", "ze",
        ];
        let mut s = semente;
        let mut v = Vec::new();
        for _ in 0..n {
            let mut p = String::new();
            for _ in 0..3 {
                s = mistura(s);
                p.push_str(silabas[(s % 16) as usize]);
            }
            v.push(p);
        }
        v.join(" ")
    }

    #[test]
    fn texto_igual_da_jaccard_um_e_texto_diferente_quase_zero() {
        let a = calcular([texto(1, 2000).as_str()]).unwrap();
        let b = calcular([texto(1, 2000).as_str()]).unwrap();
        let c = calcular([texto(2, 2000).as_str()]).unwrap();
        assert!((jaccard_estimado(&a, &b) - 1.0).abs() < f64::EPSILON);
        assert!(jaccard_estimado(&a, &c) < 0.05);
        assert!(hipotese(jaccard_estimado(&a, &c)).is_none());
    }

    #[test]
    fn robusto_a_ocr_ruim_numero_de_pagina_e_hifenizacao() {
        let limpo = texto(7, 3000);
        // Corrompe ~4% das palavras (troca uma letra), insere número de página e cabeçalho,
        // hifeniza palavras no fim de linha e muda a caixa/pontuação.
        let sujo: Vec<String> = limpo
            .split(' ')
            .enumerate()
            .map(|(i, p)| {
                let mut p = p.to_owned();
                if i % 25 == 0 {
                    p.replace_range(0..1, "x");
                }
                if i % 40 == 0 {
                    p = format!("{}-\n{}", &p[..3], &p[3..]);
                }
                if i % 150 == 0 {
                    p = format!("{p}\n{}\nLIVRO TAL", i / 150 + 1);
                }
                p.to_uppercase() + ","
            })
            .collect();
        let a = calcular([limpo.as_str()]).unwrap();
        let b = calcular([sujo.join(" ").as_str()]).unwrap();
        let j = jaccard_estimado(&a, &b);
        assert!(j > 0.6, "jaccard {j}");
        let h = hipotese(j).unwrap();
        assert_eq!(h.tipo, TipoDuplicidade::MesmaEdicao);
    }

    #[test]
    fn lsh_junta_parecidos_e_separa_diferentes() {
        let a = calcular([texto(3, 2000).as_str()]).unwrap();
        let mut t = texto(3, 2000);
        t.push_str(&format!(" {}", texto(9, 300)));
        let b = calcular([t.as_str()]).unwrap();
        let c = calcular([texto(4, 2000).as_str()]).unwrap();
        let comuns = |x: &Impressao, y: &Impressao| {
            let k = y.chaves_lsh();
            x.chaves_lsh().iter().filter(|v| k.contains(v)).count()
        };
        assert!(comuns(&a, &b) > 20);
        assert!(comuns(&a, &c) <= 2);
    }

    #[test]
    fn texto_curto_nao_gera_impressao_e_bytes_ida_e_volta() {
        assert!(calcular(["pouco texto aqui"]).is_none());
        let a = calcular([texto(5, 500).as_str()]).unwrap();
        assert_eq!(Impressao::de_bytes(&a.para_bytes()).unwrap(), a);
        assert!(Impressao::de_bytes(&[1, 2, 3]).is_none());
        assert_eq!(a.chaves_lsh().len(), BANDAS);
    }

    #[test]
    fn hipotese_nunca_e_exata() {
        for j in [0.0, 0.2, 0.3, 0.5, 0.9, 1.0] {
            if let Some(h) = hipotese(j) {
                assert_ne!(h.tipo, TipoDuplicidade::Exata);
            }
        }
    }
}
