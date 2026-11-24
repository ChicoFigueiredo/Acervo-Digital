//! Página Fila de Serviço (ADR 0006): Pedidos de Senha de contêineres cifrados e Falhas de
//! Leitura. Nada de popup: o campo de senha fica na própria linha do item. A senha só vai ao
//! agente (que a cifra no Cofre de Senhas); a página nunca a lê de volta.

use std::collections::HashMap;

use super::{DefAcao, DefGrupo, DefPagina, cabecalho, estado_da_carga, estado_vazio};
use crate::app::{AppDag, Proposito};
use crate::formato::inteiro_pt_br;
use crate::leitor::{Carga, Leitor, erro};
use bancada_casca::egui;
use dag_core::{Comando, Senha};
use dag_store::conteineres::{self, AndamentoConteineres, PedidoServico};

pub static PAGINA: DefPagina = DefPagina {
    id: "fila",
    rotulo: "Fila de Serviço",
    keytip: "F",
    icone: "fila",
    grupos: &[DefGrupo {
        id: "fila.itens",
        rotulo: "Itens",
        acoes: &[
            DefAcao {
                id: "fila.atualizar",
                rotulo: "Atualizar",
                dica: "Recarrega os itens abertos da Fila de Serviço",
            },
            DefAcao {
                id: "fila.ler_conteineres",
                rotulo: "Ler contêineres",
                dica: "Lê o conteúdo dos contêineres pendentes (retoma de onde parou)",
            },
        ],
    }],
    resumo: "Contêineres cifrados que esperam senha e leituras que falharam.",
    desenhar,
};

/// Estado da página.
#[derive(Debug, Default)]
pub struct Estado {
    vista: u64,
    pub pedidos: Carga<Vec<PedidoServico>>,
    pub andamento: Carga<AndamentoConteineres>,
    /// Senha digitada por item (nunca sai daqui a não ser no envio ao agente).
    senhas: HashMap<i64, String>,
}

impl Estado {
    /// Recolhe resultados e recarrega quando o Catálogo mudou.
    pub fn atualizar(&mut self, leitor: &Leitor, geracao: u64) {
        if self.vista != geracao {
            self.vista = geracao;
            self.pedidos
                .pedir(leitor.consultar(|c| conteineres::listar_pedidos(c, true).map_err(erro)));
            self.andamento
                .pedir(leitor.consultar(|c| conteineres::andamento(c).map_err(erro)));
        }
        self.pedidos.atualizar();
        self.andamento.atualizar();
    }

    /// Itens abertos (para o contador da página e da barra de status).
    #[must_use]
    pub fn abertos(&self) -> usize {
        self.pedidos.dado.as_ref().map_or(0, Vec::len)
    }
}

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.fila.atualizar(&app.leitor, app.geracao);
    if let Some(a) = app.fila.andamento.dado {
        ui.label(format!(
            "{} contêiner(es) lido(s) com {} membro(s); {} pendente(s); {} esperando senha; {} falha(s).",
            inteiro_pt_br(a.lidos),
            inteiro_pt_br(a.membros),
            inteiro_pt_br(a.pendentes + a.lendo),
            inteiro_pt_br(a.aguardando_senha + a.aguardando_chaveiro),
            inteiro_pt_br(a.falhas),
        ));
    }
    ui.separator();
    estado_da_carga(ui, &app.fila.pedidos);
    let Some(pedidos) = app.fila.pedidos.dado.clone() else {
        return;
    };
    if pedidos.is_empty() {
        estado_vazio(
            ui,
            "Nenhum item na fila",
            PAGINA.resumo,
            "Contêineres cifrados ou corrompidos aparecem aqui depois da leitura do conteúdo.",
        );
        return;
    }
    ui.strong(format!("{} item(ns) aberto(s)", pedidos.len()));
    for p in &pedidos {
        item(ui, app, p);
    }
}

fn item(ui: &mut egui::Ui, app: &mut AppDag, p: &PedidoServico) {
    let senha_pedida = p.tipo == "pedido_senha";
    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(if senha_pedida {
                "Pedido de Senha"
            } else {
                "Falha de Leitura"
            });
            ui.monospace(p.objeto.as_deref().unwrap_or("(arquivo removido)"));
        });
        ui.weak(&p.motivo);
        ui.horizontal_wrapped(|ui| {
            if senha_pedida {
                let campo = app.fila.senhas.entry(p.id).or_default();
                let r = ui.add(
                    egui::TextEdit::singleline(campo)
                        .password(true)
                        .hint_text("senha do contêiner")
                        .desired_width(220.0),
                );
                let enviar = ui
                    .add_enabled(!campo.is_empty(), egui::Button::new("Fornecer senha"))
                    .clicked()
                    || (r.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && !campo.is_empty());
                if enviar {
                    let senha = std::mem::take(campo);
                    app.enviar(
                        Comando::ResolverPedido {
                            id: p.id,
                            senha: Senha(senha),
                        },
                        Proposito::Exportar,
                    );
                    app.fila.senhas.remove(&p.id);
                }
            }
            if ui.button("Tentar de novo").clicked() {
                app.enviar(Comando::TentarDeNovo { id: p.id }, Proposito::Exportar);
            }
            if ui.button("Descartar").clicked() {
                app.enviar(Comando::DispensarPedido { id: p.id }, Proposito::Exportar);
            }
        });
    });
}
