//! Página Análise: Tree View com barras proporcionais e Treemap *squarified* interativo
//! (drill-down por clique, breadcrumbs, cores por tipo, finalidade, idade ou profundidade).
//! Cada nível consulta só os filhos diretos, limitados a 200 (+ "outros").

use std::time::{SystemTime, UNIX_EPOCH};

use super::tabela::{Coluna, tabela};
use super::{DefAcao, DefGrupo, DefPagina, cabecalho, estado_da_carga, estado_vazio};
use crate::app::AppDag;
use crate::bases::{BasePercentual, Bases};
use crate::cores::{
    AtributosCor, ModoCor, OUTROS, Rgb, cor_do_bloco, cor_do_texto, legenda, rotulo_da_categoria,
    rotulo_da_finalidade,
};
use crate::formato::{bytes_humanos, data_ns, idade_em_dias, inteiro_pt_br, percentual};
use crate::leitor::{Carga, Leitor, erro};
use crate::treemap::{Ret, agrupar_pequenos, squarify};
use bancada_casca::egui::{
    self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, pos2, vec2,
};
use dag_store::consulta_gui::{
    AlvoAnalise, Analise, ChaveNo, LinhaVolume, NoAnalise, analisar, listar_volumes,
};

/// Teto de filhos por nível (o resto vira "outros").
pub const LIMITE_FILHOS: usize = 200;
/// Blocos com menos pixels quadrados que isto entram em "outros".
const AREA_MINIMA_PX: f64 = 40.0;

pub static PAGINA: DefPagina = DefPagina {
    id: "analise",
    rotulo: "Análise",
    keytip: "N",
    icone: "analise",
    grupos: &[
        DefGrupo {
            id: "analise.visoes",
            rotulo: "Visões",
            acoes: &[
                DefAcao {
                    id: "analise.tree_view",
                    rotulo: "Tree View",
                    dica: "Árvore com barras proporcionais ao consumo",
                },
                DefAcao {
                    id: "analise.treemap",
                    rotulo: "Treemap",
                    dica: "Treemap interativo com drill-down",
                },
            ],
        },
        DefGrupo {
            id: "analise.modo",
            rotulo: "Modo",
            acoes: &[
                DefAcao {
                    id: "analise.por_volume",
                    rotulo: "Por volume",
                    dica: "Analisa o volume selecionado",
                },
                DefAcao {
                    id: "analise.por_computador",
                    rotulo: "Por computador",
                    dica: "Soma os volumes sem contar a mesma montagem duas vezes",
                },
                DefAcao {
                    id: "analise.filtros",
                    rotulo: "Filtros",
                    dica: "Volume, categoria, MIME, idade, tamanho e profundidade",
                },
            ],
        },
    ],
    resumo: "Tree View e Treemap do consumo de espaço, por volume ou pelo computador inteiro.",
    desenhar,
};

/// Os dois modos de análise (spec §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModoAnalise {
    /// Um volume escolhido.
    Volume,
    /// O computador inteiro: soma dos volumes, cada um uma vez.
    Computador,
}

/// Forma de apresentar o nível.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vista {
    Tree,
    Treemap,
}

/// Estado da página.
#[derive(Debug)]
pub struct Estado {
    pub modo: ModoAnalise,
    pub vista: Vista,
    pub base: BasePercentual,
    pub cor: ModoCor,
    pub volume: Option<i64>,
    pub alvo: AlvoAnalise,
    carregado: Option<(AlvoAnalise, u64)>,
    pub dados: Carga<Analise>,
    pub volumes: Carga<Vec<LinhaVolume>>,
    volumes_vista: u64,
}

impl Default for Estado {
    fn default() -> Self {
        Self {
            modo: ModoAnalise::Computador,
            vista: Vista::Treemap,
            base: BasePercentual::PastaMae,
            cor: ModoCor::Tipo,
            volume: None,
            alvo: AlvoAnalise::Computador,
            carregado: None,
            dados: Carga::default(),
            volumes: Carga::default(),
            volumes_vista: 0,
        }
    }
}

