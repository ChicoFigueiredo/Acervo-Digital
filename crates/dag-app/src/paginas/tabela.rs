//! Tabela única da GUI: colunas redimensionáveis (arraste a borda do cabeçalho; duplo clique
//! restaura), cabeçalho clicável para ordenar (▲/▼) por valor tipado, linhas zebradas e
//! virtualizadas (só as visíveis são desenhadas), rolagem horizontal quando a tabela é mais larga
//! que a janela, texto truncado com o valor completo na dica.
//!
//! Larguras e ordenação ficam na memória do egui por tabela (valem durante a sessão).

use bancada_casca::egui::{
    self, Align, Align2, Context, CursorIcon, Id, Label, Layout, Rect, Response, ScrollArea, Sense,
    Ui, vec2,
};
use std::cmp::Ordering;

/// Largura mínima de uma coluna ao arrastar.
const LARGURA_MINIMA: f32 = 36.0;

/// Uma coluna: título e largura inicial em pontos.
#[derive(Debug, Clone, Copy)]
pub struct Coluna {
    pub titulo: &'static str,
    pub largura: f32,
    /// Alinha o conteúdo à direita (números).
    pub direita: bool,
    /// O cabeçalho ordena a tabela.
    pub ordena: bool,
}

impl Coluna {
    #[must_use]
    pub const fn esq(titulo: &'static str, largura: f32) -> Self {
        Self {
            titulo,
            largura,
            direita: false,
            ordena: !titulo.is_empty(),
        }
    }

    #[must_use]
    pub const fn dir(titulo: &'static str, largura: f32) -> Self {
        Self {
            titulo,
            largura,
            direita: true,
            ordena: !titulo.is_empty(),
        }
    }

    /// Coluna de controles (caixas, botões): o cabeçalho não ordena.
    #[must_use]
    pub const fn sem_ordem(mut self) -> Self {
        self.ordena = false;
        self
    }
}

/// Valor tipado de uma célula para ordenar (nunca o texto formatado).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chave {
    /// Sem valor: vai sempre para o fim, em qualquer direção.
    Nulo,
    /// Número, tamanho em bytes ou data (segundos/nanossegundos desde a época).
    Num(i128),
    /// Texto já normalizado (ver [`Chave::texto`]).
    Texto(String),
}

impl Chave {
    /// Texto sem diferenciar maiúsculas de minúsculas (dobra Unicode, independente de locale).
    #[must_use]
    pub fn texto(t: &str) -> Self {
        Self::Texto(t.chars().flat_map(char::to_lowercase).collect())
    }

    #[must_use]
    pub fn num(n: impl Into<i128>) -> Self {
        Self::Num(n.into())
    }

    #[must_use]
    pub fn opt_num<T: Into<i128>>(n: Option<T>) -> Self {
        n.map_or(Self::Nulo, |n| Self::Num(n.into()))
    }

    /// Número real (seis casas), para percentuais e razões.
    #[must_use]
    pub fn real(x: f64) -> Self {
        if x.is_finite() {
            Self::Num((x * 1e6).round() as i128)
        } else {
            Self::Nulo
        }
    }

    #[must_use]
    pub fn opt_real(x: Option<f64>) -> Self {
        x.map_or(Self::Nulo, Self::real)
    }

    #[must_use]
    pub fn opt_texto(t: Option<&str>) -> Self {
        t.map_or(Self::Nulo, Self::texto)
    }

    fn valor_cmp(&self, o: &Self) -> Ordering {
        match (self, o) {
            (Self::Num(a), Self::Num(b)) => a.cmp(b),
            (Self::Texto(a), Self::Texto(b)) => a.cmp(b),
            (Self::Num(_), Self::Texto(_)) => Ordering::Less,
            (Self::Texto(_), Self::Num(_)) => Ordering::Greater,
            _ => Ordering::Equal,
        }
    }

    /// Comparação com a direção aplicada; `Nulo` fica sempre por último.
    fn comparar(&self, o: &Self, crescente: bool) -> Ordering {
        match (self, o) {
            (Self::Nulo, Self::Nulo) => Ordering::Equal,
            (Self::Nulo, _) => Ordering::Greater,
            (_, Self::Nulo) => Ordering::Less,
            _ => {
                let c = self.valor_cmp(o);
                if crescente { c } else { c.reverse() }
            }
        }
    }
}

