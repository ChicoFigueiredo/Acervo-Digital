//! Biblioteca > Visão Geral: bibliotecas registradas, fila, alertas, espaço recuperável seguro,
//! e os botões Sincronizar, Enfileirar e Processar (com a IA ligada ou desligada).

use bancada_casca::{Tom, egui};
use dag_core::{MotivoFila, PapelRegistro, RelatorioEspacoInfo};

use super::comandos;
use super::texto::{self, rotulo_motivo, rotulo_papel};
use super::{AcaoBib, Consulta, acao, erro_da_acao, estado_da_consulta, progresso_do_job, quadro};
use crate::app::AppDag;
use crate::formato::{bytes_humanos, data_s, inteiro_pt_br};
use crate::paginas::tabela::{Chave, Coluna, tabela};
use crate::paginas::{DefGrupo, DefPagina, cabecalho, estado_vazio};

pub static PAGINA: DefPagina = DefPagina {
    id: "biblioteca",
    rotulo: "Visão Geral",
    keytip: "V",
    icone: "biblioteca",
    grupos: &[
        DefGrupo {
            id: "biblioteca.trabalho",
            rotulo: "Trabalho",
            acoes: &[
                acao(
                    "biblioteca.sincronizar",
                    "Sincronizar",
                    "Lê a Biblioteca Homologada (somente leitura) e confere que nada mudou",
                ),
                acao(
                    "biblioteca.enfileirar",
                    "Enfileirar",
                    "Põe na fila os livros sob as raízes digitadas na Visão Geral",
                ),
                acao(
                    "biblioteca.processar",
                    "Processar",
                    "Processa a fila (duplicata exata antes de qualquer extração)",
                ),
                acao(
                    "biblioteca.atualizar",
                    "Atualizar",
                    "Relê tudo o que a seção Biblioteca mostra",
                ),
            ],
        },
        DefGrupo {
            id: "biblioteca.job",
            rotulo: "Trabalho em andamento",
            acoes: &[
                acao(
                    "biblioteca.job_pausar",
                    "Pausar",
                    "Pausa o trabalho da Biblioteca entre dois itens",
                ),
                acao(
                    "biblioteca.job_retomar",
                    "Retomar",
                    "Continua o trabalho pausado",
                ),
                acao(
                    "biblioteca.job_cancelar",
                    "Cancelar",
                    "Termina no próximo item; o que já foi feito fica gravado",
                ),
            ],
        },
    ],
    resumo: "Bibliotecas registradas, fila, alertas e espaço recuperável com segurança.",
    desenhar,
};

/// Estado de tela da página.
#[derive(Debug)]
pub struct Ui {
    /// Raízes a enfileirar, uma por linha.
    pub raizes: String,
    pub motivo: MotivoFila,
    /// Pedir a IA local no Processar (só vale com a IA ligada na configuração).
    pub usar_ia: bool,
    pub reg_caminho: String,
    pub reg_papel: PapelRegistro,
    pub reg_nome: String,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            raizes: String::new(),
            motivo: MotivoFila::Novo,
            usar_ia: true,
            reg_caminho: String::new(),
            reg_papel: PapelRegistro::Homologada,
            reg_nome: String::new(),
        }
    }
}

/// Espaço de um relatório: estimado recuperável, incerto e efetivamente recuperado, somados
/// por Volume.
#[must_use]
pub fn totais_de_espaco(r: &RelatorioEspacoInfo) -> (u64, u64, u64) {
    r.volumes.iter().fold((0, 0, 0), |(e, i, f), v| {
        (
            e + v.estimativa_recuperavel,
            i + v.economia_incerta,
            f + v.efetivamente_recuperado,
        )
    })
}

pub(super) fn sincronizar(app: &mut AppDag) {
    let Some(id) = app.biblioteca.homologada().map(|b| b.id) else {
        app.aviso(Tom::Aviso, "Registre a Biblioteca Homologada primeiro.");
        return;
    };
    app.bib_acao(AcaoBib::Sincronizar, comandos::sincronizar(id));
}

