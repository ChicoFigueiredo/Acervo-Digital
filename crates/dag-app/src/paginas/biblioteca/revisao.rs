//! Biblioteca > Revisão (B-12, WOW-08): lista de pendentes e painel lado a lado com o que foi
//! extraído do arquivo, a sugestão (com confiança, origem e evidências) e o livro homologado
//! parecido. Aceitar, corrigir e rejeitar por campo ou em lote, e desfazer o último lote.
//!
//! Todo texto que vem dos livros (metadados, trechos, evidências) é exibido como texto puro.

use std::collections::BTreeSet;

use bancada_casca::egui;
use dag_core::{AcaoRevisao, CampoLadoALado, ItemRevisaoInfo};

use super::comandos::{self, ChaveCampo, campo_pendente};
use super::texto::{
    faixa_de_confianca, formato_do_caminho, nome_do_arquivo, percentual_de_confianca, rotulo_campo,
    texto_de_sugestao, texto_inicial_de_correcao, texto_puro,
};
use super::{AcaoBib, Consulta, acao, erro_da_acao, estado_da_consulta, quadro};
use crate::app::AppDag;
use crate::formato::{bytes_humanos, hash_curto, inteiro_pt_br};
use crate::paginas::tabela::{Chave, Coluna, nota_ordem_local, tabela};
use crate::paginas::{DefGrupo, DefPagina, cabecalho, estado_vazio};

pub static PAGINA: DefPagina = DefPagina {
    id: "biblioteca-revisao",
    rotulo: "Revisão",
    keytip: "R",
    icone: "revisao",
    grupos: &[DefGrupo {
        id: "biblioteca.revisao",
        rotulo: "Revisão",
        acoes: &[
            acao(
                "biblioteca.revisao_aceitar_altas",
                "Aceitar confiança alta",
                "Aceita, em um lote, os campos pendentes de confiança alta da lista carregada",
            ),
            acao(
                "biblioteca.revisao_desfazer",
                "Desfazer último lote",
                "Devolve a pendente as decisões do último lote desta sessão",
            ),
            acao(
                "biblioteca.revisao_atualizar",
                "Atualizar revisão",
                "Relê a lista de pendentes",
            ),
        ],
    }],
    resumo: "Compare o que o arquivo diz, o que o DAG sugere e o que você já tem; decida por campo ou em lote.",
    desenhar,
};

/// Estado de tela da página.
#[derive(Debug, Default)]
pub struct Ui {
    /// Arquivo (item) aberto no painel.
    pub selecionado: Option<i64>,
    /// Campos marcados para decidir em lote.
    pub escolhidos: BTreeSet<ChaveCampo>,
    /// Campo cujas evidências aparecem.
    pub foco: Option<ChaveCampo>,
    /// Campo em correção e o texto digitado.
    pub corrigindo: Option<ChaveCampo>,
    pub texto: String,
}

impl Ui {
    /// Depois de uma nova lista: descarta o que não existe mais.
    pub fn reconciliar(&mut self, itens: &[ItemRevisaoInfo]) {
        let existe = |k: &ChaveCampo| {
            itens
                .iter()
                .any(|i| i.file_id == k.0 && i.campos.iter().any(|c| c.campo == k.1))
        };
        self.escolhidos.retain(|k| existe(k));
        if self.foco.as_ref().is_some_and(|k| !existe(k)) {
            self.foco = None;
        }
        if self.corrigindo.as_ref().is_some_and(|k| !existe(k)) {
            self.corrigindo = None;
        }
        if self
            .selecionado
            .is_some_and(|s| !itens.iter().any(|i| i.file_id == s))
        {
            self.selecionado = None;
        }
    }
}

pub(super) fn aceitar_altas(app: &mut AppDag) {
    let Some(r) = &app.biblioteca.revisao else {
        return;
    };
    let itens = comandos::aceitar_confianca_alta(&r.itens);
    if itens.is_empty() {
        return;
    }
    app.bib_acao(AcaoBib::Decidir, comandos::decidir(itens));
}

