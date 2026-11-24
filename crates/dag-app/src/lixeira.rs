//! Lixeira na GUI (ADR 0005): menus de contexto, os dois diálogos em sequência e a lista do que
//! está Na Lixeira. A GUI nunca toca no disco: ela só pede `MoverParaLixeira` e
//! `RestaurarDaLixeira` ao agente. Tudo aqui é dado e estado puros (sem egui, exceto o desenho
//! do menu), para ser testado sem display.

use std::path::Path;
use std::sync::Arc;

use bancada_casca::egui;
use dag_core::{
    AcaoLixeira, Alvo, ArquivoId, Comando, FRASE_ENTENDIMENTO, ItemLixeira, ResultadoLixeira,
};
use dag_store::consulta_gui::ChaveNo;
use dag_store::consulta_gui::{LinhaArquivo, NoAnalise, NoPasta};

use crate::leitor::{Carga, Leitor, erro};

/// Itens da lista Na Lixeira por carga.
const LIMITE_LISTA: u32 = 1000;

/// O que está sob o ponteiro quando o menu de contexto abre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlvoMenu {
    pub alvo: Alvo,
    /// Nome mostrado nos diálogos.
    pub rotulo: String,
    /// Caminho catalogado (`None` para um Volume inteiro).
    pub caminho: Option<String>,
    pub pasta: bool,
    pub na_lixeira: bool,
}

impl AlvoMenu {
    /// Nó da Análise; Volume não tem menu (não se move um Volume inteiro por aqui).
    #[must_use]
    pub fn de_no(n: &NoAnalise) -> Option<Self> {
        let (alvo, pasta) = match n.chave {
            ChaveNo::Volume(_) => return None,
            ChaveNo::Pasta(_) => (Alvo::Pasta(n.caminho.clone()?), true),
            ChaveNo::Arquivo(id) => (Alvo::Arquivo(ArquivoId(id)), false),
        };
        Some(Self {
            alvo,
            rotulo: n.nome.clone(),
            caminho: n.caminho.clone(),
            pasta,
            na_lixeira: n.na_lixeira,
        })
    }

    #[must_use]
    pub fn de_arquivo(a: &LinhaArquivo) -> Self {
        Self {
            alvo: Alvo::Arquivo(ArquivoId(a.id)),
            rotulo: a.nome.clone(),
            caminho: Some(a.caminho.clone()),
            pasta: false,
            na_lixeira: a.na_lixeira,
        }
    }

    #[must_use]
    pub fn de_pasta(p: &NoPasta) -> Self {
        Self {
            alvo: Alvo::Pasta(p.caminho.clone()),
            rotulo: p.nome.clone(),
            caminho: Some(p.caminho.clone()),
            pasta: true,
            na_lixeira: false,
        }
    }

    #[must_use]
    pub fn de_item(i: &ItemLixeira) -> Self {
        Self {
            alvo: Alvo::Arquivo(i.arquivo),
            rotulo: i.caminho.rsplit('/').next().unwrap_or_default().to_string(),
            caminho: Some(i.caminho.clone()),
            pasta: false,
            na_lixeira: true,
        }
    }
}

/// Entrada do menu de contexto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemMenu {
    MoverParaLixeira,
    RestaurarDaLixeira,
    AbrirNoGerenciador,
    CopiarCaminho,
}

impl ItemMenu {
    #[must_use]
    pub fn rotulo(self) -> &'static str {
        match self {
            Self::MoverParaLixeira => "Mover para a Lixeira…",
            Self::RestaurarDaLixeira => "Restaurar da Lixeira",
            Self::AbrirNoGerenciador => "Abrir a pasta no gerenciador de arquivos",
            Self::CopiarCaminho => "Copiar caminho",
        }
    }
}

/// Entradas do menu para `alvo`: Na Lixeira só restaura; fora dela só move.
#[must_use]
pub fn itens_do_menu(alvo: &AlvoMenu) -> Vec<ItemMenu> {
    let mut v = vec![if alvo.na_lixeira {
        ItemMenu::RestaurarDaLixeira
    } else {
        ItemMenu::MoverParaLixeira
    }];
    if alvo.caminho.is_some() {
        v.push(ItemMenu::AbrirNoGerenciador);
        v.push(ItemMenu::CopiarCaminho);
    }
    v
}

/// Desenha o menu de contexto e devolve o item escolhido.
pub fn menu_de_contexto(ui: &mut egui::Ui, alvo: &AlvoMenu) -> Option<ItemMenu> {
    let mut escolhido = None;
    for item in itens_do_menu(alvo) {
        if ui.button(item.rotulo()).clicked() {
            escolhido = Some(item);
            ui.close();
        }
    }
    escolhido
}

