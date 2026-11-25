//! Biblioteca > Alertas e Resumo (WOW-01, WOW-10): "Você já tem este livro" com título, formato,
//! hash e as localizações, marcar como visto, e o resumo do período (novidades da biblioteca).

use std::collections::{BTreeSet, HashMap};

use bancada_casca::egui;
use dag_core::AlertaInfo;
use dag_store::biblioteca as store;
use rusqlite::Connection;

use super::comandos;
use super::texto::{formato_do_caminho, hex, rotulo_tipo_alerta, texto_puro};
use super::{AcaoBib, Consulta, acao, erro_da_acao, estado_da_consulta, quadro};
use crate::app::AppDag;
use crate::formato::{bytes_humanos, data_s, hash_curto, inteiro_pt_br};
use crate::leitor::{Carga, Leitor, erro};
use crate::paginas::tabela::{Chave, Coluna, nota_ordem_local, tabela};
use crate::paginas::{DefGrupo, DefPagina, cabecalho, estado_vazio};

pub static PAGINA: DefPagina = DefPagina {
    id: "biblioteca-alertas",
    rotulo: "Alertas e Resumo",
    keytip: "T",
    icone: "alertas",
    grupos: &[DefGrupo {
        id: "biblioteca.alertas",
        rotulo: "Alertas",
        acoes: &[
            acao(
                "biblioteca.alertas_marcar_todos",
                "Marcar todos como vistos",
                "Marca como vistos os alertas novos da lista carregada",
            ),
            acao(
                "biblioteca.alertas_atualizar",
                "Atualizar alertas",
                "Relê os alertas e o resumo",
            ),
        ],
    }],
    resumo: "\"Você já tem este livro\" e as novidades da biblioteca no período.",
    desenhar,
};

/// Período do resumo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Periodo {
    Dia,
    Semana,
    Mes,
}

impl Periodo {
    pub const TODOS: [Self; 3] = [Self::Dia, Self::Semana, Self::Mes];

    #[must_use]
    pub fn rotulo(self) -> &'static str {
        match self {
            Self::Dia => "últimas 24 horas",
            Self::Semana => "últimos 7 dias",
            Self::Mes => "últimos 30 dias",
        }
    }

    #[must_use]
    pub fn segundos(self) -> i64 {
        match self {
            Self::Dia => 86_400,
            Self::Semana => 7 * 86_400,
            Self::Mes => 30 * 86_400,
        }
    }
}

fn agora_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// O que se descobre, no Catálogo, sobre o arquivo de um alerta.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Enriquecimento {
    pub hash: Option<String>,
    pub tamanho: Option<u64>,
    /// Onde o conteúdo está: a cópia nova e os formatos homologados idênticos.
    pub localizacoes: Vec<String>,
}

/// Lê, para cada `(alerta, arquivo)`, o hash, o tamanho e as localizações (somente leitura).
///
/// # Errors
/// Texto do erro do SQLite.
pub fn enriquecer(
    conn: &Connection,
    pares: &[(i64, i64)],
) -> Result<HashMap<i64, Enriquecimento>, String> {
    let mut m = HashMap::new();
    for &(alerta, file_id) in pares {
        let Some(info) = store::info_arquivo(conn, file_id).map_err(erro)? else {
            continue;
        };
        let mut e = Enriquecimento {
            hash: info.hash.map(|h| hex(&h)),
            tamanho: Some(info.tamanho),
            localizacoes: vec![format!("nova cópia: {}", info.caminho)],
        };
        if let Some(h) = info.hash {
            for a in store::buscar_por_hash(conn, &h, Some(info.tamanho)).map_err(erro)? {
                e.localizacoes
                    .push(format!("Homologada: {}", a.formato.caminho));
            }
        }
        m.insert(alerta, e);
    }
    Ok(m)
}

/// Estado de tela da página.
#[derive(Debug)]
pub struct Ui {
    pub so_nao_vistos: bool,
    pub periodo: Periodo,
    pub selecionados: BTreeSet<i64>,
    /// Sobe quando chega uma lista nova de alertas (para reler o enriquecimento).
    pub geracao_leitor: u64,
    vista: u64,
    pub enriquecimento: Carga<HashMap<i64, Enriquecimento>>,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            so_nao_vistos: true,
            periodo: Periodo::Dia,
            selecionados: BTreeSet::new(),
            geracao_leitor: 1,
            vista: 0,
            enriquecimento: Carga::default(),
        }
    }
}