pub(super) fn desfazer_ultimo(app: &mut AppDag) {
    if let Some(l) = app.biblioteca.ultimo_lote.clone() {
        app.bib_acao(AcaoBib::Desfazer, comandos::desfazer(&l));
    }
}

const COLUNAS_ITENS: [Coluna; 3] = [
    Coluna::esq("Arquivo", 220.0),
    Coluna::esq("Formato", 65.0),
    Coluna::dir("Pendentes", 75.0),
];

const COLUNAS_CAMPOS: [Coluna; 8] = [
    Coluna::esq("", 26.0).sem_ordem(),
    Coluna::esq("Campo", 100.0),
    Coluna::esq("Extraído do arquivo", 190.0),
    Coluna::esq("Sugestão do DAG", 210.0),
    Coluna::esq("Confiança", 100.0),
    Coluna::esq("Homologado parecido", 190.0),
    Coluna::esq("Estado", 85.0),
    Coluna::esq("Decidir", 215.0).sem_ordem(),
];

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.bib_quadro(ui.ctx());
    app.bib_carregar(&[Consulta::Revisao]);
    barra(ui, app);
    ui.separator();
    estado_da_consulta(ui, &app.biblioteca, Consulta::Revisao);
    let Some(dados) = app.biblioteca.revisao.as_ref() else {
        return;
    };
    if dados.itens.is_empty() {
        estado_vazio(
            ui,
            "Nada para revisar",
            PAGINA.resumo,
            "Processe a fila na Visão Geral: o que o DAG não tem certeza aparece aqui.",
        );
        return;
    }
    let altura = crate::paginas::altura_livre(ui).max(380.0);
    let mut escolhido = None;
    let mut mais = false;
    ui.horizontal_top(|ui| {
        ui.allocate_ui(egui::vec2(ui.available_width() * 0.30, altura), |ui| {
            let Some(d) = app.biblioteca.revisao.as_ref() else {
                return;
            };
            let sel = app.biblioteca.rev.selecionado;
            tabela(
                ui,
                "bib-rev-itens",
                &COLUNAS_ITENS,
                d.itens.len(),
                altura - 60.0,
                |c, i| {
                    let it = &d.itens[i];
                    match c {
                        0 => Chave::texto(it.caminho.as_deref().map_or("", nome_do_arquivo)),
                        1 => Chave::opt_texto(it.formato.as_deref()),
                        _ => Chave::num(it.pendentes),
                    }
                },
                |l, i| {
                    let it = &d.itens[i];
                    let nome = it.caminho.as_deref().map_or_else(
                        || format!("arquivo {}", it.file_id),
                        |c| texto_puro(nome_do_arquivo(c), 80),
                    );
                    if l.selecionavel(sel == Some(it.file_id), nome).clicked() {
                        escolhido = Some(it.file_id);
                    }
                    l.texto(it.formato.clone().unwrap_or_else(|| "—".into()));
                    l.texto(it.pendentes.to_string());
                },
            );
            nota_ordem_local(ui, "bib-rev-itens");
            ui.horizontal(|ui| {
                ui.weak(format!(
                    "{} de {} pendente(s)",
                    inteiro_pt_br(d.itens.len() as u64),
                    inteiro_pt_br(u64::try_from(d.total_pendentes).unwrap_or(0))
                ));
                if d.proximo.is_some()
                    && ui
                        .add_enabled(
                            !app.biblioteca.carregando(Consulta::Revisao),
                            egui::Button::new("Carregar mais"),
                        )
                        .clicked()
                {
                    mais = true;
                }
            });
        });
        ui.separator();
        ui.vertical(|ui| painel(ui, app, altura));
    });
    if let Some(f) = escolhido
        && app.biblioteca.rev.selecionado != Some(f)
    {
        app.biblioteca.rev.selecionado = Some(f);
        app.biblioteca.rev.foco = None;
        app.biblioteca.rev.corrigindo = None;
    }
    if mais {
        let cursor = app
            .biblioteca
            .revisao
            .as_ref()
            .and_then(|d| d.proximo.clone());
        app.bib_pedir(Consulta::Revisao, cursor);
    }
}

