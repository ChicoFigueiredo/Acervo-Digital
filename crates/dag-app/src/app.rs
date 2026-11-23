//! O `AppCasca` do DAG, montado a partir do registro de páginas.

use crate::TITULO;
use crate::paginas::{self, DefPagina};
use bancada_casca::{
    Aba, Acao, AppCasca, Casca, ConfigCasca, Elo, FamiliaDeIcones, Grupo, GrupoNavegacao,
    ItemNavegacao, Local, Rota, SegmentoStatus, Tom, egui,
};

const TOKENS: &str = include_str!("../dados/tokens.json");
const TEMAS: &str = include_str!("../dados/temas.json");

/// Motivo mostrado nas dicas das ações ainda não implementadas.
pub const MOTIVO_FUTURO: &str = "Chega em um incremento posterior";

/// Estado do app neste incremento: nenhum (tudo é estático).
#[derive(Debug, Default)]
pub struct AppDag;

impl AppDag {
    /// Ações da lista única (todas desabilitadas neste incremento).
    pub fn lista_de_acoes() -> Vec<Acao> {
        paginas::todas()
            .into_iter()
            .flat_map(DefPagina::acoes)
            .map(|a| {
                Acao::nova(a.id, a.rotulo)
                    .dica(a.dica)
                    .desabilitada(MOTIVO_FUTURO)
            })
            .collect()
    }

    /// Abas do Ribbon, uma por página.
    pub fn lista_de_abas() -> Vec<Aba> {
        paginas::todas()
            .into_iter()
            .map(|p| {
                p.grupos
                    .iter()
                    .fold(Aba::nova(p.id, p.rotulo, p.keytip), |aba, g| {
                        let ids: Vec<&str> = g.acoes.iter().map(|a| a.id).collect();
                        aba.grupo(Grupo::novo(g.id, g.rotulo, &ids))
                    })
            })
            .collect()
    }

    pub fn segmentos_de_status() -> Vec<SegmentoStatus> {
        vec![
            SegmentoStatus::novo("worker", "Worker: não conectado")
                .tom(Tom::Aviso)
                .dica("O worker de varredura chega em um incremento posterior"),
        ]
    }
}

impl AppCasca for AppDag {
    fn acoes(&self) -> Vec<Acao> {
        Self::lista_de_acoes()
    }

    fn abas(&self) -> Vec<Aba> {
        Self::lista_de_abas()
    }

    fn navegacao(&self) -> Vec<GrupoNavegacao> {
        vec![GrupoNavegacao::novo(
            "Acervo",
            paginas::todas()
                .into_iter()
                .map(|p| ItemNavegacao::novo(p.id, p.rotulo, p.icone))
                .collect(),
        )]
    }

    fn rota_inicial(&self) -> Rota {
        Rota::nova(paginas::todas()[0].id)
    }

    fn trilha(&self, rota: &Rota) -> Vec<Elo> {
        let rotulo = paginas::por_id(rota.como_texto()).map_or("DAG", |p| p.rotulo);
        vec![Elo::texto(rotulo)]
    }

    fn desenhar_pagina(&mut self, ui: &mut egui::Ui, rota: &Rota, _local: Local) {
        match paginas::por_id(rota.como_texto()) {
            Some(p) => (p.desenhar)(ui),
            None => {
                ui.label("Página desconhecida.");
            }
        }
    }

    fn status(&self) -> Vec<SegmentoStatus> {
        Self::segmentos_de_status()
    }

    fn ao_executar(&mut self, _ctx: &egui::Context, _id: &str) {
        // Nenhuma ação está habilitada neste incremento.
    }
}

/// Configuração da casca: título, tokens e temas embutidos; nada é lido do disco.
pub fn config() -> ConfigCasca {
    let mut c = ConfigCasca::nova(TITULO, TOKENS, TEMAS);
    c.tema_inicial = "salvia".into();
    c.icones = FamiliaDeIcones::sem_fonte();
    c
}

/// Monta a casca (valida tokens/temas).
pub fn montar() -> Result<Casca<AppDag>, Box<dyn std::error::Error>> {
    Ok(Casca::nova(AppDag, config())?)
}

