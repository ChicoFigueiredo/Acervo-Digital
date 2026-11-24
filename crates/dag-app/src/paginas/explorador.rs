//! Página Explorador: árvore preguiçosa de pastas (cada expansão lê uma página de filhos),
//! tabela paginada dos arquivos da pasta selecionada e breadcrumb. Nada é carregado em massa.

use super::tabela::{Coluna, tabela};
use super::{DefAcao, DefGrupo, DefPagina, cabecalho, estado_da_carga, estado_vazio};
use crate::app::AppDag;
use crate::cores::{rotulo_da_categoria, rotulo_da_finalidade};
use crate::formato::{bytes_humanos, data_ns, inteiro_pt_br};
use crate::leitor::{Carga, Leitor, Pendente, erro};
use bancada_casca::egui;
use dag_store::consulta_gui::{
    CursorPasta, InfoPasta, LinhaArquivo, NoPasta, PaginaArquivos, PaginaPastas, arquivos_da_pasta,
    raizes, subpastas, trilha_da_pasta,
};

/// Linhas por página de pastas e de arquivos (o teto da consulta é 500).
const PAGINA_PASTAS: usize = 200;
const PAGINA_ARQUIVOS: usize = 200;

pub static PAGINA: DefPagina = DefPagina {
    id: "explorador",
    rotulo: "Explorador",
    keytip: "E",
    icone: "explorador",
    grupos: &[
        DefGrupo {
            id: "explorador.navegar",
            rotulo: "Navegar",
            acoes: &[
                DefAcao {
                    id: "explorador.expandir",
                    rotulo: "Expandir",
                    dica: "Expande a pasta selecionada",
                },
                DefAcao {
                    id: "explorador.recolher",
                    rotulo: "Recolher",
                    dica: "Recolhe toda a árvore",
                },
                DefAcao {
                    id: "explorador.filtrar",
                    rotulo: "Filtrar",
                    dica: "Filtra a tabela de arquivos",
                },
            ],
        },
        DefGrupo {
            id: "explorador.item",
            rotulo: "Item",
            acoes: &[DefAcao {
                id: "explorador.propriedades",
                rotulo: "Propriedades",
                dica: "Tamanho lógico e alocado, categoria, MIME, datas e atributos",
            }],
        },
        DefGrupo {
            id: "explorador.saida",
            rotulo: "Saída",
            acoes: &[DefAcao {
                id: "explorador.exportar",
                rotulo: "Exportar relatório",
                dica: "Exporta a visão atual",
            }],
        },
    ],
    resumo: "Painel de pastas e arquivos catalogados: tamanhos, categoria, finalidade e datas.",
    desenhar,
};

/// Uma pasta da árvore, com seus filhos carregados sob demanda.
#[derive(Debug)]
pub struct NoArvore {
    pub pasta: NoPasta,
    pub expandido: bool,
    pub filhos: Vec<NoArvore>,
    pub proximo: Option<CursorPasta>,
    carga: Option<Pendente<PaginaPastas>>,
    pub carregado: bool,
    pub erro: Option<String>,
}

impl NoArvore {
    #[must_use]
    pub fn novo(pasta: NoPasta) -> Self {
        Self {
            pasta,
            expandido: false,
            filhos: Vec::new(),
            proximo: None,
            carga: None,
            carregado: false,
            erro: None,
        }
    }

    /// Pede a próxima página de filhos (a primeira, se ainda não carregou).
    pub fn carregar_mais(&mut self, leitor: &Leitor) {
        if self.carga.is_some() {
            return;
        }
        let id = self.pasta.id;
        let cursor = if self.carregado { self.proximo } else { None };
        self.carga =
            Some(leitor.consultar(move |c| subpastas(c, id, cursor, PAGINA_PASTAS).map_err(erro)));
    }

    pub fn expandir(&mut self, leitor: &Leitor) {
        self.expandido = true;
        if !self.carregado {
            self.carregar_mais(leitor);
        }
    }

    fn atualizar(&mut self) {
        let Some(p) = self.carga.as_mut() else { return };
        if let Some(r) = p.tentar() {
            self.carga = None;
            match r {
                Ok(pg) => {
                    self.filhos.extend(pg.itens.into_iter().map(NoArvore::novo));
                    self.proximo = pg.proximo;
                    self.carregado = true;
                    self.erro = None;
                }
                Err(e) => self.erro = Some(e),
            }
        }
    }