fn barra(ui: &mut egui::Ui, app: &mut AppDag) {
    let sem_escrita = app.motivo_sem_escrita_config();
    let altas = app
        .biblioteca
        .revisao
        .as_ref()
        .map_or(0, |r| comandos::aceitar_confianca_alta(&r.itens).len());
    let total = app.biblioteca.revisao.as_ref().map(|r| r.total_pendentes);
    let lote = app.biblioteca.ultimo_lote.clone();
    let mut aceitar_ = false;
    let mut desfazer_ = false;
    ui.horizontal_wrapped(|ui| {
        if let Some(t) = total {
            ui.strong(format!(
                "{} item(ns) pendente(s) de revisão",
                inteiro_pt_br(u64::try_from(t).unwrap_or(0))
            ));
        }
        let b = ui.add_enabled(
            sem_escrita.is_none() && altas > 0,
            egui::Button::new(format!("Aceitar confiança alta ({altas})")),
        );
        aceitar_ = match sem_escrita {
            Some(m) => b.on_disabled_hover_text(m),
            None => b.on_hover_text(
                "Um lote com todos os campos pendentes de confiança alta da lista carregada; dá para desfazer",
            ),
        }
        .clicked();
        let b = ui.add_enabled(
            sem_escrita.is_none() && lote.is_some(),
            egui::Button::new("Desfazer último lote"),
        );
        desfazer_ = b
            .on_disabled_hover_text("Nenhum lote nesta sessão")
            .on_hover_text("Devolve a pendente as decisões do último lote")
            .clicked();
    });
    if let Some(t) = &app.biblioteca.ultima_decisao {
        ui.weak(t);
    }
    erro_da_acao(ui, &app.biblioteca, &[AcaoBib::Decidir, AcaoBib::Desfazer]);
    if aceitar_ {
        aceitar_altas(app);
    }
    if desfazer_ {
        desfazer_ultimo(app);
    }
}

/// O que o painel quer fazer neste quadro (aplicado depois do desenho).
#[derive(Default)]
struct Pedido {
    lote: Vec<dag_core::ItemDecisao>,
    alternar: Option<ChaveCampo>,
    foco: Option<ChaveCampo>,
    iniciar_correcao: Option<(ChaveCampo, String)>,
    aplicar_correcao: Option<dag_core::ItemDecisao>,
    cancelar_correcao: bool,
    aceitar_selecionados: bool,
    rejeitar_selecionados: bool,
}