#[cfg(test)]
mod testes {
    use super::*;
    use bancada_casca::egui::{RawInput, Rect, pos2, vec2};
    use bancada_casca::ribbon::validar_abas;
    use bancada_casca::{Disponibilidade, ListaDeAcoes};
    use std::collections::HashSet;

    const ABAS: [&str; 8] = [
        "Início",
        "Discos",
        "Varredura",
        "Explorador",
        "Análise",
        "Duplicados",
        "Monitoramento",
        "Configurações",
    ];

    #[test]
    fn as_oito_abas_estao_presentes_na_ordem() {
        let rotulos: Vec<String> = AppDag::lista_de_abas()
            .into_iter()
            .map(|a| a.rotulo)
            .collect();
        assert_eq!(rotulos, ABAS);
    }

    #[test]
    fn ids_de_abas_acoes_e_keytips_sao_unicos() {
        let abas = AppDag::lista_de_abas();
        let ids: HashSet<_> = abas.iter().map(|a| a.id.clone()).collect();
        assert_eq!(ids.len(), abas.len());
        let keytips: HashSet<_> = abas.iter().map(|a| a.keytip.clone()).collect();
        assert_eq!(keytips.len(), abas.len());
        let acoes = AppDag::lista_de_acoes();
        let ids: HashSet<_> = acoes.iter().map(|a| a.id.clone()).collect();
        assert_eq!(ids.len(), acoes.len());
    }

    #[test]
    fn toda_acao_tem_rotulo_e_esta_desabilitada() {
        for a in AppDag::lista_de_acoes() {
            assert!(!a.rotulo.trim().is_empty(), "{} sem rótulo", a.id);
            assert!(
                matches!(a.disponibilidade, Disponibilidade::Desabilitada { .. }),
                "{}",
                a.id
            );
        }
    }

    #[test]
    fn abas_e_acoes_sao_validas_para_a_casca() {
        let lista = ListaDeAcoes::nova(AppDag::lista_de_acoes()).expect("lista válida");
        validar_abas(&AppDag::lista_de_abas(), &lista).expect("abas válidas");
        assert!(paginas::todas().iter().all(|p| p.acoes().next().is_some()));
    }

    #[test]
    fn varredura_e_duplicados_tem_as_acoes_planejadas() {
        let rotulos = |id: &str| -> Vec<&'static str> {
            paginas::por_id(id)
                .map(|p| p.acoes().map(|a| a.rotulo).collect())
                .unwrap_or_default()
        };
        for esperado in [
            "Full Scan",
            "Fast Scan",
            "Pausar",
            "Retomar",
            "Cancelar",
            "Selecionar raízes",
            "Exclusões",
        ] {
            assert!(rotulos("varredura").contains(&esperado), "{esperado}");
        }
        for esperado in [
            "Grupos",
            "Cópia Principal",
            "Marcar backup",
            "Marcar para deleção",
            "Relatório",
        ] {
            assert!(rotulos("duplicados").contains(&esperado), "{esperado}");
        }
    }

    #[test]
    fn status_mostra_worker_nao_conectado() {
        let s = AppDag::segmentos_de_status();
        assert!(s.iter().any(|s| s.texto == "Worker: não conectado"));
    }

    #[test]
    fn titulo_da_janela() {
        assert_eq!(config().titulo, "DAG — Acervo Digital");
    }

    /// Fumaça sem janela: desenha quadros com `run_ui` e entrada sintética, em todas as rotas.
    #[test]
    fn casca_desenha_todas_as_paginas_sem_display() {
        let mut casca = montar().expect("tokens e temas embutidos");
        let ctx = egui::Context::default();
        for p in paginas::todas() {
            casca.abrir(&ctx, Rota::nova(p.id), Local::Centro);
            for _ in 0..2 {
                let entrada = RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
                    ..Default::default()
                };
                ctx.run_ui(entrada, |ui| casca.desenhar(ui))
                    .drop_without_applying_deltas();
            }
        }
    }
}
