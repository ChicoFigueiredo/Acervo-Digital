//! Página Varredura: raízes, Full Scan (com ou sem hash), pausar/retomar/cancelar pelo
//! `ControleScan`, progresso ao vivo sem percentual inventado e tabela das últimas execuções.

use std::time::Duration;

use super::tabela::{Coluna, tabela};
use super::{DefAcao, DefGrupo, DefPagina, cabecalho, estado_da_carga};
use crate::app::AppDag;
use crate::formato::{bytes_humanos, data_s, duracao, inteiro_pt_br, percentual, taxa_itens};
use crate::leitor::{Carga, erro};
use crate::progresso::{FaseScan, fase_texto, fracao_global};
use crate::selecao::{
    aviso_raiz_sistema, normalizar_raiz, raiz_do_volume, raizes_dos_discos_fisicos,
};
use bancada_casca::{Tom, egui};
use dag_scanner::EstadoControle;
use dag_store::consulta_gui::{LinhaExecucao, ultimas_execucoes};

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
                    dica: "Inventário, Hash Completo e Grupos de Duplicados",
                },
                DefAcao {
                    id: "varredura.full_scan_sem_hash",
                    rotulo: "Full Scan sem hash",
                    dica: "Inventário rápido: pula Hash Completo, duplicados e marcações",
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

/// Estado da página.
#[derive(Debug, Default)]
pub struct Estado {
    /// Raízes escolhidas para a próxima varredura.
    pub raizes: Vec<String>,
    pub campo: String,
    pub execucoes: Carga<Vec<LinhaExecucao>>,
    vista: u64,
}

impl Estado {
    /// Adiciona uma raiz (sem repetir; barra final removida).
    pub fn adicionar_raiz(&mut self, caminho: impl Into<String>) {
        let c = normalizar_raiz(&caminho.into());
        if !c.is_empty() && !self.raizes.contains(&c) {
            self.raizes.push(c);
        }
    }
}

const COLUNAS_EXECUCOES: [Coluna; 11] = [
    Coluna::dir("#", 45.0),
    Coluna::esq("Modo", 55.0),
    Coluna::esq("Estado", 130.0),
    Coluna::esq("Início (UTC)", 150.0),
    Coluna::esq("Fim (UTC)", 150.0),
    Coluna::dir("Pastas", 80.0),
    Coluna::dir("Arquivos", 90.0),
    Coluna::dir("Bytes", 90.0),
    Coluna::dir("Erros", 55.0),
    Coluna::esq("Raízes", 260.0),
    Coluna::esq("", 90.0),
];

fn retomavel(estado: &str) -> bool {
    matches!(estado, "pausada" | "interrompida" | "cancelada")
}

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    if app.varredura.vista != app.geracao {
        app.varredura.vista = app.geracao;
        app.varredura.execucoes.pedir(
            app.leitor
                .consultar(|c| ultimas_execucoes(c, 30).map_err(erro)),
        );
    }
    app.varredura.execucoes.atualizar();
    app.discos.atualizar(&app.leitor, app.geracao);
    app.garantir_classificacao();

    let mut agir = Acao::Nada;
    secao_raizes(ui, app);
    ui.add_space(4.0);
    botoes(ui, app, &mut agir);
    ui.separator();
    progresso(ui, app);
    ui.add_space(6.0);
    ui.strong("Últimas varreduras");
    estado_da_carga(ui, &app.varredura.execucoes);
    let pode = app.motivo_sem_escrita();
    let altura = ui.available_height().max(120.0);
    if let Some(execs) = app.varredura.execucoes.dado.as_deref() {
        if execs.is_empty() {
            ui.weak("Nenhuma varredura registrada ainda.");
        } else {
            tabela(
                ui,
                "execucoes",
                &COLUNAS_EXECUCOES,
                execs.len(),
                altura,
                |l, i| {
                    let e = &execs[i];
                    l.texto(e.id.to_string());
                    l.texto(e.modo.clone());
                    l.texto(format!("{} / {}", e.estado, e.fase));
                    l.texto(data_s(Some(e.iniciada_em)));
                    l.texto(data_s(e.encerrada_em));
                    l.texto(inteiro_pt_br(e.pastas));
                    l.texto(inteiro_pt_br(e.arquivos));
                    l.texto(bytes_humanos(e.bytes_logicos));
                    l.texto(inteiro_pt_br(e.erros));
                    l.texto(e.raizes.join(", "));
                    l.ui(|ui| {
                        if retomavel(&e.estado)
                            && ui
                                .add_enabled(pode.is_none(), egui::Button::new("Retomar"))
                                .on_disabled_hover_text(pode.unwrap_or(""))
                                .clicked()
                        {
                            agir = Acao::Retomar(e.id);
                        }
                    });
                },
            );
        }
    }
    match agir {
        Acao::Nada => {}
        Acao::Iniciar { sem_hash } => app.iniciar_full_scan(sem_hash),
        Acao::Retomar(id) => app.retomar_execucao(id),
        Acao::AlternarPausa => app.alternar_pausa(),
        Acao::Cancelar => app.worker.cancelar(),
    }
}

