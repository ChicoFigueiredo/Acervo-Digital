//! Seção "Erros da varredura" da página Varredura: ao clicar em "Erros" numa execução, mostra os
//! problemas agrupados (esperado, aceito, aviso, erro), as pastas com mais problemas, amostras e
//! os botões para aceitar o tipo de erro numa pasta ou excluir a pasta das próximas varreduras.

use bancada_casca::egui;
use dag_core::Comando;
use dag_core::erros::{ExclusaoUsuario, RegraAceita, ResumoErros};
use dag_store::erros::{OpcoesResumo, listar_exclusoes, listar_regras, pasta_home, resumir};

use super::super::{altura_livre, estado_da_carga};
use crate::app::{AppDag, Proposito};
use crate::formato::inteiro_pt_br;
use crate::leitor::{Carga, erro};

type Regras = (Vec<RegraAceita>, Vec<ExclusaoUsuario>);

/// Estado da seção.
#[derive(Debug, Default)]
pub struct EstadoErros {
    /// Execução cujos erros estão abertos.
    pub execucao: Option<i64>,
    resumo: Carga<ResumoErros>,
    regras: Carga<Regras>,
    /// `(execução, geração)` do último pedido.
    pedido: Option<(i64, u64)>,
}

enum Acao {
    Aceitar { pasta: String, codigo: String },
    Excluir { pasta: String },
    RemoverRegra(i64),
    RemoverExclusao(i64),
    Fechar,
}

/// Desenha a seção, se houver uma execução aberta.
pub fn secao(ui: &mut egui::Ui, app: &mut AppDag) {
    let Some(id) = app.varredura.erros.execucao else {
        return;
    };
    if app.varredura.erros.pedido != Some((id, app.geracao)) {
        app.varredura.erros.pedido = Some((id, app.geracao));
        let home = pasta_home();
        app.varredura
            .erros
            .resumo
            .pedir(app.leitor.consultar(move |c| {
                resumir(c, id, &OpcoesResumo::default(), home.as_deref()).map_err(erro)
            }));
        app.varredura.erros.regras.pedir(app.leitor.consultar(|c| {
            Ok((
                listar_regras(c).map_err(erro)?,
                listar_exclusoes(c).map_err(erro)?,
            ))
        }));
    }
    app.varredura.erros.resumo.atualizar();
    app.varredura.erros.regras.atualizar();

    let pode = app.motivo_sem_escrita();
    let mut agir: Option<Acao> = None;
    ui.separator();
    ui.horizontal(|ui| {
        ui.strong(format!("Erros da varredura #{id}"));
        if ui.small_button("Fechar").clicked() {
            agir = Some(Acao::Fechar);
        }
    });
    estado_da_carga(ui, &app.varredura.erros.resumo);
    let altura = altura_livre(ui).max(180.0);
    egui::ScrollArea::vertical()
        .id_salt("erros_da_varredura")
        .max_height(altura)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if let Some(r) = app.varredura.erros.resumo.dado.as_ref() {
                resumo(ui, r, pode, &mut agir);
            }
            if let Some((aceitas, exclusoes)) = app.varredura.erros.regras.dado.as_ref() {
                regras(ui, aceitas, exclusoes, pode, &mut agir);
            }
        });

    match agir {
        None => {}
        Some(Acao::Fechar) => app.varredura.erros.execucao = None,
        Some(Acao::Aceitar { pasta, codigo }) => {
            let _ = app.enviar(
                Comando::ErrosAceitar {
                    padrao: format!("{}/**", pasta.trim_end_matches('/')),
                    codigo: Some(codigo),
                    motivo: Some("aceito pela página Varredura".into()),
                },
                Proposito::Erros,
            );
        }
        Some(Acao::Excluir { pasta }) => {
            let _ = app.enviar(
                Comando::Excluir {
                    padrao: pasta,
                    motivo: Some("excluída pela página Varredura".into()),
                },
                Proposito::Erros,
            );
        }
        Some(Acao::RemoverRegra(id)) => {
            let _ = app.enviar(Comando::ErrosRemover { id }, Proposito::Erros);
        }
        Some(Acao::RemoverExclusao(id)) => {
            let _ = app.enviar(Comando::ExclusaoRemover { id }, Proposito::Erros);
        }
    }
}

