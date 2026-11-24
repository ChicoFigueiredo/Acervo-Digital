//! Página Duplicados: grupos ordenados por espaço recuperável (paginados), detalhe com as
//! cópias, ações de marcação com confirmação (⚠ Única Cópia) e exportação de relatório.
//! A GUI só lê; toda escrita é um pedido ao worker.

use std::path::PathBuf;

use super::tabela::{Coluna, tabela};
use super::{DefAcao, DefGrupo, DefPagina, cabecalho, estado_da_carga, estado_vazio};
use crate::app::{AppDag, Proposito};
use crate::confirmacao::{AcaoMarcacao, Gravidade, Plano, comando};
use crate::formato::{bytes_humanos, data_s, hash_curto, inteiro_pt_br};
use crate::leitor::{Carga, Leitor, erro};
use bancada_casca::{Tom, egui};
use dag_core::{
    ArquivoId, ClasseRecuperavel, Comando, CopiaGrupo, DetalheGrupo, EstadoMarcacao, GrupoId,
    LIMITACOES_ESPACO, ModoPrincipal, NivelEspaco, OrdemDuplicados, ResumoDuplicados, TipoMarcacao,
    TipoRelatorio,
};
use dag_store::duplicados::{self, FiltroGrupos, PaginaGrupos};

const POR_PAGINA: u32 = 100;

pub static PAGINA: DefPagina = DefPagina {
    id: "duplicados",
    rotulo: "Duplicados",
    keytip: "U",
    icone: "duplicados",
    grupos: &[
        DefGrupo {
            id: "duplicados.grupos",
            rotulo: "Grupos",
            acoes: &[
                DefAcao {
                    id: "duplicados.grupos",
                    rotulo: "Grupos",
                    dica: "Grupos de arquivos idênticos",
                },
                DefAcao {
                    id: "duplicados.reconstruir",
                    rotulo: "Reconstruir grupos",
                    dica: "Refaz os Grupos de Duplicados a partir dos hashes atuais (minutos em Catálogos grandes)",
                },
            ],
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
                    id: "duplicados.desmarcar",
                    rotulo: "Desmarcar",
                    dica: "Remove a marcação da cópia selecionada ou do grupo",
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
    resumo: "Grupos de arquivos idênticos, Cópia Principal, marcações, Cobertura e espaço recuperável.",
    desenhar,
};

/// Estado da página.
#[derive(Debug)]
pub struct Estado {
    vista: u64,
    pub lista: Carga<PaginaGrupos>,
    pub resumo: Carga<ResumoDuplicados>,
    pub selecionado: Option<GrupoId>,
    detalhe_de: Option<(GrupoId, u64)>,
    pub detalhe: Carga<DetalheGrupo>,
    /// Cópia selecionada na tabela do detalhe.
    pub copia: Option<ArquivoId>,
    /// Marcações valem para o grupo inteiro (em vez da cópia selecionada).
    pub grupo_inteiro: bool,
    pub confirmacao: Option<Plano>,
    pub ordem: OrdemDuplicados,
    pub painel_relatorio: bool,
    pub tipo_relatorio: TipoRelatorio,
    pub caminho_relatorio: String,
}

impl Default for Estado {
    fn default() -> Self {
        Self {
            vista: 0,
            lista: Carga::default(),
            resumo: Carga::default(),
            selecionado: None,
            detalhe_de: None,
            detalhe: Carga::default(),
            copia: None,
            grupo_inteiro: true,
            confirmacao: None,
            ordem: OrdemDuplicados::Recuperavel,
            painel_relatorio: false,
            tipo_relatorio: TipoRelatorio::Marcacoes,
            caminho_relatorio: caminho_padrao(TipoRelatorio::Marcacoes),
        }
    }
}

fn nome_do_tipo(t: TipoRelatorio) -> &'static str {
    match t {
        TipoRelatorio::Cobertura => "Cobertura por volume e pasta",
        TipoRelatorio::SemCobertura => "Arquivos sem Cobertura",
        TipoRelatorio::Espaco => "Espaço recuperável",
        TipoRelatorio::Marcacoes => "Marcações",
    }
}

fn sufixo_do_tipo(t: TipoRelatorio) -> &'static str {
    match t {
        TipoRelatorio::Cobertura => "cobertura",
        TipoRelatorio::SemCobertura => "sem-cobertura",
        TipoRelatorio::Espaco => "espaco",
        TipoRelatorio::Marcacoes => "marcacoes",
    }
}

