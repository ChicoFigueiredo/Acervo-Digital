//! Base do percentual das barras (spec §9): pasta-mãe, volume ou computador. A base usada é
//! sempre rotulada na tela; quando a escolhida não existe no nível (ex.: "volume" no nível
//! Computador), a base efetiva cai para a mais ampla e o rótulo mostra a efetiva.

/// Base do percentual.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BasePercentual {
    #[default]
    PastaMae,
    Volume,
    Computador,
}

impl BasePercentual {
    pub const TODAS: [Self; 3] = [Self::PastaMae, Self::Volume, Self::Computador];

    /// Nome curto da base ("pasta-mãe").
    #[must_use]
    pub fn nome(self) -> &'static str {
        match self {
            Self::PastaMae => "pasta-mãe",
            Self::Volume => "volume",
            Self::Computador => "computador",
        }
    }

    /// Rótulo da barra ("% da pasta-mãe").
    #[must_use]
    pub fn rotulo(self) -> String {
        format!("% da {}", self.nome())
    }
}

/// Totais que servem de denominador para um nível.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bases {
    /// Total do nível mostrado (a "pasta-mãe" dos blocos).
    pub pasta_mae: u64,
    /// Total do Volume; `None` no nível Computador.
    pub volume: Option<u64>,
    /// Soma dos Volumes (cada um uma vez).
    pub computador: u64,
}

impl Bases {
    /// A base realmente usada ao pedir `pedida`.
    #[must_use]
    pub fn efetiva(&self, pedida: BasePercentual) -> BasePercentual {
        match pedida {
            BasePercentual::Volume if self.volume.is_none() => BasePercentual::Computador,
            outra => outra,
        }
    }

    /// Bytes da base efetiva.
    #[must_use]
    pub fn total(&self, pedida: BasePercentual) -> u64 {
        match self.efetiva(pedida) {
            BasePercentual::PastaMae => self.pasta_mae,
            BasePercentual::Volume => self.volume.unwrap_or(0),
            BasePercentual::Computador => self.computador,
        }
    }

    /// Percentual (0..=100) de `bytes` na base efetiva; `None` se a base é zero.
    #[must_use]
    pub fn percentual(&self, bytes: u64, pedida: BasePercentual) -> Option<f64> {
        let t = self.total(pedida);
        (t > 0).then(|| (bytes as f64 / t as f64 * 100.0).clamp(0.0, 100.0))
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    const B: Bases = Bases {
        pasta_mae: 200,
        volume: Some(1_000),
        computador: 4_000,
    };

    #[test]
    fn percentual_por_base() {
        assert_eq!(B.percentual(100, BasePercentual::PastaMae), Some(50.0));
        assert_eq!(B.percentual(100, BasePercentual::Volume), Some(10.0));
        assert_eq!(B.percentual(100, BasePercentual::Computador), Some(2.5));
    }

    #[test]
    fn base_volume_ausente_cai_para_computador_e_diz_qual_usou() {
        let b = Bases { volume: None, ..B };
        assert_eq!(
            b.efetiva(BasePercentual::Volume),
            BasePercentual::Computador
        );
        assert_eq!(b.percentual(400, BasePercentual::Volume), Some(10.0));
        assert_eq!(
            b.efetiva(BasePercentual::PastaMae),
            BasePercentual::PastaMae
        );
    }

    #[test]
    fn base_zero_nao_inventa_percentual_e_resultado_e_limitado() {
        let b = Bases {
            pasta_mae: 0,
            volume: Some(0),
            computador: 0,
        };
        assert_eq!(b.percentual(10, BasePercentual::PastaMae), None);
        assert_eq!(B.percentual(500, BasePercentual::PastaMae), Some(100.0));
    }

    #[test]
    fn rotulos_dizem_a_base() {
        assert_eq!(BasePercentual::PastaMae.rotulo(), "% da pasta-mãe");
        assert_eq!(BasePercentual::Computador.nome(), "computador");
    }
}
