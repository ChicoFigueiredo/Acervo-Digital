//! Biblioteca > Configurações: caminhos, limites, aging, regras e modo do Calibre. Lê e define a
//! configuração da Curadoria (`BibliotecaConfigObter`/`BibliotecaConfigDefinir`); o agente
//! valida e grava. Só o que mudou vai no pedido (merge-patch). Senhas nunca aparecem: o
//! servidor do Calibre só é lembrado pelo NOME da variável de ambiente que guarda a senha.

use bancada_casca::egui;
use dag_core::{ConfigBiblioteca, ModoHomologacao, PoliticaTravaCalibre};
use serde_json::Value;

use super::comandos::{config_definir, patch_de_config, patch_vazio};
use super::{AcaoBib, Consulta, acao, erro_da_acao, estado_da_consulta, quadro};
use crate::app::AppDag;
use crate::paginas::{DefGrupo, DefPagina, cabecalho};

pub static PAGINA: DefPagina = DefPagina {
    id: "biblioteca-configuracoes",
    rotulo: "Configurações",
    keytip: "G",
    icone: "configuracoes",
    grupos: &[DefGrupo {
        id: "biblioteca.configuracoes",
        rotulo: "Configuração",
        acoes: &[acao(
            "biblioteca.config_recarregar",
            "Recarregar configuração",
            "Descarta o que você digitou e relê a configuração gravada no Catálogo",
        )],
    }],
    resumo: "Caminhos, limites, aging, regras e o modo do Calibre (a senha do servidor nunca passa por aqui).",
    desenhar,
};

const MIB: u64 = 1024 * 1024;

/// Buffer de edição de uma página de configuração: a cópia editável, o que está gravado e os
/// textos das listas.
#[derive(Debug, Default)]
pub struct Edicao {
    pub base: Option<ConfigBiblioteca>,
    pub cfg: ConfigBiblioteca,
    pub idiomas: String,
    pub extensoes: String,
    pub pastas: String,
    pub max_mib: u64,
}

/// `a, b\nc` → `["a","b","c"]`: sem brancos nem repetição.
#[must_use]
pub fn parse_lista(t: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for p in t.split([',', '\n', ';']) {
        let p = p.trim();
        if !p.is_empty() && !v.iter().any(|x| x == p) {
            v.push(p.to_string());
        }
    }
    v
}

impl Edicao {
    /// Recomeça a edição a partir da configuração gravada.
    pub fn carregar(&mut self, c: &ConfigBiblioteca) {
        self.base = Some(c.clone());
        self.cfg = c.clone();
        self.idiomas = c.ocr.idiomas.join(", ");
        self.extensoes = c.extensoes_ignoradas.join(", ");
        self.pastas = c.pastas_a_classificar.join("\n");
        self.max_mib = c.max_bytes_arquivo / MIB;
    }

    /// A configuração com os textos das listas aplicados. O limite de tamanho só muda se o
    /// valor em MiB foi mexido (evita arredondar o que estava gravado).
    #[must_use]
    pub fn resultado(&self) -> ConfigBiblioteca {
        let mut c = self.cfg.clone();
        c.ocr.idiomas = parse_lista(&self.idiomas);
        c.extensoes_ignoradas = parse_lista(&self.extensoes);
        c.pastas_a_classificar = parse_lista(&self.pastas);
        if let Some(b) = &self.base {
            c.max_bytes_arquivo = if self.max_mib == b.max_bytes_arquivo / MIB {
                b.max_bytes_arquivo
            } else {
                self.max_mib.saturating_mul(MIB)
            };
        }
        c
    }

    /// Merge-patch só com as chaves de topo `chaves` que mudaram.
    #[must_use]
    pub fn patch(&self, chaves: &[&str]) -> Value {
        match &self.base {
            Some(b) => patch_de_config(b, &self.resultado(), chaves),
            None => Value::Null,
        }
    }
}

fn loopback(e: &str) -> bool {
    let sem = e
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    sem.starts_with("127.0.0.1") || sem.starts_with("localhost") || sem.starts_with("[::1]")
}

