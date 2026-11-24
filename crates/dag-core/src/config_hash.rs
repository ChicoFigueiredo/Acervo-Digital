//! Paralelismo do Hash Completo configurável: leitores por disco e discos em paralelo.
//!
//! Precedência (do mais forte ao mais fraco): pedido do cliente > flag do agente > valor
//! persistido no Catálogo > padrão. Valores fora da faixa são rejeitados, nunca ajustados.

use serde::{Deserialize, Serialize};

/// Padrão de leitores simultâneos por disco (HDD: 1 é o melhor).
pub const THREADS_POR_DISCO_PADRAO: u32 = 1;
/// Máximo aceito de leitores por disco.
pub const THREADS_POR_DISCO_MAX: u32 = 8;
/// Padrão de discos lendo ao mesmo tempo.
pub const MAX_DISCOS_PADRAO: u32 = 16;
/// Máximo aceito de discos em paralelo.
pub const MAX_DISCOS_MAX: u32 = 16;

/// Valores efetivos, já validados.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamsHash {
    pub threads_por_disco: u32,
    pub max_discos: u32,
}

impl Default for ParamsHash {
    fn default() -> Self {
        Self {
            threads_por_disco: THREADS_POR_DISCO_PADRAO,
            max_discos: MAX_DISCOS_PADRAO,
        }
    }
}

/// Valores opcionais de uma fonte (pedido, flag do agente ou Catálogo).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpcoesHash {
    #[serde(default)]
    pub threads_por_disco: Option<u32>,
    #[serde(default)]
    pub max_discos: Option<u32>,
}

impl OpcoesHash {
    /// Nenhum campo definido.
    #[must_use]
    pub fn vazia(&self) -> bool {
        self.threads_por_disco.is_none() && self.max_discos.is_none()
    }

    /// Rejeita valor fora da faixa (1..=8 threads, 1..=16 discos).
    ///
    /// # Errors
    /// Mensagem em português dizendo o campo, a faixa e o valor recebido.
    pub fn validar(&self) -> Result<(), String> {
        if let Some(t) = self.threads_por_disco
            && !(1..=THREADS_POR_DISCO_MAX).contains(&t)
        {
            return Err(format!(
                "threads por disco deve estar entre 1 e {THREADS_POR_DISCO_MAX} (recebido: {t})"
            ));
        }
        if let Some(d) = self.max_discos
            && !(1..=MAX_DISCOS_MAX).contains(&d)
        {
            return Err(format!(
                "discos em paralelo deve estar entre 1 e {MAX_DISCOS_MAX} (recebido: {d})"
            ));
        }
        Ok(())
    }

    /// Campos de `self` valem; os que faltam vêm de `fraca`.
    #[must_use]
    pub fn ou(self, fraca: Self) -> Self {
        Self {
            threads_por_disco: self.threads_por_disco.or(fraca.threads_por_disco),
            max_discos: self.max_discos.or(fraca.max_discos),
        }
    }

    /// Completa com os padrões e valida.
    ///
    /// # Errors
    /// Como [`Self::validar`].
    pub fn resolver(self) -> Result<ParamsHash, String> {
        self.validar()?;
        let p = ParamsHash::default();
        Ok(ParamsHash {
            threads_por_disco: self.threads_por_disco.unwrap_or(p.threads_por_disco),
            max_discos: self.max_discos.unwrap_or(p.max_discos),
        })
    }
}

/// Aplica a precedência pedido > agente > persistido > padrão.
///
/// # Errors
/// Valor fora da faixa em qualquer fonte.
pub fn resolver_precedencia(
    pedido: OpcoesHash,
    agente: OpcoesHash,
    persistido: OpcoesHash,
) -> Result<ParamsHash, String> {
    pedido.validar()?;
    agente.validar()?;
    persistido.ou(OpcoesHash::default()).validar()?;
    pedido.ou(agente).ou(persistido).resolver()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(t: Option<u32>, d: Option<u32>) -> OpcoesHash {
        OpcoesHash {
            threads_por_disco: t,
            max_discos: d,
        }
    }

    #[test]
    fn padrao_e_um_por_disco() {
        let p = resolver_precedencia(o(None, None), o(None, None), o(None, None)).unwrap();
        assert_eq!(
            p,
            ParamsHash {
                threads_por_disco: 1,
                max_discos: 16
            }
        );
    }

    #[test]
    fn precedencia_pedido_agente_persistido_padrao() {
        let p = resolver_precedencia(o(Some(4), None), o(Some(2), Some(8)), o(Some(3), Some(5)))
            .unwrap();
        assert_eq!(
            p,
            ParamsHash {
                threads_por_disco: 4,
                max_discos: 8
            }
        );
        let p = resolver_precedencia(o(None, None), o(None, None), o(Some(3), Some(5))).unwrap();
        assert_eq!(
            p,
            ParamsHash {
                threads_por_disco: 3,
                max_discos: 5
            }
        );
        let p = resolver_precedencia(o(None, Some(2)), o(None, None), o(Some(3), None)).unwrap();
        assert_eq!(
            p,
            ParamsHash {
                threads_por_disco: 3,
                max_discos: 2
            }
        );
    }

    #[test]
    fn valida_faixas() {
        for (t, d) in [(0, 1), (9, 1), (1, 0), (1, 17)] {
            assert!(o(Some(t), Some(d)).validar().is_err(), "{t} {d}");
        }
        assert!(o(Some(8), Some(16)).validar().is_ok());
        assert!(o(Some(1), Some(1)).validar().is_ok());
        let e = o(Some(9), None).validar().unwrap_err();
        assert!(e.contains("1 e 8") && e.contains('9'), "{e}");
        assert!(resolver_precedencia(o(None, None), o(Some(99), None), o(None, None)).is_err());
    }
}
