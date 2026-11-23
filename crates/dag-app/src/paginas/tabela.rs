//! Tabela virtualizada com colunas de largura fixa: só as linhas visíveis são desenhadas, então
//! uma página de 500 linhas custa o mesmo que uma de 20.

use bancada_casca::egui::{self, Align, Label, Layout, Response, ScrollArea, Sense, Ui, vec2};

/// Uma coluna: título e largura em pontos.
#[derive(Debug, Clone, Copy)]
pub struct Coluna {
    pub titulo: &'static str,
    pub largura: f32,
    /// Alinha o conteúdo à direita (números).
    pub direita: bool,
}

impl Coluna {
    #[must_use]
    pub const fn esq(titulo: &'static str, largura: f32) -> Self {
        Self {
            titulo,
            largura,
            direita: false,
        }
    }

    #[must_use]
    pub const fn dir(titulo: &'static str, largura: f32) -> Self {
        Self {
            titulo,
            largura,
            direita: true,
        }
    }
}

/// A linha em desenho: cada chamada a uma célula avança uma coluna.
pub struct Linha<'a> {
    ui: &'a mut Ui,
    colunas: &'a [Coluna],
    atual: usize,
    altura: f32,
}

impl std::fmt::Debug for Linha<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Linha")
            .field("atual", &self.atual)
            .finish_non_exhaustive()
    }
}

impl Linha<'_> {
    fn coluna(&mut self) -> Coluna {
        let c = self
            .colunas
            .get(self.atual)
            .copied()
            .unwrap_or(Coluna::esq("", 80.0));
        self.atual += 1;
        c
    }

    /// Célula de texto (truncado com reticências; o texto completo aparece ao pairar).
    pub fn texto(&mut self, texto: impl Into<String>) -> Response {
        let c = self.coluna();
        let texto: String = texto.into();
        let r = self
            .ui
            .allocate_ui_with_layout(
                vec2(c.largura, self.altura),
                Layout::left_to_right(Align::Center)
                    .with_main_align(if c.direita { Align::Max } else { Align::Min })
                    .with_main_justify(true),
                |ui| ui.add(Label::new(texto.clone()).truncate()),
            )
            .inner;
        r.on_hover_text(texto)
    }

    /// Célula com rótulo selecionável (clicável).
    pub fn selecionavel(&mut self, selecionado: bool, texto: impl Into<String>) -> Response {
        let c = self.coluna();
        let texto: String = texto.into();
        self.ui
            .allocate_ui_with_layout(
                vec2(c.largura, self.altura),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_width(c.largura);
                    ui.selectable_label(selecionado, truncar(&texto, c.largura))
                },
            )
            .inner
    }

    /// Célula com conteúdo livre.
    pub fn ui<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        let c = self.coluna();
        self.ui
            .allocate_ui_with_layout(
                vec2(c.largura, self.altura),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_max_width(c.largura);
                    f(ui)
                },
            )
            .inner
    }
}

/// Corta o texto para caber, aproximadamente, em `largura` pontos (8 por caractere).
fn truncar(texto: &str, largura: f32) -> String {
    let max = ((largura - 12.0) / 7.0).max(4.0) as usize;
    if texto.chars().count() <= max {
        texto.to_string()
    } else {
        let mut s: String = texto.chars().take(max.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

/// Desenha cabeçalho e as linhas visíveis de `n` linhas. `altura_max` limita a altura da tabela.
pub fn tabela(
    ui: &mut Ui,
    id: &str,
    colunas: &[Coluna],
    n: usize,
    altura_max: f32,
    mut linha: impl FnMut(&mut Linha<'_>, usize),
) {
    let espaco = ui.spacing().item_spacing.x;
    let largura_total: f32 = colunas.iter().map(|c| c.largura + espaco).sum();
    let altura_linha = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    ScrollArea::horizontal()
        .id_salt(format!("{id}-h"))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.set_min_width(largura_total);
            ui.horizontal(|ui| {
                for c in colunas {
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(c.largura, altura_linha), Sense::hover());
                    ui.painter().text(
                        if c.direita {
                            rect.right_center()
                        } else {
                            rect.left_center()
                        },
                        if c.direita {
                            egui::Align2::RIGHT_CENTER
                        } else {
                            egui::Align2::LEFT_CENTER
                        },
                        c.titulo,
                        egui::TextStyle::Body.resolve(ui.style()),
                        ui.visuals().strong_text_color(),
                    );
                }
            });
            ui.separator();
            ScrollArea::vertical()
                .id_salt(format!("{id}-v"))
                .auto_shrink([false, true])
                .max_height(altura_max.max(80.0))
                .show_rows(ui, altura_linha, n, |ui, faixa| {
                    for i in faixa {
                        ui.horizontal(|ui| {
                            let mut l = Linha {
                                ui,
                                colunas,
                                atual: 0,
                                altura: altura_linha,
                            };
                            linha(&mut l, i);
                        });
                    }
                });
        });
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn truncar_corta_com_reticencias() {
        assert_eq!(truncar("curto", 200.0), "curto");
        let t = truncar(&"a".repeat(100), 100.0);
        assert!(t.ends_with('…') && t.chars().count() < 20);
    }
}
