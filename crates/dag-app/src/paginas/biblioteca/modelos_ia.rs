//! Biblioteca > Modelos IA: a IA local (Ollama ou llama-server em loopback) e o OCR. Lê e define
//! a parte `ia` e `ocr` da configuração da Curadoria. A IA começa desligada; um endpoint fora do
//! loopback é recusado (aqui e pelo agente).

use bancada_casca::egui;
use dag_core::ConfigBiblioteca;

use super::configuracoes::{Edicao, rodape, salvar, texto_opcional, validar};
use super::{Consulta, acao, estado_da_consulta, quadro};
use crate::app::AppDag;
use crate::paginas::{DefGrupo, DefPagina, cabecalho};

pub static PAGINA: DefPagina = DefPagina {
    id: "biblioteca-modelos-ia",
    rotulo: "Modelos IA",
    keytip: "O",
    icone: "modelos",
    grupos: &[DefGrupo {
        id: "biblioteca.modelos_ia",
        rotulo: "Modelos IA",
        acoes: &[acao(
            "biblioteca.ia_recarregar",
            "Recarregar modelos",
            "Descarta o que você digitou e relê a configuração da IA e do OCR",
        )],
    }],
    resumo: "IA local (desligada por padrão, só em loopback), modelos primário e secundário e OCR.",
    desenhar,
};

const CHAVES: [&str; 2] = ["ia", "ocr"];

#[derive(Debug, Default)]
pub struct Ui {
    pub ed: Edicao,
}

impl Ui {
    pub fn carregar(&mut self, c: &ConfigBiblioteca) {
        self.ed.carregar(c);
    }
}

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.bib_quadro(ui.ctx());
    app.bib_carregar(&[Consulta::Config]);
    estado_da_consulta(ui, &app.biblioteca, Consulta::Config);
    if app.biblioteca.config.is_none() {
        return;
    }
    let mut ed = std::mem::take(&mut app.biblioteca.ia.ed);
    quadro(ui, "IA local", |ui| {
        ui.checkbox(&mut ed.cfg.ia.ativa, "Usar a IA local no processamento");
        ui.weak(if ed.cfg.ia.ativa {
            "Ligada: o processamento pode consultar os modelos abaixo (Visão Geral > Processar)."
        } else {
            "Desligada: o DAG usa só extração e regras determinísticas."
        });
        egui::Grid::new("bib-ia-campos")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Endpoint (somente loopback)");
                ui.add(
                    egui::TextEdit::singleline(&mut ed.cfg.ia.endpoint)
                        .hint_text("http://127.0.0.1:11434")
                        .desired_width(340.0),
                );
                ui.end_row();
                ui.label("Modelo primário");
                ui.add(
                    egui::TextEdit::singleline(&mut ed.cfg.ia.modelo_primario)
                        .hint_text("gemma3:12b")
                        .desired_width(340.0),
                );
                ui.end_row();
                ui.label("Modelo secundário (segundo parecer)");
                texto_opcional(ui, &mut ed.cfg.ia.modelo_secundario, "opcional", 340.0);
                ui.end_row();
                ui.label("Usar visão (capa e páginas)");
                ui.checkbox(&mut ed.cfg.ia.usar_visao, "");
                ui.end_row();
                ui.label("Tempo limite por consulta (s)");
                ui.add(egui::DragValue::new(&mut ed.cfg.ia.timeout_s).range(5..=3600));
                ui.end_row();
            });
        ui.weak(
            "O modelo secundário só é consultado quando o primário tem confiança baixa ou os dois \
             divergem; a evidência (página e trecho) continua obrigatória.",
        );
    });
    ui.add_space(6.0);
    quadro(ui, "OCR dos PDFs digitalizados", |ui| {
        ui.checkbox(
            &mut ed.cfg.ocr.ativo,
            "Fazer OCR quando o PDF não tem texto",
        );
        egui::Grid::new("bib-ocr-campos")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Idiomas (códigos do Tesseract, separados por vírgula)");
                ui.add(egui::TextEdit::singleline(&mut ed.idiomas).desired_width(200.0));
                ui.end_row();
                ui.label("Resolução (DPI)");
                ui.add(egui::DragValue::new(&mut ed.cfg.ocr.dpi).range(72..=1200));
                ui.end_row();
                ui.label("Páginas com OCR por livro (no máximo)");
                ui.add(egui::DragValue::new(&mut ed.cfg.ocr.max_paginas).range(1..=200));
                ui.end_row();
                ui.label("Páginas iniciais examinadas");
                ui.add(egui::DragValue::new(&mut ed.cfg.ocr.paginas_iniciais).range(1..=500));
                ui.end_row();
            });
    });
    ui.add_space(4.0);
    let (enviar, descartar) = rodape(ui, app, &ed, &CHAVES);
    if descartar && let Some(b) = ed.base.clone() {
        ed.carregar(&b);
    }
    app.biblioteca.ia.ed = ed;
    if let Some(p) = enviar {
        salvar(app, p);
    }
}

/// Resumo de uma linha do que está configurado (para outras páginas).
#[must_use]
pub fn resumo_da_ia(c: &ConfigBiblioteca) -> String {
    if c.ia.ativa {
        format!(
            "IA ligada: {}{}{}",
            c.ia.modelo_primario,
            c.ia.modelo_secundario
                .as_deref()
                .map(|m| format!(" + {m}"))
                .unwrap_or_default(),
            if validar(c).is_empty() {
                ""
            } else {
                " (configuração com problemas)"
            }
        )
    } else {
        "IA desligada".into()
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::paginas::biblioteca::comandos::patch_vazio;

    #[test]
    fn so_ia_e_ocr_vao_deste_buffer() {
        let mut ui = Ui::default();
        ui.carregar(&ConfigBiblioteca::default());
        assert!(patch_vazio(&ui.ed.patch(&CHAVES)));
        ui.ed.cfg.ia.ativa = true;
        ui.ed.cfg.ia.modelo_secundario = Some("qwen2.5vl:7b".into());
        ui.ed.idiomas = "por".into();
        ui.ed.cfg.tentativas_max = 9;
        let p = ui.ed.patch(&CHAVES);
        assert_eq!(p["ia"]["ativa"], true);
        assert_eq!(p["ia"]["modelo_secundario"], "qwen2.5vl:7b");
        assert_eq!(p["ocr"]["idiomas"], serde_json::json!(["por"]));
        assert!(p.get("tentativas_max").is_none());
    }

    #[test]
    fn resumo_da_ia_conta_o_estado() {
        let mut c = ConfigBiblioteca::default();
        assert_eq!(resumo_da_ia(&c), "IA desligada");
        c.ia.ativa = true;
        c.ia.modelo_secundario = Some("b".into());
        assert_eq!(resumo_da_ia(&c), "IA ligada: gemma3:12b + b");
        c.ia.endpoint = "http://exemplo.com".into();
        assert!(resumo_da_ia(&c).contains("problemas"));
    }
}
