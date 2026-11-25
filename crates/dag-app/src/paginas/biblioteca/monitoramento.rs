//! Biblioteca > Monitoramento: o progresso do trabalho da Curadoria (que chega como mensagem de
//! progresso do job), a contagem da fila e a fila de exceções (itens que o DAG não conseguiu
//! tratar sozinho).

use bancada_casca::egui;

use super::texto::{rotulo_estado_fila, rotulo_motivo, texto_puro};
use super::{Consulta, acao, estado_da_consulta, progresso_do_job, quadro};
use crate::app::AppDag;
use crate::formato::{data_s, inteiro_pt_br};
use crate::paginas::tabela::{Chave, Coluna, nota_ordem_local, tabela};
use crate::paginas::{DefGrupo, DefPagina, cabecalho, estado_vazio};

pub static PAGINA: DefPagina = DefPagina {
    id: "biblioteca-monitoramento",
    rotulo: "Monitoramento",
    keytip: "N",
    icone: "monitoramento",
    grupos: &[DefGrupo {
        id: "biblioteca.monitoramento",
        rotulo: "Monitoramento",
        acoes: &[acao(
            "biblioteca.monitor_atualizar",
            "Atualizar fila",
            "Relê a fila, as exceções e o resumo",
        )],
    }],
    resumo: "Progresso do trabalho da Biblioteca, fila e exceções.",
    desenhar,
};

const COLUNAS: [Coluna; 7] = [
    Coluna::esq("Arquivo", 380.0),
    Coluna::esq("Estado", 90.0),
    Coluna::esq("Motivo", 90.0),
    Coluna::esq("Etapa", 130.0),
    Coluna::dir("Tentativas", 80.0),
    Coluna::esq("Erro", 380.0),
    Coluna::esq("Enfileirado (UTC)", 150.0),
];

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.bib_quadro(ui.ctx());
    app.bib_carregar(&[Consulta::Fila, Consulta::Excecoes, Consulta::Resumo]);
    quadro(ui, "Trabalho em andamento", |ui| progresso_do_job(ui, app));
    ui.add_space(6.0);
    quadro(ui, "Fila", |ui| {
        estado_da_consulta(ui, &app.biblioteca, Consulta::Fila);
        match app.biblioteca.fila.as_ref().map(|f| f.contagem) {
            Some(c) => {
                let n = |x: i64| inteiro_pt_br(u64::try_from(x).unwrap_or(0));
                ui.label(format!(
                    "{} pendente(s) · {} em andamento · {} concluído(s) · {} falha(s) · {} exceção(ões)",
                    n(c.pendentes),
                    n(c.em_andamento),
                    n(c.concluidos),
                    n(c.falhos),
                    n(c.excecoes)
                ));
            }
            None => {
                ui.weak("Fila ainda não lida.");
            }
        }
        if let Some(p) = &app.biblioteca.processamento {
            ui.weak(format!(
                "Último processamento: {} item(ns), {} falha(s), {} exceção(ões), {} consulta(s) à IA, {} páginas ({} com OCR).",
                inteiro_pt_br(p.processados),
                inteiro_pt_br(p.falhas),
                inteiro_pt_br(p.excecoes),
                inteiro_pt_br(p.consultas_ia),
                inteiro_pt_br(p.paginas),
                inteiro_pt_br(p.paginas_ocr),
            ));
        }
    });
    ui.add_space(6.0);
    excecoes(ui, app);
}

fn excecoes(ui: &mut egui::Ui, app: &mut AppDag) {
    let lista = app.biblioteca.excecoes.clone();
    let mut mais = false;
    quadro(ui, "Fila de exceções", |ui| {
        estado_da_consulta(ui, &app.biblioteca, Consulta::Excecoes);
        let Some(d) = &lista else {
            return;
        };
        if d.itens.is_empty() {
            estado_vazio(
                ui,
                "Nenhuma exceção",
                PAGINA.resumo,
                "Itens que o DAG não consegue tratar sozinho (arquivo ilegível, formato sem suporte) aparecem aqui.",
            );
            return;
        }
        tabela(
            ui,
            "bib-excecoes",
            &COLUNAS,
            d.itens.len(),
            crate::paginas::altura_livre(ui).clamp(160.0, 420.0),
            |c, i| {
                let x = &d.itens[i];
                match c {
                    0 => Chave::opt_texto(x.caminho.as_deref()),
                    1 => Chave::texto(rotulo_estado_fila(x.estado)),
                    2 => Chave::texto(rotulo_motivo(x.motivo)),
                    3 => Chave::opt_texto(x.etapa.as_deref()),
                    4 => Chave::num(x.tentativas),
                    5 => Chave::opt_texto(x.erro.as_deref()),
                    _ => Chave::num(x.enfileirado_em),
                }
            },
            |l, i| {
                let x = &d.itens[i];
                l.texto(texto_puro(
                    x.caminho.as_deref().unwrap_or("(sem caminho)"),
                    200,
                ));
                l.texto(rotulo_estado_fila(x.estado));
                l.texto(rotulo_motivo(x.motivo));
                l.texto(texto_puro(x.etapa.as_deref().unwrap_or("—"), 60));
                l.texto(x.tentativas.to_string());
                l.texto(texto_puro(x.erro.as_deref().unwrap_or("—"), 200));
                l.texto(data_s(Some(x.enfileirado_em)));
            },
        );
        nota_ordem_local(ui, "bib-excecoes");
        if d.proximo.is_some()
            && ui
                .add_enabled(
                    !app.biblioteca.carregando(Consulta::Excecoes),
                    egui::Button::new("Carregar mais"),
                )
                .clicked()
        {
            mais = true;
        }
    });
    if mais {
        let cursor = lista.and_then(|d| d.proximo);
        app.bib_pedir(Consulta::Excecoes, cursor);
    }
}