fn painel(ui: &mut egui::Ui, app: &mut AppDag, altura: f32) {
    let item = app.biblioteca.revisao.as_ref().and_then(|d| {
        d.itens
            .iter()
            .find(|i| Some(i.file_id) == app.biblioteca.rev.selecionado)
            .cloned()
    });
    let Some(it) = item else {
        ui.add_space(20.0);
        ui.weak(
            "Selecione um arquivo da lista para comparar o extraído, a sugestão e o homologado.",
        );
        return;
    };
    let mut p = Pedido::default();
    cabecalho_do_item(ui, &it);
    let escolhidos = app.biblioteca.rev.escolhidos.clone();
    let corrigindo = app.biblioteca.rev.corrigindo.clone();
    let foco = app.biblioteca.rev.foco.clone();
    let sem_escrita = app.motivo_sem_escrita_config();
    tabela(
        ui,
        "bib-rev-campos",
        &COLUNAS_CAMPOS,
        it.campos.len(),
        (altura - 330.0).max(130.0),
        |c, i| {
            let f = &it.campos[i];
            match c {
                0 => Chave::num(i128::from(
                    escolhidos.contains(&(it.file_id, f.campo.clone())),
                )),
                1 => Chave::texto(&rotulo_campo(&f.campo)),
                2 => Chave::opt_texto(f.extraido.as_deref()),
                3 => Chave::opt_texto(
                    texto_de_sugestao(f.sugerido.as_deref(), &f.sugerido_lista).as_deref(),
                ),
                4 => Chave::opt_real(f.confianca.map(f64::from)),
                5 => Chave::opt_texto(f.homologado.as_deref()),
                _ => Chave::texto(&f.estado),
            }
        },
        |l, i| {
            let f = &it.campos[i];
            let chave: ChaveCampo = (it.file_id, f.campo.clone());
            let pendente = campo_pendente(&f.estado);
            l.ui(|ui| {
                let mut m = escolhidos.contains(&chave);
                if ui
                    .add_enabled(pendente, egui::Checkbox::without_text(&mut m))
                    .on_hover_text("Marcar para decidir em lote")
                    .changed()
                {
                    p.alternar = Some(chave.clone());
                }
            });
            if l.selecionavel(foco.as_ref() == Some(&chave), rotulo_campo(&f.campo))
                .on_hover_text("Ver evidências e proveniência")
                .clicked()
            {
                p.foco = Some(chave.clone());
            }
            l.texto(texto_puro(f.extraido.as_deref().unwrap_or("—"), 120));
            l.texto(texto_puro(
                &texto_de_sugestao(f.sugerido.as_deref(), &f.sugerido_lista)
                    .unwrap_or_else(|| "—".into()),
                120,
            ));
            l.texto(format!(
                "{} {}{}",
                faixa_de_confianca(f.faixa.as_deref(), f.confianca),
                percentual_de_confianca(f.confianca),
                if f.precisa_revisao { " ⚠" } else { "" }
            ));
            l.texto(texto_puro(f.homologado.as_deref().unwrap_or("—"), 120));
            l.texto(f.estado.clone());
            l.ui(|ui| {
                if pendente {
                    let ativo = sem_escrita.is_none();
                    if ui
                        .add_enabled(ativo, egui::Button::new("Aceitar").small())
                        .clicked()
                    {
                        p.lote.push(comandos::aceitar(it.file_id, &f.campo));
                    }
                    if ui
                        .add_enabled(ativo, egui::Button::new("Corrigir…").small())
                        .clicked()
                    {
                        p.iniciar_correcao = Some((
                            chave.clone(),
                            texto_inicial_de_correcao(
                                f.sugerido.as_deref(),
                                &f.sugerido_lista,
                                f.extraido.as_deref(),
                            ),
                        ));
                    }
                    if ui
                        .add_enabled(ativo, egui::Button::new("Rejeitar").small())
                        .clicked()
                    {
                        p.lote.push(comandos::rejeitar(it.file_id, &f.campo));
                    }
                }
            });
        },
    );
    nota_ordem_local(ui, "bib-rev-campos");
    lote_do_item(ui, app, &it, &mut p, sem_escrita.is_none());
    editor_de_correcao(ui, app, &it, corrigindo.as_ref(), &mut p);
    if let Some(chave) = foco
        && let Some(f) = it
            .campos
            .iter()
            .find(|c| (it.file_id, c.campo.clone()) == chave)
    {
        proveniencia(ui, f);
    }
    capa(ui, &it);
    aplicar(app, &it, p);
}

