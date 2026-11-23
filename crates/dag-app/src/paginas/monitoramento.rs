//! Página Monitoramento.

use super::{DefAcao, DefGrupo, DefPagina, estado_vazio};
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

fn desenhar(ui: &mut egui::Ui) {
    estado_vazio(ui, PAGINA.rotulo, PAGINA.resumo);
}