/// Uma escolha feita num menu de contexto, ainda por tratar.
pub type Escolha = (ItemMenu, AlvoMenu);

/// Abre uma pasta no gerenciador de arquivos do sistema (injetável nos testes).
pub trait Abridor: Send + Sync + std::fmt::Debug {
    /// # Errors
    /// Texto do erro ao lançar o programa.
    fn abrir(&self, pasta: &Path) -> Result<(), String>;
}

/// `xdg-open <pasta>`, sem shell. Só abre a pasta no gerenciador; nunca altera nada.
#[derive(Debug, Default)]
pub struct XdgOpen;

impl Abridor for XdgOpen {
    fn abrir(&self, pasta: &Path) -> Result<(), String> {
        std::process::Command::new("xdg-open")
            .arg(pasta)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("não foi possível lançar o xdg-open: {e}"))
    }
}

/// Pasta que contém `caminho` (ou o próprio caminho, se é uma pasta).
#[must_use]
pub fn pasta_para_abrir(alvo: &AlvoMenu) -> Option<std::path::PathBuf> {
    let c = alvo.caminho.as_deref()?;
    let p = Path::new(c);
    if alvo.pasta {
        Some(p.to_path_buf())
    } else {
        p.parent().map(Path::to_path_buf)
    }
}

/// Passo do diálogo de mover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Passo {
    /// Esperando a simulação do agente.
    Calculando,
    /// Primeiro diálogo: o que será movido, de onde, para onde.
    Previa(ResultadoLixeira),
    /// Segundo diálogo: confirmação explícita.
    Confirmar(ResultadoLixeira),
}

/// Os dois diálogos em sequência para mover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialogo {
    pub alvo: AlvoMenu,
    pub passo: Passo,
    /// "Marcar tudo e mover": cria a Marcação para Deleção em toda a pasta (passo explícito).
    pub marcar_tudo: bool,
    /// Confirmação digitada ou marcada do segundo diálogo.
    pub entendo: bool,
    /// Aceite explícito de mover a Única Cópia.
    pub aceita_unica: bool,
}

impl Dialogo {
    #[must_use]
    pub fn novo(alvo: AlvoMenu) -> Self {
        Self {
            alvo,
            passo: Passo::Calculando,
            marcar_tudo: false,
            entendo: false,
            aceita_unica: false,
        }
    }

    /// Comando da simulação (nada é tocado). Pede a Única Cópia só para *mostrá-la* no plano:
    /// mover de verdade exige o aceite explícito do segundo diálogo.
    #[must_use]
    pub fn comando_previa(&self) -> Comando {
        Comando::MoverParaLixeira {
            alvos: vec![self.alvo.alvo.clone()],
            confirmacao_unica_copia: true,
            marcar_tudo: self.marcar_tudo,
            simular: true,
        }
    }

    /// Recebe a simulação do agente e mostra o primeiro diálogo.
    pub fn receber_previa(&mut self, r: ResultadoLixeira) {
        self.passo = Passo::Previa(r);
        self.entendo = false;
        self.aceita_unica = false;
    }

    /// Vai do primeiro para o segundo diálogo (só se há algo a mover).
    pub fn continuar(&mut self) {
        if let Passo::Previa(r) = &self.passo
            && r.total_arquivos > 0
        {
            self.passo = Passo::Confirmar(r.clone());
        }
    }

    /// Volta ao primeiro diálogo.
    pub fn voltar(&mut self) {
        if let Passo::Confirmar(r) = &self.passo {
            self.passo = Passo::Previa(r.clone());
        }
    }

    /// Comando real de mover: só existe no segundo diálogo, com a confirmação dada (e o aceite
    /// da Única Cópia, se há).
    #[must_use]
    pub fn comando_mover(&self) -> Option<Comando> {
        let Passo::Confirmar(r) = &self.passo else {
            return None;
        };
        if !self.entendo || r.total_arquivos == 0 || (r.unicas_copias > 0 && !self.aceita_unica) {
            return None;
        }
        Some(Comando::MoverParaLixeira {
            alvos: vec![self.alvo.alvo.clone()],
            confirmacao_unica_copia: self.aceita_unica,
            marcar_tudo: self.marcar_tudo,
            simular: false,
        })
    }
}

/// `RestaurarDaLixeira` de um alvo.
#[must_use]
pub fn comando_restaurar(alvos: Vec<Alvo>) -> Comando {
    Comando::RestaurarDaLixeira { alvos }
}

