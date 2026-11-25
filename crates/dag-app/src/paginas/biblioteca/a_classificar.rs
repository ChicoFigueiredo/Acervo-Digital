//! Biblioteca > A Classificar: o que já foi cadastrado na biblioteca A Classificar, o que está
//! pronto para cadastrar, e a homologação (a única via que escreve na Biblioteca Homologada).
//!
//! O cadastro roda no agente (via `calibredb`); se o Calibre estiver aberto o agente responde
//! `calibre_ocupado` e a página explica o que fazer, sem travar a janela. A homologação exige
//! dupla confirmação: digitar o título do livro e dizer quem aprova.
//!
//! O protocolo não tem um comando que liste os cadastros nem devolve o id do cadastro (que a
//! homologação pede); por isso a lista de cadastros vem do Catálogo, lido por conexão somente
//! leitura, como nas páginas da Fase A. Toda escrita continua sendo um `Comando::Biblioteca*`.

use std::collections::{BTreeSet, HashSet};

use bancada_casca::egui;
use dag_core::ItemFilaInfo;
use dag_store::biblioteca::{self as store, EstadoCadastro};
use rusqlite::Connection;

use super::comandos;
use super::texto::{self, confirma_homologacao, nome_do_arquivo, texto_puro};
use super::{AcaoBib, Consulta, Estado, acao, erro_da_acao, estado_da_consulta, quadro};
use crate::app::AppDag;
use crate::formato::{data_s, inteiro_pt_br};
use crate::leitor::{Carga, Leitor, erro};
use crate::paginas::tabela::{Chave, Coluna, tabela};
use crate::paginas::{DefGrupo, DefPagina, cabecalho, estado_vazio};

pub static PAGINA: DefPagina = DefPagina {
    id: "biblioteca-a-classificar",
    rotulo: "A Classificar",
    keytip: "C",
    icone: "classificar",
    grupos: &[DefGrupo {
        id: "biblioteca.a_classificar",
        rotulo: "A Classificar",
        acoes: &[
            acao(
                "biblioteca.cadastrar_prontos",
                "Cadastrar prontos",
                "Cadastra na A Classificar todos os livros concluídos e revisados (o Calibre precisa estar fechado)",
            ),
            acao(
                "biblioteca.classificar_atualizar",
                "Atualizar cadastros",
                "Relê a fila e a lista de cadastros",
            ),
        ],
    }],
    resumo: "Livros cadastrados na biblioteca A Classificar e prontos para cadastrar; homologar leva um livro à Biblioteca Homologada.",
    desenhar,
};

/// Um cadastro na A Classificar, como o Catálogo o guarda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinhaCadastro {
    pub id: i64,
    pub file_id: i64,
    pub calibre_id: Option<i64>,
    /// `cadastrado`, `pendente`, `cadastrando` ou `falhou`.
    pub estado: String,
    pub erro: Option<String>,
    pub caminho: Option<String>,
    /// Título aceito (ou o mais provável) do livro.
    pub titulo: Option<String>,
    /// Homologação mais nova: estado e quem aprovou.
    pub homologacao: Option<(String, Option<String>)>,
    pub atualizado_em: i64,
}

impl LinhaCadastro {
    /// O título que se digita para confirmar a homologação: o do livro ou, na falta dele, o
    /// nome do arquivo sem extensão.
    #[must_use]
    pub fn titulo_de_confirmacao(&self) -> String {
        if let Some(t) = self.titulo.as_deref().filter(|t| !t.trim().is_empty()) {
            return t.trim().to_string();
        }
        let nome = self.caminho.as_deref().map_or("", nome_do_arquivo);
        nome.rsplit_once('.').map_or(nome, |(b, _)| b).to_string()
    }

    /// Pode ser homologado: cadastrado e ainda não aplicado.
    #[must_use]
    pub fn homologavel(&self) -> bool {
        self.estado == "cadastrado"
            && !self
                .homologacao
                .as_ref()
                .is_some_and(|(e, _)| e == "aplicada")
    }
}

