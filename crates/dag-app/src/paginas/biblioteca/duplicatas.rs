//! Biblioteca > Duplicatas ("Economize espaço com segurança", WOW-01/WOW-02): grupos por livro
//! homologado, ordenados por bytes, com as cópias, onde cada uma está, se tem Cobertura e a
//! decisão do DAG (sugerir ou bloqueado, com as razões).
//!
//! Esta página nunca move nem apaga nada. "Marcar para deleção" só muda o Catálogo, depois de
//! uma confirmação na própria página; mover para a Lixeira do Volume abre os dois diálogos que já
//! existem (ADR 0005), e só vale para cópia com decisão `sugerir`.

use std::collections::BTreeSet;

use bancada_casca::egui;
use dag_core::{Alvo, ArquivoId, CopiaDuplicata, GrupoDuplicata, PapelCopiaInfo};

use super::texto::{self, rotulo_papel_copia, texto_puro};
use super::{Consulta, acao, estado_da_consulta, quadro};
use crate::app::{AppDag, Proposito};
use crate::confirmacao::AcaoMarcacao;
use crate::formato::{bytes_humanos, hash_curto, inteiro_pt_br};
use crate::lixeira::{AlvoMenu, ItemMenu, comando_marca};
use crate::paginas::tabela::{Chave, Coluna, nota_ordem_local, tabela};
use crate::paginas::{DefGrupo, DefPagina, cabecalho, estado_vazio};

pub static PAGINA: DefPagina = DefPagina {
    id: "biblioteca-duplicatas",
    rotulo: "Duplicatas",
    keytip: "U",
    icone: "duplicados",
    grupos: &[DefGrupo {
        id: "biblioteca.duplicatas",
        rotulo: "Duplicatas",
        acoes: &[acao(
            "biblioteca.duplicatas_buscar",
            "Procurar duplicatas",
            "Refaz a busca de cópias exatas de livros da Homologada (só índices, sem OCR)",
        )],
    }],
    resumo: "Cópias exatas de livros que você já tem: quanto espaço dá para recuperar, e por que cada item é redundante.",
    desenhar,
};

/// Estado de tela da página.
#[derive(Debug, Default)]
pub struct Ui {
    /// Livro homologado do grupo aberto.
    pub grupo: Option<i64>,
    /// Cópia (arquivo do Catálogo) escolhida no detalhe.
    pub copia: Option<i64>,
    /// Cópia cuja marcação espera confirmação.
    pub confirmando: Option<i64>,
    /// Cópias que esta janela marcou nesta sessão.
    pub marcadas: BTreeSet<i64>,
}

pub(super) fn buscar(app: &mut AppDag) {
    app.biblioteca
        .atualizar(&[Consulta::Espaco, Consulta::Resumo]);
    app.bib_pedir(Consulta::Duplicatas, None);
}

/// A origem da cópia em palavras, com a Cobertura dita à parte.
#[must_use]
pub fn origem_da_copia(c: &CopiaDuplicata) -> String {
    let base = rotulo_papel_copia(c.papel);
    if c.papel == PapelCopiaInfo::EspelhoDeBiblioteca {
        format!("{base} (Cobertura)")
    } else {
        base.to_string()
    }
}

/// `Sugerir` ou `Bloqueado`.
#[must_use]
pub fn rotulo_da_decisao(c: &CopiaDuplicata) -> &'static str {
    if c.decisao.sugerir {
        "Sugerir"
    } else {
        "Bloqueado"
    }
}

/// Caminho da cópia que ficaria (a preservada da decisão), se o grupo a conhece.
#[must_use]
pub fn preservada_de(g: &GrupoDuplicata, c: &CopiaDuplicata) -> Option<String> {
    let id = c.decisao.copia_preservada?;
    g.formatos_homologados
        .iter()
        .find(|f| f.file_id == Some(id))
        .map(|f| f.caminho.clone())
        .or_else(|| {
            g.copias
                .iter()
                .find(|x| x.file_id == id)
                .map(|x| x.caminho.clone())
        })
}

/// O que a confirmação de "marcar para deleção" mostra. Puro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanoMarcacao {
    pub linhas: Vec<String>,
    /// A decisão do DAG permite sugerir? Sem isso a marcação nem é oferecida.
    pub permitido: bool,
}

