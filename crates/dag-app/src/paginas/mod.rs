//! Registro de páginas e ações. Dados puros (sem egui), testáveis sem display.

pub mod analise;
pub mod configuracoes;
pub mod conteudo;
pub mod discos;
pub mod duplicados;
pub mod explorador;
pub mod fila;
pub mod inicio;
pub mod lixeira;
pub mod monitoramento;
pub mod tabela;
pub mod varredura;

use crate::app::AppDag;
use bancada_casca::egui;

/// Uma ação planejada do Ribbon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefAcao {
    pub id: &'static str,
    pub rotulo: &'static str,
    pub dica: &'static str,
}

/// Um grupo do Ribbon dentro de uma aba.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefGrupo {
    pub id: &'static str,
    pub rotulo: &'static str,
    pub acoes: &'static [DefAcao],
}

/// Uma página: aba do Ribbon, item de navegação e corpo.
#[derive(Debug, Clone, Copy)]
pub struct DefPagina {
    pub id: &'static str,
    pub rotulo: &'static str,
    pub keytip: &'static str,
    pub icone: &'static str,
    pub grupos: &'static [DefGrupo],
    /// Resumo de uma linha da página (cabeçalho e estados vazios).
    pub resumo: &'static str,
    pub desenhar: fn(&mut egui::Ui, &mut AppDag),
}

impl DefPagina {
    pub fn acoes(&self) -> impl Iterator<Item = &'static DefAcao> {
        self.grupos.iter().flat_map(|g| g.acoes.iter())
    }
}

/// Todas as páginas, na ordem das abas do Ribbon.
pub fn todas() -> [&'static DefPagina; 10] {
    [
        &inicio::PAGINA,
        &discos::PAGINA,
        &varredura::PAGINA,
        &explorador::PAGINA,
        &analise::PAGINA,
        &duplicados::PAGINA,
        &lixeira::PAGINA,
        &fila::PAGINA,
        &monitoramento::PAGINA,
        &configuracoes::PAGINA,
    ]
}

pub fn por_id(id: &str) -> Option<&'static DefPagina> {
    todas().into_iter().find(|p| p.id == id)
}

/// Corpo de uma página dentro de uma rolagem vertical sempre visível: o que passa da janela
/// nunca é cortado. A altura visível vira o teto do conteúdo, para que as páginas que
/// preenchem o espaço (`available_height`) tenham um tamanho razoável em vez de "infinito".
pub fn com_rolagem<R>(
    ui: &mut egui::Ui,
    id: &str,
    corpo: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    let visivel = ui.available_height();
    egui::ScrollArea::vertical()
        .id_salt(("pagina-rolagem", id))
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(ui, |ui| {
            let topo = ui.min_rect().top();
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new(ALTURA_DA_PAGINA), (visivel, topo)));
            corpo(ui)
        })
}

const ALTURA_DA_PAGINA: &str = "dag-altura-da-pagina";

/// Altura que sobra na janela para a página, descontado o que já foi desenhado. Dentro da
/// rolagem de [`com_rolagem`] o `available_height` é infinito; as tabelas que preenchem o
/// espaço usam esta altura (e um mínimo próprio, para a página rolar em vez de espremer).
pub fn altura_livre(ui: &egui::Ui) -> f32 {
    let (visivel, topo) = ui
        .ctx()
        .data(|d| d.get_temp::<(f32, f32)>(egui::Id::new(ALTURA_DA_PAGINA)))
        .unwrap_or((f32::INFINITY, 0.0));
    let usado = ui.cursor().top() - topo;
    ui.available_height().min((visivel - usado).max(0.0))
}

/// Painel de estado vazio, com o resumo da página e uma dica do que fazer.
pub fn estado_vazio(ui: &mut egui::Ui, titulo: &str, resumo: &str, dica: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.heading(titulo);
        ui.add_space(8.0);
        ui.label(resumo);
        ui.add_space(8.0);
        ui.weak(dica);
    });
}

/// Cabeçalho comum: título e resumo.
pub fn cabecalho(ui: &mut egui::Ui, pagina: &DefPagina) {
    ui.heading(pagina.rotulo);
    ui.weak(pagina.resumo);
    ui.add_space(4.0);
}

/// Cor do texto de um aviso conforme o tom (o tom nunca é o único sinal: há prefixo).
fn cor_do_tom(ui: &egui::Ui, tom: bancada_casca::Tom) -> (egui::Color32, &'static str) {
    use bancada_casca::Tom;
    match tom {
        Tom::Perigo => (ui.visuals().error_fg_color, "Erro: "),
        Tom::Aviso => (ui.visuals().warn_fg_color, "Atenção: "),
        Tom::Sucesso => (egui::Color32::from_rgb(76, 175, 80), "OK: "),
        Tom::Info | Tom::Neutro => (ui.visuals().hyperlink_color, ""),
    }
}

/// Faixa de mensagens no topo da página; cada uma pode ser dispensada.
pub fn faixa_de_avisos(ui: &mut egui::Ui, avisos: &mut Vec<crate::app::Aviso>) {
    let mut remover = None;
    for (i, a) in avisos.iter().enumerate() {
        let (cor, prefixo) = cor_do_tom(ui, a.tom);
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("✕").on_hover_text("Dispensar").clicked() {
                remover = Some(i);
            }
            ui.colored_label(cor, format!("{prefixo}{}", a.texto));
        });
    }
    if let Some(i) = remover {
        avisos.remove(i);
    }
}

/// Texto "carregando…" ou o erro de uma carga, sob um cabeçalho de seção.
pub fn estado_da_carga<T>(ui: &mut egui::Ui, carga: &crate::leitor::Carga<T>) {
    if let Some(e) = &carga.erro {
        ui.colored_label(ui.visuals().warn_fg_color, e);
    } else if carga.carregando() && carga.dado.is_none() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Carregando…");
        });
    }
}

#[cfg(test)]
mod testes_rolagem {
    use super::*;
    use egui::{RawInput, Rect, pos2, vec2};

    #[test]
    fn pagina_com_muitas_linhas_rola() {
        let ctx = egui::Context::default();
        let mut saida = None;
        for _ in 0..3 {
            let entrada = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 400.0))),
                ..Default::default()
            };
            ctx.run_ui(entrada, |ui| {
                saida = Some(com_rolagem(ui, "teste", |ui| {
                    for i in 0..500 {
                        ui.label(format!("linha {i}"));
                    }
                }));
            })
            .drop_without_applying_deltas();
        }
        let s = saida.unwrap();
        assert!(s.content_size.y > s.inner_rect.height() * 5.0);
        assert!(s.state.offset.y >= 0.0);
    }

    #[test]
    fn pagina_curta_nao_rola() {
        let ctx = egui::Context::default();
        let mut saida = None;
        let entrada = RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
            ..Default::default()
        };
        ctx.run_ui(entrada, |ui| {
            saida = Some(com_rolagem(ui, "curta", |ui| {
                ui.label("oi");
            }));
        })
        .drop_without_applying_deltas();
        let s = saida.unwrap();
        assert!(s.content_size.y < s.inner_rect.height());
    }
}