/// O melhor título entre as sugestões `(estado, valor, confiança)`: o corrigido, depois o
/// aceito, depois o mais confiável ainda pendente; o rejeitado nunca.
#[must_use]
pub fn melhor_titulo(sugestoes: &[(String, Option<String>, f32)]) -> Option<String> {
    let peso = |e: &str| match e {
        "corrigida" => 3,
        "aceita" => 2,
        "pendente" => 1,
        _ => 0,
    };
    sugestoes
        .iter()
        .filter(|(e, v, _)| peso(e) > 0 && v.as_deref().is_some_and(|v| !v.trim().is_empty()))
        .max_by(|a, b| {
            peso(&a.0)
                .cmp(&peso(&b.0))
                .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
        })
        .and_then(|(_, v, _)| v.clone())
}

fn texto_do_enum<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|x| x.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Lê os cadastros do Catálogo (conexão somente leitura): cadastrados, pendentes, em curso e
/// falhos, com o caminho do arquivo, o título e a homologação mais nova.
///
/// # Errors
/// Texto do erro do SQLite.
pub fn ler_cadastros(conn: &Connection) -> Result<Vec<LinhaCadastro>, String> {
    let mut v = Vec::new();
    for estado in [
        EstadoCadastro::Cadastrado,
        EstadoCadastro::Pendente,
        EstadoCadastro::Cadastrando,
        EstadoCadastro::Falhou,
    ] {
        let pagina = store::listar_cadastros(conn, estado, None, 500).map_err(erro)?;
        for c in pagina.itens {
            let caminho = store::info_arquivo(conn, c.file_id)
                .map_err(erro)?
                .map(|i| i.caminho);
            let sugestoes: Vec<(String, Option<String>, f32)> =
                store::sugestoes_do_arquivo(conn, c.file_id)
                    .map_err(erro)?
                    .into_iter()
                    .filter(|s| texto_do_enum(&s.campo) == "titulo")
                    .map(|s| (texto_do_enum(&s.estado), s.valor, s.confianca))
                    .collect();
            let homologacao = store::homologacoes_do_cadastro(conn, c.id)
                .map_err(erro)?
                .into_iter()
                .next()
                .map(|h| (texto_do_enum(&h.estado), h.aprovado_por));
            v.push(LinhaCadastro {
                id: c.id,
                file_id: c.file_id,
                calibre_id: c.calibre_id,
                estado: texto_do_enum(&c.estado),
                erro: c.erro,
                caminho,
                titulo: melhor_titulo(&sugestoes),
                homologacao,
                atualizado_em: c.atualizado_em,
            });
        }
    }
    Ok(v)
}

/// Estado de tela da página.
#[derive(Debug, Default)]
pub struct Ui {
    /// Livros prontos marcados para cadastrar.
    pub selecionados: BTreeSet<i64>,
    /// Cadastro cuja homologação está sendo confirmada.
    pub homologando: Option<i64>,
    pub titulo_digitado: String,
    pub aprovador: String,
    pub cadastros: Carga<Vec<LinhaCadastro>>,
    vista: u64,
}

impl Ui {
    /// Recolhe resultados e relê os cadastros quando algo mudou.
    pub fn atualizar(&mut self, leitor: &Leitor, geracao: u64) {
        if self.vista != geracao {
            self.vista = geracao;
            self.cadastros.pedir(leitor.consultar(ler_cadastros));
        }
        self.cadastros.atualizar();
    }