pub(super) fn enfileirar(app: &mut AppDag) {
    let raizes = texto::raizes_de_texto(&app.biblioteca.visao.raizes);
    if raizes.is_empty() {
        app.aviso(Tom::Aviso, "Digite ao menos uma raiz (caminho absoluto).");
        return;
    }
    let motivo = app.biblioteca.visao.motivo;
    app.bib_acao(AcaoBib::Enfileirar, comandos::enfileirar(raizes, motivo));
}

pub(super) fn processar(app: &mut AppDag) {
    let ia_ligada = app.biblioteca.config.as_ref().is_some_and(|c| c.ia.ativa);
    let usar = app.biblioteca.visao.usar_ia;
    app.bib_acao(AcaoBib::Processar, comandos::processar(usar, ia_ligada));
}

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.bib_quadro(ui.ctx());
    app.bib_carregar(&[
        Consulta::Bibliotecas,
        Consulta::Resumo,
        Consulta::Espaco,
        Consulta::Fila,
        Consulta::Config,
    ]);
    estado_da_consulta(ui, &app.biblioteca, Consulta::Bibliotecas);
    quadro(ui, "Trabalho em andamento", |ui| progresso_do_job(ui, app));
    ui.add_space(6.0);
    bibliotecas(ui, app);
    ui.add_space(6.0);
    ui.columns(2, |cols| {
        fila_e_alertas(&mut cols[0], app);
        espaco(&mut cols[1], app);
    });
    ui.add_space(6.0);
    acoes(ui, app);
    ui.add_space(6.0);
    registro(ui, app);
    ui.add_space(6.0);
    ultimos_resultados(ui, app);
}

const COLUNAS: [Coluna; 8] = [
    Coluna::esq("Papel", 100.0),
    Coluna::esq("Nome", 150.0),
    Coluna::esq("Caminho", 300.0),
    Coluna::dir("Livros", 70.0),
    Coluna::dir("Formatos", 80.0),
    Coluna::esq("Estado", 90.0),
    Coluna::esq("Última sincronização (UTC)", 170.0),
    Coluna::esq("Resultado", 260.0),
];

fn bibliotecas(ui: &mut egui::Ui, app: &mut AppDag) {
    let lista = app.biblioteca.bibliotecas.clone().unwrap_or_default();
    quadro(ui, "Bibliotecas registradas", |ui| {
        if lista.is_empty() {
            if app.biblioteca.bibliotecas.is_some() {
                estado_vazio(
                    ui,
                    "Nenhuma biblioteca registrada",
                    PAGINA.resumo,
                    "Registre a Biblioteca Homologada (somente leitura) e a A Classificar abaixo.",
                );
            }
            return;
        }
        tabela(
            ui,
            "bib-bibliotecas",
            &COLUNAS,
            lista.len(),
            (lista.len() as f32 * 28.0 + 40.0).min(220.0),
            |c, i| {
                let b = &lista[i];
                match c {
                    0 => Chave::texto(rotulo_papel(b.papel)),
                    1 => Chave::texto(&b.nome),
                    2 => Chave::texto(&b.caminho),
                    3 => Chave::num(b.livros),
                    4 => Chave::num(b.formatos),
                    5 => Chave::texto(&b.estado),
                    6 => Chave::opt_num(b.ultima_sincronizacao),
                    _ => Chave::opt_texto(b.ultimo_resultado.as_deref()),
                }
            },
            |l, i| {
                let b = &lista[i];
                l.texto(rotulo_papel(b.papel));
                l.texto(texto::texto_puro(&b.nome, 80));
                l.texto(b.caminho.clone());
                l.texto(inteiro_pt_br(u64::try_from(b.livros).unwrap_or(0)));
                l.texto(inteiro_pt_br(u64::try_from(b.formatos).unwrap_or(0)));
                l.texto(b.estado.clone());
                l.texto(data_s(b.ultima_sincronizacao));
                l.texto(texto::texto_puro(
                    b.ultimo_resultado.as_deref().unwrap_or("—"),
                    120,
                ));
            },
        );
        ui.weak(
            "A Biblioteca Homologada é só leitura: o DAG a lê em modo somente leitura e nunca \
             escreve nela, a não ser numa homologação que você aprova.",
        );
    });
}