enum Acao {
    Nada,
    Iniciar { sem_hash: bool },
    Retomar(i64),
    AlternarPausa,
    Cancelar,
}

fn volumes_detectados(ui: &mut egui::Ui, app: &mut AppDag) {
    let (linhas, ocultos) = app.linhas_de_discos();
    ui.strong("Volumes detectados");
    if linhas.is_empty() {
        ui.weak("Nenhum volume detectado ainda (veja a página Discos).");
        return;
    }
    let mut alternar: Option<(String, bool)> = None;
    ui.horizontal_wrapped(|ui| {
        for d in &linhas {
            let Some(raiz) = raiz_do_volume(d) else {
                continue;
            };
            let mut marcada = app.varredura.raizes.contains(&raiz);
            let nome = d.volume.rotulo.as_deref().filter(|r| !r.is_empty());
            let texto = format!(
                "{raiz}{} [{}{}]",
                nome.map_or_else(String::new, |n| format!(" ({n})")),
                d.classe.rotulo(),
                if d.sistema { ", sistema" } else { "" }
            );
            if ui.checkbox(&mut marcada, texto).changed() {
                alternar = Some((raiz, marcada));
            }
        }
    });
    let fisicos = raizes_dos_discos_fisicos(&linhas);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!fisicos.is_empty(), egui::Button::new("Selecionar todos os discos físicos"))
            .on_hover_text("Marca os volumes físicos conectados (sem partições de sistema, removíveis ou de rede)")
            .clicked()
        {
            for r in fisicos {
                app.varredura.adicionar_raiz(r);
            }
        }
        if ocultos > 0 {
            ui.weak(format!("{ocultos} volume(s) de imagem/virtuais ocultos"));
        }
    });
    if let Some((raiz, marcada)) = alternar {
        if marcada {
            app.varredura.adicionar_raiz(raiz);
        } else {
            app.varredura.raizes.retain(|r| r != &raiz);
        }
    }
    if let Some(a) = aviso_raiz_sistema(&app.varredura.raizes) {
        ui.colored_label(ui.visuals().warn_fg_color, a);
    }
    ui.add_space(4.0);
}