/// Ordem de exibição das `n` linhas. `ordem` é `(coluna, crescente)`; a ordenação é estável
/// (empates mantêm a ordem original) e `chave(linha)` devolve a chave da coluna escolhida.
#[must_use]
pub fn ordenar_indices(
    n: usize,
    ordem: Option<(usize, bool)>,
    chave: impl Fn(usize) -> Chave,
) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..n).collect();
    if let Some((_, crescente)) = ordem {
        let chaves: Vec<Chave> = (0..n).map(chave).collect();
        idx.sort_by(|&a, &b| chaves[a].comparar(&chaves[b], crescente));
    }
    idx
}

/// Próxima ordenação ao clicar no cabeçalho da coluna `col`: coluna nova começa crescente;
/// a mesma coluna alterna entre crescente e decrescente.
#[must_use]
pub fn alternar_ordem(atual: Option<(usize, bool)>, col: usize) -> (usize, bool) {
    match atual {
        Some((c, cresc)) if c == col => (col, !cresc),
        _ => (col, true),
    }
}

#[derive(Debug, Clone, Default)]
struct EstadoTabela {
    larguras: Vec<f32>,
    ordem: Option<(usize, bool)>,
}

fn id_estado(id: &str) -> Id {
    Id::new(("dag-tabela", id))
}

/// Ordenação atual `(coluna, crescente)` da tabela `id`.
#[must_use]
pub fn ordem_atual(ctx: &Context, id: &str) -> Option<(usize, bool)> {
    ctx.data(|d| d.get_temp::<EstadoTabela>(id_estado(id)))
        .and_then(|e| e.ordem)
}

/// Define a ordenação da tabela `id` (usado por testes e atalhos).
pub fn definir_ordem(ctx: &Context, id: &str, ordem: Option<(usize, bool)>) {
    ctx.data_mut(|d| {
        let mut e = d
            .get_temp::<EstadoTabela>(id_estado(id))
            .unwrap_or_default();
        e.ordem = ordem;
        d.insert_temp(id_estado(id), e);
    });
}

/// Larguras atuais das colunas da tabela `id` (vazio antes do primeiro desenho).
#[must_use]
pub fn larguras_atuais(ctx: &Context, id: &str) -> Vec<f32> {
    ctx.data(|d| d.get_temp::<EstadoTabela>(id_estado(id)))
        .map(|e| e.larguras)
        .unwrap_or_default()
}

/// Aviso para tabelas paginadas: a ordenação do cabeçalho vale só para as linhas carregadas.
pub fn nota_ordem_local(ui: &mut Ui, id: &str) {
    if ordem_atual(ui.ctx(), id).is_some() {
        ui.weak("Ordenação do cabeçalho aplicada apenas às linhas já carregadas.");
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
                |ui| ui.add(Label::new(texto.clone()).truncate().sense(Sense::click())),
            )
            .inner;
        r.on_hover_text(texto)
    }

    /// Célula com rótulo selecionável (clicável), truncado como as demais.
    pub fn selecionavel(&mut self, selecionado: bool, texto: impl Into<String>) -> Response {
        let c = self.coluna();
        let texto: String = texto.into();
        let r = self
            .ui
            .allocate_ui_with_layout(
                vec2(c.largura, self.altura),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_width(c.largura);
                    ui.add(egui::Button::selectable(selecionado, texto.clone()).truncate())
                },
            )
            .inner;
        if truncado(&texto, c.largura) {
            r.on_hover_text(texto)
        } else {
            r
        }
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

/// Estimativa (7 pontos por caractere) de o texto não caber em `largura`.
fn truncado(texto: &str, largura: f32) -> bool {
    let max = ((largura - 12.0) / 7.0).max(4.0) as usize;
    texto.chars().count() > max
}

/// Larguras efetivas: as guardadas, com a última coluna ocupando o que sobrar da janela.
fn larguras_efetivas(base: &[f32], espaco: f32, disponivel: f32) -> Vec<f32> {
    let mut ef = base.to_vec();
    let soma: f32 = ef.iter().map(|w| w + espaco).sum();
    if let Some(ultima) = ef.last_mut()
        && soma < disponivel
    {
        *ultima += disponivel - soma;
    }
    ef
}