fn fila_e_alertas(ui: &mut egui::Ui, app: &mut AppDag) {
    let contagem = app.biblioteca.fila.as_ref().map(|f| f.contagem);
    let resumo = app.biblioteca.resumo.clone();
    quadro(ui, "Fila e alertas", |ui| {
        match contagem {
            Some(c) => {
                egui::Grid::new("bib-fila").num_columns(2).show(ui, |ui| {
                    let linha = |ui: &mut egui::Ui, r: &str, n: i64| {
                        ui.label(r);
                        ui.strong(inteiro_pt_br(u64::try_from(n).unwrap_or(0)));
                        ui.end_row();
                    };
                    linha(ui, "Pendentes", c.pendentes);
                    linha(ui, "Em andamento", c.em_andamento);
                    linha(ui, "Concluídos", c.concluidos);
                    linha(ui, "Falhas", c.falhos);
                    linha(ui, "Exceções", c.excecoes);
                });
            }
            None => {
                ui.weak("Fila ainda não lida.");
            }
        }
        if let Some(r) = &resumo {
            ui.separator();
            if r.alertas_nao_vistos > 0 {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!(
                        "Atenção: {} alerta(s) ainda não visto(s).",
                        inteiro_pt_br(u64::try_from(r.alertas_nao_vistos).unwrap_or(0))
                    ),
                );
                if ui.link("Ver alertas").clicked() {
                    bancada_casca::abrir(ui.ctx(), "biblioteca-alertas");
                }
            } else {
                ui.label("Nenhum alerta novo.");
            }
            ui.label(format!(
                "{} revisão(ões) pendente(s) · {} candidato(s) a duplicidade aberto(s)",
                inteiro_pt_br(u64::try_from(r.revisoes_pendentes).unwrap_or(0)),
                inteiro_pt_br(u64::try_from(r.candidatos_duplicidade_abertos).unwrap_or(0)),
            ));
        }
    });
}

fn espaco(ui: &mut egui::Ui, app: &mut AppDag) {
    let resumo = app.biblioteca.resumo.clone();
    let relatorio = app.biblioteca.espaco.clone();
    quadro(ui, "Espaço recuperável com segurança", |ui| {
        if let Some(r) = &resumo {
            ui.heading(bytes_humanos(r.bytes_recuperaveis_exatos));
            ui.weak("em cópias exatas de livros que você já tem na Homologada (estimativa).");
        } else {
            ui.weak("Resumo ainda não lido.");
        }
        if let Some(r) = &relatorio {
            let (est, inc, rec) = totais_de_espaco(r);
            ui.label(format!(
                "Por Volume: {} estimados · {} incertos (hard links, reflinks) · {} já recuperados",
                bytes_humanos(est),
                bytes_humanos(inc),
                bytes_humanos(rec)
            ));
            ui.weak(texto::texto_puro(&r.observacao, 300));
        }
        ui.weak("Só vira economia depois de marcar, mover para a Lixeira e esvaziá-la.");
        if ui.link("Economize espaço com segurança").clicked() {
            bancada_casca::abrir(ui.ctx(), "biblioteca-duplicatas");
        }
    });
}

