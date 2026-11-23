//! Página Início.

use super::{DefAcao, DefGrupo, DefPagina, cabecalho};
use crate::app::AppDag;
use crate::formato::{bytes_humanos, inteiro_pt_br};
use bancada_casca::egui;

pub static PAGINA: DefPagina = DefPagina {
    id: "inicio",
    rotulo: "Início",
    keytip: "I",
    icone: "inicio",
    grupos: &[
        DefGrupo {
            id: "inicio.catalogo",
            rotulo: "Catálogo",
            acoes: &[
                DefAcao {
                    id: "inicio.nova_varredura",
                    rotulo: "Nova varredura",
                    dica: "Inicia uma varredura a partir do painel inicial",
                },
                DefAcao {
                    id: "inicio.abrir_catalogo",
                    rotulo: "Abrir catálogo",
                    dica: "Abre outro catálogo SQLite",
                },
            ],
        },
        DefGrupo {
            id: "inicio.resumo",
            rotulo: "Resumo",
            acoes: &[DefAcao {
                id: "inicio.atualizar",
                rotulo: "Atualizar",
                dica: "Recarrega o resumo do catálogo",
            }],
        },
    ],
    resumo: "Resumo do catálogo: espaço total, maiores consumidores e últimas varreduras.",
    desenhar,
};

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    ui.label(format!("Catálogo: {}", app.caminho.display()));
    ui.label(app.worker.modo().texto_status());
    ui.add_space(8.0);
    match app.totais.dado {
        Some(t) if t.volumes > 0 => {
            egui::Grid::new("inicio-totais")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Volumes catalogados");
                    ui.strong(inteiro_pt_br(t.volumes));
                    ui.end_row();
                    ui.label("Pastas");
                    ui.strong(inteiro_pt_br(t.pastas));
                    ui.end_row();
                    ui.label("Arquivos");
                    ui.strong(inteiro_pt_br(t.arquivos));
                    ui.end_row();
                    ui.label("Espaço (tamanho lógico)");
                    ui.strong(bytes_humanos(t.bytes_logicos));
                    ui.end_row();
                });
            ui.weak("Soma da última varredura de cada volume; um volume montado em mais de um lugar conta uma vez.");
        }
        _ => {
            ui.heading("Catálogo vazio");
            ui.label("Escolha raízes na página Varredura e rode um Full Scan para começar.");
        }
    }
    ui.add_space(8.0);
    if ui.button("Nova varredura…").clicked() {
        bancada_casca::abrir(ui.ctx(), "varredura");
    }
}