/// Texto de uma linha para o resultado (aviso depois de mover ou restaurar).
#[must_use]
pub fn resumo_do_resultado(r: &ResultadoLixeira) -> String {
    let verbo = match r.acao {
        AcaoLixeira::Mover => "movido(s) para a Lixeira do Volume",
        AcaoLixeira::Restaurar => "restaurado(s)",
    };
    let mut t = format!(
        "{} arquivo(s) {verbo} ({}).",
        r.total_arquivos,
        crate::formato::bytes_humanos(r.total_bytes)
    );
    if !r.recusados.is_empty() {
        t.push_str(&format!(" {} recusado(s).", r.recusados.len()));
        if let Some(p) = r.recusados.first() {
            t.push_str(&format!(" Ex.: {} ({}).", p.alvo, p.motivo));
        }
    }
    if matches!(r.acao, AcaoLixeira::Mover) && !r.destinos.is_empty() {
        t.push_str(&format!(
            " Espaço só é liberado quando você esvazia {}.",
            r.destinos.join(", ")
        ));
    }
    t
}

/// O que a página Lixeira mostra.
#[derive(Debug, Clone, Default)]
pub struct ListaLixeira {
    pub itens: Vec<ItemLixeira>,
    pub total: u64,
    pub bytes: u64,
    pub descartados: u64,
}

/// Estado da GUI para a Lixeira.
#[derive(Debug)]
pub struct Estado {
    pub abridor: Arc<dyn Abridor>,
    pub dialogo: Option<Dialogo>,
    pub lista: Carga<ListaLixeira>,
    vista: u64,
    /// Itens marcados na página (para "Restaurar selecionados").
    pub marcados: std::collections::BTreeSet<i64>,
}

impl Default for Estado {
    fn default() -> Self {
        Self {
            abridor: Arc::new(XdgOpen),
            dialogo: None,
            lista: Carga::default(),
            vista: u64::MAX,
            marcados: std::collections::BTreeSet::new(),
        }
    }
}

impl Estado {
    /// Recarrega a lista quando o Catálogo mudou.
    pub fn atualizar(&mut self, leitor: &Leitor, geracao: u64) {
        if self.vista != geracao {
            self.vista = geracao;
            self.lista.pedir(leitor.consultar(|c| {
                let (itens, _, t) =
                    dag_store::lixeira::listar(c, None, LIMITE_LISTA).map_err(erro)?;
                Ok(ListaLixeira {
                    itens,
                    total: t.itens,
                    bytes: t.bytes,
                    descartados: t.descartados,
                })
            }));
        }
        if self.lista.atualizar()
            && let Some(l) = &self.lista.dado
        {
            let ids: std::collections::BTreeSet<i64> =
                l.itens.iter().map(|i| i.arquivo.0).collect();
            self.marcados.retain(|m| ids.contains(m));
        }
    }

    /// Alvos de "Restaurar selecionados".
    #[must_use]
    pub fn alvos_marcados(&self) -> Vec<Alvo> {
        self.marcados
            .iter()
            .map(|i| Alvo::Arquivo(ArquivoId(*i)))
            .collect()
    }
}

/// Texto do segundo diálogo.
pub const FRASE: &str = FRASE_ENTENDIMENTO;

#[cfg(test)]
mod testes {
    use super::*;

    fn alvo(na_lixeira: bool, caminho: bool) -> AlvoMenu {
        AlvoMenu {
            alvo: Alvo::Arquivo(ArquivoId(1)),
            rotulo: "a".into(),
            caminho: caminho.then(|| "/x/a".into()),
            pasta: false,
            na_lixeira,
        }
    }

    #[test]
    fn menu_depende_de_estar_na_lixeira() {
        assert_eq!(
            itens_do_menu(&alvo(false, true)),
            vec![
                ItemMenu::MoverParaLixeira,
                ItemMenu::AbrirNoGerenciador,
                ItemMenu::CopiarCaminho
            ]
        );
        assert_eq!(
            itens_do_menu(&alvo(true, true))[0],
            ItemMenu::RestaurarDaLixeira
        );
        assert_eq!(
            itens_do_menu(&alvo(false, false)),
            vec![ItemMenu::MoverParaLixeira]
        );
    }

    #[test]
    fn pasta_para_abrir_usa_o_pai_do_arquivo() {
        assert_eq!(
            pasta_para_abrir(&alvo(false, true)),
            Some(std::path::PathBuf::from("/x"))
        );
        let mut p = alvo(false, true);
        p.pasta = true;
        assert_eq!(pasta_para_abrir(&p), Some(std::path::PathBuf::from("/x/a")));
    }
}