#[must_use]
pub fn plano_de_marcacao(g: &GrupoDuplicata, c: &CopiaDuplicata) -> PlanoMarcacao {
    let mut linhas = vec![
        format!("Cópia: {}", c.caminho),
        format!(
            "Volume {} · {} · {}",
            texto_puro(&c.volume, 60),
            bytes_humanos(c.tamanho),
            origem_da_copia(c)
        ),
    ];
    match preservada_de(g, c) {
        Some(p) => linhas.push(format!("Fica: {p}")),
        None if c.decisao.sugerir => linhas
            .push("Fica: a cópia preservada indicada pela decisão (id no Catálogo)".to_string()),
        None => {}
    }
    linhas.push(
        "Só o Catálogo muda: nenhum arquivo é apagado, movido ou modificado. Mover para a \
         Lixeira do Volume é um passo separado, com dois diálogos."
            .to_string(),
    );
    PlanoMarcacao {
        linhas,
        permitido: c.decisao.sugerir,
    }
}

/// O alvo do fluxo de Marcação/Lixeira que já existe, para uma cópia desta página.
#[must_use]
pub fn alvo_da_copia(c: &CopiaDuplicata) -> AlvoMenu {
    AlvoMenu {
        alvo: Alvo::Arquivo(ArquivoId(c.file_id)),
        rotulo: texto::nome_do_arquivo(&c.caminho).to_string(),
        caminho: Some(c.caminho.clone()),
        pasta: false,
        na_lixeira: false,
        marca: None,
        arquivos: None,
    }
}

const COLUNAS_GRUPOS: [Coluna; 6] = [
    Coluna::esq("Livro", 230.0),
    Coluna::esq("Autores", 140.0),
    Coluna::dir("Cópias", 60.0),
    Coluna::dir("Sugeridas", 75.0),
    Coluna::dir("Bytes das cópias", 110.0),
    Coluna::dir("Bytes sugeridos", 110.0),
];

const COLUNAS_COPIAS: [Coluna; 8] = [
    Coluna::esq("", 24.0).sem_ordem(),
    Coluna::esq("Formato", 65.0),
    Coluna::dir("Tamanho", 85.0),
    Coluna::esq("Origem", 190.0),
    Coluna::esq("Volume", 110.0),
    Coluna::esq("Caminho", 380.0),
    Coluna::esq("Cobertura", 80.0),
    Coluna::esq("Decisão", 90.0),
];

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.bib_quadro(ui.ctx());
    app.bib_carregar(&[Consulta::Bibliotecas, Consulta::Espaco]);
    let sincronizada = app
        .biblioteca
        .homologada()
        .is_some_and(|b| b.ultima_sincronizacao.is_some());
    if sincronizada {
        app.bib_carregar(&[Consulta::Duplicatas]);
    }
    ui.heading("Economize espaço com segurança");
    resumo(ui, app);
    ui.separator();
    estado_da_consulta(ui, &app.biblioteca, Consulta::Duplicatas);
    if app.biblioteca.bibliotecas.is_some() && !sincronizada {
        estado_vazio(
            ui,
            "Sincronize a Biblioteca Homologada primeiro",
            PAGINA.resumo,
            "Em Visão Geral: registre a Homologada e clique em Sincronizar. As duplicatas vêm da comparação de hashes.",
        );
        return;
    }
    let Some(dados) = app.biblioteca.duplicatas.as_ref() else {
        return;
    };
    if dados.grupos.is_empty() {
        estado_vazio(
            ui,
            "Nenhuma cópia exata encontrada",
            PAGINA.resumo,
            "Só arquivos com o mesmo conteúdo (hash e tamanho) de um formato da Homologada aparecem aqui.",
        );
        return;
    }
    let altura = crate::paginas::altura_livre(ui).max(360.0);
    corpo(ui, app, altura);
}

fn resumo(ui: &mut egui::Ui, app: &mut AppDag) {
    let mut buscar_ = false;
    let (totais, sugeridos, grupos, mais) = match &app.biblioteca.duplicatas {
        Some(d) => (
            d.bytes_totais(),
            d.bytes_sugeridos(),
            d.grupos.len(),
            d.proximo.is_some(),
        ),
        None => (0, 0, 0, false),
    };
    let sem_escrita = app.motivo_sem_escrita_config();
    ui.horizontal_wrapped(|ui| {
        if app.biblioteca.duplicatas.is_some() {
            ui.strong(format!(
                "Dá para sugerir {} de {} em cópias exatas",
                bytes_humanos(sugeridos),
                bytes_humanos(totais)
            ));
            ui.label(format!(
                "· {} livro(s){}",
                inteiro_pt_br(grupos as u64),
                if mais { " (há mais páginas)" } else { "" }
            ));
        }
        let b = ui.add_enabled(
            sem_escrita.is_none() && !app.biblioteca.carregando(Consulta::Duplicatas),
            egui::Button::new("Procurar de novo"),
        );
        buscar_ = match sem_escrita {
            Some(m) => b.on_disabled_hover_text(m),
            None => b.on_hover_text("Refaz a busca; também registra alertas novos"),
        }
        .clicked();
    });
    ui.weak(
        "Estimativa, não promessa: nada é apagado por aqui. O espaço só é liberado depois de \
         marcar, mover para a Lixeira do Volume e esvaziá-la pelo gerenciador de arquivos.",
    );
    if buscar_ {
        buscar(app);
    }
}