impl Estado {
    /// Troca o modo e leva o alvo para a raiz dele.
    pub fn usar_modo(&mut self, modo: ModoAnalise) {
        self.modo = modo;
        self.alvo = match modo {
            ModoAnalise::Computador => AlvoAnalise::Computador,
            ModoAnalise::Volume => self
                .volume
                .or_else(|| self.primeiro_volume())
                .map_or(AlvoAnalise::Computador, AlvoAnalise::Volume),
        };
    }

    fn primeiro_volume(&self) -> Option<i64> {
        let v = self.volumes.dado.as_ref()?;
        v.iter()
            .find(|v| v.bytes_catalogados.is_some_and(|b| b > 0))
            .or_else(|| v.first())
            .map(|v| v.id)
    }

    /// Recolhe resultados e dispara a consulta do nível quando alvo ou Catálogo mudaram.
    pub fn atualizar(&mut self, leitor: &Leitor, geracao: u64) {
        if self.volumes_vista != geracao {
            self.volumes_vista = geracao;
            self.volumes
                .pedir(leitor.consultar(|c| listar_volumes(c).map_err(erro)));
        }
        if self.volumes.atualizar() && self.modo == ModoAnalise::Volume && self.volume.is_none() {
            self.usar_modo(ModoAnalise::Volume);
        }
        if self.carregado != Some((self.alvo, geracao)) {
            self.carregado = Some((self.alvo, geracao));
            let alvo = self.alvo;
            self.dados
                .pedir(leitor.consultar(move |c| analisar(c, alvo, LIMITE_FILHOS).map_err(erro)));
        }
        self.dados.atualizar();
    }
}

fn alvo_do_no(chave: ChaveNo) -> Option<AlvoAnalise> {
    match chave {
        ChaveNo::Volume(v) => Some(AlvoAnalise::Volume(v)),
        ChaveNo::Pasta(p) => Some(AlvoAnalise::Pasta(p)),
        ChaveNo::Arquivo(_) => None,
    }
}

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

fn agora_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

fn cor_do_no(a: &Analise, n: &NoAnalise, modo: ModoCor, agora: i64) -> Rgb {
    cor_do_bloco(
        modo,
        &AtributosCor {
            categoria: n.categoria.as_deref(),
            finalidade: n.finalidade.as_deref(),
            idade_dias: idade_em_dias(n.mtime_ns, agora),
            profundidade: a.profundidade + u32::from(a.alvo != AlvoAnalise::Computador),
        },
    )
}

/// Um bloco do treemap: um filho ou o agrupamento "outros".
#[derive(Debug, Clone)]
struct Bloco {
    /// Índice em `Analise::filhos`; `None` = "outros".
    filho: Option<usize>,
    bytes: u64,
    /// Só para "outros": quantos itens foram agrupados (sem contar o corte da consulta).
    agrupados: usize,
}

