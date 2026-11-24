//! Página Lixeira (ADR 0005) e os dois diálogos de "Mover para a Lixeira".

use super::tabela::{Coluna, tabela};
use super::{DefAcao, DefGrupo, DefPagina, cabecalho, estado_da_carga, estado_vazio};
use crate::app::{AppDag, Proposito};
use crate::formato::{bytes_humanos, data_ns, inteiro_pt_br};
use crate::lixeira::{AlvoMenu, FRASE, Passo, comando_restaurar};
use bancada_casca::egui;
use dag_core::ResultadoLixeira;

pub static PAGINA: DefPagina = DefPagina {
    id: "lixeira",
    rotulo: "Lixeira",
    keytip: "L",
    icone: "lixeira",
    grupos: &[
        DefGrupo {
            id: "lixeira.itens",
            rotulo: "Itens",
            acoes: &[
                DefAcao {
                    id: "lixeira.restaurar",
                    rotulo: "Restaurar selecionados",
                    dica: "Devolve os itens marcados ao caminho de origem",
                },
                DefAcao {
                    id: "lixeira.atualizar",
                    rotulo: "Atualizar",
                    dica: "Relê a lista Na Lixeira",
                },
            ],
        },
        DefGrupo {
            id: "lixeira.ajuda",
            rotulo: "Ajuda",
            acoes: &[DefAcao {
                id: "lixeira.sobre",
                rotulo: "Como esvaziar",
                dica: "O DAG nunca apaga: esvazie a DAG-Lixeira pelo gerenciador de arquivos",
            }],
        },
    ],
    resumo: "Cópias com Marcação para Deleção movidas para a DAG-Lixeira do próprio Volume; restaure daqui.",
    desenhar,
};

const COLUNAS: [Coluna; 5] = [
    Coluna::esq("", 30.0),
    Coluna::esq("Origem", 420.0),
    Coluna::dir("Tamanho", 90.0),
    Coluna::esq("Movido em (UTC)", 150.0),
    Coluna::esq("", 110.0),
];

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.lixeira.atualizar(&app.leitor, app.geracao);
    estado_da_carga(ui, &app.lixeira.lista);
    let Some(l) = app.lixeira.lista.dado.clone() else {
        return;
    };
    banner(ui, l.total, l.bytes, l.descartados);
    if l.itens.is_empty() {
        estado_vazio(
            ui,
            "Nada Na Lixeira",
            PAGINA.resumo,
            "Use o menu de contexto (botão direito) em Explorador, Análise ou Duplicados.",
        );
        return;
    }
    let mut restaurar_um = None;
    let mut alternar = None;
    let altura = super::altura_livre(ui) - 34.0;
    tabela(
        ui,
        "lixeira-itens",
        &COLUNAS,
        l.itens.len(),
        altura,
        |linha, i| {
            let it = &l.itens[i];
            let marcado = app.lixeira.marcados.contains(&it.arquivo.0);
            linha.ui(|ui| {
                let mut m = marcado;
                if ui.checkbox(&mut m, "").changed() {
                    alternar = Some(it.arquivo.0);
                }
            });
            linha.texto(it.origem.clone());
            linha.texto(bytes_humanos(it.bytes));
            linha.texto(it.movido_em.map_or_else(
                || "—".into(),
                |s| data_ns(Some(s.saturating_mul(1_000_000_000))),
            ));
            linha.ui(|ui| {
                if ui
                    .small_button("Restaurar")
                    .on_hover_text(it.caminho.clone())
                    .clicked()
                {
                    restaurar_um = Some(it.arquivo);
                }
            });
        },
    );
    if let Some(id) = alternar
        && !app.lixeira.marcados.remove(&id)
    {
        app.lixeira.marcados.insert(id);
    }
    if let Some(id) = restaurar_um {
        app.enviar(
            comando_restaurar(vec![dag_core::Alvo::Arquivo(id)]),
            Proposito::LixeiraRestaurar,
        );
    }
}

/// Faixa fixa: o que a Lixeira é e que o DAG nunca apaga.
fn banner(ui: &mut egui::Ui, total: u64, bytes: u64, descartados: u64) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(format!(
            "{} item(ns) Na Lixeira, {}. Mover para a Lixeira não libera espaço: o DAG nunca \
             apaga arquivos. Para liberar o espaço, esvazie a pasta DAG-Lixeira de cada Volume \
             pelo gerenciador de arquivos; no próximo scan o item vira Descartado ({} registro(s)).",
            inteiro_pt_br(total),
            bytes_humanos(bytes),
            inteiro_pt_br(descartados),
        ));
    });
}