/// Desenha cabeçalho e as linhas visíveis de `n` linhas. `altura_max` limita a altura da tabela.
///
/// `chave(coluna, linha)` dá o valor tipado da célula para ordenar; `linha(l, i)` desenha a
/// linha de dado `i` (já na ordem escolhida).
pub fn tabela(
    ui: &mut Ui,
    id: &str,
    colunas: &[Coluna],
    n: usize,
    altura_max: f32,
    chave: impl Fn(usize, usize) -> Chave,
    mut linha: impl FnMut(&mut Linha<'_>, usize),
) {
    let espaco = ui.spacing().item_spacing.x;
    let altura_linha = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    let eid = id_estado(id);
    let mut est = ui
        .ctx()
        .data(|d| d.get_temp::<EstadoTabela>(eid))
        .unwrap_or_default();
    if est.larguras.len() != colunas.len() {
        est.larguras = colunas.iter().map(|c| c.largura).collect();
    }
    if est.ordem.is_some_and(|(c, _)| c >= colunas.len()) {
        est.ordem = None;
    }
    let ef = larguras_efetivas(&est.larguras, espaco, ui.available_width());
    let colunas_ef: Vec<Coluna> = colunas
        .iter()
        .zip(&ef)
        .map(|(c, &w)| Coluna { largura: w, ..*c })
        .collect();
    let largura_total: f32 = ef.iter().map(|w| w + espaco).sum();
    let ordem = est.ordem;
    let indices = ordenar_indices(n, ordem, |i| {
        ordem.map_or(Chave::Nulo, |(c, _)| chave(c, i))
    });
    let mut novo = est;

    ScrollArea::horizontal()
        .id_salt(format!("{id}-h"))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.set_min_width(largura_total);
            ui.horizontal(|ui| {
                for (i, c) in colunas_ef.iter().enumerate() {
                    cabecalho(ui, eid, i, c, &colunas[i], ordem, &mut novo);
                }
            });
            ui.separator();
            ScrollArea::vertical()
                .id_salt(format!("{id}-v"))
                .auto_shrink([false, true])
                .max_height(altura_max.max(80.0))
                .show_rows(ui, altura_linha, n, |ui, faixa| {
                    for k in faixa {
                        let Some(&i) = indices.get(k) else { continue };
                        if k % 2 == 1 {
                            let r = Rect::from_min_size(
                                ui.cursor().min,
                                vec2(largura_total, altura_linha),
                            );
                            ui.painter()
                                .rect_filled(r, 0.0, ui.visuals().faint_bg_color);
                        }
                        ui.horizontal(|ui| {
                            let mut l = Linha {
                                ui,
                                colunas: &colunas_ef,
                                atual: 0,
                                altura: altura_linha,
                            };
                            linha(&mut l, i);
                        });
                    }
                });
        });
    ui.ctx().data_mut(|d| d.insert_temp(eid, novo));
}

