//! Página Monitoramento.

use super::{DefAcao, DefGrupo, DefPagina, cabecalho};
use crate::app::AppDag;
use crate::worker::ModoWorker;
use bancada_casca::egui;

pub static PAGINA: DefPagina = DefPagina {
    id: "monitoramento",
    rotulo: "Monitoramento",
    keytip: "M",
    icone: "monitoramento",
    grupos: &[
        DefGrupo {
            id: "monitoramento.observador",
            rotulo: "Observador",
            acoes: &[
                DefAcao {
                    id: "monitoramento.iniciar",
                    rotulo: "Iniciar monitor",
                    dica: "Liga o monitor de eventos",
                },
                DefAcao {
                    id: "monitoramento.parar",
                    rotulo: "Parar monitor",
                    dica: "Desliga o monitor de eventos",
                },
                DefAcao {
                    id: "monitoramento.reconciliar",
                    rotulo: "Reconciliar",
                    dica: "Revarre o escopo após lacuna de eventos",
                },
            ],
        },
        DefGrupo {
            id: "monitoramento.registros",
            rotulo: "Registros",
            acoes: &[DefAcao {
                id: "monitoramento.logs",
                rotulo: "Logs e falhas",
                dica: "Mostra logs e falhas do monitor",
            }],
        },
    ],
    resumo: "Taxa de eventos, backlog, latência, raízes cobertas, logs e falhas.",
    desenhar,
};

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    informacoes_da_sessao(ui, app);
    ui.add_space(8.0);
    ui.label("O monitoramento contínuo (inotify, backlog, reconciliação) e o Fast Scan chegam no incremento 5.");
    ui.weak(
        "Hoje a GUI só varre quando você pede (Full Scan), e a varredura é sempre somente leitura.",
    );
}

/// Catálogo, worker e trava, comuns às páginas que ainda são marcadores.
pub fn informacoes_da_sessao(ui: &mut egui::Ui, app: &AppDag) {
    egui::Grid::new(concat!("sessao-", file!()))
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Catálogo");
            ui.monospace(app.caminho.display().to_string());
            ui.end_row();
            ui.label("Trava do escritor");
            ui.monospace(
                crate::app::caminho_da_trava(&app.caminho)
                    .display()
                    .to_string(),
            );
            ui.end_row();
            ui.label("Worker");
            ui.label(app.worker.modo().texto_status());
            ui.end_row();
            if let ModoWorker::SomenteLeitura { motivo } | ModoWorker::Falhou { motivo } =
                app.worker.modo()
            {
                ui.label("Motivo");
                ui.label(motivo);
                ui.end_row();
            }
            ui.label("Varredura");
            ui.label(if app.scan_ativo() {
                "em andamento"
            } else {
                "ociosa"
            });
            ui.end_row();
        });
}