/// Monta os blocos: filhos grandes o bastante e um "outros" com o resto.
fn montar_blocos(a: &Analise, area: Ret) -> Vec<Bloco> {
    let valores: Vec<u64> = a.filhos.iter().map(|f| f.bytes).collect();
    let g = agrupar_pequenos(&valores, a.outros_bytes, area, AREA_MINIMA_PX);
    let mut blocos: Vec<Bloco> = g
        .visiveis
        .iter()
        .map(|&i| Bloco {
            filho: Some(i),
            bytes: valores[i],
            agrupados: 0,
        })
        .collect();
    let resto = g.bytes_agrupados + a.outros_bytes;
    if resto > 0 {
        blocos.push(Bloco {
            filho: None,
            bytes: resto,
            agrupados: g.agrupados.len(),
        });
    }
    blocos
}

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.analise.atualizar(&app.leitor, app.geracao);
    controles(ui, app);
    ui.separator();
    estado_da_carga(ui, &app.analise.dados);
    let est = &app.analise;
    let Some(a) = est.dados.dado.as_ref() else {
        if !est.dados.carregando() && est.dados.erro.is_none() {
            estado_vazio(
                ui,
                "Sem dados",
                PAGINA.resumo,
                "Faça um Full Scan na página Varredura.",
            );
        }
        return;
    };
    if a.filhos.is_empty() && a.bytes_nivel == 0 {
        estado_vazio(
            ui,
            "Nada catalogado neste nível",
            PAGINA.resumo,
            "Faça um Full Scan na página Varredura (ou volte um nível pelo caminho acima).",
        );
        let _ = breadcrumbs(ui, a);
        return;
    }
    let mut destino = breadcrumbs(ui, a);
    let bases = Bases {
        pasta_mae: a.bytes_nivel,
        volume: a.bytes_volume,
        computador: a.bytes_computador,
    };
    let efetiva = bases.efetiva(est.base);
    ui.horizontal(|ui| {
        ui.strong(format!("Total do nível: {}", bytes_humanos(a.bytes_nivel)));
        ui.label(format!(
            "· Base das porcentagens: {} ({})",
            efetiva.nome(),
            bytes_humanos(bases.total(est.base))
        ));
        if a.truncado {
            ui.weak(format!(
                "· mostrando os {LIMITE_FILHOS} maiores; o resto está em \"Outros\""
            ));
        }
    });
    let mut menu = None;
    let destino_visao = match est.vista {
        Vista::Tree => tree_view(ui, a, &bases, est.base, est.cor, &mut menu),
        Vista::Treemap => treemap(ui, a, &bases, est.base, est.cor, &mut menu),
    };
    destino = destino.or(destino_visao);
    if let Some(m) = menu {
        app.tratar_escolha(ui.ctx(), m);
    }
    if let Some(d) = destino {
        app.analise.alvo = d;
    }
}

fn controles(ui: &mut egui::Ui, app: &mut AppDag) {
    let mut modo = app.analise.modo;
    let mut vista = app.analise.vista;
    let mut volume = app.analise.volume;
    ui.horizontal_wrapped(|ui| {
        ui.label("Modo:");
        ui.selectable_value(&mut modo, ModoAnalise::Volume, "Por volume");
        ui.selectable_value(&mut modo, ModoAnalise::Computador, "Por computador")
            .on_hover_text("Soma os volumes do catálogo, sem contar duas vezes um volume montado em mais de um lugar");
        if modo == ModoAnalise::Volume {
            let vols = app.analise.volumes.dado.as_deref().unwrap_or(&[]);
            let atual = volume
                .and_then(|id| vols.iter().find(|v| v.id == id))
                .map_or_else(|| "Escolha…".to_string(), nome_do_volume);
            egui::ComboBox::from_id_salt("analise-volume")
                .selected_text(atual)
                .show_ui(ui, |ui| {
                    for v in vols {
                        ui.selectable_value(&mut volume, Some(v.id), nome_do_volume(v));
                    }
                });
        }
        ui.separator();
        ui.label("Visão:");
        ui.selectable_value(&mut vista, Vista::Tree, "Tree View");
        ui.selectable_value(&mut vista, Vista::Treemap, "Treemap");
        ui.separator();
        ui.label("Barras em % da:");
        egui::ComboBox::from_id_salt("analise-base")
            .selected_text(app.analise.base.nome())
            .show_ui(ui, |ui| {
                for b in BasePercentual::TODAS {
                    ui.selectable_value(&mut app.analise.base, b, b.nome());
                }
            });
        if vista == Vista::Treemap {
            ui.label("Cor por:");
            egui::ComboBox::from_id_salt("analise-cor")
                .selected_text(app.analise.cor.rotulo())
                .show_ui(ui, |ui| {
                    for c in ModoCor::TODOS {
                        ui.selectable_value(&mut app.analise.cor, c, c.rotulo());
                    }
                });
        }
    });
    app.analise.vista = vista;
    let volume_mudou = volume != app.analise.volume;
    app.analise.volume = volume;
    if modo != app.analise.modo || (modo == ModoAnalise::Volume && volume_mudou) {
        app.analise.usar_modo(modo);
    }
}

