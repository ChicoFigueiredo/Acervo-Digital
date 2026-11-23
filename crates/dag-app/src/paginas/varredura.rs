//! Página Varredura.

use super::{DefAcao, DefGrupo, DefPagina, estado_vazio};
use bancada_casca::egui;

pub static PAGINA: DefPagina = DefPagina {
    id: "varredura",
    rotulo: "Varredura",
    keytip: "V",
    icone: "varredura",
    grupos: &[
        DefGrupo {
            id: "varredura.iniciar",
            rotulo: "Iniciar",
            acoes: &[
                DefAcao {
                    id: "varredura.full_scan",
                    rotulo: "Full Scan",
                    dica: "Varredura completa por etapas",
                },
                DefAcao {
                    id: "varredura.fast_scan",
                    rotulo: "Fast Scan",
                    dica: "Reconciliação rápida sem recalcular itens inalterados",
                },
            ],
        },
        DefGrupo {
            id: "varredura.controle",
            rotulo: "Controle",
            acoes: &[
                DefAcao {
                    id: "varredura.pausar",
                    rotulo: "Pausar",
                    dica: "Pausa a varredura em andamento com checkpoint",
                },
                DefAcao {
                    id: "varredura.retomar",
                    rotulo: "Retomar",
                    dica: "Retoma a varredura pausada ou interrompida",
                },
                DefAcao {
                    id: "varredura.cancelar",
                    rotulo: "Cancelar",
                    dica: "Cancela a varredura em andamento",
                },
            ],
        },
        DefGrupo {
            id: "varredura.escopo",
            rotulo: "Escopo",
            acoes: &[
                DefAcao {
                    id: "varredura.selecionar_raizes",
                    rotulo: "Selecionar raízes",
                    dica: "Escolhe as raízes a varrer",
                },
                DefAcao {
                    id: "varredura.exclusoes",
                    rotulo: "Exclusões",
                    dica: "Gerencia exclusões, com justificativa",
                },
            ],
        },
    ],
    resumo: "Controle das varreduras: fases, progresso, fila, velocidade e erros.",
    desenhar,
};

fn desenhar(ui: &mut egui::Ui) {
    estado_vazio(ui, PAGINA.rotulo, PAGINA.resumo);
}
