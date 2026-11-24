//! O `AppCasca` do DAG: estado da sessão (worker, leitor do Catálogo, estado de cada página),
//! ações do Ribbon, barra de status e despacho dos eventos do worker.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bancada_casca::egui;
use bancada_casca::{
    Aba, Acao, AppCasca, Casca, ConfigCasca, Elo, FamiliaDeIcones, Grupo, GrupoNavegacao,
    ItemNavegacao, Local, Rota, SegmentoStatus, Tom,
};
use dag_agent::{ErroAgente, caminho_catalogo_do_ambiente, caminho_do_lock};
use dag_core::{Comando, ModoVarredura, Resposta, VolumeDescoberto};
use dag_scanner::EstadoControle;
use dag_store::consulta_gui::{self, TotaisCatalogo};

use crate::confirmacao::{AcaoMarcacao, AlvoAcao, planejar};
use crate::formato::{bytes_humanos, inteiro_pt_br};
use crate::leitor::{Acordador, Carga, Leitor, erro};
use crate::paginas::{self, DefPagina, analise, discos, duplicados, explorador, varredura};
use crate::progresso::{AndamentoHash, FaseScan, ProgressoScan, restante_estimado, taxa_do_hash};
use crate::selecao::Classificacao;
use crate::worker::{ConfigWorker, Evento, ModoWorker, Worker};

const TOKENS: &str = include_str!("../dados/tokens.json");
const TEMAS: &str = include_str!("../dados/temas.json");

/// Motivo mostrado nas dicas das ações que ainda não existem.
pub const MOTIVO_FUTURO: &str = "Chega em um incremento posterior";
const MOTIVO_SEM_ESCRITA: &str = "Este processo não é o escritor do Catálogo";
const MOTIVO_SCAN_ATIVO: &str = "Há uma varredura em andamento";

/// Quantos avisos ficam na faixa de mensagens.
const MAX_AVISOS: usize = 4;

/// Parâmetros do app.
#[derive(Debug, Clone)]
pub struct ConfigApp {
    pub caminho: PathBuf,
    /// Volumes fixos em vez da descoberta do sistema (testes).
    pub volumes_fixos: Option<Vec<VolumeDescoberto>>,
}

impl ConfigApp {
    /// Catálogo padrão (`DAG_CATALOGO`, depois `$XDG_DATA_HOME/dag/catalogo.sqlite`).
    ///
    /// # Errors
    /// Se não há como achar o diretório de dados.
    pub fn do_ambiente() -> Result<Self, ErroAgente> {
        Ok(Self::com_caminho(caminho_catalogo_do_ambiente(None)?))
    }

    #[must_use]
    pub fn com_caminho(caminho: PathBuf) -> Self {
        Self {
            caminho,
            volumes_fixos: None,
        }
    }
}

/// Prefixo das mensagens da recuperação ao abrir (a faixa mantém só a mais recente).
const PREFIXO_ABERTURA: &str = "Abertura:";

/// Mensagem na faixa no topo da página.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aviso {
    pub tom: Tom,
    pub texto: String,
}

/// Para que serve um pedido em voo, para tratar a resposta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proposito {
    Volumes,
    Scan { sem_hash: bool },
    Marcacao(AcaoMarcacao),
    Exportar,
}

/// Varredura em andamento, vista pela GUI.
#[derive(Debug)]
pub struct ScanAtivo {
    pub pedido: u64,
    pub sem_hash: bool,
    pub inicio: Instant,
    pub progresso: Option<ProgressoScan>,
    /// Último progresso do hash (arquivos e bytes), vindo do agente.
    pub bytes_hash: Option<dag_core::ProgressoHash>,
    pub amostras_hash: Vec<(AndamentoHash, Duration)>,
}

impl ScanAtivo {
    fn novo(pedido: u64, sem_hash: bool) -> Self {
        Self {
            pedido,
            sem_hash,
            inicio: Instant::now(),
            progresso: None,
            bytes_hash: None,
            amostras_hash: Vec::new(),
        }
    }

    /// Fase atual do scanner, se já reportou alguma.
    #[must_use]
    pub fn fase(&self) -> Option<FaseScan> {
        self.progresso.as_ref().map(|p| p.fase)
    }

    /// Última contagem da fila de hash.
    #[must_use]
    pub fn andamento_hash(&self) -> Option<AndamentoHash> {
        self.amostras_hash.last().map(|a| a.0)
    }

    /// Arquivos por segundo do hash entre as duas últimas amostras.
    #[must_use]
    pub fn taxa_hash(&self) -> Option<f64> {
        let n = self.amostras_hash.len();
        (n >= 2)
            .then(|| taxa_do_hash(self.amostras_hash[n - 2], self.amostras_hash[n - 1]))
            .flatten()
    }

    #[must_use]
    pub fn restante_hash(&self) -> Option<Duration> {
        restante_estimado(&self.andamento_hash()?, self.taxa_hash())
    }
}