fn nome_do_volume(v: &LinhaVolume) -> String {
    format!(
        "{} ({})",
        v.rotulo.as_deref().unwrap_or("sem rótulo"),
        v.montagem.as_deref().unwrap_or("sem montagem")
    )
}

/// Caminho clicável do computador até o nível atual.
fn breadcrumbs(ui: &mut egui::Ui, a: &Analise) -> Option<AlvoAnalise> {
    let mut destino = None;
    ui.horizontal_wrapped(|ui| {
        for (i, e) in a.trilha.iter().enumerate() {
            if i > 0 {
                ui.label("›");
            }
            let ultimo = i + 1 == a.trilha.len();
            if ultimo {
                ui.strong(&e.nome);
            } else if ui.link(&e.nome).clicked() {
                destino = Some(e.alvo);
            }
        }
    });
    destino
}

// ---------------------------------------------------------------------- Tree View

const COLUNAS_TREE: [Coluna; 6] = [
    Coluna::esq("Nome", 280.0),
    Coluna::esq("Proporção", 260.0),
    Coluna::dir("Tamanho", 90.0),
    Coluna::dir("Alocado", 90.0),
    Coluna::dir("Arquivos", 80.0),
    Coluna::esq("Modificado (UTC)", 150.0),
];

fn tree_view(
    ui: &mut egui::Ui,
    a: &Analise,
    bases: &Bases,
    base: BasePercentual,
    modo_cor: ModoCor,
    menu: &mut Option<crate::lixeira::Escolha>,
) -> Option<AlvoAnalise> {
    let agora = agora_s();
    let mut destino = None;
    let extra = usize::from(a.outros_bytes > 0);
    let altura = super::altura_livre(ui).max(200.0);
    let efetiva = bases.efetiva(base);
    let mut colunas = COLUNAS_TREE;
    colunas[1].titulo = match efetiva {
        BasePercentual::PastaMae => "Barra: % da pasta-mãe",
        BasePercentual::Volume => "Barra: % do volume",
        BasePercentual::Computador => "Barra: % do computador",
    };
    tabela(
        ui,
        "analise-tree",
        &colunas,
        a.filhos.len() + extra,
        altura,
        |l, i| {
            if let Some(n) = a.filhos.get(i) {
                let rotulo = if n.navegavel() {
                    format!("▸ {}", n.nome)
                } else {
                    n.nome.clone()
                };
                let r = if n.navegavel() {
                    let r = l
                        .selecionavel(false, rotulo)
                        .on_hover_text("Clique para descer");
                    if r.clicked() {
                        destino = alvo_do_no(n.chave);
                    }
                    r
                } else {
                    l.texto(rotulo)
                };
                if let Some(alvo) = crate::lixeira::AlvoMenu::de_no(n) {
                    super::lixeira::menu(&r, &alvo, menu);
                }
                let cor = cor_do_no(a, n, modo_cor, agora);
                barra(l, bases.percentual(n.bytes, base), cor);
                l.texto(bytes_humanos(n.bytes));
                l.texto(n.alocado.map_or_else(|| "—".into(), bytes_humanos));
                l.texto(n.arquivos.map_or_else(|| "—".into(), inteiro_pt_br));
                l.texto(data_ns(n.mtime_ns));
            } else {
                l.texto("Outros (itens menores)");
                barra(l, bases.percentual(a.outros_bytes, base), OUTROS);
                l.texto(bytes_humanos(a.outros_bytes));
                l.texto("—");
                l.texto("—");
                l.texto("—");
            }
        },
    );
    destino
}