/// Uma célula de cabeçalho: título com seta, clique ordena, a borda direita redimensiona.
fn cabecalho(
    ui: &mut Ui,
    eid: Id,
    i: usize,
    c: &Coluna,
    original: &Coluna,
    ordem: Option<(usize, bool)>,
    novo: &mut EstadoTabela,
) {
    let altura = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    let (rect, resp) = ui.allocate_exact_size(
        vec2(c.largura, altura),
        if c.ordena {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let seta = match ordem {
        Some((col, true)) if col == i => " ▲",
        Some((col, false)) if col == i => " ▼",
        _ => "",
    };
    let cor = if resp.hovered() && c.ordena {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    let (ancora, pos) = if c.direita {
        (Align2::RIGHT_CENTER, rect.right_center() - vec2(4.0, 0.0))
    } else {
        (Align2::LEFT_CENTER, rect.left_center() + vec2(2.0, 0.0))
    };
    ui.painter_at(rect).text(
        pos,
        ancora,
        format!("{}{seta}", c.titulo),
        egui::TextStyle::Button.resolve(ui.style()),
        cor,
    );
    if c.ordena {
        if resp.clicked() {
            novo.ordem = Some(alternar_ordem(ordem, i));
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        resp.on_hover_text("Clique para ordenar; arraste a borda direita para redimensionar");
    }
    // Espaçador: arrastar redimensiona; duplo clique restaura a largura inicial.
    let espaco = ui.spacing().item_spacing.x;
    let borda = Rect::from_min_max(
        egui::pos2(rect.right() - 2.0, rect.top()),
        egui::pos2(rect.right() + espaco.min(6.0), rect.bottom()),
    );
    let h = ui.interact(borda, Id::new((eid, "borda", i)), Sense::click_and_drag());
    if h.hovered() || h.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        ui.painter()
            .rect_filled(borda, 0.0, ui.visuals().widgets.hovered.bg_stroke.color);
    }
    if h.dragged() {
        novo.larguras[i] = (c.largura + h.drag_delta().x).max(LARGURA_MINIMA);
    }
    if h.double_clicked() {
        novo.larguras[i] = original.largura;
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use egui::{Event, PointerButton, RawInput, Rect, pos2};

    fn ordem(chaves: &[Chave], cresc: bool) -> Vec<usize> {
        ordenar_indices(chaves.len(), Some((0, cresc)), |i| chaves[i].clone())
    }

    #[test]
    fn tamanhos_ordenam_por_bytes_e_nao_pelo_texto() {
        // "900 B" < "10 KiB" < "2 MiB" por valor, mas "10 KiB" < "2 MiB" < "900 B" como texto.
        let k = [
            Chave::num(2_097_152_u64),
            Chave::num(900_u64),
            Chave::num(10_240_u64),
        ];
        assert_eq!(ordem(&k, true), vec![1, 2, 0]);
        assert_eq!(ordem(&k, false), vec![0, 2, 1]);
    }

    #[test]
    fn datas_e_numeros_negativos_ordenam_por_valor() {
        let k = [Chave::num(-5), Chave::num(1_700_000_000_i64), Chave::num(0)];
        assert_eq!(ordem(&k, true), vec![0, 2, 1]);
    }

    #[test]
    fn texto_ignora_caixa_e_nulos_ficam_no_fim_nas_duas_direcoes() {
        let k = [
            Chave::texto("banana"),
            Chave::Nulo,
            Chave::texto("Abacaxi"),
            Chave::texto("cereja"),
        ];
        assert_eq!(ordem(&k, true), vec![2, 0, 3, 1]);
        assert_eq!(ordem(&k, false), vec![3, 0, 2, 1]);
    }

    #[test]
    fn ordenacao_e_estavel_nos_empates() {
        let k = [Chave::num(1), Chave::num(0), Chave::num(1), Chave::num(0)];
        assert_eq!(ordem(&k, true), vec![1, 3, 0, 2]);
        // Decrescente também mantém a ordem original entre iguais.
        assert_eq!(ordem(&k, false), vec![0, 2, 1, 3]);
    }

    #[test]
    fn sem_ordem_mantem_a_ordem_original_e_reais_comparam() {
        assert_eq!(ordenar_indices(3, None, |_| Chave::Nulo), vec![0, 1, 2]);
        let k = [Chave::real(99.5), Chave::real(100.0), Chave::opt_real(None)];
        assert_eq!(ordem(&k, true), vec![0, 1, 2]);
        assert_eq!(ordem(&k, false), vec![1, 0, 2]);
    }

    #[test]
    fn clicar_alterna_a_direcao_e_coluna_nova_comeca_crescente() {
        assert_eq!(alternar_ordem(None, 2), (2, true));
        assert_eq!(alternar_ordem(Some((2, true)), 2), (2, false));
        assert_eq!(alternar_ordem(Some((2, false)), 2), (2, true));
        assert_eq!(alternar_ordem(Some((2, false)), 0), (0, true));
    }

    #[test]
    fn ultima_coluna_ocupa_o_que_sobra() {
        let ef = larguras_efetivas(&[100.0, 100.0], 8.0, 500.0);

        assert!((ef.iter().map(|w| w + 8.0).sum::<f32>() - 500.0).abs() < 0.01);
        // Mais larga que a janela: nada muda (rolagem horizontal).
        assert_eq!(
            larguras_efetivas(&[400.0, 400.0], 8.0, 500.0),
            vec![400.0, 400.0]
        );
    }

    const COLS: [Coluna; 3] = [
        Coluna::esq("Nome", 120.0),
        Coluna::dir("Tamanho", 90.0),
        Coluna::esq("", 60.0),
    ];
    const DADOS: [(&str, u64); 3] = [("b", 900), ("A", 2_097_152), ("c", 10_240)];

    /// Um quadro com a tabela; devolve as linhas na ordem em que foram desenhadas.
    fn quadro(ctx: &Context, eventos: Vec<Event>) -> Vec<usize> {
        let mut vistos = Vec::new();
        let entrada = RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 400.0))),
            events: eventos,
            ..Default::default()
        };
        ctx.run_ui(entrada, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                tabela(
                    ui,
                    "teste",
                    &COLS,
                    DADOS.len(),
                    300.0,
                    |c, i| match c {
                        0 => Chave::texto(DADOS[i].0),
                        _ => Chave::num(DADOS[i].1),
                    },
                    |l, i| {
                        vistos.push(i);
                        l.texto(DADOS[i].0);
                        l.texto(DADOS[i].1.to_string());
                        l.texto("");
                    },
                );
            });
        })
        .drop_without_applying_deltas();
        vistos
    }

    fn toque(p: egui::Pos2, baixo: bool) -> Event {
        Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed: baixo,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Posição do cabeçalho da coluna `i` (primeiro quadro: margem do painel + larguras).
    fn centro_do_cabecalho(i: usize) -> egui::Pos2 {
        let x: f32 = COLS.iter().take(i).map(|c| c.largura + 8.0).sum();
        pos2(8.0 + x + 20.0, 8.0 + 12.0)
    }

    #[test]
    fn cabecalho_clicavel_alterna_a_ordem_e_guarda_o_estado() {
        let ctx = Context::default();
        assert_eq!(quadro(&ctx, vec![]), vec![0, 1, 2]);
        assert_eq!(ordem_atual(&ctx, "teste"), None);
        let p = centro_do_cabecalho(1);
        for ev in [
            vec![Event::PointerMoved(p)],
            vec![toque(p, true)],
            vec![toque(p, false)],
        ] {
            quadro(&ctx, ev);
        }
        assert_eq!(ordem_atual(&ctx, "teste"), Some((1, true)));
        assert_eq!(
            quadro(&ctx, vec![]),
            vec![0, 2, 1],
            "tamanho crescente por bytes"
        );
        for ev in [vec![toque(p, true)], vec![toque(p, false)]] {
            quadro(&ctx, ev);
        }
        assert_eq!(ordem_atual(&ctx, "teste"), Some((1, false)));
        assert_eq!(quadro(&ctx, vec![]), vec![1, 2, 0]);
    }

    #[test]
    fn coluna_sem_titulo_nao_ordena() {
        let ctx = Context::default();
        quadro(&ctx, vec![]);
        let p = centro_do_cabecalho(2);
        for ev in [
            vec![Event::PointerMoved(p)],
            vec![toque(p, true)],
            vec![toque(p, false)],
        ] {
            quadro(&ctx, ev);
        }
        assert_eq!(ordem_atual(&ctx, "teste"), None);
    }

    #[test]
    fn arrastar_a_borda_redimensiona_e_a_largura_fica_lembrada() {
        let ctx = Context::default();
        quadro(&ctx, vec![]);
        let antes = larguras_atuais(&ctx, "teste");
        assert_eq!(antes[0], 120.0);
        let borda = pos2(8.0 + 120.0 + 1.0, 8.0 + 12.0);
        let destino = borda + vec2(50.0, 0.0);
        for ev in [
            vec![Event::PointerMoved(borda)],
            vec![toque(borda, true)],
            vec![Event::PointerMoved(destino)],
            vec![Event::PointerMoved(destino + vec2(1.0, 0.0))],
            vec![toque(destino, false)],
        ] {
            quadro(&ctx, ev);
        }
        quadro(&ctx, vec![]);
        let depois = larguras_atuais(&ctx, "teste");
        assert!(
            depois[0] > 150.0,
            "largura depois do arraste: {}",
            depois[0]
        );
        assert_eq!(ordem_atual(&ctx, "teste"), None, "arrastar não ordena");
    }

    #[test]
    fn truncado_estima_pelo_tamanho() {
        assert!(!truncado("curto", 200.0));
        assert!(truncado(&"a".repeat(100), 100.0));
    }
}
