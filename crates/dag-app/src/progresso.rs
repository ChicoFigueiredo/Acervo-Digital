//! Progresso realista da varredura (spec §9): mostra o que é contagem real (diretórios,
//! arquivos, bytes, taxa, fila) e só calcula fração quando o denominador é conhecido.
//! Durante o inventário e a agregação **não existe** percentual global (não se sabe quantos
//! diretórios faltam); a etapa de hash tem total conhecido (fila de hash).

use std::time::Duration;

use dag_core::{FaseInventario, ProgressoInventario};

/// Fase do inventário, como o scanner a informa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaseScan {
    Inventario,
    Pausado,
    Agregacao,
    /// Fim do inventário: seguem classificação, hash e duplicados (a cargo do agente).
    Concluido,
}

/// Instantâneo de progresso, copiado do evento de progresso do agente.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressoScan {
    pub execucao: i64,
    pub fase: FaseScan,
    pub pastas: u64,
    pub arquivos: u64,
    pub especiais: u64,
    pub bytes_logicos: u64,
    pub ignorados: u64,
    pub erros: u64,
    /// Diretórios descobertos e ainda não visitados (aproximado): é a fila.
    pub pendentes: u64,
    pub decorrido: Duration,
    pub itens_por_s: f64,
}

impl From<&ProgressoInventario> for ProgressoScan {
    fn from(p: &ProgressoInventario) -> Self {
        Self {
            execucao: p.execucao.0,
            fase: match p.fase {
                FaseInventario::Inventario => FaseScan::Inventario,
                FaseInventario::Pausado => FaseScan::Pausado,
                FaseInventario::Agregacao => FaseScan::Agregacao,
                FaseInventario::Concluido => FaseScan::Concluido,
            },
            pastas: p.pastas,
            arquivos: p.arquivos,
            especiais: p.especiais,
            bytes_logicos: p.bytes_logicos,
            ignorados: p.ignorados,
            erros: p.erros,
            pendentes: p.diretorios_pendentes,
            decorrido: Duration::from_millis(p.decorrido_ms),
            itens_por_s: p.entradas_por_segundo,
        }
    }
}

impl ProgressoScan {
    /// Itens inventariados (pastas + arquivos + especiais).
    #[must_use]
    pub fn itens(&self) -> u64 {
        self.pastas + self.arquivos + self.especiais
    }
}

/// Nome da fase mostrada ao usuário. `pausado` vem do controle (vale mesmo antes de o scanner
/// emitir o evento de pausa); `sem_hash` decide o que vem depois do inventário.
#[must_use]
pub fn fase_texto(fase: Option<FaseScan>, pausado: bool, sem_hash: bool) -> &'static str {
    if pausado || fase == Some(FaseScan::Pausado) {
        return "Pausada";
    }
    match fase {
        None => "Preparando (descobrindo volumes)",
        Some(FaseScan::Inventario | FaseScan::Pausado) => "Inventário (pastas e arquivos)",
        Some(FaseScan::Agregacao) => "Agregando totais por pasta",
        Some(FaseScan::Concluido) if sem_hash => "Finalizando (classificação por caminho)",
        Some(FaseScan::Concluido) => "Hash Completo e Duplicados",
    }
}

/// Fração global 0..=1 só quando o denominador é conhecido: na etapa de hash
/// (`concluidos/total` da fila). Inventário e agregação devolvem `None`.
#[must_use]
pub fn fracao_global(
    fase: Option<FaseScan>,
    sem_hash: bool,
    hash: Option<&AndamentoHash>,
) -> Option<f32> {
    if fase != Some(FaseScan::Concluido) || sem_hash {
        return None;
    }
    let h = hash?;
    (h.total > 0).then(|| (h.concluidos as f32 / h.total as f32).clamp(0.0, 1.0))
}

/// Contagem da fila de hash num instante.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AndamentoHash {
    pub total: u64,
    pub concluidos: u64,
}

/// Arquivos por segundo entre duas amostras; `None` sem avanço ou sem tempo decorrido.
#[must_use]
pub fn taxa_do_hash(
    antes: (AndamentoHash, Duration),
    depois: (AndamentoHash, Duration),
) -> Option<f64> {
    let dt = depois.1.checked_sub(antes.1)?.as_secs_f64();
    let feitos = depois.0.concluidos.checked_sub(antes.0.concluidos)?;
    (dt > 0.0 && feitos > 0).then(|| feitos as f64 / dt)
}

