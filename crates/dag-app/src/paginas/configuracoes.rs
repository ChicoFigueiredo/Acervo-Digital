//! Página Configurações.

use super::{DefAcao, DefGrupo, DefPagina, cabecalho, estado_da_carga};
use crate::app::AppDag;
use crate::leitor::{Carga, Leitor, erro};
use crate::worker::ModoWorker;
use bancada_casca::egui;
use dag_core::{
    MAX_DISCOS_MAX, MAX_DISCOS_PADRAO, OpcoesHash, THREADS_POR_DISCO_MAX, THREADS_POR_DISCO_PADRAO,
};
use dag_store::configuracao;

/// Estado da página: o que está gravado no Catálogo e o que o usuário está editando.
#[derive(Debug)]
pub struct Estado {
    pub gravado: Carga<OpcoesHash>,
    vista: u64,
    /// Valores nos campos de edição.
    pub threads_por_disco: u32,
    pub max_discos: u32,
    /// Os campos já receberam o valor gravado (não sobrescreve a edição em curso).
    inicializado: bool,
}

impl Default for Estado {
    fn default() -> Self {
        Self {
            gravado: Carga::default(),
            vista: 0,
            threads_por_disco: THREADS_POR_DISCO_PADRAO,
            max_discos: MAX_DISCOS_PADRAO,
            inicializado: false,
        }
    }
}

impl Estado {
    /// Recolhe a leitura e a dispara quando o Catálogo mudou.
    pub fn atualizar(&mut self, leitor: &Leitor, geracao: u64) {
        if self.vista != geracao {
            self.vista = geracao;
            self.inicializado = false;
            self.gravado
                .pedir(leitor.consultar(|c| configuracao::ler_hash(c).map_err(erro)));
        }
        if self.gravado.atualizar()
            && !self.inicializado
            && let Some(o) = self.gravado.dado
        {
            let p = o.resolver().unwrap_or_default();
            self.threads_por_disco = p.threads_por_disco;
            self.max_discos = p.max_discos;
            self.inicializado = true;
        }
    }
}

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
    app.configuracoes.atualizar(&app.leitor, app.geracao);
    informacoes_da_sessao(ui, app);
    ui.add_space(8.0);
    hash_em_paralelo(ui, app);
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
            ui.label("Agente");
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

/// Paralelismo do Hash Completo: threads por disco e discos em paralelo.
fn hash_em_paralelo(ui: &mut egui::Ui, app: &mut AppDag) {
    ui.heading("Hash Completo");
    estado_da_carga(ui, &app.configuracoes.gravado);
    let mut salvar = false;
    egui::Grid::new("config-hash")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Threads de hash por disco");
            ui.add(
                egui::DragValue::new(&mut app.configuracoes.threads_por_disco)
                    .range(1..=THREADS_POR_DISCO_MAX),
            );
            ui.end_row();
            ui.label("Discos em paralelo");
            ui.add(
                egui::DragValue::new(&mut app.configuracoes.max_discos).range(1..=MAX_DISCOS_MAX),
            );
            ui.end_row();
        });
    ui.weak(
        "HDD: 1 thread por disco é o melhor (mais leituras simultâneas só aumentam as buscas). \
         SSD/NVMe podem ganhar com 2 a 4. Padrão: 1 thread por disco e 16 discos em paralelo.",
    );
    ui.weak(
        "A mudança vale a partir da próxima varredura, verificação ou retomada; \
         o que já está rodando mantém o valor com que começou.",
    );
    let motivo = app.motivo_sem_escrita_config();
    ui.horizontal(|ui| {
        let botao = ui.add_enabled(motivo.is_none(), egui::Button::new("Salvar"));
        if botao.clicked() {
            salvar = true;
        }
        if ui.button("Restaurar padrão").clicked() {
            app.configuracoes.threads_por_disco = THREADS_POR_DISCO_PADRAO;
            app.configuracoes.max_discos = MAX_DISCOS_PADRAO;
        }
        if let Some(m) = motivo {
            ui.weak(m);
        }
    });
    if salvar {
        let (t, d) = (
            app.configuracoes.threads_por_disco,
            app.configuracoes.max_discos,
        );
        app.salvar_config_hash(t, d);
    }
}