    fn recolher(&mut self) {
        self.expandido = false;
        self.filhos.iter_mut().for_each(NoArvore::recolher);
    }

    /// Total de nós carregados na subárvore (testes e diagnóstico).
    #[must_use]
    pub fn contar(&self) -> usize {
        1 + self.filhos.iter().map(NoArvore::contar).sum::<usize>()
    }
}

/// Estado da página.
#[derive(Debug)]
pub struct Estado {
    pub raizes: Carga<Vec<NoPasta>>,
    pub arvore: Vec<NoArvore>,
    vista: u64,
    pub selecionada: Option<NoPasta>,
    pub trilha: Carga<Vec<InfoPasta>>,
    pub arquivos: Carga<PaginaArquivos>,
    pub propriedades: bool,
}

impl Default for Estado {
    fn default() -> Self {
        Self {
            raizes: Carga::default(),
            arvore: Vec::new(),
            vista: 0,
            selecionada: None,
            trilha: Carga::default(),
            arquivos: Carga::default(),
            propriedades: true,
        }
    }
}

fn buscar(nos: &mut [NoArvore], id: i64) -> Option<&mut NoArvore> {
    for n in nos {
        if n.pasta.id == id {
            return Some(n);
        }
        if let Some(f) = buscar(&mut n.filhos, id) {
            return Some(f);
        }
    }
    None
}

impl Estado {
    /// Recolhe resultados e recarrega a árvore quando o Catálogo mudou.
    pub fn atualizar(&mut self, leitor: &Leitor, geracao: u64) {
        if self.vista != geracao {
            self.vista = geracao;
            self.raizes
                .pedir(leitor.consultar(|c| raizes(c).map_err(erro)));
            // A pasta selecionada pode ter mudado de tamanho: relê os arquivos dela.
            if let Some(p) = self.selecionada.clone() {
                self.selecionar(leitor, p);
            }
        }
        if self.raizes.atualizar() {
            self.arvore = self
                .raizes
                .dado
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(NoArvore::novo)
                .collect();
        }
        for n in &mut self.arvore {
            atualizar_recursivo(n);
        }
        self.trilha.atualizar();
        self.arquivos.atualizar_com(|dado, nova| match dado {
            Some(d) => {
                d.itens.extend(nova.itens);
                d.proximo = nova.proximo;
            }
            None => *dado = Some(nova),
        });
    }

    /// Seleciona a pasta: lê o breadcrumb e a primeira página de arquivos.
    pub fn selecionar(&mut self, leitor: &Leitor, pasta: NoPasta) {
        let id = pasta.id;
        self.selecionada = Some(pasta);
        self.arquivos.limpar();
        self.arquivos.pedir(
            leitor
                .consultar(move |c| arquivos_da_pasta(c, id, None, PAGINA_ARQUIVOS).map_err(erro)),
        );
        self.trilha
            .pedir(leitor.consultar(move |c| trilha_da_pasta(c, id).map_err(erro)));
    }

    fn mais_arquivos(&mut self, leitor: &Leitor) {
        let (Some(p), Some(cursor)) = (
            self.selecionada.as_ref(),
            self.arquivos.dado.as_ref().and_then(|d| d.proximo),
        ) else {
            return;
        };
        if self.arquivos.carregando() {
            return;
        }
        let id = p.id;
        self.arquivos.pedir(leitor.consultar(move |c| {
            arquivos_da_pasta(c, id, Some(cursor), PAGINA_ARQUIVOS).map_err(erro)
        }));
    }

    /// Expande a pasta selecionada (se está na árvore carregada).
    pub fn expandir_selecionada(&mut self, leitor: &Leitor) {
        if let Some(id) = self.selecionada.as_ref().map(|p| p.id)
            && let Some(n) = buscar(&mut self.arvore, id)
        {
            n.expandir(leitor);
        }
    }

    pub fn recolher_tudo(&mut self) {
        self.arvore.iter_mut().for_each(NoArvore::recolher);
    }
}

fn atualizar_recursivo(n: &mut NoArvore) {
    n.atualizar();
    for f in &mut n.filhos {
        atualizar_recursivo(f);
    }
}