/// Caminho sugerido para o relatório: `$HOME/dag-<tipo>.csv`.
fn caminho_padrao(t: TipoRelatorio) -> String {
    let base = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    base.join(format!("dag-{}.csv", sufixo_do_tipo(t)))
        .display()
        .to_string()
}

/// O caminho é `.csv` ou `.json` (o que o exportador aceita).
#[must_use]
pub fn caminho_de_relatorio_valido(c: &str) -> bool {
    let c = c.trim().to_lowercase();
    c.len() > 5 && (c.ends_with(".csv") || c.ends_with(".json"))
}

impl Estado {
    fn pedir_pagina(&mut self, leitor: &Leitor, cursor: Option<String>) {
        let ordem = self.ordem;
        self.lista.pedir(leitor.consultar(move |c| {
            let filtro = FiltroGrupos {
                ordem,
                ..FiltroGrupos::default()
            };
            duplicados::listar_grupos(c, &filtro, POR_PAGINA, cursor.as_deref()).map_err(erro)
        }));
    }

    /// Recolhe resultados e recarrega lista, resumo e detalhe quando o Catálogo mudou.
    pub fn atualizar(&mut self, leitor: &Leitor, geracao: u64) {
        if self.vista != geracao {
            self.vista = geracao;
            self.lista.limpar();
            self.pedir_pagina(leitor, None);
            self.resumo
                .pedir(leitor.consultar(|c| duplicados::resumo_duplicados(c).map_err(erro)));
        }
        self.lista.atualizar_com(|dado, nova| match dado {
            Some(d) => {
                d.grupos.extend(nova.grupos);
                d.total = nova.total;
                d.proximo_cursor = nova.proximo_cursor;
            }
            None => *dado = Some(nova),
        });
        self.resumo.atualizar();
        if let Some(g) = self.selecionado
            && self.detalhe_de != Some((g, geracao))
        {
            self.detalhe_de = Some((g, geracao));
            self.detalhe
                .pedir(leitor.consultar(move |c| duplicados::detalhar_grupo(c, g).map_err(erro)));
        }
        self.detalhe.atualizar();
    }

    fn selecionar(&mut self, g: GrupoId) {
        if self.selecionado != Some(g) {
            self.selecionado = Some(g);
            self.copia = None;
            self.detalhe_de = None;
            self.detalhe.limpar();
        }
    }
}

const COLUNAS_GRUPOS: [Coluna; 7] = [
    Coluna::dir("Tamanho", 85.0),
    Coluna::dir("Cópias", 55.0),
    Coluna::dir("Offline", 55.0),
    Coluna::dir("Recuperável", 95.0),
    Coluna::dir("Marcado", 85.0),
    Coluna::esq("Principal", 75.0),
    Coluna::esq("Hash", 80.0),
];

const COLUNAS_COPIAS: [Coluna; 8] = [
    Coluna::esq("", 24.0),
    Coluna::esq("Volume", 120.0),
    Coluna::esq("Dispositivo", 85.0),
    Coluna::esq("Caminho", 380.0),
    Coluna::esq("Estado", 120.0),
    Coluna::esq("Marcação", 120.0),
    Coluna::esq("Cobertura", 130.0),
    Coluna::esq("Espaço", 90.0),
];

fn texto_marcacao(c: &CopiaGrupo) -> String {
    match &c.marcacao {
        None => "—".into(),
        Some(m) => {
            let tipo = match m.tipo {
                TipoMarcacao::Backup => "Backup",
                TipoMarcacao::Delecao => "Deleção",
            };
            match m.estado {
                EstadoMarcacao::Ativa => tipo.to_string(),
                EstadoMarcacao::Pendente => format!("{tipo} (pendente)"),
            }
        }
    }
}

fn texto_cobertura(c: &CopiaGrupo) -> String {
    match (c.cobertura.coberto, c.cobertura.nao_verificada_desde) {
        (true, Some(t)) => format!("Sim (não verificada desde {})", data_s(Some(t))),
        (true, None) => "Sim".into(),
        (false, _) => "Não".into(),
    }
}

