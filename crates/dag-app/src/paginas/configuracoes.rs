//! Página Configurações.

use super::{DefAcao, DefGrupo, DefPagina, estado_vazio};
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

fn desenhar(ui: &mut egui::Ui) {
    estado_vazio(ui, PAGINA.rotulo, PAGINA.resumo);
}