    /// Livros concluídos na fila, sem revisão pendente e ainda não cadastrados.
    #[must_use]
    pub fn prontos<'a>(&self, b: &'a Estado) -> Vec<&'a ItemFilaInfo> {
        let em_revisao: HashSet<i64> = b
            .revisao
            .as_ref()
            .map(|r| {
                r.itens
                    .iter()
                    .filter(|i| i.pendentes > 0)
                    .map(|i| i.file_id)
                    .collect()
            })
            .unwrap_or_default();
        let ja: HashSet<i64> = self
            .cadastros
            .dado
            .as_ref()
            .map(|c| {
                c.iter()
                    .filter(|l| l.estado != "falhou")
                    .map(|l| l.file_id)
                    .collect()
            })
            .unwrap_or_default();
        b.concluidos.as_ref().map_or_else(Vec::new, |f| {
            f.itens
                .iter()
                .filter(|i| !em_revisao.contains(&i.file_id) && !ja.contains(&i.file_id))
                .collect()
        })
    }
}

pub(super) fn cadastrar_prontos(app: &mut AppDag) {
    let ids: Vec<i64> = app
        .biblioteca
        .classificar
        .prontos(&app.biblioteca)
        .iter()
        .map(|i| i.file_id)
        .collect();
    if ids.is_empty() {
        return;
    }
    app.bib_acao(AcaoBib::Cadastrar, comandos::cadastrar(ids));
}

const COLUNAS_PRONTOS: [Coluna; 5] = [
    Coluna::esq("", 26.0).sem_ordem(),
    Coluna::esq("Arquivo", 420.0),
    Coluna::esq("Motivo", 90.0),
    Coluna::dir("Prioridade", 80.0),
    Coluna::dir("Tentativas", 80.0),
];

const COLUNAS_CADASTROS: [Coluna; 7] = [
    Coluna::dir("Cadastro", 70.0),
    Coluna::esq("Título", 260.0),
    Coluna::esq("Arquivo", 320.0),
    Coluna::dir("Livro no Calibre", 110.0),
    Coluna::esq("Estado", 100.0),
    Coluna::esq("Homologação", 190.0),
    Coluna::esq("Ações", 120.0).sem_ordem(),
];

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.bib_quadro(ui.ctx());
    app.bib_carregar(&[
        Consulta::Bibliotecas,
        Consulta::Concluidos,
        Consulta::Revisao,
    ]);
    let geracao = app.biblioteca.geracao_leitor.max(app.geracao);
    app.biblioteca.classificar.atualizar(&app.leitor, geracao);
    estado_da_consulta(ui, &app.biblioteca, Consulta::Concluidos);
    biblioteca(ui, app);
    bloqueio(ui, app);
    ui.add_space(6.0);
    prontos(ui, app);
    ui.add_space(6.0);
    cadastrados(ui, app);
    homologacao(ui, app);
}

fn biblioteca(ui: &mut egui::Ui, app: &AppDag) {
    quadro(ui, "Biblioteca A Classificar", |ui| {
        match app.biblioteca.a_classificar() {
            Some(b) => {
                ui.label(format!(
                    "{} — {}",
                    texto_puro(&b.nome, 80),
                    texto_puro(&b.caminho, 200)
                ));
                ui.weak(format!(
                    "{} livro(s) cadastrado(s) · {} formato(s) · {}",
                    inteiro_pt_br(u64::try_from(b.livros).unwrap_or(0)),
                    inteiro_pt_br(u64::try_from(b.formatos).unwrap_or(0)),
                    texto_puro(&b.estado, 40)
                ));
            }
            None if app.biblioteca.bibliotecas.is_some() => {
                estado_vazio(
                    ui,
                    "Nenhuma biblioteca A Classificar registrada",
                    PAGINA.resumo,
                    "Registre uma na Visão Geral (papel A Classificar): se o caminho ainda não for uma biblioteca, o DAG a cria.",
                );
            }
            None => {
                ui.weak("Lendo as bibliotecas…");
            }
        }
    });
}

