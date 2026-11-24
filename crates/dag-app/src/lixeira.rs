//! Lixeira na GUI (ADR 0005): menus de contexto, os dois diálogos em sequência e a lista do que
//! está Na Lixeira. A GUI nunca toca no disco: ela só pede `MoverParaLixeira` e
//! `RestaurarDaLixeira` ao agente. Tudo aqui é dado e estado puros (sem egui, exceto o desenho
//! do menu), para ser testado sem display.

use std::path::Path;
use std::sync::Arc;

use bancada_casca::egui;
use dag_core::{
    AcaoLixeira, Alvo, ArquivoId, Comando, CopiaGrupo, FRASE_ENTENDIMENTO, ItemLixeira,
    ResultadoLixeira, TipoMarcacao,
};
use dag_store::consulta_gui::{ChaveNo, MarcaVista};
use dag_store::consulta_gui::{LinhaArquivo, NoAnalise, NoPasta};

use crate::leitor::{Carga, Leitor, erro};

/// Itens da lista Na Lixeira por carga.
const LIMITE_LISTA: u32 = 1000;

/// Marcação para Deleção que vale para o alvo, para o cabeçalho do menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarcaMenu {
    /// `gui`, `cli` ou `mcp`, quando conhecido.
    pub autor: Option<String>,
    /// Criação (segundos Unix), quando conhecida.
    pub em: Option<i64>,
}

impl MarcaMenu {
    /// Só uma Marcação para Deleção conta aqui; Marcação de Backup não.
    #[must_use]
    pub fn de_vista(m: Option<&MarcaVista>) -> Option<Self> {
        let m = m.filter(|m| m.e_delecao())?;
        Some(Self {
            autor: Some(m.autor.clone()),
            em: Some(m.criada_em),
        })
    }
}

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
    /// Marcação para Deleção atual (`None` = sem marcação).
    pub marca: Option<MarcaMenu>,
    /// Pasta: arquivos catalogados dentro (para a confirmação de marcar em lote).
    pub arquivos: Option<u64>,
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
            marca: MarcaMenu::de_vista(n.marca.as_ref()),
            arquivos: if pasta { n.arquivos } else { None },
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
            marca: MarcaMenu::de_vista(a.marca.as_ref()),
            arquivos: None,
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
            marca: MarcaMenu::de_vista(p.marca.as_ref()),
            arquivos: Some(p.arquivos),
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
            marca: None,
            arquivos: None,
        }
    }

    /// Cópia de um Grupo de Duplicados (página Duplicados). A página só conhece o tipo da
    /// marcação, não autor nem data.
    #[must_use]
    pub fn de_copia(c: &CopiaGrupo) -> Self {
        let nome = c.caminho.rsplit('/').next().unwrap_or_default().to_string();
        Self {
            alvo: Alvo::Arquivo(c.arquivo),
            rotulo: nome,
            caminho: Some(c.caminho.clone()),
            pasta: false,
            na_lixeira: false,
            marca: c
                .marcacao
                .as_ref()
                .filter(|m| m.tipo == TipoMarcacao::Delecao)
                .map(|_| MarcaMenu {
                    autor: None,
                    em: None,
                }),
            arquivos: None,
        }
    }

    /// Está com Marcação para Deleção?
    #[must_use]
    pub fn marcado(&self) -> bool {
        self.marca.is_some()
    }

    /// Linha de estado no topo do menu.
    #[must_use]
    pub fn cabecalho(&self) -> String {
        if self.na_lixeira {
            return "Na Lixeira".into();
        }
        match &self.marca {
            None => "Sem marcação para deleção".into(),
            Some(m) => {
                let mut t = "Marcado para deleção".to_string();
                if let Some(a) = &m.autor {
                    t.push_str(&format!(" por {a}"));
                }
                if let Some(e) = m.em {
                    let d = crate::formato::data_s(Some(e));
                    t.push_str(&format!(" em {}", d.split(' ').next().unwrap_or(&d)));
                }
                t
            }
        }
    }
}

/// Entrada do menu de contexto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemMenu {
    /// Abre o próprio arquivo (`xdg-open`).
    AbrirArquivo,
    /// Abre a própria pasta.
    AbrirPasta,
    /// Abre a pasta que contém o arquivo.
    AbrirNoGerenciador,
    MarcarDelecao,
    DesmarcarDelecao,
    MoverParaLixeira,
    RestaurarDaLixeira,
    CopiarCaminho,
}