impl Ui {
    /// Início do período do resumo (segundos Unix).
    #[must_use]
    pub fn desde(&self) -> Option<i64> {
        Some(agora_s() - self.periodo.segundos())
    }

    fn atualizar(&mut self, leitor: &Leitor, pares: Vec<(i64, i64)>) {
        if self.vista != self.geracao_leitor {
            self.vista = self.geracao_leitor;
            self.enriquecimento
                .pedir(leitor.consultar(move |c| enriquecer(c, &pares)));
        }
        self.enriquecimento.atualizar();
    }
}

/// Como um alerta aparece na tabela.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescricaoAlerta {
    pub tipo: String,
    pub titulo: String,
    pub formato: String,
    pub hash: String,
    pub localizacoes: Vec<String>,
}

/// Título, formato, hash e localizações de um alerta, a partir do que ele carrega e do que o
/// Catálogo sabe do arquivo.
#[must_use]
pub fn descrever(a: &AlertaInfo, enr: Option<&Enriquecimento>) -> DescricaoAlerta {
    let campo = |k: &str| a.dados.get(k).and_then(|v| v.as_str());
    let caminho = campo("caminho").unwrap_or("");
    let titulo = campo("livro")
        .or_else(|| campo("titulo"))
        .map_or_else(|| "—".to_string(), |t| texto_puro(t, 160));
    let mut loc: Vec<String> = Vec::new();
    match enr {
        Some(e) => loc.extend(e.localizacoes.iter().cloned()),
        None if !caminho.is_empty() => loc.push(format!("nova cópia: {caminho}")),
        None => {}
    }
    DescricaoAlerta {
        tipo: rotulo_tipo_alerta(&a.tipo),
        titulo,
        formato: formato_do_caminho(caminho),
        hash: enr
            .and_then(|e| e.hash.as_deref())
            .map_or_else(|| "—".into(), hash_curto),
        localizacoes: loc,
    }
}

pub(super) fn marcar_todos(app: &mut AppDag) {
    let ids: Vec<i64> = app
        .biblioteca
        .alertas
        .as_ref()
        .map(|a| {
            a.itens
                .iter()
                .filter(|i| i.visto_em.is_none())
                .map(|i| i.id)
                .collect()
        })
        .unwrap_or_default();
    if !ids.is_empty() {
        app.bib_acao(AcaoBib::MarcarVisto, comandos::marcar_vistos(ids));
    }
}

const COLUNAS: [Coluna; 8] = [
    Coluna::esq("", 26.0).sem_ordem(),
    Coluna::esq("Alerta", 170.0),
    Coluna::esq("Título", 260.0),
    Coluna::esq("Formato", 70.0),
    Coluna::esq("Hash", 80.0),
    Coluna::esq("Quando (UTC)", 150.0),
    Coluna::esq("Visto", 60.0),
    Coluna::esq("Onde está", 520.0),
];

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.bib_quadro(ui.ctx());
    app.bib_carregar(&[Consulta::Alertas, Consulta::Resumo]);
    let pares: Vec<(i64, i64)> = app
        .biblioteca
        .alertas
        .as_ref()
        .map(|a| {
            a.itens
                .iter()
                .filter_map(|i| i.file_id.map(|f| (i.id, f)))
                .collect()
        })
        .unwrap_or_default();
    app.biblioteca.alerta.atualizar(&app.leitor, pares);
    resumo(ui, app);
    ui.add_space(6.0);
    alertas(ui, app);
}