fn acoes(ui: &mut egui::Ui, app: &mut AppDag) {
    let ia_ligada = app.biblioteca.config.as_ref().map(|c| c.ia.ativa);
    let ocupado = app.bib_job_ativo();
    let sem_escrita = app.motivo_sem_escrita_config();
    let tem_homologada = app.biblioteca.homologada().is_some();
    let mut sincronizar_ = false;
    let mut enfileirar_ = false;
    let mut processar_ = false;
    quadro(ui, "Trabalhos", |ui| {
        ui.horizontal_wrapped(|ui| {
            let motivo = sem_escrita
                .map(str::to_string)
                .or_else(|| (!tem_homologada).then(|| "Registre a Homologada primeiro".into()))
                .or_else(|| ocupado.then(|| "Há um trabalho em andamento".into()));
            let b = ui.add_enabled(motivo.is_none(), egui::Button::new("Sincronizar"));
            let b = match &motivo {
                Some(m) => b.on_disabled_hover_text(m),
                None => b.on_hover_text(
                    "Lê a Homologada em modo somente leitura, liga os formatos ao Catálogo e \
                     prova que nada mudou",
                ),
            };
            sincronizar_ = b.clicked();
        });
        ui.add_space(4.0);
        ui.label("Raízes a enfileirar (uma por linha, caminhos absolutos):");
        ui.add(
            egui::TextEdit::multiline(&mut app.biblioteca.visao.raizes)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text("/mnt/e2/Downloads"),
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Motivo:");
            egui::ComboBox::from_id_salt("bib-motivo")
                .selected_text(rotulo_motivo(app.biblioteca.visao.motivo))
                .show_ui(ui, |ui| {
                    for m in [
                        MotivoFila::Novo,
                        MotivoFila::Alterado,
                        MotivoFila::Reprocessar,
                    ] {
                        ui.selectable_value(&mut app.biblioteca.visao.motivo, m, rotulo_motivo(m));
                    }
                });
            let vazio = texto::raizes_de_texto(&app.biblioteca.visao.raizes).is_empty();
            let motivo = sem_escrita
                .map(str::to_string)
                .or_else(|| vazio.then(|| "Digite ao menos uma raiz".into()))
                .or_else(|| ocupado.then(|| "Há um trabalho em andamento".into()));
            let b = ui.add_enabled(motivo.is_none(), egui::Button::new("Enfileirar"));
            enfileirar_ = match &motivo {
                Some(m) => b.on_disabled_hover_text(m),
                None => b.on_hover_text("Barato: só registra os livros na fila, sem extrair nada"),
            }
            .clicked();
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let ia = ia_ligada == Some(true);
            ui.add_enabled(
                ia,
                egui::Checkbox::new(
                    &mut app.biblioteca.visao.usar_ia,
                    "Consultar a IA local no processamento",
                ),
            )
            .on_disabled_hover_text("A IA está desligada na configuração (Modelos IA)");
            ui.weak(if ia {
                "IA ligada"
            } else {
                "IA desligada: só extração e regras"
            });
            let motivo = sem_escrita
                .map(str::to_string)
                .or_else(|| ocupado.then(|| "Há um trabalho em andamento".into()));
            let b = ui.add_enabled(motivo.is_none(), egui::Button::new("Processar fila"));
            processar_ = match &motivo {
                Some(m) => b.on_disabled_hover_text(m),
                None => b.on_hover_text(
                    "Duplicata exata é achada antes de qualquer extração; dá para pausar e cancelar",
                ),
            }
            .clicked();
        });
        erro_da_acao(
            ui,
            &app.biblioteca,
            &[
                AcaoBib::Sincronizar,
                AcaoBib::Enfileirar,
                AcaoBib::Processar,
            ],
        );
    });
    if sincronizar_ {
        sincronizar(app);
    }
    if enfileirar_ {
        enfileirar(app);
    }
    if processar_ {
        processar(app);
    }
}

fn registro(ui: &mut egui::Ui, app: &mut AppDag) {
    let mut registrar = false;
    let sem_escrita = app.motivo_sem_escrita_config();
    egui::CollapsingHeader::new("Registrar uma biblioteca")
        .default_open(app.biblioteca.bibliotecas.as_ref().is_some_and(Vec::is_empty))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("bib-reg-papel")
                    .selected_text(rotulo_papel(app.biblioteca.visao.reg_papel))
                    .show_ui(ui, |ui| {
                        for p in [PapelRegistro::Homologada, PapelRegistro::AClassificar] {
                            ui.selectable_value(
                                &mut app.biblioteca.visao.reg_papel,
                                p,
                                rotulo_papel(p),
                            );
                        }
                    });
                ui.add(
                    egui::TextEdit::singleline(&mut app.biblioteca.visao.reg_caminho)
                        .hint_text("/caminho/da/biblioteca (vazio na A Classificar usa o destino da configuração)")
                        .desired_width(420.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut app.biblioteca.visao.reg_nome)
                        .hint_text("nome (opcional)")
                        .desired_width(140.0),
                );
                let pode = sem_escrita.is_none()
                    && (app.biblioteca.visao.reg_papel == PapelRegistro::AClassificar
                        || !app.biblioteca.visao.reg_caminho.trim().is_empty());
                let b = ui.add_enabled(pode, egui::Button::new("Registrar"));
                registrar = match sem_escrita {
                    Some(m) => b.on_disabled_hover_text(m),
                    None => b.on_disabled_hover_text("Informe o caminho da Homologada"),
                }
                .clicked();
            });
            ui.weak(
                "Homologada: biblioteca Calibre que você já usa (só leitura). A Classificar: \
                 biblioteca Calibre separada, criada pelo DAG se o caminho ainda não for uma.",
            );
            erro_da_acao(ui, &app.biblioteca, &[AcaoBib::Registrar]);
        });
    if registrar {
        let v = &app.biblioteca.visao;
        let cmd = comandos::registrar(&v.reg_caminho, v.reg_papel, &v.reg_nome);
        app.bib_acao(AcaoBib::Registrar, cmd);
    }
}