/// Resumo da última varredura terminada nesta sessão.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumoScan {
    pub execucao: i64,
    pub desfecho: String,
    pub pastas: u64,
    pub arquivos: u64,
    pub bytes_logicos: u64,
    pub erros: u64,
    pub ignorados: u64,
    pub duracao_ms: u64,
    pub grupos_duplicados: Option<u64>,
    pub hasheados: Option<u64>,
}

/// O estado do app: tudo o que as páginas e a casca compartilham.
#[derive(Debug)]
pub struct AppDag {
    pub caminho: PathBuf,
    pub worker: Worker,
    pub leitor: Leitor,
    pub acordador: Acordador,
    /// Sobe a cada mudança no Catálogo; as páginas recarregam quando ela muda.
    pub geracao: u64,
    pub avisos: Vec<Aviso>,
    pub pedidos: HashMap<u64, Proposito>,
    pub scan: Option<ScanAtivo>,
    pub ultimo_scan: Option<ResumoScan>,
    pub totais: Carga<TotaisCatalogo>,
    totais_geracao: u64,
    pub discos: discos::Estado,
    /// Classe dos volumes (física, imagem...), vinda de `ListarVolumes`.
    pub classificacao: Classificacao,
    classificacao_pedida: bool,
    pub varredura: varredura::Estado,
    pub explorador: explorador::Estado,
    pub analise: analise::Estado,
    pub duplicados: duplicados::Estado,
}

impl AppDag {
    /// Sobe o worker e o leitor sobre o Catálogo de `cfg`.
    #[must_use]
    pub fn novo(cfg: ConfigApp) -> Self {
        let acordador = Acordador::default();
        let worker = Worker::iniciar(
            ConfigWorker {
                caminho: cfg.caminho.clone(),
                volumes_fixos: cfg.volumes_fixos,
                socket: None,
                abertura: dag_agent::PoliticaAbertura::default(),
            },
            acordador.clone(),
        );
        let leitor = Leitor::novo(cfg.caminho.clone(), acordador.clone());
        Self {
            caminho: cfg.caminho,
            worker,
            leitor,
            acordador,
            geracao: 1,
            avisos: Vec::new(),
            pedidos: HashMap::new(),
            scan: None,
            ultimo_scan: None,
            totais: Carga::default(),
            totais_geracao: 0,
            discos: discos::Estado::default(),
            classificacao: Classificacao::default(),
            classificacao_pedida: false,
            varredura: varredura::Estado::default(),
            explorador: explorador::Estado::default(),
            analise: analise::Estado::default(),
            duplicados: duplicados::Estado::default(),
        }
    }

    // ------------------------------------------------------------------ consultas de estado

    #[must_use]
    pub fn pode_escrever(&self) -> bool {
        self.worker.modo().pode_escrever()
    }

    #[must_use]
    pub fn scan_ativo(&self) -> bool {
        self.scan.is_some()
    }