/// Barra proporcional com o percentual escrito dentro.
fn barra(l: &mut super::tabela::Linha<'_>, pct: Option<f64>, cor: Rgb) {
    l.ui(|ui| {
        let largura = 240.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(largura, 16.0), Sense::hover());
        let fundo = ui.visuals().extreme_bg_color;
        ui.painter().rect_filled(rect, CornerRadius::same(3), fundo);
        if let Some(p) = pct {
            let w = (rect.width() * (p / 100.0) as f32).max(if p > 0.0 { 2.0 } else { 0.0 });
            let cheio = Rect::from_min_size(rect.min, vec2(w, rect.height()));
            ui.painter()
                .rect_filled(cheio, CornerRadius::same(3), rgb(cor));
        }
        ui.painter().text(
            rect.left_center() + vec2(6.0, 0.0),
            Align2::LEFT_CENTER,
            percentual(pct),
            FontId::proportional(12.0),
            ui.visuals().text_color(),
        );
        resp.on_hover_text(match pct {
            Some(p) => format!("{} da base", percentual(Some(p))),
            None => "Base zero".into(),
        });
    });
}

// ---------------------------------------------------------------------- Treemap

fn nome_do_bloco(a: &Analise, b: &Bloco) -> String {
    match b.filho {
        Some(i) => a.filhos[i].nome.clone(),
        None => "Outros".into(),
    }
}