fn ultimos_resultados(ui: &mut egui::Ui, app: &AppDag) {
    let b = &app.biblioteca;
    if b.sincronizacao.is_none() && b.enfileiramento.is_none() && b.processamento.is_none() {
        return;
    }
    quadro(ui, "Últimos resultados desta sessão", |ui| {
        if let Some(s) = &b.sincronizacao {
            ui.label(format!(
                "Sincronização: {} livro(s), {} formato(s); {} ligado(s) ao Catálogo; {} hash(es) \
                 importado(s), {} calculado(s); {} formato(s) sem arquivo no Catálogo; \
                 metadata.db {}; biblioteca {} ({} ms).",
                inteiro_pt_br(s.livros),
                inteiro_pt_br(s.formatos),
                inteiro_pt_br(s.formatos_ligados_ao_catalogo),
                inteiro_pt_br(s.hashes_importados),
                inteiro_pt_br(s.hashes_calculados),
                inteiro_pt_br(s.formatos_sem_arquivo_no_catalogo),
                texto::texto_puro(&s.comparacao_metadata_db, 30),
                if s.biblioteca_intacta {
                    "intacta"
                } else {
                    "ALTERADA"
                },
                inteiro_pt_br(s.duracao_ms),
            ));
            for d in s.diferencas.iter().take(5) {
                ui.weak(format!("diferença: {}", texto::texto_puro(d, 200)));
            }
        }
        if let Some(e) = &b.enfileiramento {
            ui.label(format!(
                "Enfileiramento: {} examinado(s), {} enfileirado(s), {} já na fila, {} ignorado(s).",
                inteiro_pt_br(e.examinados),
                inteiro_pt_br(e.enfileirados),
                inteiro_pt_br(e.ja_na_fila),
                inteiro_pt_br(e.ignorados),
            ));
        }
        if let Some(p) = &b.processamento {
            ui.label(format!(
                "Processamento: {} item(ns), {} duplicata(s) exata(s), {} para revisão, {} \
                 exceção(ões), {} falha(s), {} consulta(s) à IA, {} páginas ({} com OCR), {} \
                 potenciais.{}",
                inteiro_pt_br(p.processados),
                inteiro_pt_br(p.duplicatas_exatas),
                inteiro_pt_br(p.revisao_pendente),
                inteiro_pt_br(p.excecoes),
                inteiro_pt_br(p.falhas),
                inteiro_pt_br(p.consultas_ia),
                inteiro_pt_br(p.paginas),
                inteiro_pt_br(p.paginas_ocr),
                bytes_humanos(p.espaco_potencial_bytes),
                if p.orfaos_retomados > 0 {
                    format!(" {} item(ns) órfão(s) retomado(s).", p.orfaos_retomados)
                } else {
                    String::new()
                }
            ));
        }
    });
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_core::EspacoVolumeInfo;

    #[test]
    fn totais_de_espaco_soma_os_volumes() {
        let v = |e: u64, i: u64, r: u64| EspacoVolumeInfo {
            volume_id: 1,
            caminhos: 0,
            arquivos_fisicos: 0,
            bytes_logicos: 0,
            bytes_alocados: 0,
            estimativa_recuperavel: e,
            economia_incerta: i,
            efetivamente_recuperado: r,
            caminhos_hardlink_ignorados: 0,
            membros_ignorados: 0,
        };
        let r = RelatorioEspacoInfo {
            volumes: vec![v(10, 1, 0), v(5, 2, 3)],
            grupos: vec![],
            observacao: "estimativa".into(),
        };
        assert_eq!(totais_de_espaco(&r), (15, 3, 3));
    }
}