/// Lista de itens (até 8) e totais do plano de um resultado.
fn corpo_do_plano(ui: &mut egui::Ui, r: &ResultadoLixeira) {
    ui.label(format!(
        "{} arquivo(s), {}.",
        inteiro_pt_br(r.total_arquivos),
        bytes_humanos(r.total_bytes)
    ));
    for m in r.movidos.iter().take(6) {
        ui.monospace(format!("{}  →  {}", m.de, m.para));
    }
    if r.total_arquivos as usize > r.movidos.len().min(6) {
        ui.weak("… e mais arquivos (lista completa no agente).");
    }
    if !r.destinos.is_empty() {
        ui.label(format!("Destino: {}", r.destinos.join(", ")));
    }
    for rec in r.recusados.iter().take(5) {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("Recusado: {} ({})", rec.alvo, rec.motivo),
        );
    }
    if r.recusados.len() > 5 {
        ui.weak(format!("… e mais {} recusado(s)", r.recusados.len() - 5));
    }
}

/// Os dois diálogos em sequência para mover para a Lixeira.
pub fn dialogos(ctx: &egui::Context, app: &mut AppDag) {
    let Some(mut d) = app.lixeira.dialogo.clone() else {
        return;
    };
    let original = d.clone();
    let mut fechar = false;
    let mut novo_previa = false;
    let mut enviar = None;
    let titulo = match d.passo {
        Passo::Confirmar(_) => "Mover para a Lixeira: confirmação final",
        _ => "Mover para a Lixeira",
    };
    egui::Window::new(titulo)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.strong(&d.alvo.rotulo);
            ui.add_space(4.0);
            match d.passo.clone() {
                Passo::Calculando => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Calculando o que seria movido…");
                    });
                    if ui.button("Cancelar").clicked() {
                        fechar = true;
                    }
                }
                Passo::Previa(r) => {
                    corpo_do_plano(ui, &r);
                    if d.alvo.pasta
                        && ui
                            .checkbox(
                                &mut d.marcar_tudo,
                                "Marcar tudo para deleção e mover (a pasta inteira)",
                            )
                            .on_hover_text(
                                "Sem isto, uma pasta só vai se todo arquivo já tem Marcação para Deleção",
                            )
                            .changed()
                    {
                        novo_previa = true;
                    }
                    if r.unicas_copias > 0 {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!(
                                "Atenção: {} arquivo(s) são a Única Cópia no Catálogo.",
                                r.unicas_copias
                            ),
                        );
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(r.total_arquivos > 0, egui::Button::new("Continuar"))
                            .clicked()
                        {
                            d.continuar();
                        }
                        if ui.button("Cancelar").clicked() {
                            fechar = true;
                        }
                    });
                }
                Passo::Confirmar(r) => {
                    ui.label(format!(
                        "Última chance: {} arquivo(s), {}, vão para a DAG-Lixeira do próprio Volume. \
                         Nada é apagado; dá para restaurar depois.",
                        inteiro_pt_br(r.total_arquivos),
                        bytes_humanos(r.total_bytes)
                    ));
                    ui.checkbox(&mut d.entendo, FRASE);
                    if r.unicas_copias > 0 {
                        ui.checkbox(
                            &mut d.aceita_unica,
                            format!(
                                "Aceito mover {} arquivo(s) que são a Única Cópia",
                                r.unicas_copias
                            ),
                        );
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let cmd = d.comando_mover();
                        if ui
                            .add_enabled(cmd.is_some(), egui::Button::new("Mover para a Lixeira"))
                            .clicked()
                        {
                            enviar = cmd;
                        }
                        if ui.button("Voltar").clicked() {
                            d.voltar();
                        }
                        if ui.button("Cancelar").clicked() {
                            fechar = true;
                        }
                    });
                }
            }
        });
    if fechar {
        app.lixeira.dialogo = None;
        return;
    }
    if let Some(cmd) = enviar {
        app.lixeira.dialogo = None;
        app.enviar(cmd, Proposito::LixeiraMover);
        return;
    }
    if novo_previa {
        d.passo = Passo::Calculando;
        let cmd = d.comando_previa();
        app.lixeira.dialogo = Some(d);
        app.enviar(cmd, Proposito::LixeiraPrevia);
        return;
    }
    if d != original {
        app.lixeira.dialogo = Some(d);
    }
}

/// Entrada única para os menus de contexto das outras páginas.
pub fn menu(
    ui_resposta: &egui::Response,
    alvo: &AlvoMenu,
    escolha: &mut Option<crate::lixeira::Escolha>,
) {
    ui_resposta.context_menu(|ui| {
        if let Some(item) = crate::lixeira::menu_de_contexto(ui, alvo) {
            *escolha = Some((item, alvo.clone()));
        }
    });
}
