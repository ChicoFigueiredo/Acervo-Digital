//! Página Varredura: raízes, Full Scan (com ou sem hash), pausar/retomar/cancelar pelo
//! `ControleScan`, progresso ao vivo sem percentual inventado e tabela das últimas execuções.

use std::time::Duration;

use super::tabela::{Chave, Coluna, tabela};
use super::{DefAcao, DefGrupo, DefPagina, cabecalho, estado_da_carga};
use crate::app::AppDag;
use crate::formato::{bytes_humanos, data_s, duracao, inteiro_pt_br, taxa_itens};
use crate::leitor::{Carga, erro};
use crate::selecao::{
    aviso_raiz_sistema, normalizar_raiz, raiz_do_volume, raizes_dos_discos_fisicos,
};
use bancada_casca::{Tom, egui};
use dag_core::PainelJob;
use dag_core::painel::{self, EstadoDisco, EtapaPainel, PainelHash};
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

const COLUNAS_DISCOS: [Coluna; 8] = [
    Coluna::esq("Disco", 170.0),
    Coluna::esq("Volumes", 80.0),
    Coluna::esq("Threads", 70.0),
    Coluna::dir("Arquivos", 150.0),
    Coluna::dir("%", 60.0),
    Coluna::dir("MB/s", 70.0),
    Coluna::esq("Estado", 110.0),
    Coluna::esq("Restante", 110.0),
];

const COLUNAS_JOBS: [Coluna; 6] = [
    Coluna::dir("#", 50.0),
    Coluna::esq("Tipo", 120.0),
    Coluna::esq("Origem", 100.0),
    Coluna::esq("Estado", 110.0),
    Coluna::esq("Progresso", 320.0),
    Coluna::esq("", 90.0).sem_ordem(),
];

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
    Coluna::esq("", 90.0).sem_ordem(),
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
    trabalhos_do_agente(ui, app);
    ui.add_space(6.0);
    ui.strong("Últimas varreduras");
    estado_da_carga(ui, &app.varredura.execucoes);
    let pode = app.motivo_sem_escrita();
    let altura = super::altura_livre(ui).max(240.0);
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
                |c, i| {
                    let e = &execs[i];
                    match c {
                        0 => Chave::num(e.id),
                        1 => Chave::texto(&e.modo),
                        2 => Chave::texto(&format!("{} / {}", e.estado, e.fase)),
                        3 => Chave::num(e.iniciada_em),
                        4 => Chave::opt_num(e.encerrada_em),
                        5 => Chave::num(e.pastas),
                        6 => Chave::num(e.arquivos),
                        7 => Chave::num(e.bytes_logicos),
                        8 => Chave::num(e.erros),
                        _ => Chave::texto(&e.raizes.join(", ")),
                    }
                },
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
        ui.weak("Nenhuma raiz escolhida. A varredura só lê: o DAG nunca altera o que varre.");
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
    if app.scan.is_none() {
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
    }
    if let Some(p) = app.painel_do_scan() {
        desenhar_painel(ui, &p);
    }
}

/// Desenha o painel do trabalho em andamento: o que roda, onde, totais, discos e tempo restante.
/// Pública para o teste de desenho sem display.
pub fn desenhar_painel(ui: &mut egui::Ui, p: &PainelJob) {
    ui.horizontal(|ui| {
        if !p.pausado {
            ui.spinner();
        }
        ui.strong(format!(
            "Em andamento — {}{}",
            p.titulo,
            if p.pausado { " (pausado)" } else { "" }
        ));
        if let Some(s) = p.decorrido_s {
            ui.weak(format!("há {}", painel::duracao_curta(s)));
        }
    });
    ui.label(format!(
        "Onde: {}{}",
        p.onde.texto(),
        p.quem
            .map_or_else(String::new, |q| format!(" · iniciado por: {q}"))
    ));
    match p.etapa {
        EtapaPainel::NaFila => {
            ui.weak("Na fila do agente, esperando o trabalho em curso.");
        }
        EtapaPainel::Preparando => {
            ui.weak(match &p.etapa_texto {
                Some(t) => format!("Preparando: {t}"),
                None => "Preparando: descobrindo volumes e conferindo o Catálogo…".to_string(),
            });
        }
        EtapaPainel::PreparandoHash => {
            ui.weak(
                "Montando a fila de hash (pode levar vários minutos em Catálogos grandes). \
                 Os totais aparecem em seguida.",
            );
        }
        _ => {}
    }
    if let Some(h) = &p.hash {
        painel_hash(ui, h);
    }
    if let Some(i) = &p.inventario {
        ui.weak(
            "Sem percentual global: o total ainda não é conhecido durante o inventário. \
             Os números abaixo são o que já foi encontrado.",
        );
        egui::Grid::new("progresso-inventario")
            .num_columns(4)
            .show(ui, |ui| {
                ui.label("Pastas encontradas");
                ui.strong(inteiro_pt_br(i.pastas));
                ui.label("Arquivos encontrados");
                ui.strong(inteiro_pt_br(i.arquivos));
                ui.end_row();
                ui.label("Bytes encontrados");
                ui.strong(bytes_humanos(i.bytes));
                ui.label("Velocidade");
                ui.strong(taxa_itens(i.itens_por_s, "itens"));
                ui.end_row();
                ui.label("Pastas por visitar");
                ui.strong(inteiro_pt_br(i.pendentes));
                ui.label("Erros");
                ui.strong(inteiro_pt_br(i.erros));
                ui.end_row();
            });
    }
    if let Some(c) = &p.catalogo {
        ui.weak(format!(
            "Catálogo: {} arquivos{}",
            inteiro_pt_br(c.arquivos),
            c.com_hash.map_or_else(String::new, |h| format!(
                ", {} com hash ({})",
                inteiro_pt_br(h),
                painel::pct_texto(painel::percentual(h, c.arquivos))
            ))
        ));
    }
}

