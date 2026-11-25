//! Pontuação da fila bibliográfica (B-06): economia potencial (tamanho e suspeita de
//! duplicidade), novidade e envelhecimento (aging). O aging cresce sem limite, então todo
//! item acaba passando qualquer chegada nova: arquivos pequenos não morrem de fome.
//! Limites por dispositivo, pausas e concorrência são do chamador.

use serde::{Deserialize, Serialize};

/// Item da fila.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemFila {
    pub id: i64,
    pub tamanho: u64,
    /// 0..=1: chance de ser duplicata (hash conhecido de outro arquivo, mesmo nome...).
    pub suspeita_duplicidade: f64,
    /// Instante de entrada na fila, em segundos.
    pub entrou_em: u64,
}

/// Política configurável.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PoliticaPrioridade {
    pub peso_tamanho: f64,
    pub peso_duplicidade: f64,
    pub peso_novidade: f64,
    /// Tamanho em que a pontuação de tamanho chega a 50 %.
    pub tamanho_referencia: u64,
    /// Meia-vida da novidade, em segundos.
    pub meia_vida_novidade_s: u64,
    /// Pontos ganhos por hora de espera. Maior que zero garante ausência de starvation.
    pub aging_por_hora: f64,
}

impl Default for PoliticaPrioridade {
    fn default() -> Self {
        Self {
            peso_tamanho: 1.0,
            peso_duplicidade: 0.8,
            peso_novidade: 0.3,
            tamanho_referencia: 200 * 1024 * 1024,
            meia_vida_novidade_s: 6 * 3600,
            aging_por_hora: 0.05,
        }
    }
}

impl PoliticaPrioridade {
    /// Maior pontuação base possível (sem aging).
    #[must_use]
    pub fn teto_base(&self) -> f64 {
        self.peso_tamanho + self.peso_duplicidade + self.peso_novidade
    }

    /// Horas de espera que bastam para um item de base zero superar qualquer chegada nova.
    #[must_use]
    pub fn horas_para_nao_passar_fome(&self) -> f64 {
        if self.aging_por_hora <= 0.0 {
            f64::INFINITY
        } else {
            self.teto_base() / self.aging_por_hora
        }
    }
}

/// Pontuação do item no instante `agora` (segundos).
#[must_use]
pub fn pontuacao(item: &ItemFila, agora: u64, p: &PoliticaPrioridade) -> f64 {
    let t = item.tamanho as f64;
    let ref_ = p.tamanho_referencia.max(1) as f64;
    let tam = t / (t + ref_);
    let espera_s = agora.saturating_sub(item.entrou_em) as f64;
    let novidade = if p.meia_vida_novidade_s == 0 {
        0.0
    } else {
        0.5_f64.powf(espera_s / p.meia_vida_novidade_s as f64)
    };
    let aging = p.aging_por_hora * espera_s / 3600.0;
    p.peso_tamanho * tam
        + p.peso_duplicidade * item.suspeita_duplicidade.clamp(0.0, 1.0)
        + p.peso_novidade * novidade
        + aging
}

/// Índices dos itens em ordem de processamento (maior pontuação primeiro; desempate por
/// entrada mais antiga e depois por id).
#[must_use]
pub fn ordenar(itens: &[ItemFila], agora: u64, p: &PoliticaPrioridade) -> Vec<usize> {
    let mut v: Vec<(usize, f64)> = itens
        .iter()
        .enumerate()
        .map(|(i, it)| (i, pontuacao(it, agora, p)))
        .collect();
    v.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then(itens[a.0].entrou_em.cmp(&itens[b.0].entrou_em))
            .then(itens[a.0].id.cmp(&itens[b.0].id))
    });
    v.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod testes {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn item(id: i64, mib: u64, susp: f64, t: u64) -> ItemFila {
        ItemFila {
            id,
            tamanho: mib * MIB,
            suspeita_duplicidade: susp,
            entrou_em: t,
        }
    }

    #[test]
    fn grande_e_suspeito_passam_na_frente_no_inicio() {
        let p = PoliticaPrioridade::default();
        let itens = [item(1, 1, 0.0, 0), item(2, 900, 0.0, 0), item(3, 1, 0.9, 0)];
        let o = ordenar(&itens, 0, &p);
        assert_eq!(itens[o[0]].id, 2);
        assert_eq!(itens[o[2]].id, 1);
    }

    #[test]
    fn monotonia_do_tamanho_e_da_suspeita() {
        let p = PoliticaPrioridade::default();
        assert!(pontuacao(&item(1, 500, 0.0, 0), 0, &p) > pontuacao(&item(1, 5, 0.0, 0), 0, &p));
        assert!(pontuacao(&item(1, 5, 0.9, 0), 0, &p) > pontuacao(&item(1, 5, 0.1, 0), 0, &p));
    }

    #[test]
    fn aging_cresce_com_a_espera() {
        let p = PoliticaPrioridade {
            peso_novidade: 0.0,
            ..Default::default()
        };
        let i = item(1, 1, 0.0, 0);
        assert!(pontuacao(&i, 10 * 3600, &p) > pontuacao(&i, 3600, &p));
    }

    /// Simulação: chegam grandes sem parar, 3 por passo (10 min) contra 1 atendido por passo.
    /// O pequeno que chegou primeiro é atendido: depois de `horas_para_nao_passar_fome` nenhuma
    /// chegada nova o ultrapassa, e só restam as que já estavam na frente (no máximo as
    /// chegadas desse intervalo, 3 por passo, a 1 por passo de atendimento).
    #[test]
    fn simulacao_pequeno_nao_morre_de_fome() {
        let p = PoliticaPrioridade::default();
        let limite_h = p.horas_para_nao_passar_fome();
        assert!(limite_h.is_finite());
        let passo_s = 600u64;
        let mut fila = vec![item(0, 1, 0.0, 0)];
        let mut proximo_id = 1;
        let mut atendido_em: Option<u64> = None;
        let max_passos = (limite_h * 3600.0 / passo_s as f64 * 3.0) as u64 + 100;
        for passo in 0..max_passos {
            let agora = passo * passo_s;
            for _ in 0..3 {
                fila.push(item(proximo_id, 800, 0.5, agora));
                proximo_id += 1;
            }
            let o = ordenar(&fila, agora, &p);
            let escolhido = fila.remove(o[0]);
            if escolhido.id == 0 {
                atendido_em = Some(agora);
                break;
            }
        }
        let t = atendido_em.expect("o item pequeno deveria ser atendido");
        assert!(
            (t as f64) <= limite_h * 3.0 * 3600.0 + 7200.0,
            "atendido em {} h, limite {limite_h:.1} h",
            t as f64 / 3600.0
        );
    }

    #[test]
    fn sem_aging_a_fome_e_possivel() {
        let p = PoliticaPrioridade {
            aging_por_hora: 0.0,
            ..Default::default()
        };
        assert!(p.horas_para_nao_passar_fome().is_infinite());
        let pequeno = item(0, 1, 0.0, 0);
        let grande = item(1, 800, 0.5, 100 * 3600);
        let agora = 100 * 3600;
        assert!(pontuacao(&grande, agora, &p) > pontuacao(&pequeno, agora, &p));
    }

    #[test]
    fn ordem_e_deterministica_em_empate() {
        let p = PoliticaPrioridade::default();
        let itens = [item(5, 10, 0.2, 0), item(4, 10, 0.2, 0)];
        assert_eq!(ordenar(&itens, 0, &p), vec![1, 0]);
    }
}
