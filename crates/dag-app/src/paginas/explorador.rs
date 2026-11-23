//! Página Explorador.

use super::{DefAcao, DefGrupo, DefPagina, estado_vazio};
use bancada_casca::egui;

pub static PAGINA: DefPagina = DefPagina {
    id: "explorador",
    rotulo: "Explorador",
    keytip: "E",
    icone: "explorador",
    grupos: &[
        DefGrupo {
            id: "explorador.navegar",
            rotulo: "Navegar",
            acoes: &[
                DefAcao {
                    id: "explorador.expandir",
                    rotulo: "Expandir",
                    dica: "Expande a pasta selecionada",
                },
                DefAcao {
                    id: "explorador.recolher",
                    rotulo: "Recolher",
                    dica: "Recolhe a pasta selecionada",
                },
                DefAcao {
                    id: "explorador.filtrar",
                    rotulo: "Filtrar",
                    dica: "Filtra a tabela de arquivos",
                },
            ],
        },
        DefGrupo {
            id: "explorador.item",
            rotulo: "Item",
            acoes: &[DefAcao {
                id: "explorador.propriedades",
                rotulo: "Propriedades",
                dica: "Tamanho lógico e alocado, categoria, MIME, datas e atributos",
            }],
        },
        DefGrupo {
            id: "explorador.saida",
            rotulo: "Saída",
            acoes: &[DefAcao {
                id: "explorador.exportar",
                rotulo: "Exportar relatório",
                dica: "Exporta a visão atual",
            }],
        },
    ],
    resumo: "Volumes, árvore de pastas e tabela de arquivos do catálogo.",
    desenhar,
};

fn desenhar(ui: &mut egui::Ui) {
    estado_vazio(ui, PAGINA.rotulo, PAGINA.resumo);
}