/// Problemas da configuração (a IA só em loopback; números positivos; caminhos absolutos).
#[must_use]
pub fn validar(c: &ConfigBiblioteca) -> Vec<String> {
    let mut e = Vec::new();
    if c.ia.ativa {
        if !loopback(&c.ia.endpoint) {
            e.push("O endpoint da IA precisa ser local (127.0.0.1, localhost ou [::1]).".into());
        }
        if c.ia.modelo_primario.trim().is_empty() {
            e.push("Informe o modelo primário da IA.".into());
        }
    }
    if c.ia.timeout_s == 0 {
        e.push("O tempo limite da IA precisa ser maior que zero.".into());
    }
    if c.ocr.ativo && c.ocr.idiomas.is_empty() {
        e.push("Informe ao menos um idioma para o OCR (por exemplo por, eng).".into());
    }
    if c.ocr.dpi == 0 {
        e.push("O DPI do OCR precisa ser maior que zero.".into());
    }
    if !c.destino_a_classificar.trim().starts_with('/') {
        e.push("O destino da A Classificar precisa ser um caminho absoluto.".into());
    }
    if c.extracoes_por_dispositivo == 0 {
        e.push("Extrações por dispositivo: no mínimo 1.".into());
    }
    if c.tentativas_max == 0 {
        e.push("Tentativas: no mínimo 1.".into());
    }
    if c.max_bytes_arquivo == 0 {
        e.push("O tamanho máximo por arquivo precisa ser maior que zero.".into());
    }
    if !c.aging_por_hora.is_finite() || c.aging_por_hora < 0.0 {
        e.push("O aging por hora precisa ser um número maior ou igual a zero.".into());
    }
    if c.homologacao.modo == ModoHomologacao::Servidor
        && c.homologacao
            .url
            .as_deref()
            .is_none_or(|u| u.trim().is_empty())
    {
        e.push("No modo servidor, informe a URL do calibre-server.".into());
    }
    e
}

/// Rótulos do modo e da política de trava.
#[must_use]
pub fn rotulo_modo(m: ModoHomologacao) -> &'static str {
    match m {
        ModoHomologacao::Local => "local (calibredb direto na pasta)",
        ModoHomologacao::Servidor => "servidor (calibre-server)",
    }
}

#[must_use]
pub fn rotulo_trava(p: PoliticaTravaCalibre) -> &'static str {
    match p {
        PoliticaTravaCalibre::Estrita => "estrita (recusa se o Calibre estiver aberto)",
        PoliticaTravaCalibre::ApenasServidor => "só consulta o servidor",
    }
}

/// Edita um texto opcional (vazio = `None`).
pub fn texto_opcional(ui: &mut egui::Ui, valor: &mut Option<String>, dica: &str, largura: f32) {
    let mut t = valor.clone().unwrap_or_default();
    if ui
        .add(
            egui::TextEdit::singleline(&mut t)
                .hint_text(dica)
                .desired_width(largura),
        )
        .changed()
    {
        *valor = (!t.trim().is_empty()).then_some(t);
    }
}

/// Rodapé comum: erros de validação, Salvar e Descartar. Devolve o patch a enviar, se salvou.
pub fn rodape(
    ui: &mut egui::Ui,
    app: &mut AppDag,
    ed: &Edicao,
    chaves: &[&str],
) -> (Option<Value>, bool) {
    let resultado = ed.resultado();
    let erros = validar(&resultado);
    let patch = ed.patch(chaves);
    let mudou = !patch_vazio(&patch);
    let sem_escrita = app.motivo_sem_escrita_config();
    let em_voo = app.biblioteca.acao_em_voo(AcaoBib::ConfigDefinir);
    let mut salvar = None;
    let mut descartar = false;
    ui.separator();
    for e in &erros {
        ui.colored_label(ui.visuals().warn_fg_color, format!("Atenção: {e}"));
    }
    ui.horizontal(|ui| {
        let pode = mudou && erros.is_empty() && sem_escrita.is_none() && !em_voo;
        let b = ui.add_enabled(pode, egui::Button::new("Salvar"));
        let b = if let Some(m) = sem_escrita {
            b.on_disabled_hover_text(m)
        } else if !mudou {
            b.on_disabled_hover_text("Nada mudou")
        } else {
            b.on_hover_text("O agente valida e grava no Catálogo")
        };
        if b.clicked() {
            salvar = Some(patch.clone());
        }
        descartar = ui
            .add_enabled(mudou, egui::Button::new("Descartar alterações"))
            .clicked();
        if mudou {
            ui.weak("Alterações ainda não salvas.");
        }
        if em_voo {
            ui.spinner();
        }
    });
    erro_da_acao(ui, &app.biblioteca, &[AcaoBib::ConfigDefinir]);
    (salvar, descartar)
}

/// Envia o patch ao agente.
pub fn salvar(app: &mut AppDag, patch: Value) {
    app.bib_acao(AcaoBib::ConfigDefinir, config_definir(patch));
}

#[derive(Debug, Default)]
pub struct Ui {
    pub ed: Edicao,
}