impl ItemMenu {
    #[must_use]
    pub fn rotulo(self) -> &'static str {
        match self {
            Self::AbrirArquivo => "Abrir arquivo",
            Self::AbrirPasta => "Abrir pasta",
            Self::AbrirNoGerenciador => "Abrir a pasta que contém o arquivo",
            Self::MarcarDelecao => "Marcar para deleção",
            Self::DesmarcarDelecao => "Desmarcar para deleção",
            Self::MoverParaLixeira => "Mover para a Lixeira…",
            Self::RestaurarDaLixeira => "Restaurar da Lixeira",
            Self::CopiarCaminho => "Copiar caminho",
        }
    }
}

/// Entradas do menu para `alvo`. Na Lixeira só restaura; fora dela marca/desmarca (nível 1) e
/// move (nível 2, só com marcação; sem marcação a pasta oferece "marcar tudo e mover" no diálogo).
#[must_use]
pub fn itens_do_menu(alvo: &AlvoMenu) -> Vec<ItemMenu> {
    let mut v = Vec::new();
    if alvo.caminho.is_some() {
        if alvo.pasta {
            v.push(ItemMenu::AbrirPasta);
        } else {
            v.push(ItemMenu::AbrirArquivo);
            v.push(ItemMenu::AbrirNoGerenciador);
        }
    }
    if alvo.na_lixeira {
        v.push(ItemMenu::RestaurarDaLixeira);
    } else {
        v.push(if alvo.marcado() {
            ItemMenu::DesmarcarDelecao
        } else {
            ItemMenu::MarcarDelecao
        });
        v.push(ItemMenu::MoverParaLixeira);
    }
    if alvo.caminho.is_some() {
        v.push(ItemMenu::CopiarCaminho);
    }
    v
}

/// Desenha o menu de contexto e devolve o item escolhido.
pub fn menu_de_contexto(ui: &mut egui::Ui, alvo: &AlvoMenu) -> Option<ItemMenu> {
    let mut escolhido = None;
    ui.label(egui::RichText::new(alvo.cabecalho()).weak().italics());
    ui.separator();
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

/// Abre um arquivo ou pasta com o programa padrão do sistema (injetável nos testes).
pub trait Abridor: Send + Sync + std::fmt::Debug {
    /// # Errors
    /// Texto do erro ao lançar o programa.
    fn abrir(&self, caminho: &Path) -> Result<(), String>;
}

/// `xdg-open <caminho>`, sem shell, destacado da GUI (grupo de processos próprio; um thread colhe
/// o filho para não deixar zumbi). Só abre; nunca altera nada.
#[derive(Debug, Default)]
pub struct XdgOpen;

impl Abridor for XdgOpen {
    fn abrir(&self, caminho: &Path) -> Result<(), String> {
        use std::os::unix::process::CommandExt;
        let mut filho = std::process::Command::new("xdg-open")
            .arg(caminho)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("não foi possível lançar o xdg-open: {e}"))?;
        std::thread::spawn(move || {
            let _ = filho.wait();
        });
        Ok(())
    }
}

/// O que abrir para o item: o próprio caminho (arquivo ou pasta) ou a pasta que contém o arquivo.
#[must_use]
pub fn caminho_para_abrir(alvo: &AlvoMenu, item: ItemMenu) -> Option<std::path::PathBuf> {
    let c = alvo.caminho.as_deref()?;
    let p = Path::new(c);
    match item {
        ItemMenu::AbrirArquivo | ItemMenu::AbrirPasta => Some(p.to_path_buf()),
        ItemMenu::AbrirNoGerenciador => p.parent().map(Path::to_path_buf),
        _ => None,
    }
}

/// Comando de marcar ou desmarcar (só o Catálogo muda).
#[must_use]
pub fn comando_marca(alvo: &AlvoMenu, marcar: bool) -> Comando {
    if marcar {
        Comando::MarcarDelecao {
            alvo: alvo.alvo.clone(),
        }
    } else {
        Comando::Desmarcar {
            alvo: alvo.alvo.clone(),
        }
    }
}

/// Confirmação de marcar ou desmarcar uma pasta inteira (regra que vale para todos os arquivos
/// catalogados dentro dela).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmaMarca {
    pub alvo: AlvoMenu,
    pub marcar: bool,
}