/// Calibre ocupado: explica o que fazer (nada foi gravado) e continua usável.
fn bloqueio(ui: &mut egui::Ui, app: &mut AppDag) {
    let Some(m) = app.biblioteca.bloqueio_calibre.clone() else {
        return;
    };
    let mut dispensar = false;
    egui::Frame::group(ui.style())
        .stroke(egui::Stroke::new(1.5, ui.visuals().warn_fg_color))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Atenção: o Calibre está ocupado com esta biblioteca.",
            );
            ui.label(texto_puro(&m, 400));
            ui.label(
                "Feche o Calibre (ou pare o calibre-server) e clique em Cadastrar de novo. Nada \
                 foi gravado e o que ficou pendente continua na lista; cadastrar é idempotente.",
            );
            dispensar = ui.small_button("Entendi").clicked();
        });
    if dispensar {
        app.biblioteca.bloqueio_calibre = None;
    }
}

fn prontos(ui: &mut egui::Ui, app: &mut AppDag) {
    let lista: Vec<ItemFilaInfo> = app
        .biblioteca
        .classificar
        .prontos(&app.biblioteca)
        .into_iter()
        .cloned()
        .collect();
    let sem_escrita = app.motivo_sem_escrita_config();
    let sem_destino = app.biblioteca.a_classificar().is_none();
    let em_voo = app.biblioteca.acao_em_voo(AcaoBib::Cadastrar);
    let mut alternar = None;
    let mut cadastrar: Option<Vec<i64>> = None;
    quadro(ui, "Prontos para cadastrar", |ui| {
        ui.weak(
            "Livros concluídos na fila, sem campo pendente de revisão e ainda não cadastrados. \
             O cadastro usa o calibredb, é idempotente e não altera a Biblioteca Homologada.",
        );
        if lista.is_empty() {
            ui.label("Nenhum livro pronto agora.");
        } else {
            let selecionados = app.biblioteca.classificar.selecionados.clone();
            tabela(
                ui,
                "bib-prontos",
                &COLUNAS_PRONTOS,
                lista.len(),
                (lista.len() as f32 * 28.0 + 40.0).min(260.0),
                |c, i| {
                    let it = &lista[i];
                    match c {
                        0 => Chave::num(i128::from(selecionados.contains(&it.file_id))),
                        1 => Chave::opt_texto(it.caminho.as_deref()),
                        2 => Chave::texto(texto::rotulo_motivo(it.motivo)),
                        3 => Chave::num(it.prioridade),
                        _ => Chave::num(it.tentativas),
                    }
                },
                |l, i| {
                    let it = &lista[i];
                    l.ui(|ui| {
                        let mut m = selecionados.contains(&it.file_id);
                        if ui.checkbox(&mut m, "").changed() {
                            alternar = Some(it.file_id);
                        }
                    });
                    l.texto(texto_puro(
                        it.caminho.as_deref().unwrap_or("(sem caminho)"),
                        200,
                    ));
                    l.texto(texto::rotulo_motivo(it.motivo));
                    l.texto(it.prioridade.to_string());
                    l.texto(it.tentativas.to_string());
                },
            );
        }
        let n_sel = app.biblioteca.classificar.selecionados.len();
        ui.horizontal_wrapped(|ui| {
            let motivo = sem_escrita
                .map(str::to_string)
                .or_else(|| sem_destino.then(|| "Registre a biblioteca A Classificar".into()))
                .or_else(|| em_voo.then(|| "Cadastrando…".into()));
            let b = ui.add_enabled(
                motivo.is_none() && n_sel > 0,
                egui::Button::new(format!("Cadastrar selecionados ({n_sel})")),
            );
            if b.clicked() {
                cadastrar = Some(
                    app.biblioteca
                        .classificar
                        .selecionados
                        .iter()
                        .copied()
                        .collect(),
                );
            }
            let b = ui.add_enabled(
                motivo.is_none() && !lista.is_empty(),
                egui::Button::new(format!("Cadastrar todos os prontos ({})", lista.len())),
            );
            if let Some(m) = &motivo {
                b.on_disabled_hover_text(m);
            } else if b.clicked() {
                cadastrar = Some(lista.iter().map(|i| i.file_id).collect());
            }
            if em_voo {
                ui.spinner();
                ui.weak("Cadastrando na A Classificar… a janela continua livre.");
            }
        });
        erro_da_acao(ui, &app.biblioteca, &[AcaoBib::Cadastrar]);
        resultados_do_cadastro(ui, app);
    });
    if let Some(id) = alternar
        && !app.biblioteca.classificar.selecionados.remove(&id)
    {
        app.biblioteca.classificar.selecionados.insert(id);
    }
    if let Some(ids) = cadastrar {
        app.biblioteca.classificar.selecionados.clear();
        app.bib_acao(AcaoBib::Cadastrar, comandos::cadastrar(ids));
    }
}