fn corpo(ui: &mut egui::Ui, app: &mut AppDag, altura: f32) {
    let mut escolhido = None;
    let mut mais = false;
    ui.horizontal_top(|ui| {
        ui.allocate_ui(egui::vec2(ui.available_width() * 0.40, altura), |ui| {
            let Some(d) = app.biblioteca.duplicatas.as_ref() else {
                return;
            };
            let sel = app.biblioteca.dup.grupo;
            tabela(
                ui,
                "bib-dup-grupos",
                &COLUNAS_GRUPOS,
                d.grupos.len(),
                altura - 60.0,
                |c, i| {
                    let g = &d.grupos[i];
                    match c {
                        0 => Chave::texto(&g.livro.titulo),
                        1 => Chave::texto(&g.livro.autores.join(" & ")),
                        2 => Chave::num(g.copias.len() as u64),
                        3 => {
                            Chave::num(g.copias.iter().filter(|x| x.decisao.sugerir).count() as u64)
                        }
                        4 => Chave::num(g.bytes_totais),
                        _ => Chave::num(g.bytes_sugeridos),
                    }
                },
                |l, i| {
                    let g = &d.grupos[i];
                    if l.selecionavel(sel == Some(g.livro.id), texto_puro(&g.livro.titulo, 90))
                        .clicked()
                    {
                        escolhido = Some(g.livro.id);
                    }
                    l.texto(texto_puro(&g.livro.autores.join(" & "), 80));
                    l.texto(g.copias.len().to_string());
                    l.texto(
                        g.copias
                            .iter()
                            .filter(|x| x.decisao.sugerir)
                            .count()
                            .to_string(),
                    );
                    l.texto(bytes_humanos(g.bytes_totais));
                    l.texto(bytes_humanos(g.bytes_sugeridos));
                },
            );
            nota_ordem_local(ui, "bib-dup-grupos");
            if d.proximo.is_some()
                && ui
                    .add_enabled(
                        !app.biblioteca.carregando(Consulta::Duplicatas),
                        egui::Button::new("Carregar mais"),
                    )
                    .clicked()
            {
                mais = true;
            }
        });
        ui.separator();
        ui.vertical(|ui| detalhe(ui, app, altura));
    });
    if let Some(g) = escolhido
        && app.biblioteca.dup.grupo != Some(g)
    {
        app.biblioteca.dup.grupo = Some(g);
        app.biblioteca.dup.copia = None;
        app.biblioteca.dup.confirmando = None;
    }
    if mais {
        let cursor = app
            .biblioteca
            .duplicatas
            .as_ref()
            .and_then(|d| d.proximo.clone());
        app.bib_pedir(Consulta::Duplicatas, cursor);
    }
}