    /// Por que uma escrita não pode ser feita agora; `None` se pode.
    #[must_use]
    pub fn motivo_sem_escrita(&self) -> Option<&'static str> {
        if !self.pode_escrever() {
            Some(MOTIVO_SEM_ESCRITA)
        } else if self.scan_ativo() {
            Some(MOTIVO_SCAN_ATIVO)
        } else {
            None
        }
    }

    #[must_use]
    pub fn estado_do_scan(&self) -> Option<EstadoControle> {
        self.scan.as_ref().and(self.worker.estado_controle())
    }

    // ------------------------------------------------------------------ ações sobre o worker

    pub fn aviso(&mut self, tom: Tom, texto: impl Into<String>) {
        self.avisos.push(Aviso {
            tom,
            texto: texto.into(),
        });
        let excesso = self.avisos.len().saturating_sub(MAX_AVISOS);
        self.avisos.drain(..excesso);
    }

    /// Envia um comando ao worker e lembra para que serve.
    pub fn enviar(&mut self, comando: Comando, proposito: Proposito) -> u64 {
        let id = self.worker.enviar(comando);
        self.pedidos.insert(id, proposito);
        id
    }

    /// Dispara o Full Scan sobre as raízes escolhidas.
    pub fn iniciar_full_scan(&mut self, sem_hash: bool) {
        self.iniciar_scan(ModoVarredura::FullScan, sem_hash);
    }

    /// Dispara o Fast Scan sobre as raízes escolhidas.
    pub fn iniciar_fast_scan(&mut self) {
        self.iniciar_scan(ModoVarredura::FastScan, true);
    }

    /// Define as raízes como `raizes` e dispara a varredura (a partir da página Discos).
    pub fn varrer_raizes(&mut self, raizes: Vec<String>, modo: ModoVarredura) {
        for r in raizes {
            self.varredura.adicionar_raiz(r);
        }
        self.iniciar_scan(modo, modo == ModoVarredura::FastScan);
    }

    fn iniciar_scan(&mut self, modo: ModoVarredura, sem_hash: bool) {
        if let Some(m) = self.motivo_sem_escrita() {
            self.aviso(Tom::Aviso, m);
            return;
        }
        let raizes = self.varredura.raizes.clone();
        if raizes.is_empty() {
            self.aviso(Tom::Aviso, "Escolha ao menos uma raiz para varrer.");
            return;
        }
        let id = self.enviar(
            Comando::IniciarScan {
                modo,
                raizes,
                sem_hash,
                elevar: false,
                releitura_total: false,
            },
            Proposito::Scan { sem_hash },
        );
        self.scan = Some(ScanAtivo::novo(id, sem_hash));
    }

    /// Retoma uma execução interrompida, pausada ou cancelada.
    pub fn retomar_execucao(&mut self, execucao: i64) {
        if let Some(m) = self.motivo_sem_escrita() {
            self.aviso(Tom::Aviso, m);
            return;
        }
        let id = self.enviar(
            Comando::Retomar {
                execucao: Some(dag_core::ExecucaoId(execucao)),
            },
            Proposito::Scan { sem_hash: false },
        );
        self.scan = Some(ScanAtivo::novo(id, false));
    }

    /// Linhas classificadas da página Discos (e quantas estão ocultas).
    #[must_use]
    pub fn linhas_de_discos(&self) -> (Vec<crate::selecao::LinhaDisco>, usize) {
        match self.discos.volumes.dado.as_deref() {
            Some(v) => {
                crate::selecao::montar_linhas(v, &self.classificacao, self.discos.mostrar_todos)
            }
            None => (Vec::new(), 0),
        }
    }

    /// Pede a classificação dos volumes uma vez por sessão (se puder escrever): sem ela a página
    /// Discos usa a classe presumida pelo filesystem.
    pub fn garantir_classificacao(&mut self) {
        if !self.classificacao_pedida && self.motivo_sem_escrita().is_none() {
            self.classificacao_pedida = true;
            self.enviar(Comando::ListarVolumes, Proposito::Volumes);
        }
    }

    pub fn atualizar_volumes(&mut self) {
        if let Some(m) = self.motivo_sem_escrita() {
            self.aviso(Tom::Aviso, m);
            return;
        }
        self.enviar(Comando::ListarVolumes, Proposito::Volumes);
    }

    /// Recarrega tudo o que vem do Catálogo.
    pub fn invalidar(&mut self) {
        self.geracao += 1;
    }

    /// Pausa ou retoma a varredura em andamento pela alça compartilhada.
    pub fn alternar_pausa(&mut self) {
        match self.worker.estado_controle() {
            Some(EstadoControle::Executando) => self.worker.pausar(),
            Some(EstadoControle::Pausado) => self.worker.continuar(),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ eventos

    /// Processa o que chegou do worker e inicia leituras pendentes. Chamado todo quadro.
    pub fn processar_eventos(&mut self) {
        for e in self.worker.drenar() {
            match e {
                Evento::Modo(m) => {
                    match &m {
                        ModoWorker::SomenteLeitura { motivo } => self.aviso(
                            Tom::Aviso,
                            format!("Somente leitura: {motivo}. Varreduras e marcações ficam desabilitadas."),
                        ),
                        ModoWorker::Falhou { motivo } => {
                            self.aviso(Tom::Perigo, format!("O worker não abriu o Catálogo: {motivo}"));
                        }
                        _ => {}
                    }
                    self.invalidar();
                }
                Evento::Iniciado { .. } => {}
                Evento::Abertura { texto, fim } => {
                    // Em andamento: faixa de aviso; o relatório final a substitui.
                    self.avisos
                        .retain(|a| !a.texto.starts_with(PREFIXO_ABERTURA));
                    if !texto.is_empty() {
                        let tom = if fim { Tom::Info } else { Tom::Aviso };
                        self.aviso(tom, format!("{PREFIXO_ABERTURA} {texto}"));
                    }
                    self.invalidar();
                }
                Evento::Progresso(p) => {
                    if let Some(s) = self.scan.as_mut() {
                        s.progresso = Some(p);
                    }
                }
                Evento::Hash(h) => {
                    if let Some(s) = self.scan.as_mut() {
                        s.amostras_hash.push((
                            AndamentoHash {
                                total: h.arquivos_total,
                                concluidos: h.arquivos_feitos,
                            },
                            s.inicio.elapsed(),
                        ));
                        if s.amostras_hash.len() > 8 {
                            s.amostras_hash.remove(0);
                        }
                        s.bytes_hash = Some(h);
                    }
                }
                Evento::Resposta { id, resposta } => self.tratar_resposta(id, *resposta),
            }
        }
        if self.totais_geracao != self.geracao {
            self.totais_geracao = self.geracao;
            self.totais.pedir(
                self.leitor
                    .consultar(|c| consulta_gui::totais_do_catalogo(c).map_err(erro)),
            );
        }
        self.totais.atualizar();
    }

    fn tratar_resposta(&mut self, id: u64, resposta: Resposta) {
        let proposito = self.pedidos.remove(&id);
        let era_scan = self.scan.as_ref().is_some_and(|s| s.pedido == id);
        if era_scan {
            self.scan = None;
        }
        match resposta {
            Resposta::Erro { codigo, mensagem } => {
                self.aviso(Tom::Perigo, format!("{mensagem} [{codigo}]"));
            }
            Resposta::Volumes { volumes, ocultos } => {
                let on = volumes.iter().filter(|v| v.conectado).count();
                self.aviso(
                    Tom::Sucesso,
                    format!(
                        "{} volume(s) real(is), {on} conectado(s); {} de imagem/virtuais ocultos.",
                        volumes.len(),
                        ocultos.len()
                    ),
                );
                self.classificacao = Classificacao::de_resposta(&volumes, &ocultos);
                self.invalidar();
            }
            Resposta::ScanConcluido {
                execucao,
                desfecho,
                pastas,
                arquivos,
                bytes_logicos,
                ignorados,
                erros,
                duracao_ms,
                hash,
                duplicados,
                ..
            } => {
                let tom = match desfecho.as_str() {
                    "concluida" => Tom::Sucesso,
                    _ => Tom::Aviso,
                };
                let texto = match desfecho.as_str() {
                    "cancelada" => format!(
                        "Varredura #{} cancelada; pode ser retomada na tabela de execuções.",
                        execucao.0
                    ),
                    "concluida_com_erros" => {
                        format!("Varredura #{} concluída com {erros} erro(s).", execucao.0)
                    }
                    _ => format!("Varredura #{} concluída.", execucao.0),
                };
                self.aviso(tom, texto);
                self.ultimo_scan = Some(ResumoScan {
                    execucao: execucao.0,
                    desfecho,
                    pastas,
                    arquivos,
                    bytes_logicos,
                    erros,
                    ignorados,
                    duracao_ms,
                    grupos_duplicados: duplicados.as_ref().map(|d| d.grupos),
                    hasheados: hash.as_ref().map(|h| h.hasheados),
                });
                self.invalidar();
            }
            Resposta::Marcacao { resultado } => {
                let mut t = format!(
                    "{} marcação(ões) registrada(s), {} já existiam.",
                    resultado.marcacoes.len(),
                    resultado.ja_marcadas
                );
                if resultado.unicas_copias_total > 0 {
                    t.push_str(&format!(
                        " ⚠ Única Cópia: {} arquivo(s) sem outra cópia no Catálogo (ex.: {}).",
                        resultado.unicas_copias_total,
                        resultado
                            .unicas_copias
                            .first()
                            .map_or("—", |a| a.caminho.as_str())
                    ));
                }
                for a in &resultado.avisos {
                    t.push(' ');
                    t.push_str(a);
                }
                let tom = if resultado.unicas_copias_total > 0 {
                    Tom::Aviso
                } else {
                    Tom::Sucesso
                };
                self.aviso(tom, t);
                self.invalidar();
            }
            Resposta::Desmarcado { removidas } => {
                self.aviso(
                    Tom::Sucesso,
                    format!("{} marcação(ões) removida(s).", removidas.len()),
                );
                self.invalidar();
            }
            Resposta::Grupo { .. } => {
                if matches!(proposito, Some(Proposito::Marcacao(_))) {
                    self.aviso(Tom::Sucesso, "Cópia Principal definida.");
                }
                self.invalidar();
            }
            Resposta::RelatorioExportado {
                caminho, linhas, ..
            } => {
                self.aviso(
                    Tom::Sucesso,
                    format!(
                        "Relatório gravado em {caminho} ({} linha(s)).",
                        inteiro_pt_br(linhas)
                    ),
                );
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ ações do Ribbon

    /// Motivo de a ação estar desabilitada agora; `None` = habilitada.
    #[must_use]
    pub fn motivo_desabilitada(&self, id: &str) -> Option<String> {
        let futuro = || Some(MOTIVO_FUTURO.to_string());
        let m = |s: Option<&'static str>| s.map(str::to_string);
        match id {
            "inicio.abrir_catalogo"
            | "varredura.exclusoes"
            | "explorador.filtrar"
            | "explorador.exportar"
            | "analise.filtros" => futuro(),
            id if id.starts_with("monitoramento.") => {
                Some("O monitoramento chega no incremento 5".into())
            }
            id if id.starts_with("configuracoes.") => futuro(),
            "varredura.fast_scan" => Some("Fast Scan chega no incremento 5".into()),
            "discos.detectar" => m(self.motivo_sem_escrita()),
            "discos.usar_na_varredura" => self
                .discos
                .marcados
                .is_empty()
                .then(|| "Marque ao menos um volume (caixa \"Varrer\")".to_string()),
            "discos.incluir" => self
                .discos
                .montagem_selecionada()
                .is_none()
                .then(|| "Selecione um volume com ponto de montagem".to_string()),
            "varredura.full_scan" | "varredura.full_scan_sem_hash" => m(self.motivo_sem_escrita())
                .or_else(|| {
                    self.varredura
                        .raizes
                        .is_empty()
                        .then_some("Escolha ao menos uma raiz")
                        .map(str::to_string)
                }),
            "varredura.pausar" => (self.estado_do_scan() != Some(EstadoControle::Executando))
                .then(|| "Nenhuma varredura em execução".to_string()),
            "varredura.retomar" => (self.estado_do_scan() != Some(EstadoControle::Pausado))
                .then(|| "Nenhuma varredura pausada".to_string()),
            "varredura.cancelar" => {
                (!self.scan_ativo()).then(|| "Nenhuma varredura em andamento".to_string())
            }
            "explorador.expandir" | "explorador.propriedades" => self
                .explorador
                .selecionada
                .is_none()
                .then(|| "Selecione uma pasta".to_string()),
            "duplicados.copia_principal"
            | "duplicados.marcar_backup"
            | "duplicados.marcar_delecao"
            | "duplicados.desmarcar" => m(self.motivo_sem_escrita()).or_else(|| {
                self.duplicados
                    .selecionado
                    .is_none()
                    .then(|| "Selecione um grupo".to_string())
            }),
            _ => None,
        }
    }

    /// Executa uma ação do Ribbon.
    pub fn executar_acao(&mut self, ctx: &egui::Context, id: &str) {
        if let Some(motivo) = self.motivo_desabilitada(id) {
            self.aviso(Tom::Aviso, motivo);
            return;
        }
        let abrir = |rota: &str| bancada_casca::abrir(ctx, rota);
        match id {
            "inicio.nova_varredura" | "varredura.selecionar_raizes" => abrir("varredura"),
            "inicio.atualizar" | "duplicados.grupos" => self.invalidar(),
            "discos.detectar" => self.atualizar_volumes(),
            "discos.propriedades" => self.discos.detalhes = !self.discos.detalhes,
            "discos.usar_na_varredura" => {
                let (linhas, _) = self.linhas_de_discos();
                let raizes =
                    crate::selecao::raizes_dos_selecionados(&linhas, &self.discos.marcados);
                for r in raizes {
                    self.varredura.adicionar_raiz(r);
                }
                abrir("varredura");
            }
            "discos.incluir" => {
                if let Some(m) = self.discos.montagem_selecionada() {
                    self.varredura.adicionar_raiz(m);
                    abrir("varredura");
                }
            }
            "varredura.full_scan" => self.iniciar_full_scan(false),
            "varredura.full_scan_sem_hash" => self.iniciar_full_scan(true),
            "varredura.pausar" | "varredura.retomar" => self.alternar_pausa(),
            "varredura.cancelar" => self.worker.cancelar(),
            "explorador.expandir" => self.explorador.expandir_selecionada(&self.leitor),
            "explorador.recolher" => self.explorador.recolher_tudo(),
            "explorador.propriedades" => {
                self.explorador.propriedades = !self.explorador.propriedades;
            }
            "analise.tree_view" => self.analise.vista = analise::Vista::Tree,
            "analise.treemap" => self.analise.vista = analise::Vista::Treemap,
            "analise.por_volume" => self.analise.usar_modo(analise::ModoAnalise::Volume),
            "analise.por_computador" => self.analise.usar_modo(analise::ModoAnalise::Computador),
            "duplicados.copia_principal" => self.pedir_confirmacao(AcaoMarcacao::DefinirPrincipal),
            "duplicados.marcar_backup" => self.pedir_confirmacao(AcaoMarcacao::MarcarBackup),
            "duplicados.marcar_delecao" => self.pedir_confirmacao(AcaoMarcacao::MarcarDelecao),
            "duplicados.desmarcar" => self.pedir_confirmacao(AcaoMarcacao::Desmarcar),
            "duplicados.relatorio" => {
                self.duplicados.painel_relatorio = true;
                abrir("duplicados");
            }
            _ => {}
        }
    }

    /// Monta o plano da ação sobre a seleção atual e abre o diálogo de confirmação.
    pub fn pedir_confirmacao(&mut self, acao: AcaoMarcacao) {
        let Some(detalhe) = self.duplicados.detalhe.dado.as_ref() else {
            self.aviso(
                Tom::Aviso,
                "Selecione um grupo (e espere o detalhe carregar).",
            );
            return;
        };
        let alvo = match (acao, self.duplicados.copia, self.duplicados.grupo_inteiro) {
            (AcaoMarcacao::DefinirPrincipal, Some(c), _) | (_, Some(c), false) => {
                AlvoAcao::Copia(c)
            }
            (AcaoMarcacao::DefinirPrincipal, None, _) | (_, None, false) => {
                self.aviso(Tom::Aviso, "Selecione uma cópia na tabela de cópias.");
                return;
            }
            (_, _, true) => AlvoAcao::Grupo,
        };
        match planejar(detalhe, acao, alvo) {
            Ok(p) => self.duplicados.confirmacao = Some(p),
            Err(e) => self.aviso(Tom::Aviso, e),
        }
    }

    // ------------------------------------------------------------------ barra de status

    #[must_use]
    pub fn segmentos_de_status(&self) -> Vec<SegmentoStatus> {
        let modo = self.worker.modo();
        let tom = match modo {
            ModoWorker::Escritor | ModoWorker::Servico => Tom::Sucesso,
            ModoWorker::Iniciando | ModoWorker::Encerrado => Tom::Neutro,
            ModoWorker::SomenteLeitura { .. } => Tom::Aviso,
            ModoWorker::Falhou { .. } => Tom::Perigo,
        };
        let dica_worker = match modo {
            ModoWorker::SomenteLeitura { motivo } | ModoWorker::Falhou { motivo } => {
                format!("{motivo}\n{}", self.caminho.display())
            }
            _ => format!("Catálogo: {}", self.caminho.display()),
        };
        let mut v = vec![
            SegmentoStatus::novo("worker", modo.texto_status())
                .tom(tom)
                .dica(dica_worker),
        ];
        v.push(self.segmento_da_fase());
        let totais = self.totais.dado.map_or_else(
            || "Catálogo: —".to_string(),
            |t| {
                format!(
                    "Catálogo: {} volume(s) · {} arquivo(s) · {}",
                    inteiro_pt_br(t.volumes),
                    inteiro_pt_br(t.arquivos),
                    bytes_humanos(t.bytes_logicos)
                )
            },
        );
        v.push(
            SegmentoStatus::novo("totais", totais)
                .a_direita()
                .dica("Soma dos volumes da última varredura de cada um"),
        );
        v
    }

    fn segmento_da_fase(&self) -> SegmentoStatus {
        let Some(s) = &self.scan else {
            return SegmentoStatus::novo("fase", "Varredura: ociosa");
        };
        let pausado = self.estado_do_scan() == Some(EstadoControle::Pausado);
        let fase = crate::progresso::fase_texto(s.fase(), pausado, s.sem_hash);
        let detalhe = match (&s.progresso, s.andamento_hash()) {
            (_, Some(h)) if s.fase() == Some(FaseScan::Concluido) && !s.sem_hash => format!(
                " — {}/{} arquivos",
                inteiro_pt_br(h.concluidos),
                inteiro_pt_br(h.total)
            ),
            (Some(p), _) => format!(
                " — {} pastas, {} arquivos",
                inteiro_pt_br(p.pastas),
                inteiro_pt_br(p.arquivos)
            ),
            _ => String::new(),
        };
        SegmentoStatus::novo("fase", format!("Varredura: {fase}{detalhe}"))
            .tom(if pausado { Tom::Aviso } else { Tom::Info })
            .dica("Veja a página Varredura para os detalhes")
    }

    /// Ações da lista única, com o estado deste quadro.
    #[must_use]
    pub fn lista_de_acoes(&self) -> Vec<Acao> {
        paginas::todas()
            .into_iter()
            .flat_map(DefPagina::acoes)
            .map(|a| {
                let mut acao = Acao::nova(a.id, a.rotulo).dica(a.dica);
                match a.id {
                    "analise.tree_view" => {
                        acao = acao.alternancia(self.analise.vista == analise::Vista::Tree);
                    }
                    "analise.treemap" => {
                        acao = acao.alternancia(self.analise.vista == analise::Vista::Treemap);
                    }
                    "analise.por_volume" => {
                        acao = acao.alternancia(self.analise.modo == analise::ModoAnalise::Volume);
                    }
                    "analise.por_computador" => {
                        acao =
                            acao.alternancia(self.analise.modo == analise::ModoAnalise::Computador);
                    }
                    _ => {}
                }
                match self.motivo_desabilitada(a.id) {
                    Some(m) => acao.desabilitada(m),
                    None => acao,
                }
            })
            .collect()
    }

    /// Abas do Ribbon, uma por página.
    #[must_use]
    pub fn lista_de_abas() -> Vec<Aba> {
        paginas::todas()
            .into_iter()
            .map(|p| {
                p.grupos
                    .iter()
                    .fold(Aba::nova(p.id, p.rotulo, p.keytip), |aba, g| {
                        let ids: Vec<&str> = g.acoes.iter().map(|a| a.id).collect();
                        aba.grupo(Grupo::novo(g.id, g.rotulo, &ids))
                    })
            })
            .collect()
    }
}

impl AppCasca for AppDag {
    fn acoes(&self) -> Vec<Acao> {
        self.lista_de_acoes()
    }

    fn abas(&self) -> Vec<Aba> {
        Self::lista_de_abas()
    }

    fn navegacao(&self) -> Vec<GrupoNavegacao> {
        vec![GrupoNavegacao::novo(
            "Acervo",
            paginas::todas()
                .into_iter()
                .map(|p| ItemNavegacao::novo(p.id, p.rotulo, p.icone))
                .collect(),
        )]
    }

    fn rota_inicial(&self) -> Rota {
        Rota::nova(paginas::todas()[0].id)
    }

    fn trilha(&self, rota: &Rota) -> Vec<Elo> {
        let rotulo = paginas::por_id(rota.como_texto()).map_or("DAG", |p| p.rotulo);
        vec![Elo::texto(rotulo)]
    }

    fn desenhar_pagina(&mut self, ui: &mut egui::Ui, rota: &Rota, _local: Local) {
        self.acordador.definir(ui.ctx());
        self.processar_eventos();
        if self.scan_ativo() {
            // Cadência baixa enquanto roda; os eventos do worker acordam a UI quando chegam.
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        paginas::faixa_de_avisos(ui, &mut self.avisos);
        match paginas::por_id(rota.como_texto()) {
            Some(p) => (p.desenhar)(ui, self),
            None => {
                ui.label("Página desconhecida.");
            }
        }
        duplicados::dialogo_de_confirmacao(ui.ctx(), self);
    }

    fn status(&self) -> Vec<SegmentoStatus> {
        self.segmentos_de_status()
    }

    fn ao_executar(&mut self, ctx: &egui::Context, id: &str) {
        self.acordador.definir(ctx);
        self.executar_acao(ctx, id);
    }
}

/// Configuração da casca: título, tokens e temas embutidos; nada é lido do disco.
#[must_use]
pub fn config() -> ConfigCasca {
    let mut c = ConfigCasca::nova(crate::TITULO, TOKENS, TEMAS);
    c.tema_inicial = "salvia".into();
    c.icones = FamiliaDeIcones::sem_fonte();
    c
}

/// Monta a casca (valida tokens/temas) e sobe o worker.
///
/// # Errors
/// Tokens ou temas embutidos inválidos.
pub fn montar(cfg: ConfigApp) -> Result<Casca<AppDag>, Box<dyn std::error::Error>> {
    Ok(Casca::nova(AppDag::novo(cfg), config())?)
}

/// Caminho do arquivo de trava do Catálogo (para mostrar nas páginas de Configurações).
#[must_use]
pub fn caminho_da_trava(caminho: &std::path::Path) -> PathBuf {
    caminho_do_lock(caminho)
}

#[cfg(test)]
mod testes {
    use super::*;
    use bancada_casca::ribbon::validar_abas;
    use bancada_casca::{Disponibilidade, ListaDeAcoes};
    use std::collections::HashSet;

    const ABAS: [&str; 8] = [
        "Início",
        "Discos",
        "Varredura",
        "Explorador",
        "Análise",
        "Duplicados",
        "Monitoramento",
        "Configurações",
    ];

    fn app() -> (tempfile::TempDir, AppDag) {
        let tmp = tempfile::tempdir().unwrap();
        let app = AppDag::novo(ConfigApp::com_caminho(tmp.path().join("dados/c.sqlite")));
        (tmp, app)
    }

    /// Processa eventos até o worker abrir o Catálogo.
    fn esperar_worker(app: &mut AppDag) {
        let fim = Instant::now() + Duration::from_secs(30);
        while *app.worker.modo() == ModoWorker::Iniciando && Instant::now() < fim {
            app.processar_eventos();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn habilitada(app: &AppDag, id: &str) -> bool {
        app.lista_de_acoes()
            .into_iter()
            .find(|a| a.id == id)
            .unwrap_or_else(|| panic!("ação {id} não existe"))
            .esta_habilitada()
    }

    #[test]
    fn as_oito_abas_estao_presentes_na_ordem() {
        let rotulos: Vec<String> = AppDag::lista_de_abas()
            .into_iter()
            .map(|a| a.rotulo)
            .collect();
        assert_eq!(rotulos, ABAS);
    }

    #[test]
    fn ids_de_abas_acoes_e_keytips_sao_unicos() {
        let (_t, app) = app();
        let abas = AppDag::lista_de_abas();
        let ids: HashSet<_> = abas.iter().map(|a| a.id.clone()).collect();
        assert_eq!(ids.len(), abas.len());
        let keytips: HashSet<_> = abas.iter().map(|a| a.keytip.clone()).collect();
        assert_eq!(keytips.len(), abas.len());
        let acoes = app.lista_de_acoes();
        let ids: HashSet<_> = acoes.iter().map(|a| a.id.clone()).collect();
        assert_eq!(ids.len(), acoes.len());
    }

    #[test]
    fn toda_acao_tem_rotulo_e_motivo_quando_desabilitada() {
        let (_t, app) = app();
        for a in app.lista_de_acoes() {
            assert!(!a.rotulo.trim().is_empty(), "{} sem rótulo", a.id);
            if let Disponibilidade::Desabilitada { motivo } = &a.disponibilidade {
                assert!(!motivo.trim().is_empty(), "{} sem motivo", a.id);
            }
        }
    }

    #[test]
    fn abas_e_acoes_sao_validas_para_a_casca() {
        let (_t, app) = app();
        let lista = ListaDeAcoes::nova(app.lista_de_acoes()).expect("lista válida");
        validar_abas(&AppDag::lista_de_abas(), &lista).expect("abas válidas");
        assert!(paginas::todas().iter().all(|p| p.acoes().next().is_some()));
    }

    #[test]
    fn varredura_e_duplicados_tem_as_acoes_planejadas() {
        let rotulos = |id: &str| -> Vec<&'static str> {
            paginas::por_id(id)
                .map(|p| p.acoes().map(|a| a.rotulo).collect())
                .unwrap_or_default()
        };
        for esperado in [
            "Full Scan",
            "Full Scan sem hash",
            "Fast Scan",
            "Pausar",
            "Retomar",
            "Cancelar",
            "Selecionar raízes",
            "Exclusões",
        ] {
            assert!(rotulos("varredura").contains(&esperado), "{esperado}");
        }
        for esperado in [
            "Grupos",
            "Cópia Principal",
            "Marcar backup",
            "Marcar para deleção",
            "Desmarcar",
            "Relatório",
        ] {
            assert!(rotulos("duplicados").contains(&esperado), "{esperado}");
        }
        assert!(rotulos("discos").contains(&"Atualizar volumes"));
    }

    #[test]
    fn acoes_seguem_o_estado_da_sessao() {
        let (_t, mut app) = app();
        esperar_worker(&mut app);
        assert_eq!(*app.worker.modo(), ModoWorker::Escritor);
        // Sem raízes: não dá para varrer; com raiz, dá.
        assert!(!habilitada(&app, "varredura.full_scan"));
        app.varredura.adicionar_raiz("/tmp/");
        app.varredura.adicionar_raiz("/tmp");
        assert_eq!(
            app.varredura.raizes,
            ["/tmp"],
            "sem repetir, sem barra final"
        );
        assert!(habilitada(&app, "varredura.full_scan"));
        assert!(habilitada(&app, "varredura.full_scan_sem_hash"));
        assert!(habilitada(&app, "discos.detectar"));
        // Pausar/Retomar/Cancelar só com varredura rodando.
        for id in [
            "varredura.pausar",
            "varredura.retomar",
            "varredura.cancelar",
        ] {
            assert!(!habilitada(&app, id), "{id}");
        }
        // Fast Scan, monitor e configurações seguem adiados.
        for id in [
            "varredura.fast_scan",
            "monitoramento.iniciar",
            "configuracoes.limites",
            "varredura.exclusoes",
        ] {
            assert!(!habilitada(&app, id), "{id}");
        }
        // Páginas desta entrega estão ativas.
        for id in [
            "analise.treemap",
            "analise.por_volume",
            "explorador.recolher",
            "duplicados.relatorio",
        ] {
            assert!(habilitada(&app, id), "{id}");
        }
        // Duplicados: ações de marcação exigem grupo selecionado.
        assert!(!habilitada(&app, "duplicados.marcar_backup"));
        // Alternâncias refletem o estado.
        app.analise.vista = analise::Vista::Tree;
        let tree = app
            .lista_de_acoes()
            .into_iter()
            .find(|a| a.id == "analise.tree_view")
            .unwrap();
        assert!(tree.marcada);
    }

    #[test]
    fn status_mostra_worker_fase_e_totais() {
        let (_t, mut app) = app();
        let s = app.segmentos_de_status();
        assert!(
            s.iter()
                .any(|s| s.id == "worker" && s.texto == "Worker: iniciando…")
        );
        esperar_worker(&mut app);
        let s = app.segmentos_de_status();
        let worker = s.iter().find(|s| s.id == "worker").unwrap();
        assert_eq!(worker.texto, "Worker: escritor (esta sessão)");
        assert!(
            s.iter()
                .any(|s| s.id == "fase" && s.texto == "Varredura: ociosa")
        );
        assert!(
            s.iter()
                .any(|s| s.id == "totais" && s.texto.starts_with("Catálogo:"))
        );
    }

    #[test]
    fn outro_escritor_aparece_na_barra_de_status() {
        let tmp = tempfile::tempdir().unwrap();
        let caminho = tmp.path().join("c.sqlite");
        let dono = dag_agent::Agente::abrir(&caminho).unwrap().silencioso();
        let mut app = AppDag::novo(ConfigApp::com_caminho(caminho));
        esperar_worker(&mut app);
        let s = app.segmentos_de_status();
        let worker = s.iter().find(|s| s.id == "worker").unwrap();
        assert_eq!(worker.texto, "Worker: outro processo é o escritor");
        assert_eq!(worker.tom, Tom::Aviso);
        // Sem escrita: varrer e detectar volumes ficam desabilitados, com motivo.
        app.varredura.adicionar_raiz("/tmp");
        assert!(!habilitada(&app, "varredura.full_scan"));
        assert!(!habilitada(&app, "discos.detectar"));
        assert_eq!(app.motivo_sem_escrita(), Some(MOTIVO_SEM_ESCRITA));
        drop(dono);
    }

    #[test]
    fn titulo_da_janela() {
        assert_eq!(config().titulo, "DAG — Acervo Digital");
    }
}