fn texto_espaco(c: ClasseRecuperavel) -> &'static str {
    match c {
        ClasseRecuperavel::Nao => "—",
        ClasseRecuperavel::Marcada => "marcado",
        ClasseRecuperavel::Estimada => "estimado",
    }
}

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.duplicados.atualizar(&app.leitor, app.geracao);
    resumo_de_espaco(ui, app);
    ui.separator();
    estado_da_carga(ui, &app.duplicados.lista);
    let vazio = app
        .duplicados
        .lista
        .dado
        .as_ref()
        .is_some_and(|p| p.grupos.is_empty());
    if vazio {
        estado_vazio(
            ui,
            "Nenhum grupo de duplicados",
            PAGINA.resumo,
            "Rode um Full Scan com hash: só arquivos com Hash Completo igual formam um grupo.",
        );
    } else if app.duplicados.lista.dado.is_some() {
        let altura = (super::altura_livre(ui)
            - if app.duplicados.painel_relatorio {
                150.0
            } else {
                40.0
            })
        .max(240.0);
        corpo(ui, app, altura);
    }
    painel_de_relatorio(ui, app);
}

fn resumo_de_espaco(ui: &mut egui::Ui, app: &mut AppDag) {
    ui.horizontal_wrapped(|ui| {
        match app.duplicados.resumo.dado.as_ref() {
            Some(r) => {
                ui.strong(format!(
                    "Espaço recuperável (estimativa): {}",
                    bytes_humanos(r.espaco_recuperavel_bytes)
                ));
                ui.label(format!(
                    "· {} grupo(s) · {} cópia(s)",
                    inteiro_pt_br(r.grupos),
                    inteiro_pt_br(r.copias)
                ));
                if r.marcacoes_pendentes > 0 {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("· {} marcação(ões) pendente(s)", r.marcacoes_pendentes),
                    );
                }
            }
            None if app.duplicados.resumo.carregando() => {
                ui.spinner();
                ui.weak("Calculando o espaço recuperável…");
            }
            None => {}
        }
        if ui.small_button("Relatório…").clicked() {
            app.duplicados.painel_relatorio = !app.duplicados.painel_relatorio;
        }
    });
    ui.weak(LIMITACOES_ESPACO[0]);
}

fn corpo(ui: &mut egui::Ui, app: &mut AppDag, altura: f32) {
    let ordem_antes = app.duplicados.ordem;
    let mut mais = false;
    let mut escolhido = None;
    ui.horizontal_top(|ui| {
        ui.allocate_ui(egui::vec2(ui.available_width() * 0.42, altura), |ui| {
            ui.horizontal(|ui| {
                ui.label("Ordenar por:");
                egui::ComboBox::from_id_salt("dup-ordem")
                    .selected_text(match app.duplicados.ordem {
                        OrdemDuplicados::Recuperavel => "espaço recuperável",
                        OrdemDuplicados::Tamanho => "tamanho do arquivo",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.duplicados.ordem,
                            OrdemDuplicados::Recuperavel,
                            "espaço recuperável",
                        );
                        ui.selectable_value(
                            &mut app.duplicados.ordem,
                            OrdemDuplicados::Tamanho,
                            "tamanho do arquivo",
                        );
                    });
            });
            if let Some(p) = app.duplicados.lista.dado.as_ref() {
                let sel = app.duplicados.selecionado;
                tabela(
                    ui,
                    "dup-grupos",
                    &COLUNAS_GRUPOS,
                    p.grupos.len(),
                    altura - 70.0,
                    |l, i| {
                        let g = &p.grupos[i];
                        if l.selecionavel(sel == Some(g.id), bytes_humanos(g.tamanho))
                            .clicked()
                        {
                            escolhido = Some(g.id);
                        }
                        l.texto(g.copias.to_string());
                        l.texto(g.copias_offline.to_string());
                        l.texto(bytes_humanos(g.espaco_recuperavel_bytes));
                        l.texto(bytes_humanos(g.bytes_marcados_delecao));
                        l.texto(match g.modo_principal {
                            ModoPrincipal::Auto => "auto",
                            ModoPrincipal::Manual => "manual",
                        });
                        l.texto(hash_curto(&g.hash.to_string()));
                    },
                );
                ui.horizontal(|ui| {
                    ui.weak(format!(
                        "{} de {} grupo(s)",
                        inteiro_pt_br(p.grupos.len() as u64),
                        inteiro_pt_br(p.total)
                    ));
                    if p.proximo_cursor.is_some()
                        && ui
                            .add_enabled(
                                !app.duplicados.lista.carregando(),
                                egui::Button::new("Carregar mais"),
                            )
                            .clicked()
                    {
                        mais = true;
                    }
                });
            }
        });
        ui.separator();
        ui.vertical(|ui| detalhe(ui, app, altura));
    });
    if let Some(g) = escolhido {
        app.duplicados.selecionar(g);
    }
    if app.duplicados.ordem != ordem_antes {
        app.duplicados.lista.limpar();
        app.duplicados.pedir_pagina(&app.leitor, None);
    }
    if mais {
        let cursor = app
            .duplicados
            .lista
            .dado
            .as_ref()
            .and_then(|p| p.proximo_cursor.clone());
        app.duplicados.pedir_pagina(&app.leitor, cursor);
    }
}

