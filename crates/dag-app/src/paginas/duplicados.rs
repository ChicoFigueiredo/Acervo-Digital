//! Página Duplicados (A-36).

use super::{DefAcao, DefGrupo, DefPagina, estado_vazio};
use bancada_casca::egui;

pub static PAGINA: DefPagina = DefPagina {
    id: "duplicados",
    rotulo: "Duplicados",
    keytip: "U",
    icone: "duplicados",
    grupos: &[
        DefGrupo {
            id: "duplicados.grupos",
            rotulo: "Grupos",
            acoes: &[DefAcao {
                id: "duplicados.grupos",
                rotulo: "Grupos",
                dica: "Grupos de arquivos idênticos",
            }],
        },
        DefGrupo {
            id: "duplicados.marcacao",
            rotulo: "Marcação",
            acoes: &[
                DefAcao {
                    id: "duplicados.copia_principal",
                    rotulo: "Cópia Principal",
                    dica: "Define a Cópia Principal do grupo",
                },
                DefAcao {
                    id: "duplicados.marcar_backup",
                    rotulo: "Marcar backup",
                    dica: "Marca a cópia como backup",
                },
                DefAcao {
                    id: "duplicados.marcar_delecao",
                    rotulo: "Marcar para deleção",
                    dica: "Apenas marca no catálogo; o DAG nunca apaga arquivos",
                },
            ],
        },
        DefGrupo {
            id: "duplicados.saida",
            rotulo: "Saída",
            acoes: &[DefAcao {
                id: "duplicados.relatorio",
                rotulo: "Relatório",
                dica: "Cobertura e espaço recuperável",
            }],
        },
    ],
    resumo: "Grupos de duplicados, Cópia Principal, marcações, cobertura e espaço recuperável.",
    desenhar,
};

fn desenhar(ui: &mut egui::Ui) {
    estado_vazio(ui, PAGINA.rotulo, PAGINA.resumo);
}