fn painel_hash(ui: &mut egui::Ui, h: &PainelHash) {
    if h.threads_por_disco > 0 || h.max_discos > 0 {
        ui.label(format!(
            "Configuração em vigor: {} thread(s) por disco, até {} disco(s) em paralelo",
            h.threads_por_disco, h.max_discos
        ));
    }
    if h.arquivos_total == 0 {
        return;
    }
    let f = h.pct_arquivos.map_or(0.0, |p| (p / 100.0) as f32);
    ui.add(egui::ProgressBar::new(f).desired_width(520.0).text(format!(
        "{} com hash de {} arquivos ({})",
        inteiro_pt_br(h.com_hash),
        inteiro_pt_br(h.arquivos_total),
        painel::pct_texto(h.pct_arquivos)
    )));
    let fb = h.pct_bytes.map_or(0.0, |p| (p / 100.0) as f32);
    ui.add(
        egui::ProgressBar::new(fb)
            .desired_width(520.0)
            .text(format!(
                "{} de {} lidos nesta etapa ({})",
                bytes_humanos(h.bytes_lidos),
                bytes_humanos(h.bytes_total),
                painel::pct_texto(h.pct_bytes)
            )),
    );
    egui::Grid::new("progresso-hash")
        .num_columns(4)
        .show(ui, |ui| {
            ui.label("Pendentes");
            ui.strong(inteiro_pt_br(h.pendentes));
            ui.label("Hashes reaproveitados");
            ui.strong(inteiro_pt_br(h.reaproveitados));
            ui.end_row();
            ui.label("Erros");
            ui.strong(inteiro_pt_br(h.erros));
            ui.label("Vazão");
            ui.strong(format!(
                "{} · {}",
                h.arquivos_por_min.map_or_else(
                    || "estimando arquivos/min…".to_string(),
                    |a| format!("{} arquivos/min", inteiro_pt_br(a as u64))
                ),
                painel::vazao_texto(h.bytes_por_seg)
            ));
            ui.end_row();
            ui.label("Tempo restante");
            ui.strong(painel::eta_texto(h.eta_s));
            ui.end_row();
        });
    if h.discos.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.strong("Por disco físico");
    let altura = (h.discos.len() as f32 * 26.0 + 40.0).min(260.0);
    tabela(
        ui,
        "progresso-discos",
        &COLUNAS_DISCOS,
        h.discos.len(),
        altura,
        |c, i| {
            let d = &h.discos[i];
            match c {
                0 => Chave::texto(&d.nome),
                1 => Chave::opt_num(d.volumes.iter().min().copied()),
                2 => Chave::num(d.leitores_ativos),
                3 => Chave::num(d.feitos),
                4 => Chave::opt_real(d.pct),
                5 => Chave::real(d.bytes_por_seg),
                6 => Chave::texto(d.estado.rotulo()),
                _ => Chave::opt_real(d.eta_s.map(|s| s as f64)),
            }
        },
        |l, i| {
            let d = &h.discos[i];
            l.texto(d.nome.clone());
            l.texto(
                d.volumes
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            l.texto(format!("{}/{}", d.leitores_ativos, d.threads));
            l.texto(format!(
                "{} / {}",
                inteiro_pt_br(d.feitos),
                inteiro_pt_br(d.total)
            ));
            l.texto(painel::pct_texto(d.pct));
            l.texto(format!("{:.1}", d.bytes_por_seg / (1024.0 * 1024.0)).replace('.', ","));
            l.texto(d.estado.rotulo());
            l.texto(match d.estado {
                EstadoDisco::Concluido => "—".to_string(),
                _ => d
                    .eta_s
                    .map_or_else(|| "estimando…".to_string(), painel::duracao_curta),
            });
        },
    );
    // Arquivos grandes em leitura: o contador de arquivos só anda quando um termina.
    for d in &h.discos {
        for l in &d.lendo {
            ui.label(format!("{}: {}", d.nome, painel::lendo_texto(l)));
        }
    }
}

/// Todos os trabalhos do agente (qualquer cliente os pode ter iniciado), com Cancelar para os
/// que ainda esperam na fila.
fn trabalhos_do_agente(ui: &mut egui::Ui, app: &AppDag) {
    let jobs: Vec<_> = app
        .worker
        .jobs()
        .iter()
        .filter(|j| j.tipo != dag_core::TipoJob::Monitor)
        .collect();
    if jobs.len() < 2 {
        return;
    }
    ui.add_space(4.0);
    ui.strong("Trabalhos do agente");
    tabela(
        ui,
        "trabalhos-do-agente",
        &COLUNAS_JOBS,
        jobs.len(),
        (jobs.len() as f32 * 26.0 + 40.0).min(220.0),
        |c, i| {
            let j = jobs[i];
            match c {
                0 => Chave::num(j.id),
                1 => Chave::texto(j.tipo.rotulo()),
                2 => Chave::texto(j.origem.rotulo()),
                3 => Chave::texto(j.estado.rotulo()),
                _ => Chave::texto(&j.resumo_progresso()),
            }
        },
        |l, i| {
            let j = jobs[i];
            l.texto(format!("#{}", j.id));
            l.texto(j.tipo.rotulo());
            l.texto(j.origem.rotulo());
            l.texto(j.estado.rotulo());
            l.texto(j.resumo_progresso());
            l.ui(|ui| {
                if j.estado == dag_core::EstadoJob::Enfileirado
                    && ui
                        .small_button("Cancelar")
                        .on_hover_text("Tira da fila sem começar")
                        .clicked()
                {
                    app.worker.cancelar_job(j.id);
                }
            });
        },
    );
}
