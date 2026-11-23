//! Página Configurações.

use super::{DefAcao, DefGrupo, DefPagina, cabecalho};
use crate::app::AppDag;
use crate::worker::ModoWorker;
use bancada_casca::egui;

pub static PAGINA: DefPagina = DefPagina {
    id: "configuracoes",
    rotulo: "Configurações",
    keytip: "C",
    icone: "configuracoes",
    grupos: &[
        DefGrupo {
            id: "configuracoes.servico",
            rotulo: "Serviço",
            acoes: &[DefAcao {
                id: "configuracoes.servico",
                rotulo: "Serviço opcional",
                dica: "Instala ou remove o serviço de segundo plano (opt-in)",
            }],
        },
        DefGrupo {
            id: "configuracoes.varredura",
            rotulo: "Varredura",
            acoes: &[
                DefAcao {
                    id: "configuracoes.limites",
                    rotulo: "Scan e limites",
                    dica: "Limites de recursos da varredura",
                },
                DefAcao {
                    id: "configuracoes.permissoes",
                    rotulo: "Permissões",
                    dica: "Níveis de permissão; elevação só sob demanda via Polkit",
                },
                DefAcao {
                    id: "configuracoes.exclusoes",
                    rotulo: "Política de exclusão",
                    dica: "Política padrão de exclusões",
                },
            ],
        },
    ],
    resumo: "Serviço opcional, limites de varredura, permissões, observadores e preferências.",
    desenhar,
};

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    informacoes_da_sessao(ui, app);
    ui.add_space(8.0);
    ui.label("Serviço opcional, limites de varredura, permissões e política de exclusão chegam em incrementos posteriores.");
    ui.weak("Nenhum serviço é instalado nem ativado sem o seu comando explícito.");
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