impl Ui {
    pub fn carregar(&mut self, c: &ConfigBiblioteca) {
        self.ed.carregar(c);
    }
}

const CHAVES: [&str; 11] = [
    "destino_a_classificar",
    "pastas_a_classificar",
    "extracoes_por_dispositivo",
    "max_bytes_arquivo",
    "tentativas_max",
    "tamanho_minimo_duplicata",
    "debounce_s",
    "extensoes_ignoradas",
    "aging_por_hora",
    "versao_regras",
    "usar_unshare",
];
const CHAVES_CALIBRE: [&str; 2] = ["politica_trava", "homologacao"];

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.bib_quadro(ui.ctx());
    app.bib_carregar(&[Consulta::Config]);
    estado_da_consulta(ui, &app.biblioteca, Consulta::Config);
    if app.biblioteca.config.is_none() {
        return;
    }
    let mut ed = std::mem::take(&mut app.biblioteca.config_ui.ed);
    quadro(ui, "Caminhos", |ui| {
        ui.label("Destino para criar a A Classificar (quando o caminho registrado ainda não é uma biblioteca):");
        ui.add(
            egui::TextEdit::singleline(&mut ed.cfg.destino_a_classificar)
                .desired_width(f32::INFINITY),
        );
        ui.label("Trechos de caminho que identificam pastas de download ou a classificar (um por linha):");
        ui.add(
            egui::TextEdit::multiline(&mut ed.pastas)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
    });
    ui.add_space(6.0);
    quadro(ui, "Limites e ritmo", |ui| {
        egui::Grid::new("bib-cfg-limites")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Extrações por dispositivo físico");
                ui.add(egui::DragValue::new(&mut ed.cfg.extracoes_por_dispositivo).range(1..=8));
                ui.end_row();
                ui.label("Tamanho máximo por arquivo (MiB)");
                ui.add(egui::DragValue::new(&mut ed.max_mib).range(1..=1_048_576));
                ui.end_row();
                ui.label("Tentativas por livro");
                ui.add(egui::DragValue::new(&mut ed.cfg.tentativas_max).range(1..=20));
                ui.end_row();
                ui.label("Tamanho mínimo para duplicata exata (bytes)");
                ui.add(
                    egui::DragValue::new(&mut ed.cfg.tamanho_minimo_duplicata).range(0..=1_048_576),
                );
                ui.end_row();
                ui.label("Espera do arquivo estável antes de enfileirar (s)");
                ui.add(egui::DragValue::new(&mut ed.cfg.debounce_s).range(0..=3600));
                ui.end_row();
                ui.label("Aging: pontos por hora de espera na fila");
                ui.add(
                    egui::DragValue::new(&mut ed.cfg.aging_por_hora)
                        .speed(0.01)
                        .range(0.0..=10.0),
                );
                ui.end_row();
            });
        ui.label("Extensões de download incompleto, ignoradas (separadas por vírgula):");
        ui.add(egui::TextEdit::singleline(&mut ed.extensoes).desired_width(f32::INFINITY));
    });
    ui.add_space(6.0);
    quadro(ui, "Regras", |ui| {
        ui.horizontal(|ui| {
            ui.label("Versão das regras de extração e classificação:");
            ui.add(egui::TextEdit::singleline(&mut ed.cfg.versao_regras).desired_width(120.0));
        });
        ui.weak("Mudar a versão reprocessa os livros já extraídos (CA-B11).");
    });
    ui.add_space(6.0);
    quadro(ui, "Modo do Calibre", |ui| {
        ui.checkbox(
            &mut ed.cfg.usar_unshare,
            "Isolar a trava do Calibre com unshare (desligado por padrão)",
        );
        ui.horizontal(|ui| {
            ui.label("Trava de escrita:");
            egui::ComboBox::from_id_salt("bib-cfg-trava")
                .selected_text(rotulo_trava(ed.cfg.politica_trava))
                .show_ui(ui, |ui| {
                    for p in [
                        PoliticaTravaCalibre::Estrita,
                        PoliticaTravaCalibre::ApenasServidor,
                    ] {
                        ui.selectable_value(&mut ed.cfg.politica_trava, p, rotulo_trava(p));
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Homologação:");
            egui::ComboBox::from_id_salt("bib-cfg-modo")
                .selected_text(rotulo_modo(ed.cfg.homologacao.modo))
                .show_ui(ui, |ui| {
                    for m in [ModoHomologacao::Local, ModoHomologacao::Servidor] {
                        ui.selectable_value(&mut ed.cfg.homologacao.modo, m, rotulo_modo(m));
                    }
                });
        });
        if ed.cfg.homologacao.modo == ModoHomologacao::Servidor {
            egui::Grid::new("bib-cfg-servidor")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("URL do servidor");
                    texto_opcional(
                        ui,
                        &mut ed.cfg.homologacao.url,
                        "http://127.0.0.1:17790",
                        320.0,
                    );
                    ui.end_row();
                    ui.label("Id da biblioteca");
                    texto_opcional(
                        ui,
                        &mut ed.cfg.homologacao.id_biblioteca,
                        "id no servidor",
                        320.0,
                    );
                    ui.end_row();
                    ui.label("Usuário");
                    texto_opcional(ui, &mut ed.cfg.homologacao.usuario, "usuário", 320.0);
                    ui.end_row();
                    ui.label("Variável de ambiente da senha");
                    texto_opcional(
                        ui,
                        &mut ed.cfg.homologacao.senha_env,
                        "DAG_CALIBRE_SENHA",
                        320.0,
                    );
                    ui.end_row();
                });
            ui.weak(
                "A senha do servidor fica só na variável de ambiente do agente: a GUI nunca a \
                 pede, mostra ou grava.",
            );
        }
    });
    let mut chaves: Vec<&str> = CHAVES.to_vec();
    chaves.extend(CHAVES_CALIBRE);
    let (enviar, descartar) = rodape(ui, app, &ed, &chaves);
    if descartar && let Some(b) = ed.base.clone() {
        ed.carregar(&b);
    }
    app.biblioteca.config_ui.ed = ed;
    if let Some(p) = enviar {
        salvar(app, p);
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn lista_de_texto_sem_brancos_nem_repeticao() {
        assert_eq!(parse_lista(" por, eng ,por;\n\nspa"), ["por", "eng", "spa"]);
        assert!(parse_lista("  ").is_empty());
    }

    #[test]
    fn edicao_sem_mudanca_nao_gera_patch() {
        let mut e = Edicao::default();
        e.carregar(&ConfigBiblioteca::default());
        assert!(patch_vazio(&e.patch(&[])));
        // Os textos das listas, sem mexer, reproduzem as listas gravadas.
        assert_eq!(e.resultado(), ConfigBiblioteca::default());
    }

    #[test]
    fn so_o_que_mudou_vai_e_o_limite_em_mib_nao_arredonda() {
        let base = ConfigBiblioteca {
            max_bytes_arquivo: 5 * MIB + 123,
            ..ConfigBiblioteca::default()
        };
        let mut e = Edicao::default();
        e.carregar(&base);
        e.cfg.tentativas_max = 5;
        e.pastas = "novos\nbaixados".into();
        let p = e.patch(&CHAVES);
        assert_eq!(p["tentativas_max"], 5);
        assert_eq!(
            p["pastas_a_classificar"],
            serde_json::json!(["novos", "baixados"])
        );
        assert!(p.get("max_bytes_arquivo").is_none(), "{p}");
        e.max_mib = 9;
        let p = e.patch(&CHAVES);
        assert_eq!(p["max_bytes_arquivo"], 9 * MIB);
        // As chaves de IA ficam de fora desta página.
        e.cfg.ia.ativa = true;
        assert!(e.patch(&CHAVES).get("ia").is_none());
    }

    #[test]
    fn validacao_pega_ia_remota_e_numeros_invalidos() {
        let mut c = ConfigBiblioteca::default();
        assert!(validar(&c).is_empty());
        c.ia.ativa = true;
        c.ia.endpoint = "http://10.0.0.5:11434".into();
        assert!(validar(&c).iter().any(|m| m.contains("local")));
        c.ia.endpoint = "http://localhost:11434".into();
        assert!(validar(&c).is_empty());
        c.ia.modelo_primario = " ".into();
        assert!(!validar(&c).is_empty());
        c.ia.modelo_primario = "gemma3:12b".into();
        c.tentativas_max = 0;
        c.destino_a_classificar = "relativo".into();
        c.homologacao.modo = ModoHomologacao::Servidor;
        let e = validar(&c);
        assert_eq!(e.len(), 3, "{e:?}");
        c.aging_por_hora = f64::NAN;
        assert!(validar(&c).iter().any(|m| m.contains("aging")));
    }

    #[test]
    fn patch_nunca_leva_senha() {
        // O contrato só tem o NOME da variável de ambiente; nada com a senha existe na config.
        let json = serde_json::to_string(&ConfigBiblioteca::default()).unwrap();
        assert!(!json.contains("senha\":\""), "{json}");
        assert!(json.contains("senha_env"));
    }
}