fn resultados_do_cadastro(ui: &mut egui::Ui, app: &AppDag) {
    let Some(r) = &app.biblioteca.cadastro else {
        return;
    };
    if r.is_empty() {
        return;
    }
    ui.separator();
    ui.weak("Resultado do último cadastro:");
    for x in r.iter().take(8) {
        ui.label(format!(
            "• arquivo {}: {}{}{}",
            x.file_id,
            texto_puro(&x.estado, 40),
            x.calibre_id
                .map(|c| format!(" (livro {c} no Calibre)"))
                .unwrap_or_default(),
            x.erro
                .as_deref()
                .map(|e| format!(" — {}", texto_puro(e, 200)))
                .unwrap_or_default()
        ));
    }
    if r.len() > 8 {
        ui.weak(format!("… e mais {}", r.len() - 8));
    }
}

fn cadastrados(ui: &mut egui::Ui, app: &mut AppDag) {
    let lista = app
        .biblioteca
        .classificar
        .cadastros
        .dado
        .clone()
        .unwrap_or_default();
    let sem_escrita = app.motivo_sem_escrita_config();
    let ocupado = app.biblioteca.acao_em_voo(AcaoBib::Homologar);
    let mut abrir = None;
    quadro(ui, "Cadastrados na A Classificar", |ui| {
        if let Some(e) = &app.biblioteca.classificar.cadastros.erro {
            ui.colored_label(ui.visuals().warn_fg_color, texto_puro(e, 300));
        }
        if lista.is_empty() {
            ui.label("Nenhum cadastro ainda.");
            return;
        }
        tabela(
            ui,
            "bib-cadastros",
            &COLUNAS_CADASTROS,
            lista.len(),
            (crate::paginas::altura_livre(ui) - 40.0).clamp(140.0, 320.0),
            |c, i| {
                let l = &lista[i];
                match c {
                    0 => Chave::num(l.id),
                    1 => Chave::opt_texto(l.titulo.as_deref()),
                    2 => Chave::opt_texto(l.caminho.as_deref()),
                    3 => Chave::opt_num(l.calibre_id),
                    4 => Chave::texto(&l.estado),
                    5 => Chave::opt_texto(l.homologacao.as_ref().map(|h| h.0.as_str())),
                    _ => Chave::Nulo,
                }
            },
            |l, i| {
                let x = &lista[i];
                l.texto(x.id.to_string());
                l.texto(texto_puro(
                    x.titulo.as_deref().unwrap_or("(sem título sugerido)"),
                    100,
                ));
                l.texto(texto_puro(x.caminho.as_deref().unwrap_or("—"), 160));
                l.texto(x.calibre_id.map_or_else(|| "—".into(), |c| c.to_string()));
                l.texto(match &x.erro {
                    Some(e) => format!("{} ({})", x.estado, texto_puro(e, 60)),
                    None => x.estado.clone(),
                });
                l.texto(match &x.homologacao {
                    Some((e, quem)) => format!(
                        "{e}{}",
                        quem.as_deref()
                            .map(|q| format!(" por {}", texto_puro(q, 40)))
                            .unwrap_or_default()
                    ),
                    None => "—".into(),
                });
                l.ui(|ui| {
                    if x.homologavel() {
                        let b = ui.add_enabled(
                            sem_escrita.is_none() && !ocupado,
                            egui::Button::new("Homologar…").small(),
                        );
                        if b.on_hover_text(
                            "Leva o livro à Biblioteca Homologada, com a sua aprovação",
                        )
                        .clicked()
                        {
                            abrir = Some(x.id);
                        }
                    }
                });
            },
        );
        ui.weak(format!(
            "Atualizado em {}.",
            data_s(lista.iter().map(|l| l.atualizado_em).max())
        ));
    });
    if let Some(id) = abrir {
        app.biblioteca.classificar.homologando = Some(id);
        app.biblioteca.classificar.titulo_digitado.clear();
    }
}

