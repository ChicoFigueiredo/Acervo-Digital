//! Página Monitoramento.

use super::{DefAcao, DefGrupo, DefPagina, cabecalho};
use crate::app::AppDag;
use crate::formato::inteiro_pt_br;
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
    ui.strong("Monitor do agente");
    match app.worker.monitor() {
        Some(m) => {
            egui::Grid::new("monitor-do-agente")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Estado");
                    ui.label(m.situacao_texto());
                    ui.end_row();
                    ui.label("Backlog");
                    ui.label(inteiro_pt_br(m.backlog));
                    ui.end_row();
                    ui.label("Lacunas abertas");
                    ui.label(inteiro_pt_br(m.lacunas_abertas));
                    ui.end_row();
                    ui.label("Overflows do inotify");
                    ui.label(inteiro_pt_br(m.overflows));
                    ui.end_row();
                    ui.label("Watches");
                    ui.label(format!(
                        "{} de {}",
                        inteiro_pt_br(m.watches),
                        inteiro_pt_br(m.watches_limite)
                    ));
                    ui.end_row();
                    ui.label("Eventos processados");
                    ui.label(inteiro_pt_br(m.eventos_processados));
                    ui.end_row();
                    if let Some(e) = &m.erro {
                        ui.label("Erro");
                        ui.label(e);
                        ui.end_row();
                    }
                });
        }
        None => {
            ui.weak("Sem informações do monitor (o agente não está conectado ou não monitora).");
        }
    }
    ui.add_space(8.0);
    ui.strong("Ponto de Confiança");
    if app.worker.confianca().is_empty() {
        ui.weak("Nenhuma Raiz catalogada ainda.");
    }
    for c in app.worker.confianca() {
        let texto = match (c.confiavel_em, &c.motivo_perda) {
            (Some(_), _) => "confiável: só o monitoramento a mantém em dia".to_string(),
            (None, Some(m)) => format!("perdido ({m})"),
            (None, None) => "ainda não confiável".to_string(),
        };
        ui.label(format!("{} — {texto}", c.raiz));
    }
    ui.add_space(8.0);
    ui.weak(
        "O monitoramento roda no agente em segundo plano; quem ele cede a vez (varreduras da \
         GUI, da CLI ou do MCP) aparece em Varredura.",
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
            ui.label("Agente");
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