fn resumo(ui: &mut egui::Ui, app: &mut AppDag) {
    let antes = app.biblioteca.alerta.periodo;
    quadro(ui, "Novidades da biblioteca", |ui| {
        ui.horizontal(|ui| {
            ui.label("Período:");
            egui::ComboBox::from_id_salt("bib-periodo")
                .selected_text(app.biblioteca.alerta.periodo.rotulo())
                .show_ui(ui, |ui| {
                    for p in Periodo::TODOS {
                        ui.selectable_value(&mut app.biblioteca.alerta.periodo, p, p.rotulo());
                    }
                });
        });
        estado_da_consulta(ui, &app.biblioteca, Consulta::Resumo);
        let Some(r) = &app.biblioteca.resumo else {
            return;
        };
        let n = |x: i64| inteiro_pt_br(u64::try_from(x).unwrap_or(0));
        ui.label(format!(
            "De {} a {} (UTC): {} livro(s) novo(s) na fila · {} revisão(ões) pendente(s) · {} \
             candidato(s) a duplicidade · {} recuperáveis em cópias exatas · {} alerta(s) não visto(s).",
            data_s(Some(r.de)),
            data_s(Some(r.ate)),
            n(r.livros_novos_na_fila),
            n(r.revisoes_pendentes),
            n(r.candidatos_duplicidade_abertos),
            bytes_humanos(r.bytes_recuperaveis_exatos),
            n(r.alertas_nao_vistos),
        ));
        ui.weak(format!(
            "Fila: {} pendente(s), {} em andamento, {} concluído(s), {} falha(s), {} exceção(ões).",
            n(r.fila.pendentes),
            n(r.fila.em_andamento),
            n(r.fila.concluidos),
            n(r.fila.falhos),
            n(r.fila.excecoes),
        ));
        if r.alertas_por_tipo.is_empty() {
            ui.weak("Nenhum alerta no período.");
        }
        for (tipo, total, nao_vistos) in &r.alertas_por_tipo {
            ui.label(format!(
                "• {}: {} no período, {} não visto(s)",
                rotulo_tipo_alerta(tipo),
                n(*total),
                n(*nao_vistos)
            ));
        }
        if !app.biblioteca.alertas_recentes.is_empty() {
            ui.separator();
            ui.weak("Anunciados agora pelo agente:");
            for a in &app.biblioteca.alertas_recentes {
                ui.label(format!("• {a}"));
            }
        }
    });
    if app.biblioteca.alerta.periodo != antes {
        app.biblioteca.atualizar(&[Consulta::Resumo]);
    }
}