fn secao_raizes(ui: &mut egui::Ui, app: &mut AppDag) {
    volumes_detectados(ui, app);
    ui.strong("Raízes da varredura (volumes marcados e pastas avulsas)");
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(&mut app.varredura.campo)
                .hint_text("/caminho/da/pasta ou ponto de montagem")
                .desired_width(420.0),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button("Adicionar").clicked() || enter) && !app.varredura.campo.trim().is_empty() {
            let c = std::mem::take(&mut app.varredura.campo);
            match std::fs::metadata(c.trim()) {
                Ok(m) if m.is_dir() => app.varredura.adicionar_raiz(c),
                Ok(_) => app.aviso(Tom::Aviso, format!("{} não é uma pasta.", c.trim())),
                Err(e) => app.aviso(
                    Tom::Aviso,
                    format!("Não foi possível ler {}: {e}", c.trim()),
                ),
            }
        }
    });
    let mut remover = None;
    if app.varredura.raizes.is_empty() {
        ui.weak("Nenhuma raiz escolhida. A varredura é somente leitura: o DAG nunca altera o que varre.");
    }
    for (i, r) in app.varredura.raizes.iter().enumerate() {
        ui.horizontal(|ui| {
            if ui.small_button("Remover").clicked() {
                remover = Some(i);
            }
            ui.monospace(r);
        });
    }
    if let Some(i) = remover {
        app.varredura.raizes.remove(i);
    }
}

fn botoes(ui: &mut egui::Ui, app: &AppDag, agir: &mut Acao) {
    let sem_escrita = app.motivo_sem_escrita();
    let sem_raiz = app.varredura.raizes.is_empty();
    let motivo_iniciar = sem_escrita.or(sem_raiz.then_some("Escolha ao menos uma raiz"));
    let estado = app.estado_do_scan();
    ui.horizontal(|ui| {
        let botao = |ui: &mut egui::Ui, rotulo: &str, motivo: Option<&str>, dica: &str| {
            let b = ui.add_enabled(motivo.is_none(), egui::Button::new(rotulo));
            match motivo {
                Some(m) => b.on_disabled_hover_text(m).clicked(),
                None => b.on_hover_text(dica).clicked(),
            }
        };
        if botao(
            ui,
            "Full Scan",
            motivo_iniciar,
            "Inventário, Hash Completo e duplicados",
        ) {
            *agir = Acao::Iniciar { sem_hash: false };
        }
        if botao(
            ui,
            "Full Scan sem hash",
            motivo_iniciar,
            "Só o inventário (sem hash, duplicados nem marcações)",
        ) {
            *agir = Acao::Iniciar { sem_hash: true };
        }
        ui.separator();
        let m_pausa =
            (estado != Some(EstadoControle::Executando)).then_some("Nenhuma varredura em execução");
        if botao(ui, "Pausar", m_pausa, "Pausa com checkpoint") {
            *agir = Acao::AlternarPausa;
        }
        let m_ret =
            (estado != Some(EstadoControle::Pausado)).then_some("Nenhuma varredura pausada");
        if botao(ui, "Retomar", m_ret, "Continua a varredura pausada") {
            *agir = Acao::AlternarPausa;
        }
        let m_canc = (!app.scan_ativo()).then_some("Nenhuma varredura em andamento");
        if botao(
            ui,
            "Cancelar",
            m_canc,
            "Cancela depois de gravar o que já foi lido; dá para retomar",
        ) {
            *agir = Acao::Cancelar;
        }
        ui.separator();
        let _ = botao(ui, "Fast Scan", Some("Fast Scan chega no incremento 5"), "");
    });
}