fn detalhe(ui: &mut egui::Ui, app: &mut AppDag, altura: f32) {
    let grupo = app
        .biblioteca
        .duplicatas
        .as_ref()
        .and_then(|d| {
            d.grupos
                .iter()
                .find(|g| Some(g.livro.id) == app.biblioteca.dup.grupo)
        })
        .cloned();
    let Some(g) = grupo else {
        ui.add_space(20.0);
        ui.weak("Selecione um livro para ver as cópias, onde estão e por que são redundantes.");
        return;
    };
    quadro(ui, "O livro na Biblioteca Homologada", |ui| {
        ui.label(format!(
            "{} — {}",
            texto_puro(&g.livro.titulo, 160),
            texto_puro(&g.livro.autores.join(" & "), 160)
        ));
        ui.weak(format!(
            "{}{}{}",
            g.livro
                .editora
                .as_deref()
                .map(|e| format!("{} · ", texto_puro(e, 60)))
                .unwrap_or_default(),
            g.livro
                .isbn13
                .as_deref()
                .map(|i| format!("ISBN {} · ", texto_puro(i, 20)))
                .unwrap_or_default(),
            g.livro
                .estante
                .as_deref()
                .map(|e| format!("estante {}", texto_puro(e, 60)))
                .unwrap_or_default(),
        ));
        for f in &g.formatos_homologados {
            ui.label(format!(
                "Fica na Homologada: {} · {} · {} · hash {}",
                texto_puro(&f.formato.to_uppercase(), 10),
                bytes_humanos(f.tamanho),
                f.caminho,
                hash_curto(&f.hash)
            ));
        }
    });
    let mut sel = app.biblioteca.dup.copia;
    tabela(
        ui,
        "bib-dup-copias",
        &COLUNAS_COPIAS,
        g.copias.len(),
        (altura - 300.0).max(110.0),
        |c, i| {
            let x = &g.copias[i];
            match c {
                0 => Chave::num(i128::from(!x.decisao.sugerir)),
                1 => Chave::texto(&x.formato),
                2 => Chave::num(x.tamanho),
                3 => Chave::texto(&origem_da_copia(x)),
                4 => Chave::texto(&x.volume),
                5 => Chave::texto(&x.caminho),
                6 => Chave::num(i128::from(x.coberta)),
                _ => Chave::texto(rotulo_da_decisao(x)),
            }
        },
        |l, i| {
            let x = &g.copias[i];
            if l.selecionavel(
                sel == Some(x.file_id),
                if x.decisao.sugerir { "●" } else { "○" },
            )
            .clicked()
            {
                sel = Some(x.file_id);
            }
            l.texto(x.formato.to_uppercase());
            l.texto(bytes_humanos(x.tamanho));
            l.texto(origem_da_copia(x));
            l.texto(texto_puro(&x.volume, 60));
            l.texto(x.caminho.clone());
            l.texto(if x.coberta { "sim" } else { "não" });
            l.texto(rotulo_da_decisao(x));
        },
    );
    app.biblioteca.dup.copia = sel;
    ui.weak("● pode ser sugerida à Lixeira · ○ bloqueada. Cobertura = cópia com Marcação de Backup em outro Dispositivo Físico.");
    let Some(copia) = g.copias.iter().find(|c| Some(c.file_id) == sel).cloned() else {
        return;
    };
    decisao_da_copia(ui, app, &g, &copia);
}