fn desenhar_no(
    ui: &mut egui::Ui,
    no: &mut NoArvore,
    nivel: usize,
    leitor: &Leitor,
    selecionada: Option<i64>,
    escolhida: &mut Option<NoPasta>,
    menu: &mut Option<crate::lixeira::Escolha>,
) {
    ui.horizontal(|ui| {
        ui.add_space(nivel as f32 * 14.0);
        if no.pasta.tem_subpastas {
            let seta = if no.expandido { "▾" } else { "▸" };
            if ui.small_button(seta).clicked() {
                if no.expandido {
                    no.expandido = false;
                } else {
                    no.expandir(leitor);
                }
            }
        } else {
            ui.add_space(20.0);
        }
        let rotulo = format!(
            "{}  ({})",
            no.pasta.nome,
            bytes_humanos(no.pasta.bytes_logicos)
        );
        let r = ui
            .selectable_label(selecionada == Some(no.pasta.id), rotulo)
            .on_hover_text(no.pasta.caminho.clone());
        if r.clicked() {
            *escolhida = Some(no.pasta.clone());
        }
        super::lixeira::menu(&r, &crate::lixeira::AlvoMenu::de_pasta(&no.pasta), menu);
        if r.double_clicked() && no.pasta.tem_subpastas {
            no.expandir(leitor);
        }
    });
    if no.expandido {
        for f in &mut no.filhos {
            desenhar_no(ui, f, nivel + 1, leitor, selecionada, escolhida, menu);
        }
        if no.carga.is_some() {
            ui.horizontal(|ui| {
                ui.add_space((nivel + 1) as f32 * 14.0 + 20.0);
                ui.spinner();
            });
        }
        if let Some(e) = &no.erro {
            ui.colored_label(ui.visuals().warn_fg_color, e);
        }
        if no.proximo.is_some() && no.carga.is_none() {
            let mut mais = false;
            ui.horizontal(|ui| {
                ui.add_space((nivel + 1) as f32 * 14.0 + 20.0);
                mais = ui.small_button("Carregar mais pastas…").clicked();
            });
            if mais {
                no.carregar_mais(leitor);
            }
        }
    }
}

const COLUNAS: [Coluna; 10] = [
    Coluna::esq("Nome", 260.0),
    Coluna::dir("Lógico", 85.0),
    Coluna::dir("Alocado", 85.0),
    Coluna::esq("Categoria", 105.0),
    Coluna::esq("MIME", 170.0),
    Coluna::esq("Finalidade", 105.0),
    Coluna::dir("Conf.", 50.0),
    Coluna::esq("Modificado (UTC)", 150.0),
    Coluna::esq("Criado (UTC)", 150.0),
    Coluna::esq("Indicadores", 140.0),
];