fn detalhe(ui: &mut egui::Ui, app: &mut AppDag, altura: f32) {
    if app.duplicados.selecionado.is_none() {
        ui.add_space(20.0);
        ui.weak("Selecione um grupo para ver as cópias.");
        return;
    }
    estado_da_carga(ui, &app.duplicados.detalhe);
    let Some(d) = app.duplicados.detalhe.dado.as_ref() else {
        return;
    };
    let g = &d.grupo;
    ui.strong(format!(
        "Grupo {} — {} por cópia, {} cópia(s)",
        g.id,
        bytes_humanos(g.tamanho),
        g.copias
    ));
    ui.weak(format!(
        "Hash {} · Cópia Principal: {} · recuperável {} (marcado {} + estimado {})",
        g.hash,
        match g.modo_principal {
            ModoPrincipal::Auto => "automática (Prioridade de Volume)",
            ModoPrincipal::Manual => "escolhida por você",
        },
        bytes_humanos(g.espaco_recuperavel_bytes),
        bytes_humanos(g.bytes_marcados_delecao),
        bytes_humanos(g.bytes_redundantes_estimados),
    ));
    let mut sel = app.duplicados.copia;
    let mut menu = None;
    tabela(
        ui,
        "dup-copias",
        &COLUNAS_COPIAS,
        d.copias.len(),
        (altura - 170.0).max(100.0),
        |l, i| {
            let c = &d.copias[i];
            let r = l.selecionavel(sel == Some(c.arquivo), if c.principal { "★" } else { " " });
            if r.clicked() {
                sel = Some(c.arquivo);
            }
            super::lixeira::menu(&r, &crate::lixeira::AlvoMenu::de_copia(c), &mut menu);
            l.texto(
                c.volume_rotulo
                    .clone()
                    .unwrap_or_else(|| format!("volume {}", c.volume)),
            );
            l.texto(
                c.dispositivo
                    .map_or_else(|| "—".into(), |x| format!("disco {x}")),
            );
            let extra = if c.outros_caminhos > 0 {
                format!("  (+{} hard link)", c.outros_caminhos)
            } else {
                String::new()
            };
            let r = l.texto(format!("{}{extra}", c.caminho));
            super::lixeira::menu(&r, &crate::lixeira::AlvoMenu::de_copia(c), &mut menu);
            l.texto(if c.conectado {
                "Conectado".to_string()
            } else {
                format!("Offline (visto {})", data_s(c.visto_em))
            });
            l.texto(texto_marcacao(c));
            l.texto(texto_cobertura(c));
            l.texto(texto_espaco(c.recuperavel));
        },
    );
    app.duplicados.copia = sel;
    if let Some(m) = menu {
        app.tratar_escolha(ui.ctx(), m);
    }
    ui.weak("★ = Cópia Principal. Cobertura = cópia com Marcação de Backup em outro Dispositivo Físico.");
    let motivo = app.motivo_sem_escrita();
    let mut pedir = None;
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(&mut app.duplicados.grupo_inteiro, true, "Grupo inteiro")
            .on_hover_text("Backup: todas as cópias. Deleção: todas menos a Cópia Principal.");
        ui.radio_value(
            &mut app.duplicados.grupo_inteiro,
            false,
            "Cópia selecionada",
        );
    });
    ui.horizontal_wrapped(|ui| {
        for acao in [
            AcaoMarcacao::DefinirPrincipal,
            AcaoMarcacao::MarcarBackup,
            AcaoMarcacao::MarcarDelecao,
            AcaoMarcacao::Desmarcar,
        ] {
            let b = ui.add_enabled(motivo.is_none(), egui::Button::new(acao.rotulo()));
            let b = match motivo {
                Some(m) => b.on_disabled_hover_text(m),
                None => b.on_hover_text("Pede confirmação; só o Catálogo muda"),
            };
            if b.clicked() {
                pedir = Some(acao);
            }
        }
    });
    if let Some(a) = pedir {
        app.pedir_confirmacao(a);
    }
}