fn decisao_da_copia(ui: &mut egui::Ui, app: &mut AppDag, g: &GrupoDuplicata, c: &CopiaDuplicata) {
    let plano = plano_de_marcacao(g, c);
    let motivo = app.motivo_sem_escrita();
    let ja_marcada = app.biblioteca.dup.marcadas.contains(&c.file_id);
    let mut pedir = false;
    let mut confirmar = false;
    let mut cancelar = false;
    let mut mover = false;
    quadro(ui, "Decisão do DAG para esta cópia", |ui| {
        if c.decisao.sugerir {
            ui.colored_label(
                egui::Color32::from_rgb(76, 175, 80),
                "OK: pode ser SUGERIDA à Lixeira do Volume (você ainda confirma tudo).",
            );
        } else {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Atenção: bloqueada. O DAG não oferece marcar esta cópia.",
            );
            for r in &c.decisao.razoes {
                ui.label(format!("• {}", texto_puro(r, 300)));
            }
        }
        if !c.decisao.passos_obrigatorios.is_empty() {
            ui.weak("Passos obrigatórios:");
            for p in &c.decisao.passos_obrigatorios {
                ui.weak(format!("  – {}", texto_puro(p, 300)));
            }
        }
        if let Some(p) = preservada_de(g, c) {
            ui.label(format!("Cópia preservada: {p}"));
        }
        ui.add_space(4.0);
        if app.biblioteca.dup.confirmando == Some(c.file_id) {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong("Confirmar: marcar para deleção");
                for l in &plano.linhas {
                    ui.label(l);
                }
                ui.horizontal(|ui| {
                    confirmar = ui.button("Confirmar marcação").clicked();
                    cancelar = ui.button("Cancelar").clicked();
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                let ok = plano.permitido && motivo.is_none();
                let b = ui.add_enabled(ok, egui::Button::new("Marcar para deleção…"));
                pedir = if let Some(m) = motivo {
                    b.on_disabled_hover_text(m)
                } else if !plano.permitido {
                    b.on_disabled_hover_text("A decisão do DAG bloqueia esta cópia")
                } else {
                    b.on_hover_text("Pede confirmação; só o Catálogo muda")
                }
                .clicked();
                let b = ui.add_enabled(ok, egui::Button::new("Mover para a Lixeira…"));
                mover = b
                    .on_hover_text(
                        "Abre os dois diálogos da Lixeira do Volume; o agente exige a marcação",
                    )
                    .clicked();
                if ja_marcada {
                    ui.weak("Marcada nesta sessão.");
                }
            });
        }
    });
    if pedir {
        app.biblioteca.dup.confirmando = Some(c.file_id);
    }
    if cancelar {
        app.biblioteca.dup.confirmando = None;
    }
    if confirmar {
        app.biblioteca.dup.confirmando = None;
        app.biblioteca.dup.marcadas.insert(c.file_id);
        app.enviar(
            comando_marca(&alvo_da_copia(c), true),
            Proposito::Marcacao(AcaoMarcacao::MarcarDelecao),
        );
    }
    if mover {
        let ctx = ui.ctx().clone();
        app.tratar_escolha(&ctx, (ItemMenu::MoverParaLixeira, alvo_da_copia(c)));
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_core::{DecisaoDescarteInfo, FormatoHomologadoRef, LivroRef};

    fn copia(
        id: i64,
        sugerir: bool,
        preservada: Option<i64>,
        papel: PapelCopiaInfo,
    ) -> CopiaDuplicata {
        CopiaDuplicata {
            file_id: id,
            caminho: format!("/dl/{id}.epub"),
            volume: "Dados".into(),
            volume_id: 1,
            dispositivo_id: Some(1),
            tamanho: 1000,
            formato: "epub".into(),
            hash: "ab".repeat(32),
            papel,
            coberta: papel == PapelCopiaInfo::EspelhoDeBiblioteca,
            decisao: DecisaoDescarteInfo {
                sugerir,
                copia_preservada: preservada,
                razoes: if sugerir {
                    vec![]
                } else {
                    vec!["sem Cobertura".into()]
                },
                passos_obrigatorios: vec![],
            },
        }
    }

    fn grupo() -> GrupoDuplicata {
        GrupoDuplicata {
            livro: LivroRef {
                id: 1,
                calibre_id: 1,
                titulo: "Cálculo".into(),
                autores: vec!["Stewart".into()],
                isbn13: None,
                editora: None,
                idioma: None,
                estante: None,
                tags: vec![],
                caminho: "Stewart/Calculo".into(),
            },
            formatos_homologados: vec![FormatoHomologadoRef {
                formato_id: 5,
                formato: "EPUB".into(),
                caminho: "/lib/Stewart/Calculo.epub".into(),
                tamanho: 1000,
                hash: "ab".repeat(32),
                file_id: Some(77),
            }],
            copias: vec![
                copia(10, true, Some(77), PapelCopiaInfo::DownloadOuAClassificar),
                copia(11, false, None, PapelCopiaInfo::EspelhoDeBiblioteca),
            ],
            bytes_totais: 2000,
            bytes_sugeridos: 1000,
        }
    }

    #[test]
    fn origem_e_decisao_em_palavras() {
        let g = grupo();
        assert_eq!(origem_da_copia(&g.copias[0]), "download ou A Classificar");
        assert_eq!(
            origem_da_copia(&g.copias[1]),
            "espelho de biblioteca (Cobertura)"
        );
        assert_eq!(rotulo_da_decisao(&g.copias[0]), "Sugerir");
        assert_eq!(rotulo_da_decisao(&g.copias[1]), "Bloqueado");
    }

    #[test]
    fn preservada_resolve_pelo_formato_homologado_ou_outra_copia() {
        let g = grupo();
        assert_eq!(
            preservada_de(&g, &g.copias[0]).as_deref(),
            Some("/lib/Stewart/Calculo.epub")
        );
        assert_eq!(preservada_de(&g, &g.copias[1]), None);
        let mut g2 = grupo();
        g2.copias[0].decisao.copia_preservada = Some(11);
        assert_eq!(
            preservada_de(&g2, &g2.copias[0]).as_deref(),
            Some("/dl/11.epub")
        );
    }

    #[test]
    fn plano_so_e_permitido_quando_a_decisao_sugere() {
        let g = grupo();
        let p = plano_de_marcacao(&g, &g.copias[0]);
        assert!(p.permitido);
        assert!(p.linhas.iter().any(|l| l.starts_with("Fica: /lib/")));
        assert!(p.linhas.iter().any(|l| l.contains("Só o Catálogo muda")));
        let b = plano_de_marcacao(&g, &g.copias[1]);
        assert!(!b.permitido);
    }

    #[test]
    fn alvo_vai_para_o_fluxo_existente_de_marcacao_e_lixeira() {
        let g = grupo();
        let a = alvo_da_copia(&g.copias[0]);
        assert_eq!(a.alvo, Alvo::Arquivo(ArquivoId(10)));
        assert_eq!(a.rotulo, "10.epub");
        assert!(!a.pasta && !a.na_lixeira);
        match comando_marca(&a, true) {
            dag_core::Comando::MarcarDelecao { alvo } => {
                assert_eq!(alvo, Alvo::Arquivo(ArquivoId(10)));
            }
            o => panic!("{o:?}"),
        }
    }
}
