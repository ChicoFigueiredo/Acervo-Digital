//! Registro de páginas e ações. Dados puros (sem egui), testáveis sem display.

pub mod analise;
pub mod configuracoes;
pub mod discos;
pub mod duplicados;
pub mod explorador;
pub mod inicio;
pub mod monitoramento;
pub mod varredura;

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
    /// Texto do estado vazio ("chega em incremento posterior").
    pub resumo: &'static str,
    pub desenhar: fn(&mut egui::Ui),
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

/// Painel de estado vazio comum a todas as páginas deste incremento.
pub fn estado_vazio(ui: &mut egui::Ui, titulo: &str, resumo: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.heading(titulo);
        ui.add_space(8.0);
        ui.label(resumo);
        ui.add_space(8.0);
        ui.label("Este recurso chega em um incremento posterior.");
        ui.weak("As ações do Ribbon desta aba estão desabilitadas por enquanto.");
    });
}