impl ConfirmaMarca {
    /// Texto com a contagem de arquivos afetados.
    #[must_use]
    pub fn texto(&self) -> String {
        let n = self
            .alvo
            .arquivos
            .map_or_else(|| "todos os".to_string(), crate::formato::inteiro_pt_br);
        let verbo = if self.marcar { "Marcar" } else { "Desmarcar" };
        format!(
            "{verbo} para deleção {n} arquivo(s) catalogado(s) em \"{}\"? Só o Catálogo muda: nada é movido nem apagado.",
            self.alvo.rotulo
        )
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
    /// Confirmação de marcar/desmarcar uma pasta inteira.
    pub confirma_marca: Option<ConfirmaMarca>,
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
            confirma_marca: None,
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
            marca: None,
            arquivos: None,
        }
    }

    fn marcado() -> MarcaMenu {
        MarcaMenu {
            autor: Some("gui".into()),
            em: Some(1_700_000_000),
        }
    }

    #[test]
    fn menu_de_arquivo() {
        assert_eq!(
            itens_do_menu(&alvo(false, true)),
            vec![
                ItemMenu::AbrirArquivo,
                ItemMenu::AbrirNoGerenciador,
                ItemMenu::MarcarDelecao,
                ItemMenu::MoverParaLixeira,
                ItemMenu::CopiarCaminho
            ]
        );
        assert_eq!(
            itens_do_menu(&alvo(false, false)),
            vec![ItemMenu::MarcarDelecao, ItemMenu::MoverParaLixeira]
        );
    }

    #[test]
    fn menu_de_pasta_abre_a_propria_pasta() {
        let mut p = alvo(false, true);
        p.pasta = true;
        let v = itens_do_menu(&p);
        assert_eq!(v[0], ItemMenu::AbrirPasta);
        assert!(!v.contains(&ItemMenu::AbrirArquivo));
        assert!(!v.contains(&ItemMenu::AbrirNoGerenciador));
        assert_eq!(
            caminho_para_abrir(&p, ItemMenu::AbrirPasta),
            Some(std::path::PathBuf::from("/x/a"))
        );
    }

    #[test]
    fn menu_marcado_oferece_desmarcar() {
        let mut a = alvo(false, true);
        a.marca = Some(marcado());
        let v = itens_do_menu(&a);
        assert!(v.contains(&ItemMenu::DesmarcarDelecao));
        assert!(!v.contains(&ItemMenu::MarcarDelecao));
        assert!(
            a.cabecalho()
                .starts_with("Marcado para deleção por gui em ")
        );
        assert_eq!(alvo(false, true).cabecalho(), "Sem marcação para deleção");
    }

    #[test]
    fn menu_na_lixeira_so_restaura() {
        let v = itens_do_menu(&alvo(true, true));
        assert!(v.contains(&ItemMenu::RestaurarDaLixeira));
        assert!(!v.contains(&ItemMenu::MarcarDelecao));
        assert!(!v.contains(&ItemMenu::MoverParaLixeira));
        assert_eq!(alvo(true, true).cabecalho(), "Na Lixeira");
    }

    #[test]
    fn abrir_arquivo_usa_o_proprio_caminho_e_a_pasta_o_pai() {
        let a = alvo(false, true);
        assert_eq!(
            caminho_para_abrir(&a, ItemMenu::AbrirArquivo),
            Some(std::path::PathBuf::from("/x/a"))
        );
        assert_eq!(
            caminho_para_abrir(&a, ItemMenu::AbrirNoGerenciador),
            Some(std::path::PathBuf::from("/x"))
        );
    }

    #[test]
    fn comando_de_marca_e_so_catalogo() {
        let a = alvo(false, true);
        assert!(matches!(
            comando_marca(&a, true),
            Comando::MarcarDelecao { .. }
        ));
        assert!(matches!(
            comando_marca(&a, false),
            Comando::Desmarcar { .. }
        ));
    }

    #[test]
    fn confirmacao_de_pasta_diz_quantos_arquivos() {
        let mut p = alvo(false, true);
        p.pasta = true;
        p.arquivos = Some(1234);
        let c = ConfirmaMarca {
            alvo: p,
            marcar: true,
        };
        assert!(c.texto().contains("1.234"));
    }
}