fn progresso(ui: &mut egui::Ui, app: &AppDag) {
    let Some(s) = &app.scan else {
        if let Some(r) = &app.ultimo_scan {
            ui.strong(format!("Última varredura desta sessão: #{}", r.execucao));
            ui.label(format!(
                "{} — {} pastas, {} arquivos, {} em {}; {} erro(s), {} ignorado(s).{}{}",
                r.desfecho,
                inteiro_pt_br(r.pastas),
                inteiro_pt_br(r.arquivos),
                bytes_humanos(r.bytes_logicos),
                duracao(Duration::from_millis(r.duracao_ms)),
                inteiro_pt_br(r.erros),
                inteiro_pt_br(r.ignorados),
                r.hasheados.map_or_else(String::new, |h| format!(
                    " {} arquivo(s) hasheado(s).",
                    inteiro_pt_br(h)
                )),
                r.grupos_duplicados.map_or_else(String::new, |g| format!(
                    " {} grupo(s) de duplicados.",
                    inteiro_pt_br(g)
                )),
            ));
        } else {
            ui.weak("Nenhuma varredura em andamento.");
        }
        return;
    };
    let pausado = app.estado_do_scan() == Some(EstadoControle::Pausado);
    let fase = fase_texto(s.fase(), pausado, s.sem_hash);
    ui.horizontal(|ui| {
        if !pausado {
            ui.spinner();
        }
        ui.strong(format!("Em andamento — {fase}"));
        ui.weak(format!("({})", duracao(s.inicio.elapsed())));
    });
    let hash = s.andamento_hash();
    match fracao_global(s.fase(), s.sem_hash, hash.as_ref()) {
        Some(f) => {
            let h = hash.unwrap_or_default();
            ui.add(egui::ProgressBar::new(f).desired_width(480.0).text(format!(
                "{} de {} arquivos ({})",
                inteiro_pt_br(h.concluidos),
                inteiro_pt_br(h.total),
                percentual(Some(f64::from(f) * 100.0))
            )));
            let mut linha = Vec::new();
            if let Some(b) = &s.bytes_hash {
                linha.push(format!(
                    "{} de {} lidos",
                    bytes_humanos(b.bytes_lidos),
                    bytes_humanos(b.bytes_total)
                ));
            }
            if let Some(t) = s.taxa_hash() {
                linha.push(taxa_itens(t, "arquivos"));
            }
            if let Some(r) = s.restante_hash() {
                linha.push(format!("restante estimado: {} (estimativa)", duracao(r)));
            }
            if !linha.is_empty() {
                ui.weak(linha.join(" · "));
            }
            if let Some(b) = &s.bytes_hash {
                // Um disco físico por thread: mostra o andamento de cada disco.
                for d in &b.discos {
                    ui.weak(format!(
                        "Disco {}: {} de {} lidos ({}/s){}",
                        d.disco,
                        bytes_humanos(d.bytes_lidos),
                        bytes_humanos(d.bytes_total),
                        bytes_humanos(d.bytes_por_seg as u64),
                        if d.concluido { " — concluído" } else { "" }
                    ));
                }
            }
        }
        None => {
            ui.weak(
                "Sem percentual global: o total de diretórios ainda não é conhecido durante \
                 o inventário. Os números abaixo são contagens reais.",
            );
        }
    }
    if let Some(p) = &s.progresso {
        egui::Grid::new("progresso-scan")
            .num_columns(4)
            .show(ui, |ui| {
                ui.label("Diretórios");
                ui.strong(inteiro_pt_br(p.pastas));
                ui.label("Arquivos");
                ui.strong(inteiro_pt_br(p.arquivos));
                ui.end_row();
                ui.label("Bytes inventariados");
                ui.strong(bytes_humanos(p.bytes_logicos));
                ui.label("Entradas especiais");
                ui.strong(inteiro_pt_br(p.especiais));
                ui.end_row();
                ui.label("Velocidade");
                ui.strong(taxa_itens(p.itens_por_s, "itens"));
                ui.label("Fila (diretórios pendentes)");
                ui.strong(inteiro_pt_br(p.pendentes));
                ui.end_row();
                ui.label("Ignorados");
                ui.strong(inteiro_pt_br(p.ignorados));
                ui.label("Erros");
                ui.strong(inteiro_pt_br(p.erros));
                ui.end_row();
                ui.label("Volume atual");
                ui.strong("—");
                ui.label("Duração do inventário");
                ui.strong(duracao(p.decorrido));
                ui.end_row();
            });
        if s.fase() == Some(FaseScan::Concluido) && !s.sem_hash && hash.is_none() {
            ui.weak(
                "Preparando a fila de Hash Completo… (a contagem de arquivos aparece em instantes)",
            );
        }
    } else {
        ui.weak("Descobrindo volumes e preparando o inventário…");
    }
}