fn cabecalho_do_item(ui: &mut egui::Ui, it: &ItemRevisaoInfo) {
    quadro(ui, "Arquivo em revisão", |ui| {
        ui.label(texto_puro(
            it.caminho.as_deref().unwrap_or("(sem caminho)"),
            300,
        ));
        ui.weak(format!(
            "{} · {} · hash {} · {} campo(s) pendente(s){}",
            it.formato
                .clone()
                .unwrap_or_else(|| formato_do_caminho(it.caminho.as_deref().unwrap_or(""))),
            it.tamanho.map_or_else(|| "—".into(), bytes_humanos),
            it.hash.as_deref().map_or_else(|| "—".into(), hash_curto),
            it.pendentes,
            match (it.obra_id, it.edicao_id) {
                (Some(o), Some(e)) => format!(" · obra {o}, edição {e}"),
                _ => String::new(),
            }
        ));
        for a in &it.avisos {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("Atenção: {}", texto_puro(a, 300)),
            );
        }
        if !it.homologados_parecidos.is_empty() {
            ui.separator();
            ui.weak("Livros homologados parecidos:");
            for h in &it.homologados_parecidos {
                ui.label(format!(
                    "• {} — {} ({}{})",
                    texto_puro(&h.livro.titulo, 120),
                    texto_puro(&h.livro.autores.join(" & "), 80),
                    texto_puro(&h.motivo, 80),
                    h.relacao
                        .as_deref()
                        .map(|r| format!(", {}", texto_puro(r, 40)))
                        .unwrap_or_default()
                ));
            }
        }
    });
}

fn lote_do_item(ui: &mut egui::Ui, app: &AppDag, it: &ItemRevisaoInfo, p: &mut Pedido, pode: bool) {
    let n = app.biblioteca.rev.escolhidos.len();
    let pendentes = it
        .campos
        .iter()
        .filter(|c| campo_pendente(&c.estado))
        .count();
    ui.horizontal_wrapped(|ui| {
        let b = ui.add_enabled(
            pode && n > 0,
            egui::Button::new(format!("Aceitar selecionados ({n})")),
        );
        p.aceitar_selecionados = b.clicked();
        let b = ui.add_enabled(
            pode && n > 0,
            egui::Button::new(format!("Rejeitar selecionados ({n})")),
        );
        p.rejeitar_selecionados = b.clicked();
        if ui
            .add_enabled(
                pode && pendentes > 0,
                egui::Button::new(format!("Aceitar todos deste arquivo ({pendentes})")),
            )
            .clicked()
        {
            p.lote
                .extend(comandos::pendentes_do_item(it, AcaoRevisao::Aceitar));
        }
        if ui
            .add_enabled(
                pode && pendentes > 0,
                egui::Button::new(format!("Rejeitar todos deste arquivo ({pendentes})")),
            )
            .clicked()
        {
            p.lote
                .extend(comandos::pendentes_do_item(it, AcaoRevisao::Rejeitar));
        }
    });
}

fn editor_de_correcao(
    ui: &mut egui::Ui,
    app: &mut AppDag,
    it: &ItemRevisaoInfo,
    corrigindo: Option<&ChaveCampo>,
    p: &mut Pedido,
) {
    let Some((fid, campo)) = corrigindo else {
        return;
    };
    if *fid != it.file_id {
        return;
    }
    quadro(ui, &format!("Corrigir: {}", rotulo_campo(campo)), |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.biblioteca.rev.texto)
                .desired_width(f32::INFINITY)
                .hint_text("novo valor; listas (autores, tags) separadas por |"),
        );
        ui.horizontal(|ui| {
            let d = comandos::corrigir(*fid, campo, &app.biblioteca.rev.texto);
            if ui
                .add_enabled(d.is_some(), egui::Button::new("Aplicar correção"))
                .clicked()
            {
                p.aplicar_correcao = d;
            }
            if ui.button("Cancelar").clicked() {
                p.cancelar_correcao = true;
            }
        });
    });
}

fn proveniencia(ui: &mut egui::Ui, f: &CampoLadoALado) {
    quadro(
        ui,
        &format!("Por que sugeriu isto? — {}", rotulo_campo(&f.campo)),
        |ui| {
            ui.label(format!(
                "Confiança {} ({}) · origem {} · modelo {}",
                percentual_de_confianca(f.confianca),
                faixa_de_confianca(f.faixa.as_deref(), f.confianca),
                texto_puro(f.origem.as_deref().unwrap_or("—"), 40),
                texto_puro(f.modelo.as_deref().unwrap_or("regras (sem IA)"), 60),
            ));
            for m in &f.motivos {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("Atenção: {}", texto_puro(m, 300)),
                );
            }
            if f.evidencias.is_empty() {
                ui.weak("Sem evidências registradas para este campo.");
            }
            egui::ScrollArea::vertical()
                .id_salt("bib-rev-evid")
                .max_height(140.0)
                .show(ui, |ui| {
                    for e in &f.evidencias {
                        // Trecho do livro: sempre texto puro.
                        ui.label(
                            egui::RichText::new(format!("“{}”", texto_puro(e, 400))).monospace(),
                        );
                    }
                });
        },
    );
}

