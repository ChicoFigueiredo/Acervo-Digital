//! Registro de páginas e ações. Dados puros (sem egui), testáveis sem display.

pub mod analise;
pub mod configuracoes;
pub mod discos;
pub mod duplicados;
pub mod explorador;
pub mod inicio;
pub mod monitoramento;
pub mod tabela;
pub mod varredura;

use crate::app::AppDag;
use bancada_casca::egui;

/// Uma ação planejada do Ribbon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefAcao {
    pub id: &'static str,
    pub rotulo: &'static str,
    pub dica: &'static str,
}

/// Um grupo do Ribbon dentro de uma aba.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefGrupo {
    pub id: &'static str,
    pub rotulo: &'static str,
    pub acoes: &'static [DefAcao],
}

/// Uma página: aba do Ribbon, item de navegação e corpo.
#[derive(Debug, Clone, Copy)]
pub struct DefPagina {
    pub id: &'static str,
    pub rotulo: &'static str,
    pub keytip: &'static str,
    pub icone: &'static str,
    pub grupos: &'static [DefGrupo],
    /// Resumo de uma linha da página (cabeçalho e estados vazios).
    pub resumo: &'static str,
    pub desenhar: fn(&mut egui::Ui, &mut AppDag),
}

impl DefPagina {
    pub fn acoes(&self) -> impl Iterator<Item = &'static DefAcao> {
        self.grupos.iter().flat_map(|g| g.acoes.iter())
    }
}

/// Todas as páginas, na ordem das abas do Ribbon.
pub fn todas() -> [&'static DefPagina; 8] {
    [
        &inicio::PAGINA,
        &discos::PAGINA,
        &varredura::PAGINA,
        &explorador::PAGINA,
        &analise::PAGINA,
        &duplicados::PAGINA,
        &monitoramento::PAGINA,
        &configuracoes::PAGINA,
    ]
}

pub fn por_id(id: &str) -> Option<&'static DefPagina> {
    todas().into_iter().find(|p| p.id == id)
}

/// Painel de estado vazio, com o resumo da página e uma dica do que fazer.
pub fn estado_vazio(ui: &mut egui::Ui, titulo: &str, resumo: &str, dica: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.heading(titulo);
        ui.add_space(8.0);
        ui.label(resumo);
        ui.add_space(8.0);
        ui.weak(dica);
    });
}

/// Cabeçalho comum: título e resumo.
pub fn cabecalho(ui: &mut egui::Ui, pagina: &DefPagina) {
    ui.heading(pagina.rotulo);
    ui.weak(pagina.resumo);
    ui.add_space(4.0);
}

/// Cor do texto de um aviso conforme o tom (o tom nunca é o único sinal: há prefixo).
fn cor_do_tom(ui: &egui::Ui, tom: bancada_casca::Tom) -> (egui::Color32, &'static str) {
    use bancada_casca::Tom;
    match tom {
        Tom::Perigo => (ui.visuals().error_fg_color, "Erro: "),
        Tom::Aviso => (ui.visuals().warn_fg_color, "Atenção: "),
        Tom::Sucesso => (egui::Color32::from_rgb(76, 175, 80), "OK: "),
        Tom::Info | Tom::Neutro => (ui.visuals().hyperlink_color, ""),
    }
}

/// Faixa de mensagens no topo da página; cada uma pode ser dispensada.
pub fn faixa_de_avisos(ui: &mut egui::Ui, avisos: &mut Vec<crate::app::Aviso>) {
    let mut remover = None;
    for (i, a) in avisos.iter().enumerate() {
        let (cor, prefixo) = cor_do_tom(ui, a.tom);
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("✕").on_hover_text("Dispensar").clicked() {
                remover = Some(i);
            }
            ui.colored_label(cor, format!("{prefixo}{}", a.texto));
        });
    }
    if let Some(i) = remover {
        avisos.remove(i);
    }
}

/// Texto "carregando…" ou o erro de uma carga, sob um cabeçalho de seção.
pub fn estado_da_carga<T>(ui: &mut egui::Ui, carga: &crate::leitor::Carga<T>) {
    if let Some(e) = &carga.erro {
        ui.colored_label(ui.visuals().warn_fg_color, e);
    } else if carga.carregando() && carga.dado.is_none() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Carregando…");
        });
    }
}
