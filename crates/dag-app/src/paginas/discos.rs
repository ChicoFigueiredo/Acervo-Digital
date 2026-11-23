//! Página Discos.

use super::{DefAcao, DefGrupo, DefPagina, estado_vazio};
use bancada_casca::egui;

pub static PAGINA: DefPagina = DefPagina {
    id: "discos",
    rotulo: "Discos",
    keytip: "D",
    icone: "discos",
    grupos: &[
        DefGrupo {
            id: "discos.volumes",
            rotulo: "Volumes",
            acoes: &[
                DefAcao {
                    id: "discos.detectar",
                    rotulo: "Detectar volumes",
                    dica: "Detecta volumes, partições e pontos de montagem",
                },
                DefAcao {
                    id: "discos.propriedades",
                    rotulo: "Propriedades",
                    dica: "Mostra capacidade, espaço livre e sistema de arquivos",
                },
            ],
        },
        DefGrupo {
            id: "discos.catalogo",
            rotulo: "Catálogo",
            acoes: &[DefAcao {
                id: "discos.incluir",
                rotulo: "Incluir no catálogo",
                dica: "Inclui o volume selecionado nas raízes do catálogo",
            }],
        },
    ],
    resumo: "Volumes detectados, capacidade, espaço livre e pontos de montagem.",
    desenhar,
};

fn desenhar(ui: &mut egui::Ui) {
    estado_vazio(ui, PAGINA.rotulo, PAGINA.resumo);
}