/// Tempo restante **estimado** (a tela deve rotulá-lo como estimativa); `None` sem taxa.
#[must_use]
pub fn restante_estimado(h: &AndamentoHash, taxa: Option<f64>) -> Option<Duration> {
    let t = taxa.filter(|t| *t > 0.0)?;
    let falta = h.total.saturating_sub(h.concluidos);
    (falta > 0).then(|| Duration::from_secs_f64(falta as f64 / t))
}

#[cfg(test)]
mod testes {
    use super::*;

    fn h(total: u64, concluidos: u64) -> AndamentoHash {
        AndamentoHash { total, concluidos }
    }

    #[test]
    fn inventario_e_agregacao_nao_tem_percentual_global() {
        for f in [
            None,
            Some(FaseScan::Inventario),
            Some(FaseScan::Agregacao),
            Some(FaseScan::Pausado),
        ] {
            assert_eq!(fracao_global(f, false, Some(&h(10, 5))), None);
        }
        assert_eq!(
            fracao_global(Some(FaseScan::Concluido), true, Some(&h(10, 5))),
            None
        );
    }

    #[test]
    fn hash_tem_fracao_quando_o_total_e_conhecido() {
        let f = fracao_global(Some(FaseScan::Concluido), false, Some(&h(200, 50)));
        assert_eq!(f, Some(0.25));
        assert_eq!(
            fracao_global(Some(FaseScan::Concluido), false, Some(&h(0, 0))),
            None
        );
        assert_eq!(fracao_global(Some(FaseScan::Concluido), false, None), None);
        assert_eq!(
            fracao_global(Some(FaseScan::Concluido), false, Some(&h(10, 99))),
            Some(1.0)
        );
    }

    #[test]
    fn textos_de_fase() {
        assert_eq!(
            fase_texto(None, false, false),
            "Preparando (descobrindo volumes)"
        );
        assert_eq!(
            fase_texto(Some(FaseScan::Inventario), false, false),
            "Inventário (pastas e arquivos)"
        );
        assert_eq!(
            fase_texto(Some(FaseScan::Inventario), true, false),
            "Pausada"
        );
        assert_eq!(fase_texto(Some(FaseScan::Pausado), false, false), "Pausada");
        assert_eq!(
            fase_texto(Some(FaseScan::Concluido), false, false),
            "Hash Completo e Duplicados"
        );
        assert_eq!(
            fase_texto(Some(FaseScan::Concluido), false, true),
            "Finalizando (classificação por caminho)"
        );
    }

    #[test]
    fn taxa_e_estimativa_so_com_dados() {
        let a = (h(1000, 100), Duration::from_secs(10));
        let b = (h(1000, 300), Duration::from_secs(20));
        let taxa = taxa_do_hash(a, b);
        assert_eq!(taxa, Some(20.0));
        assert_eq!(restante_estimado(&b.0, taxa), Some(Duration::from_secs(35)));
        assert_eq!(taxa_do_hash(b, b), None, "sem tempo decorrido");
        assert_eq!(
            taxa_do_hash(a, (h(1000, 100), Duration::from_secs(20))),
            None
        );
        assert_eq!(restante_estimado(&b.0, None), None);
        assert_eq!(restante_estimado(&h(10, 10), Some(5.0)), None);
    }

    #[test]
    fn converte_do_progresso_do_agente() {
        let p = ProgressoInventario {
            execucao: dag_core::ExecucaoId(3),
            fase: FaseInventario::Agregacao,
            pastas: 2,
            arquivos: 10,
            especiais: 1,
            bytes_logicos: 99,
            ignorados: 4,
            erros: 5,
            diretorios_pendentes: 6,
            decorrido_ms: 7000,
            entradas_por_segundo: 8.5,
        };
        let v = ProgressoScan::from(&p);
        assert_eq!(v.fase, FaseScan::Agregacao);
        assert_eq!(v.itens(), 13);
        assert_eq!((v.pendentes, v.erros, v.ignorados), (6, 5, 4));
    }
}