/// Diálogo modal de confirmação das marcações, com o aviso ⚠ Única Cópia.
pub fn dialogo_de_confirmacao(ctx: &egui::Context, app: &mut AppDag) {
    let Some(plano) = app.duplicados.confirmacao.clone() else {
        return;
    };
    let mut decisao = None;
    egui::Window::new(format!("Confirmar: {}", plano.acao.rotulo()))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(format!(
                "{} cópia(s) do grupo {} serão afetadas:",
                plano.afetadas.len(),
                plano.grupo
            ));
            for (_, caminho) in plano.afetadas.iter().take(8) {
                ui.monospace(caminho);
            }
            if plano.afetadas.len() > 8 {
                ui.weak(format!("… e mais {}", plano.afetadas.len() - 8));
            }
            ui.add_space(6.0);
            for a in &plano.avisos {
                match a.gravidade {
                    Gravidade::Atencao => {
                        ui.colored_label(ui.visuals().warn_fg_color, &a.texto);
                    }
                    Gravidade::Info => {
                        ui.label(&a.texto);
                    }
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let rotulo = if plano.unica_copia {
                    "Confirmar mesmo assim"
                } else {
                    "Confirmar"
                };
                if ui.button(rotulo).clicked() {
                    decisao = Some(true);
                }
                if ui.button("Cancelar").clicked() {
                    decisao = Some(false);
                }
            });
        });
    match decisao {
        Some(true) => {
            app.duplicados.confirmacao = None;
            app.enviar(comando(&plano), Proposito::Marcacao(plano.acao));
        }
        Some(false) => app.duplicados.confirmacao = None,
        None => {}
    }
}

fn painel_de_relatorio(ui: &mut egui::Ui, app: &mut AppDag) {
    if !app.duplicados.painel_relatorio {
        return;
    }
    ui.separator();
    ui.strong("Exportar relatório (CSV ou JSON)");
    let mut exportar = false;
    ui.horizontal_wrapped(|ui| {
        let antes = app.duplicados.tipo_relatorio;
        egui::ComboBox::from_id_salt("dup-rel-tipo")
            .selected_text(nome_do_tipo(antes))
            .show_ui(ui, |ui| {
                for t in [
                    TipoRelatorio::Marcacoes,
                    TipoRelatorio::Espaco,
                    TipoRelatorio::Cobertura,
                    TipoRelatorio::SemCobertura,
                ] {
                    ui.selectable_value(&mut app.duplicados.tipo_relatorio, t, nome_do_tipo(t));
                }
            });
        if app.duplicados.tipo_relatorio != antes {
            app.duplicados.caminho_relatorio = caminho_padrao(app.duplicados.tipo_relatorio);
        }
        ui.add(
            egui::TextEdit::singleline(&mut app.duplicados.caminho_relatorio)
                .hint_text("/caminho/relatorio.csv")
                .desired_width(380.0),
        );
        let valido = caminho_de_relatorio_valido(&app.duplicados.caminho_relatorio);
        exportar = ui
            .add_enabled(valido, egui::Button::new("Exportar"))
            .on_disabled_hover_text("Use um caminho terminado em .csv ou .json")
            .on_hover_text("Grava o relatório inteiro; nunca sobrescreve um arquivo existente")
            .clicked();
    });
    if exportar {
        let cmd = Comando::Relatorio {
            tipo: app.duplicados.tipo_relatorio,
            nivel: (app.duplicados.tipo_relatorio == TipoRelatorio::Espaco)
                .then_some(NivelEspaco::Volume),
            limite: None,
            cursor: None,
            saida: Some(app.duplicados.caminho_relatorio.trim().to_string()),
        };
        app.enviar(cmd, Proposito::Exportar);
        app.aviso(Tom::Info, "Exportando o relatório…");
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn caminho_de_relatorio_exige_csv_ou_json() {
        assert!(caminho_de_relatorio_valido("/tmp/x.csv"));
        assert!(caminho_de_relatorio_valido(" /tmp/X.JSON "));
        assert!(!caminho_de_relatorio_valido("/tmp/x.txt"));
        assert!(!caminho_de_relatorio_valido(".csv"));
        assert!(!caminho_de_relatorio_valido(""));
    }
}
