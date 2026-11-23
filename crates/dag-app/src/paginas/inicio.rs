//! Página Início.

use super::{DefAcao, DefGrupo, DefPagina, estado_vazio};
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

fn desenhar(ui: &mut egui::Ui) {
    estado_vazio(ui, PAGINA.rotulo, PAGINA.resumo);
}