fn treemap(
    ui: &mut egui::Ui,
    a: &Analise,
    bases: &Bases,
    base: BasePercentual,
    modo_cor: ModoCor,
    menu: &mut Option<crate::lixeira::Escolha>,
) -> Option<AlvoAnalise> {
    let agora = agora_s();
    let h = (super::altura_livre(ui) - 30.0).max(220.0);
    let (area, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let ret = Ret::novo(
        f64::from(area.min.x),
        f64::from(area.min.y),
        f64::from(area.width()),
        f64::from(area.height()),
    );
    let blocos = montar_blocos(a, ret);
    let valores: Vec<u64> = blocos.iter().map(|b| b.bytes).collect();
    let rects = squarify(&valores, ret);
    let pintor = ui.painter_at(area);
    pintor.rect_filled(area, CornerRadius::ZERO, ui.visuals().extreme_bg_color);
    let ponteiro = resp.hover_pos();
    let mut sob_ponteiro = None;
    for (i, (b, r)) in blocos.iter().zip(&rects).enumerate() {
        if r.area() <= 0.0 {
            continue;
        }
        let bloco = Rect::from_min_size(pos2(r.x as f32, r.y as f32), vec2(r.w as f32, r.h as f32));
        if ponteiro.is_some_and(|p| bloco.contains(p)) {
            sob_ponteiro = Some(i);
        }
        let fundo = match b.filho {
            Some(f) => cor_do_no(a, &a.filhos[f], modo_cor, agora),
            None => OUTROS,
        };
        let miolo = bloco.shrink(1.0);
        pintor.rect_filled(miolo, CornerRadius::same(2), rgb(fundo));
        if miolo.width() > 46.0 && miolo.height() > 16.0 {
            let texto = rgb(cor_do_texto(fundo));
            let p = pintor.with_clip_rect(miolo);
            p.text(
                miolo.min + vec2(4.0, 3.0),
                Align2::LEFT_TOP,
                nome_do_bloco(a, b),
                FontId::proportional(12.0),
                texto,
            );
            if miolo.height() > 32.0 {
                p.text(
                    miolo.min + vec2(4.0, 17.0),
                    Align2::LEFT_TOP,
                    bytes_humanos(b.bytes),
                    FontId::proportional(11.0),
                    texto,
                );
            }
        }
    }
    if let Some(i) = sob_ponteiro {
        let r = &rects[i];
        let bloco = Rect::from_min_size(pos2(r.x as f32, r.y as f32), vec2(r.w as f32, r.h as f32));
        pintor.rect_stroke(
            bloco.shrink(1.0),
            CornerRadius::same(2),
            Stroke::new(2.0, ui.visuals().strong_text_color()),
            egui::StrokeKind::Inside,
        );
        let b = &blocos[i];
        resp.clone().on_hover_ui_at_pointer(|ui| {
            tooltip(ui, a, b, bases, base, agora);
        });
    }
    if let Some(f) = sob_ponteiro.and_then(|i| blocos[i].filho)
        && let Some(alvo) = crate::lixeira::AlvoMenu::de_no(&a.filhos[f])
    {
        super::lixeira::menu(&resp, &alvo, menu);
    }
    let destino = if resp.clicked() {
        sob_ponteiro
            .and_then(|i| blocos[i].filho)
            .and_then(|f| alvo_do_no(a.filhos[f].chave))
    } else {
        None
    };
    if sob_ponteiro
        .and_then(|i| blocos[i].filho)
        .is_some_and(|f| a.filhos[f].navegavel())
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    legenda_de_cores(ui, a, modo_cor);
    destino
}

fn tooltip(
    ui: &mut egui::Ui,
    a: &Analise,
    b: &Bloco,
    bases: &Bases,
    base: BasePercentual,
    agora: i64,
) {
    let efetiva = bases.efetiva(base);
    let pct = |bytes| {
        format!(
            "{} da {}",
            percentual(bases.percentual(bytes, base)),
            efetiva.nome()
        )
    };
    match b.filho {
        Some(i) => {
            let n = &a.filhos[i];
            ui.strong(&n.nome);
            ui.label(format!("{} — {}", bytes_humanos(n.bytes), pct(n.bytes)));
            if let Some(al) = n.alocado {
                ui.label(format!("Alocado: {}", bytes_humanos(al)));
            }
            if let Some(c) = &n.categoria {
                ui.label(format!("Tipo: {}", rotulo_da_categoria(c)));
            }
            if let Some(f) = &n.finalidade {
                ui.label(format!("Finalidade: {}", rotulo_da_finalidade(f)));
            }
            if let Some(d) = idade_em_dias(n.mtime_ns, agora) {
                ui.label(format!(
                    "Modificado há {} dia(s) ({})",
                    inteiro_pt_br(d),
                    data_ns(n.mtime_ns)
                ));
            }
            if let Some(q) = n.arquivos {
                ui.label(format!("{} arquivo(s)", inteiro_pt_br(q)));
            }
            ui.weak(if n.navegavel() {
                "Clique para descer"
            } else {
                "Arquivo"
            });
        }
        None => {
            ui.strong("Outros");
            ui.label(format!("{} — {}", bytes_humanos(b.bytes), pct(b.bytes)));
            ui.label(format!(
                "{} item(ns) pequeno(s) demais para um bloco{}",
                b.agrupados,
                if a.truncado {
                    ", mais os que ficaram fora do limite da consulta"
                } else {
                    ""
                }
            ));
        }
    }
}

fn legenda_de_cores(ui: &mut egui::Ui, a: &Analise, modo: ModoCor) {
    let presentes = |nome: &str| -> bool {
        match modo {
            ModoCor::Tipo => a
                .filhos
                .iter()
                .any(|f| f.categoria.as_deref().map(rotulo_da_categoria).as_deref() == Some(nome)),
            ModoCor::Finalidade => a.filhos.iter().any(|f| {
                f.finalidade.as_deref().map(rotulo_da_finalidade).as_deref() == Some(nome)
            }),
            _ => true,
        }
    };
    ui.horizontal_wrapped(|ui| {
        ui.weak(format!("Cor por {}:", modo.rotulo().to_lowercase()));
        for (rotulo, cor) in legenda(modo) {
            if !presentes(&rotulo) {
                continue;
            }
            let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(2), rgb(cor));
            ui.label(rotulo);
        }
        let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
        ui.painter()
            .rect_filled(r, CornerRadius::same(2), rgb(OUTROS));
        ui.label("Outros / sem informação");
    });
}
