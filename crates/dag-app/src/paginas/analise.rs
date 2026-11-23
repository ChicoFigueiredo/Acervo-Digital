//! Página Análise.

use super::{DefAcao, DefGrupo, DefPagina, estado_vazio};
use bancada_casca::egui;

pub static PAGINA: DefPagina = DefPagina {
    id: "analise",
    rotulo: "Análise",
    keytip: "N",
    icone: "analise",
    grupos: &[
        DefGrupo {
            id: "analise.visoes",
            rotulo: "Visões",
            acoes: &[
                DefAcao {
                    id: "analise.tree_view",
                    rotulo: "Tree View",
                    dica: "Árvore com barras proporcionais ao consumo",
                },
                DefAcao {
                    id: "analise.treemap",
                    rotulo: "Treemap",
                    dica: "Treemap interativo com drill-down",
                },
            ],
        },
        DefGrupo {
            id: "analise.modo",
            rotulo: "Modo",
            acoes: &[
                DefAcao {
                    id: "analise.por_volume",
                    rotulo: "Por volume",
                    dica: "Analisa o volume selecionado",
                },
                DefAcao {
                    id: "analise.por_computador",
                    rotulo: "Por computador",
                    dica: "Soma os volumes sem contar a mesma montagem duas vezes",
                },
                DefAcao {
                    id: "analise.filtros",
                    rotulo: "Filtros",
                    dica: "Volume, categoria, MIME, idade, tamanho e profundidade",
                },
            ],
        },
    ],
    resumo: "Tree View e Treemap do consumo de espaço, por volume ou por computador.",
    desenhar,
};

fn desenhar(ui: &mut egui::Ui) {
    estado_vazio(ui, PAGINA.rotulo, PAGINA.resumo);
}
