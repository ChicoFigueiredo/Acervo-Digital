//! Hash Completo: impressão digital de 100% do conteúdo (32 bytes).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Hash Completo de um arquivo. Texto: 64 dígitos hexadecimais minúsculos.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HashCompleto(pub [u8; 32]);

/// Texto que não é um Hash Completo válido.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("hash inválido: esperados 64 dígitos hexadecimais")]
pub struct HashInvalido;

impl fmt::Display for HashCompleto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for HashCompleto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HashCompleto({self})")
    }
}

impl FromStr for HashCompleto {
    type Err = HashInvalido;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        if bytes.len() != 64 {
            return Err(HashInvalido);
        }
        let mut out = [0u8; 32];
        for (i, par) in bytes.as_chunks::<2>().0.iter().enumerate() {
            let hi = (par[0] as char).to_digit(16).ok_or(HashInvalido)?;
            let lo = (par[1] as char).to_digit(16).ok_or(HashInvalido)?;
            out[i] = u8::try_from(hi * 16 + lo).map_err(|_| HashInvalido)?;
        }
        Ok(Self(out))
    }
}

impl Serialize for HashCompleto {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for HashCompleto {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let texto = String::deserialize(d)?;
        texto.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_e_volta_hex() {
        let mut b = [0u8; 32];
        b[0] = 0xab;
        b[31] = 0x01;
        let h = HashCompleto(b);
        let s = h.to_string();
        assert_eq!(s.len(), 64);
        assert!(s.starts_with("ab") && s.ends_with("01"));
        assert_eq!(s.parse::<HashCompleto>(), Ok(h));
        assert_eq!(s.to_uppercase().parse::<HashCompleto>(), Ok(h));
    }

    #[test]
    fn rejeita_invalidos() {
        assert_eq!("abc".parse::<HashCompleto>(), Err(HashInvalido));
        assert_eq!("zz".repeat(32).parse::<HashCompleto>(), Err(HashInvalido));
        assert_eq!("é".repeat(32).parse::<HashCompleto>(), Err(HashInvalido));
    }

    #[test]
    fn serde_como_string() {
        let h = HashCompleto([7; 32]);
        let j = serde_json::to_string(&h).unwrap();
        assert_eq!(j, format!("\"{}\"", "07".repeat(32)));
        assert_eq!(serde_json::from_str::<HashCompleto>(&j).unwrap(), h);
    }
}