/// Dupla confirmação da homologação, na própria página: digitar o título e dizer quem aprova.
fn homologacao(ui: &mut egui::Ui, app: &mut AppDag) {
    let Some(id) = app.biblioteca.classificar.homologando else {
        return;
    };
    let linha = app
        .biblioteca
        .classificar
        .cadastros
        .dado
        .as_ref()
        .and_then(|c| c.iter().find(|l| l.id == id))
        .cloned();
    let Some(linha) = linha else {
        app.biblioteca.classificar.homologando = None;
        return;
    };
    let titulo = linha.titulo_de_confirmacao();
    let mut aprovar = false;
    let mut cancelar = false;
    ui.add_space(6.0);
    quadro(ui, "Homologar: levar à Biblioteca Homologada", |ui| {
        ui.label(
            "Isto escreve na Biblioteca Homologada, pelo calibredb, com a sua aprovação \
             registrada e um rollback. É a única operação do DAG que a altera.",
        );
        ui.label(format!(
            "Livro: {} (cadastro {id})",
            texto_puro(&titulo, 200)
        ));
        if linha.titulo.is_none() {
            ui.weak("Sem título sugerido: digite o nome do arquivo, sem a extensão.");
        }
        ui.label("1. Digite o título do livro para confirmar:");
        ui.add(
            egui::TextEdit::singleline(&mut app.biblioteca.classificar.titulo_digitado)
                .hint_text(texto_puro(&titulo, 80))
                .desired_width(f32::INFINITY),
        );
        ui.label("2. Quem aprova:");
        ui.add(
            egui::TextEdit::singleline(&mut app.biblioteca.classificar.aprovador)
                .hint_text("seu nome")
                .desired_width(260.0),
        );
        let c = &app.biblioteca.classificar;
        let ok = confirma_homologacao(&titulo, &c.titulo_digitado, &c.aprovador);
        ui.horizontal(|ui| {
            let b = ui.add_enabled(
                ok && !app.biblioteca.acao_em_voo(AcaoBib::Homologar),
                egui::Button::new("Aprovar e homologar"),
            );
            aprovar = b
                .on_disabled_hover_text("Digite o título exato e o seu nome")
                .clicked();
            cancelar = ui.button("Cancelar").clicked();
        });
        erro_da_acao(ui, &app.biblioteca, &[AcaoBib::Homologar]);
    });
    if cancelar {
        app.biblioteca.classificar.homologando = None;
    }
    if aprovar {
        let quem = app.biblioteca.classificar.aprovador.clone();
        app.biblioteca.classificar.homologando = None;
        app.biblioteca.classificar.titulo_digitado.clear();
        app.bib_acao(AcaoBib::Homologar, comandos::homologar(id, &quem));
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_core::{EstadoFilaInfo, MotivoFila};

    fn s(estado: &str, v: Option<&str>, c: f32) -> (String, Option<String>, f32) {
        (estado.into(), v.map(String::from), c)
    }

    #[test]
    fn melhor_titulo_prefere_corrigido_depois_aceito_e_ignora_rejeitado() {
        assert_eq!(
            melhor_titulo(&[
                s("pendente", Some("P"), 0.99),
                s("aceita", Some("A"), 0.5),
                s("corrigida", Some("C"), 0.1),
                s("rejeitada", Some("R"), 1.0),
            ])
            .as_deref(),
            Some("C")
        );
        assert_eq!(
            melhor_titulo(&[
                s("pendente", Some("P1"), 0.4),
                s("pendente", Some("P2"), 0.8)
            ])
            .as_deref(),
            Some("P2")
        );
        assert_eq!(melhor_titulo(&[s("rejeitada", Some("R"), 1.0)]), None);
        assert_eq!(melhor_titulo(&[s("aceita", Some("  "), 1.0)]), None);
        assert_eq!(melhor_titulo(&[]), None);
    }

    fn linha(
        titulo: Option<&str>,
        caminho: &str,
        estado: &str,
        hom: Option<&str>,
    ) -> LinhaCadastro {
        LinhaCadastro {
            id: 3,
            file_id: 9,
            calibre_id: Some(1),
            estado: estado.into(),
            erro: None,
            caminho: Some(caminho.into()),
            titulo: titulo.map(String::from),
            homologacao: hom.map(|h| (h.to_string(), None)),
            atualizado_em: 1,
        }
    }

    #[test]
    fn titulo_de_confirmacao_cai_no_nome_do_arquivo() {
        assert_eq!(
            linha(Some(" O Livro "), "/x/a.pdf", "cadastrado", None).titulo_de_confirmacao(),
            "O Livro"
        );
        assert_eq!(
            linha(None, "/x/Meu Livro.v2.epub", "cadastrado", None).titulo_de_confirmacao(),
            "Meu Livro.v2"
        );
    }

    #[test]
    fn so_cadastrado_nao_aplicado_e_homologavel() {
        assert!(linha(None, "/a.pdf", "cadastrado", None).homologavel());
        assert!(linha(None, "/a.pdf", "cadastrado", Some("proposta")).homologavel());
        assert!(!linha(None, "/a.pdf", "cadastrado", Some("aplicada")).homologavel());
        assert!(!linha(None, "/a.pdf", "falhou", None).homologavel());
    }

    fn item(id: i64) -> ItemFilaInfo {
        ItemFilaInfo {
            id,
            file_id: id,
            caminho: Some(format!("/x/{id}.pdf")),
            motivo: MotivoFila::Novo,
            prioridade: 1,
            estado: EstadoFilaInfo::Concluido,
            etapa: None,
            tentativas: 0,
            erro: None,
            enfileirado_em: 0,
        }
    }

    #[test]
    fn prontos_descartam_revisao_pendente_e_ja_cadastrados() {
        use super::super::{DadosFila, DadosRevisao};
        #[allow(unused_mut)]
        let mut e = Estado {
            concluidos: Some(DadosFila {
                itens: vec![item(1), item(2), item(3), item(4)],
                ..DadosFila::default()
            }),
            ..Estado::default()
        };
        let mut ui = Ui::default();
        // Sem revisão nem cadastros carregados: todos prontos.
        assert_eq!(ui.prontos(&e).len(), 4);
        let rev = |id: i64, pend: i64| dag_core::ItemRevisaoInfo {
            file_id: id,
            caminho: None,
            tamanho: None,
            hash: None,
            formato: None,
            pendentes: pend,
            campos: vec![],
            homologados_parecidos: vec![],
            obra_id: None,
            edicao_id: None,
            avisos: vec![],
        };
        e.revisao = Some(DadosRevisao {
            itens: vec![rev(2, 3), rev(3, 0)],
            total_pendentes: 1,
            proximo: None,
        });
        ui.cadastros.dado = Some(vec![
            linha(None, "/x/4.pdf", "cadastrado", None).tap_file(4),
            linha(None, "/x/1.pdf", "falhou", None).tap_file(1),
        ]);
        let ids: Vec<i64> = ui.prontos(&e).iter().map(|i| i.file_id).collect();
        // 2 está em revisão; 4 já foi cadastrado; 1 falhou e pode ser tentado de novo.
        assert_eq!(ids, [1, 3]);
    }

    impl LinhaCadastro {
        fn tap_file(mut self, f: i64) -> Self {
            self.file_id = f;
            self
        }
    }
}