fn resumo(ui: &mut egui::Ui, r: &ResumoErros, pode: Option<&str>, agir: &mut Option<Acao>) {
    ui.label(format!(
        "{} problema(s): {} erro(s) real(is), {} aviso(s), {} esperado(s), {} aceito(s), {} informativo(s). Estado: {}.",
        inteiro_pt_br(r.total),
        inteiro_pt_br(r.reais),
        inteiro_pt_br(r.avisos),
        inteiro_pt_br(r.esperados),
        inteiro_pt_br(r.aceitos),
        inteiro_pt_br(r.informativos),
        r.estado.replace('_', " "),
    ));
    if r.total == 0 {
        ui.weak("Nenhum problema registrado nesta varredura.");
        return;
    }
    ui.add_space(4.0);
    ui.strong("Por código");
    egui::Grid::new("erros_por_codigo")
        .striped(true)
        .show(ui, |ui| {
            for t in ["Desfecho", "Fase", "Código", "Qtd", "Dica"] {
                ui.strong(t);
            }
            ui.end_row();
            for g in &r.por_codigo {
                ui.label(&g.rotulo);
                ui.label(&g.fase);
                ui.monospace(&g.codigo);
                ui.label(inteiro_pt_br(g.total));
                ui.label(g.dica.as_deref().unwrap_or(""));
                ui.end_row();
            }
        });
    ui.add_space(4.0);
    ui.strong("Pastas com mais problemas");
    egui::Grid::new("erros_por_pasta")
        .striped(true)
        .show(ui, |ui| {
            for t in ["Qtd", "Pasta", "Códigos", "", ""] {
                ui.strong(t);
            }
            ui.end_row();
            for p in &r.por_pasta {
                ui.label(inteiro_pt_br(p.total));
                ui.monospace(&p.pasta);
                ui.label(
                    p.codigos
                        .iter()
                        .map(|(c, n)| format!("{c}: {}", inteiro_pt_br(*n)))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                let codigo = p.codigos.first().map(|(c, _)| c.clone());
                let b = ui.add_enabled(
                    pode.is_none() && codigo.is_some(),
                    egui::Button::new("Aceitar este tipo nesta pasta"),
                );
                let b = match pode {
                    Some(m) => b.on_disabled_hover_text(m),
                    None => b.on_hover_text(
                        "Regra \"Erros aceitos\": este código nesta pasta passa a contar como aceito",
                    ),
                };
                if b.clicked()
                    && let Some(codigo) = codigo
                {
                    *agir = Some(Acao::Aceitar {
                        pasta: p.pasta.clone(),
                        codigo,
                    });
                }
                let b = ui.add_enabled(pode.is_none(), egui::Button::new("Excluir pasta"));
                let b = match pode {
                    Some(m) => b.on_disabled_hover_text(m),
                    None => b.on_hover_text("Não varre esta pasta nas próximas varreduras"),
                };
                if b.clicked() {
                    *agir = Some(Acao::Excluir {
                        pasta: p.pasta.clone(),
                    });
                }
                ui.end_row();
            }
        });
    egui::CollapsingHeader::new("Amostras")
        .id_salt("erros_amostras")
        .show(ui, |ui| {
            for a in &r.amostras {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("[{}]", a.rotulo));
                    ui.monospace(&a.codigo);
                    ui.label(a.caminho.as_deref().unwrap_or("(sem caminho)"));
                    if let Some(m) = &a.motivo {
                        ui.weak(m);
                    }
                });
            }
        });
}

fn regras(
    ui: &mut egui::Ui,
    aceitas: &[RegraAceita],
    exclusoes: &[ExclusaoUsuario],
    pode: Option<&str>,
    agir: &mut Option<Acao>,
) {
    egui::CollapsingHeader::new(format!(
        "Regras ({} aceita(s), {} exclusão(ões))",
        aceitas.len(),
        exclusoes.len()
    ))
    .id_salt("erros_regras")
    .show(ui, |ui| {
        ui.weak(
            "Embutidas: arquivos vivos (caches, logs, -wal/-shm, *.lock) que mudam ou somem, \
             symlinks de NTFS ilegíveis e limites de volume contam como esperados.",
        );
        for a in aceitas {
            ui.horizontal(|ui| {
                ui.monospace(format!("{} [{}]", a.padrao, a.codigo));
                ui.weak(&a.motivo);
                if ui
                    .add_enabled(pode.is_none(), egui::Button::new("Remover").small())
                    .clicked()
                {
                    *agir = Some(Acao::RemoverRegra(a.id));
                }
            });
        }
        for e in exclusoes {
            ui.horizontal(|ui| {
                ui.monospace(format!("excluir {}", e.padrao));
                ui.weak(&e.motivo);
                if ui
                    .add_enabled(pode.is_none(), egui::Button::new("Remover").small())
                    .clicked()
                {
                    *agir = Some(Acao::RemoverExclusao(e.id));
                }
            });
        }
    });
}