fn indicadores(a: &LinhaArquivo) -> String {
    let mut v = Vec::new();
    if a.tem_hash {
        v.push("hash");
    }
    if a.duplicado {
        v.push("duplicado");
    }
    match a.marcacao.as_deref() {
        Some("backup") => v.push("backup"),
        Some("delecao") => v.push("deleção"),
        _ => {}
    }
    if a.tipo != "arquivo" {
        v.push(match a.tipo.as_str() {
            "symlink" => "link",
            _ => "especial",
        });
    }
    v.join(" · ")
}

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.explorador.atualizar(&app.leitor, app.geracao);
    estado_da_carga(ui, &app.explorador.raizes);
    if app.explorador.arvore.is_empty()
        && app.explorador.raizes.erro.is_none()
        && !app.explorador.raizes.carregando()
    {
        estado_vazio(
            ui,
            "Nada catalogado ainda",
            PAGINA.resumo,
            "Faça um Full Scan na página Varredura para popular a árvore.",
        );
        return;
    }
    let altura = super::altura_livre(ui).max(200.0);
    let leitor = &app.leitor;
    let est = &mut app.explorador;
    let mut escolhida = None;
    let mut menu = None;
    let mut carregar_mais_arquivos = false;
    let mut ir_para: Option<i64> = None;
    ui.horizontal_top(|ui| {
        ui.allocate_ui(egui::vec2(330.0, altura), |ui| {
            ui.set_min_width(330.0);
            ui.strong("Pastas");
            egui::ScrollArea::both()
                .id_salt("explorador-arvore")
                .auto_shrink([false, false])
                .max_height(altura - 28.0)
                .show(ui, |ui| {
                    let sel = est.selecionada.as_ref().map(|p| p.id);
                    for n in &mut est.arvore {
                        desenhar_no(ui, n, 0, leitor, sel, &mut escolhida, &mut menu);
                    }
                });
        });
        ui.separator();
        ui.vertical(|ui| {
            let Some(p) = est.selecionada.as_ref() else {
                ui.add_space(20.0);
                ui.weak("Selecione uma pasta na árvore para ver os arquivos dela.");
                return;
            };
            // Breadcrumb.
            ui.horizontal_wrapped(|ui| {
                if let Some(t) = est.trilha.dado.as_ref() {
                    for (i, e) in t.iter().enumerate() {
                        if i > 0 {
                            ui.label("›");
                        }
                        if ui.link(&e.nome).on_hover_text(&e.caminho).clicked() {
                            ir_para = Some(e.id);
                        }
                    }
                } else {
                    ui.label(&p.caminho);
                }
            });
            if est.propriedades {
                ui.weak(format!(
                    "{} — lógico {} · alocado {} · {} arquivo(s) · {} pasta(s) · agregado: {}",
                    p.nome,
                    bytes_humanos(p.bytes_logicos),
                    p.bytes_alocados
                        .map_or_else(|| "indisponível".into(), bytes_humanos),
                    inteiro_pt_br(p.arquivos),
                    inteiro_pt_br(p.pastas),
                    p.estado_agregado.as_deref().unwrap_or("pendente"),
                ));
            }
            estado_da_carga(ui, &est.arquivos);
            if let Some(pg) = est.arquivos.dado.as_ref() {
                if pg.itens.is_empty() {
                    ui.weak("Esta pasta não tem arquivos diretos.");
                } else {
                    let restante = ui.available_height() - 34.0;
                    tabela(
                        ui,
                        "explorador-arquivos",
                        &COLUNAS,
                        pg.itens.len(),
                        restante,
                        |l, i| {
                            let a = &pg.itens[i];
                            let r = l.texto(a.nome.clone());
                            super::lixeira::menu(
                                &r,
                                &crate::lixeira::AlvoMenu::de_arquivo(a),
                                &mut menu,
                            );
                            l.texto(bytes_humanos(a.bytes_logicos));
                            l.texto(a.bytes_alocados.map_or_else(|| "—".into(), bytes_humanos));
                            l.texto(
                                a.categoria
                                    .as_deref()
                                    .map_or_else(|| "—".into(), rotulo_da_categoria),
                            );
                            l.texto(a.mime.clone().unwrap_or_else(|| "—".into()));
                            l.texto(
                                a.finalidade
                                    .as_deref()
                                    .map_or_else(|| "—".into(), rotulo_da_finalidade),
                            );
                            l.texto(
                                a.confianca
                                    .map_or_else(|| "—".into(), |c| format!("{:.0}%", c * 100.0)),
                            );
                            l.texto(data_ns(a.mtime_ns));
                            l.texto(data_ns(a.birthtime_ns));
                            l.texto(indicadores(a));
                        },
                    );
                    ui.horizontal(|ui| {
                        ui.weak(format!(
                            "{} arquivo(s) carregado(s)",
                            inteiro_pt_br(pg.itens.len() as u64)
                        ));
                        if pg.proximo.is_some()
                            && ui
                                .add_enabled(
                                    !est.arquivos.carregando(),
                                    egui::Button::new("Carregar mais arquivos"),
                                )
                                .clicked()
                        {
                            carregar_mais_arquivos = true;
                        }
                    });
                }
            }
        });
    });
    if let Some(p) = escolhida {
        est.selecionar(&app.leitor, p);
    }
    if let Some(id) = ir_para {
        let na_arvore = buscar(&mut est.arvore, id).map(|n| n.pasta.clone());
        let da_trilha = est
            .trilha
            .dado
            .as_ref()
            .and_then(|t| t.iter().find(|i| i.id == id))
            .map(|i| NoPasta {
                id: i.id,
                volume_id: i.volume_id,
                nome: i.nome.clone(),
                caminho: i.caminho.clone(),
                bytes_logicos: 0,
                bytes_alocados: None,
                arquivos: 0,
                pastas: 0,
                estado_agregado: None,
                tem_subpastas: true,
                mtime_ns: None,
            });
        let atual = est.selecionada.as_ref().map(|p| p.id);
        if let Some(p) = na_arvore.or(da_trilha).filter(|p| Some(p.id) != atual) {
            est.selecionar(&app.leitor, p);
        }
    }
    if carregar_mais_arquivos {
        est.mais_arquivos(&app.leitor);
    }
    if let Some(m) = menu {
        app.tratar_escolha(ui.ctx(), m);
    }
}