fn alertas(ui: &mut egui::Ui, app: &mut AppDag) {
    let sem_escrita = app.motivo_sem_escrita_config();
    let antes = app.biblioteca.alerta.so_nao_vistos;
    let mut marcar: Option<Vec<i64>> = None;
    let mut alternar = None;
    ui.horizontal_wrapped(|ui| {
        ui.strong("Você já tem este livro");
        ui.checkbox(&mut app.biblioteca.alerta.so_nao_vistos, "só os não vistos");
        let n = app.biblioteca.alerta.selecionados.len();
        let b = ui.add_enabled(
            sem_escrita.is_none() && n > 0,
            egui::Button::new(format!("Marcar selecionados como vistos ({n})")),
        );
        if b.clicked() {
            marcar = Some(app.biblioteca.alerta.selecionados.iter().copied().collect());
        }
    });
    if app.biblioteca.alerta.so_nao_vistos != antes {
        app.biblioteca.atualizar(&[Consulta::Alertas]);
    }
    estado_da_consulta(ui, &app.biblioteca, Consulta::Alertas);
    erro_da_acao(ui, &app.biblioteca, &[AcaoBib::MarcarVisto]);
    let Some(d) = app.biblioteca.alertas.clone() else {
        return;
    };
    if d.itens.is_empty() {
        estado_vazio(
            ui,
            "Nenhum alerta",
            PAGINA.resumo,
            "Quando um livro que você já tem aparecer em uma pasta de download, o aviso vem para cá.",
        );
        return;
    }
    let enr = app
        .biblioteca
        .alerta
        .enriquecimento
        .dado
        .clone()
        .unwrap_or_default();
    let sel = app.biblioteca.alerta.selecionados.clone();
    let descr: Vec<DescricaoAlerta> = d
        .itens
        .iter()
        .map(|a| descrever(a, enr.get(&a.id)))
        .collect();
    let altura = crate::paginas::altura_livre(ui).clamp(220.0, 520.0);
    tabela(
        ui,
        "bib-alertas",
        &COLUNAS,
        d.itens.len(),
        altura,
        |c, i| {
            let (a, x) = (&d.itens[i], &descr[i]);
            match c {
                0 => Chave::num(i128::from(sel.contains(&a.id))),
                1 => Chave::texto(&x.tipo),
                2 => Chave::texto(&x.titulo),
                3 => Chave::texto(&x.formato),
                4 => Chave::texto(&x.hash),
                5 => Chave::num(a.criado_em),
                6 => Chave::opt_num(a.visto_em),
                _ => Chave::texto(&x.localizacoes.join(" | ")),
            }
        },
        |l, i| {
            let (a, x) = (&d.itens[i], &descr[i]);
            l.ui(|ui| {
                let mut m = sel.contains(&a.id);
                if ui
                    .add_enabled(a.visto_em.is_none(), egui::Checkbox::without_text(&mut m))
                    .changed()
                {
                    alternar = Some(a.id);
                }
            });
            l.texto(x.tipo.clone());
            l.texto(x.titulo.clone());
            l.texto(x.formato.clone());
            l.texto(x.hash.clone());
            l.texto(data_s(Some(a.criado_em)));
            l.texto(if a.visto_em.is_some() { "sim" } else { "não" });
            l.texto(texto_puro(&x.localizacoes.join(" | "), 400));
        },
    );
    nota_ordem_local(ui, "bib-alertas");
    ui.horizontal(|ui| {
        ui.weak(format!(
            "{} alerta(s) carregado(s) · {} não visto(s) no total",
            inteiro_pt_br(d.itens.len() as u64),
            inteiro_pt_br(u64::try_from(d.nao_vistos_total).unwrap_or(0))
        ));
        if d.proximo.is_some()
            && ui
                .add_enabled(
                    !app.biblioteca.carregando(Consulta::Alertas),
                    egui::Button::new("Carregar mais"),
                )
                .clicked()
        {
            app.bib_pedir(Consulta::Alertas, d.proximo.clone());
        }
    });
    if let Some(id) = alternar
        && !app.biblioteca.alerta.selecionados.remove(&id)
    {
        app.biblioteca.alerta.selecionados.insert(id);
    }
    if let Some(ids) = marcar {
        app.bib_acao(AcaoBib::MarcarVisto, comandos::marcar_vistos(ids));
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn alerta(dados: serde_json::Value) -> AlertaInfo {
        AlertaInfo {
            id: 1,
            chave: None,
            tipo: "ja_tem_este_livro".into(),
            file_id: Some(9),
            dados,
            criado_em: 1,
            visto_em: None,
            silenciado: false,
        }
    }

    #[test]
    fn descreve_com_o_que_o_alerta_carrega() {
        let a = alerta(serde_json::json!({"livro": "Cálculo\u{202E}", "caminho": "/dl/c.epub"}));
        let d = descrever(&a, None);
        assert_eq!(d.tipo, "Você já tem este livro");
        assert_eq!(d.titulo, "Cálculo");
        assert_eq!(d.formato, "EPUB");
        assert_eq!(d.hash, "—");
        assert_eq!(d.localizacoes, ["nova cópia: /dl/c.epub"]);
    }

    #[test]
    fn descreve_com_hash_e_localizacoes_do_catalogo() {
        let a = alerta(serde_json::json!({"livro": "L", "caminho": "/dl/l.pdf"}));
        let e = Enriquecimento {
            hash: Some("abcdef0123456789".into()),
            tamanho: Some(5),
            localizacoes: vec![
                "nova cópia: /dl/l.pdf".into(),
                "Homologada: /lib/l.pdf".into(),
            ],
        };
        let d = descrever(&a, Some(&e));
        assert_eq!(d.hash, "abcdef01");
        assert_eq!(d.localizacoes.len(), 2);
        let sem = descrever(&alerta(serde_json::json!({})), None);
        assert_eq!(sem.titulo, "—");
        assert!(sem.localizacoes.is_empty());
    }

    #[test]
    fn periodo_vira_janela_em_segundos() {
        let u = Ui::default();
        let desde = u.desde().unwrap();
        let agora = agora_s();
        assert!((agora - desde - 86_400).abs() < 5);
        assert_eq!(Periodo::Mes.segundos(), 30 * 86_400);
        assert!(u.so_nao_vistos);
    }
}