/// A capa só aparece quando a resposta do agente a traz (campo `capa`); o contrato atual não
/// carrega imagens.
fn capa(ui: &mut egui::Ui, it: &ItemRevisaoInfo) {
    if let Some(c) = it.campos.iter().find(|c| c.campo == "capa") {
        ui.weak(format!(
            "Capa: {}",
            texto_puro(c.sugerido.as_deref().unwrap_or("sem valor"), 120)
        ));
    }
}

fn aplicar(app: &mut AppDag, it: &ItemRevisaoInfo, p: Pedido) {
    if let Some(k) = p.alternar
        && !app.biblioteca.rev.escolhidos.remove(&k)
    {
        app.biblioteca.rev.escolhidos.insert(k);
    }
    if let Some(k) = p.foco {
        app.biblioteca.rev.foco = Some(k);
    }
    if let Some((k, t)) = p.iniciar_correcao {
        app.biblioteca.rev.corrigindo = Some(k);
        app.biblioteca.rev.texto = t;
    }
    if p.cancelar_correcao {
        app.biblioteca.rev.corrigindo = None;
    }
    let mut lote = p.lote;
    if p.aceitar_selecionados || p.rejeitar_selecionados {
        let acao = if p.aceitar_selecionados {
            AcaoRevisao::Aceitar
        } else {
            AcaoRevisao::Rejeitar
        };
        lote.extend(comandos::lote_dos_escolhidos(
            std::slice::from_ref(it),
            &app.biblioteca.rev.escolhidos,
            acao,
        ));
    }
    if let Some(d) = p.aplicar_correcao {
        app.biblioteca.rev.corrigindo = None;
        lote.push(d);
    }
    if !lote.is_empty() {
        app.bib_acao(AcaoBib::Decidir, comandos::decidir(lote));
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_core::CampoLadoALado;

    fn campo(nome: &str) -> CampoLadoALado {
        CampoLadoALado {
            campo: nome.into(),
            extraido: None,
            sugerido: Some("x".into()),
            sugerido_lista: vec![],
            confianca: Some(0.9),
            faixa: Some("alta".into()),
            origem: None,
            modelo: None,
            evidencias: vec![],
            precisa_revisao: false,
            motivos: vec![],
            homologado: None,
            estado: "pendente".into(),
            sugestao_id: Some(1),
        }
    }

    fn item(id: i64, campos: &[&str]) -> ItemRevisaoInfo {
        ItemRevisaoInfo {
            membro: None,
            proposta_homologada: None,
            paginas: vec![],
            paginas_ocr: 0,
            file_id: id,
            caminho: None,
            tamanho: None,
            hash: None,
            formato: None,
            pendentes: 0,
            campos: campos.iter().map(|c| campo(c)).collect(),
            homologados_parecidos: vec![],
            obra_id: None,
            edicao_id: None,
            avisos: vec![],
        }
    }

    #[test]
    fn reconciliar_descarta_o_que_sumiu_da_lista() {
        let mut ui = Ui {
            selecionado: Some(2),
            foco: Some((2, "titulo".into())),
            corrigindo: Some((1, "autores".into())),
            ..Ui::default()
        };
        ui.escolhidos.insert((1, "titulo".into()));
        ui.escolhidos.insert((2, "titulo".into()));
        ui.reconciliar(&[item(1, &["titulo"])]);
        assert_eq!(ui.selecionado, None);
        assert_eq!(ui.foco, None);
        assert_eq!(ui.corrigindo, None);
        assert_eq!(ui.escolhidos.len(), 1);
    }
}
