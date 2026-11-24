//! O [`Agente`]: abre o Catálogo e executa [`Comando`]s.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use dag_core::Plataforma as _;
use dag_core::{
    Alvo, Autor, ClasseVolume, Comando, EstadoExecucao, EtapaVarredura, EventoInfo, ExecucaoId,
    FaseInventario, ModoVarredura, OpcoesHash, OrdemDuplicados, Pedido, Progresso,
    ProgressoInventario, Resposta, ResumoCatalogo, ResumoDuplicados, ResumoHash, ResumoMonitorInfo,
    ResumoReconciliacao, ResumoVerificacao, SaudeMonitorInfo, StatusConfianca, StatusDiscoHash,
    StatusExecucao, StatusHash, TipoDispositivo, TipoMarcacao, TipoRelatorio, VolumeDescoberto,
    VolumeId, VolumeInfo,
};
use dag_monitor::{ConfigMonitor, Monitor};
use dag_platform_linux::PlataformaLinux;
use dag_scanner::{
    ConfigHash, ConfigScan, ControleScan, DesfechoScan, EtapaHash, Exclusao, FaseProgresso,
    FastScan, FullScan, Progresso as ProgressoScanner, ProgressoHash as ProgressoHashScanner,
    ResultadoScan, VerificacaoIntegridade,
};
use dag_store::Catalogo;
use dag_store::confianca;
use dag_store::configuracao;
use dag_store::duplicados::{self, Escopo, FiltroGrupos};
use dag_store::erros as erros_db;
use dag_store::hashes;
use dag_store::inventario::{self, EstadoScan, FaseScan};
use dag_store::reconciliacao;
use rusqlite::{Connection, OpenFlags, params};

use crate::caminho::preparar;
use crate::classificador::ClassificadorPorExtensao;
use crate::cofre::CofreLazy;
use crate::erro::ErroAgente;
use crate::exibir::progresso_stderr;
use crate::explorar::explorar;
use crate::finalidade::classificar_execucao;
use crate::trava::GuardaEscritor;

/// Destino dos eventos de progresso do agente.
pub type CallbackProgresso = Arc<dyn Fn(&Progresso) + Send + Sync>;

const STATUS_LIMITE: i64 = 10;
const PAGINA_PADRAO: u32 = 50;
const PAGINA_MAXIMA: u32 = 1000;
/// Heartbeat mais antigo que isto (s) = monitor morto (a trava é solta com o processo).
const HEARTBEAT_VALIDO_S: i64 = 15;

/// Diretório do arquivo de reserva do Cofre: ao lado do Catálogo.
fn dir_do_cofre(catalogo: &Path) -> PathBuf {
    catalogo
        .parent()
        .map_or_else(|| PathBuf::from("segredos"), |p| p.join("segredos"))
}

pub(crate) fn agora() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// `true` para comandos que só leem o Catálogo (não exigem ser o escritor). Relatório com
/// `saida` grava um arquivo escolhido pelo usuário, nunca o Catálogo.
#[must_use]
pub fn somente_leitura(c: &Comando) -> bool {
    matches!(
        c,
        Comando::Status { .. }
            | Comando::Explorar { .. }
            | Comando::ListarDuplicados { .. }
            | Comando::DetalharGrupo { .. }
            | Comando::Relatorio { .. }
            | Comando::Eventos { .. }
            | Comando::SaudeMonitor
            | Comando::ListarJobs
            | Comando::ListarLixeira { .. }
            | Comando::MoverParaLixeira { simular: true, .. }
            | Comando::StatusConteineres
            | Comando::ListarPedidos { .. }
            | Comando::ErrosDaVarredura { .. }
            | Comando::ErrosRegras
            | Comando::CofreListar
            | Comando::ExplorarConteiner { .. }
            | Comando::ConfigHash {
                threads_por_disco: None,
                max_discos: None
            }
    )
}

/// Incremento em que o comando passa a existir, se ainda não foi implementado.
#[must_use]
pub fn incremento_pendente(c: &Comando) -> Option<u8> {
    // O serviço (incremento 6) existe: ver `servico.rs`.
    let _ = c;
    None
}

/// Raiz catalogada, conectada e acessível.
#[derive(Debug, Clone)]
pub(crate) struct RaizDisponivel {
    pub caminho: PathBuf,
    pub volume_id: i64,
    pub path_raw: Vec<u8>,
}

/// O Catálogo aberto, com (ou sem) a trava de escritor.
pub struct Agente {
    pub(crate) catalogo: Catalogo,
    caminho: PathBuf,
    guarda: Option<GuardaEscritor>,
    pub(crate) controle: ControleScan,
    pub(crate) progresso: Option<CallbackProgresso>,
    volumes_fixos: Option<Vec<VolumeDescoberto>>,
    pub(crate) autor: Autor,
    /// Conferência de `st_dev` do `rename` da Lixeira (injetável nos testes).
    pub(crate) dispositivos: Arc<dyn crate::lixeira::Dispositivos>,
    /// Paralelismo do hash dado por flag do agente (vence o persistido, perde para o pedido).
    opcoes_hash: OpcoesHash,
    /// Cofre de Senhas (chave no keyring do sistema; abre só quando alguém o usa).
    pub(crate) cofre: Arc<CofreLazy>,
}

impl fmt::Debug for Agente {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Agente")
            .field("caminho", &self.caminho)
            .field("escritor", &self.guarda.is_some())
            .finish_non_exhaustive()
    }
}

impl Agente {
    /// Abre como **escritor**: adquire a trava, cria o diretório (0700) e o banco (0600) se
    /// faltarem e migra o esquema.
    ///
    /// # Errors
    /// [`ErroAgente::EscritorOcupado`] se outro processo já é o escritor; falhas do Catálogo.
    pub fn abrir(caminho: impl Into<PathBuf>) -> Result<Self, ErroAgente> {
        let caminho = caminho.into();
        let guarda = GuardaEscritor::adquirir(&caminho)?;
        preparar(&caminho)?;
        let catalogo = Catalogo::abrir(&caminho)?;
        Ok(Self::montar(catalogo, caminho, Some(guarda)))
    }

    /// Abre só para leitura (sem trava): serve `Status` e `Explorar` mesmo com um escritor
    /// ativo. Não cria nada e recusa Catálogo em versão diferente da desta build.
    ///
    /// # Errors
    /// [`ErroAgente::SemCatalogo`], [`ErroAgente::CatalogoDesatualizado`] ou falha do Catálogo.
    pub fn abrir_leitura(caminho: impl Into<PathBuf>) -> Result<Self, ErroAgente> {
        let caminho = caminho.into();
        if !caminho.is_file() {
            return Err(ErroAgente::SemCatalogo(caminho));
        }
        let encontrada: u32 = {
            let c = Connection::open_with_flags(&caminho, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            c.query_row("PRAGMA user_version", [], |l| l.get(0))?
        };
        let esperada = dag_store::versao_do_esquema();
        if encontrada != esperada {
            return Err(ErroAgente::CatalogoDesatualizado {
                caminho,
                encontrada,
                esperada,
            });
        }
        let catalogo = Catalogo::abrir(&caminho)?;
        Ok(Self::montar(catalogo, caminho, None))
    }

    fn montar(catalogo: Catalogo, caminho: PathBuf, guarda: Option<GuardaEscritor>) -> Self {
        let cofre = Arc::new(CofreLazy::novo(dir_do_cofre(&caminho), None));
        Self {
            catalogo,
            caminho,
            guarda,
            controle: ControleScan::novo(),
            progresso: Some(progresso_stderr()),
            volumes_fixos: None,
            autor: Autor::Cli,
            dispositivos: Self::dispositivos_padrao(),
            opcoes_hash: OpcoesHash::default(),
            cofre,
        }
    }

    /// Usa outro chaveiro do sistema no Cofre de Senhas (testes passam um falso em memória).
    #[must_use]
    pub fn com_chaveiro(mut self, c: Arc<dyn bancada_segredos::ChaveiroDoSistema>) -> Self {
        self.cofre = Arc::new(CofreLazy::novo(dir_do_cofre(&self.caminho), Some(c)));
        self
    }

    /// Paralelismo do Hash Completo vindo de flag do agente (`--threads-por-disco`,
    /// `--max-discos`). Precedência: pedido > flag do agente > persistido > padrão.
    ///
    /// # Errors
    /// Valor fora da faixa (1..=8 threads, 1..=16 discos).
    pub fn com_opcoes_hash(mut self, o: OpcoesHash) -> Result<Self, ErroAgente> {
        o.validar().map_err(ErroAgente::Invalido)?;
        self.opcoes_hash = o;
        Ok(self)
    }

    /// Configuração efetiva do hash para uma execução: pedido > agente > persistido > padrão.
    fn config_hash(&self, pedido: OpcoesHash) -> Result<ConfigHash, ErroAgente> {
        config_hash_efetiva(self.catalogo.armazem().conexao(), pedido, self.opcoes_hash)
    }

    /// Quem originou os pedidos (gravado no histórico de Marcações). Padrão: CLI.
    #[must_use]
    pub fn com_autor(mut self, autor: Autor) -> Self {
        self.autor = autor;
        self
    }

    /// Callback de progresso (padrão: linha em stderr). Recebe inventário, reconciliação, hash
    /// com bytes, verificação e mudanças de etapa; pode ser chamado de threads do trabalho.
    #[must_use]
    pub fn com_progresso(mut self, f: impl Fn(&Progresso) + Send + Sync + 'static) -> Self {
        self.progresso = Some(Arc::new(f));
        self
    }

    /// Troca (ou remove) o destino do progresso numa instância já montada.
    pub fn definir_progresso(&mut self, f: Option<CallbackProgresso>) {
        self.progresso = f;
    }

    /// Emite uma mudança de etapa sem contadores.
    pub(crate) fn mensagem(&self, texto: impl Into<String>) {
        if let Some(p) = &self.progresso {
            p(&Progresso::Mensagem {
                texto: texto.into(),
            });
        }
    }

    /// Diagnóstico de memória: uma linha em stderr (journal do serviço) com o RSS e o pico do
    /// processo ao fim de cada etapa longa. Em campo, é o que mostra qual etapa consome memória.
    pub(crate) fn marcar_memoria(&self, etapa: &str) {
        if let Some((rss, pico)) = memoria_do_processo() {
            eprintln!(
                "agente: memória após {etapa}: RSS {} MiB (pico {} MiB)",
                rss / 1024,
                pico / 1024
            );
        }
    }

    /// Emite o passo estruturado da rotina de abertura (retomada, passada de verificação).
    pub(crate) fn passo(&self, passo: dag_core::PassoAbertura) {
        if let Some(p) = &self.progresso {
            p(&Progresso::Passo(passo));
        }
    }

    /// Consolida o WAL no arquivo principal (encerramento limpo do serviço).
    pub fn checkpoint(&self) {
        let _ = self
            .catalogo
            .armazem()
            .conexao()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
    }

    /// Sem saída de progresso.
    #[must_use]
    pub fn silencioso(mut self) -> Self {
        self.progresso = None;
        self
    }

    /// Usa estes volumes em vez da descoberta do sistema (testes).
    #[must_use]
    pub fn com_volumes_fixos(mut self, volumes: Vec<VolumeDescoberto>) -> Self {
        self.volumes_fixos = Some(volumes);
        self
    }

    /// Alça para cancelar a varredura em andamento de outra thread (Ctrl-C).
    #[must_use]
    pub fn controle(&self) -> ControleScan {
        self.controle.clone()
    }

    #[must_use]
    pub fn caminho_catalogo(&self) -> &Path {
        &self.caminho
    }

    #[must_use]
    pub fn catalogo(&self) -> &Catalogo {
        &self.catalogo
    }

    /// Esta instância é a escritora?
    #[must_use]
    pub fn eh_escritor(&self) -> bool {
        self.guarda.is_some()
    }

    /// Executa um [`Pedido`] com o autor dele (GUI, CLI ou MCP).
    pub fn executar_pedido(&mut self, pedido: Pedido) -> Resposta {
        let anterior = std::mem::replace(&mut self.autor, pedido.autor);
        let r = self.executar(pedido.comando);
        self.autor = anterior;
        r
    }

    /// Executa um comando; falhas viram [`Resposta::Erro`].
    pub fn executar(&mut self, comando: Comando) -> Resposta {
        self.tentar(comando).unwrap_or_else(|e| e.para_resposta())
    }

    fn tentar(&mut self, comando: Comando) -> Result<Resposta, ErroAgente> {
        if let Comando::Servico { acao } = &comando {
            let con = self
                .guarda
                .is_some()
                .then(|| self.catalogo.armazem().conexao());
            return Ok(crate::servico::executar(*acao, &self.caminho, con));
        }
        if let Some(n) = incremento_pendente(&comando) {
            return Err(ErroAgente::NaoImplementado(n));
        }
        if self.guarda.is_none() && !somente_leitura(&comando) {
            return Err(ErroAgente::Invalido(
                "agente aberto só para leitura não executa este comando".into(),
            ));
        }
        match comando {
            Comando::ListarVolumes => self.listar_volumes(),
            Comando::IniciarScan {
                modo: ModoVarredura::FastScan,
                raizes,
                sem_hash,
                elevar,
                threads_por_disco,
                max_discos,
                ..
            } => self.fast_scan(
                &raizes,
                sem_hash,
                elevar,
                OpcoesHash {
                    threads_por_disco,
                    max_discos,
                },
            ),
            Comando::IniciarScan {
                raizes,
                sem_hash,
                elevar,
                releitura_total,
                threads_por_disco,
                max_discos,
                ..
            } => self.full_scan(
                &raizes,
                sem_hash,
                elevar,
                releitura_total,
                OpcoesHash {
                    threads_por_disco,
                    max_discos,
                },
            ),
            Comando::Monitorar {
                raizes,
                reconciliar_a_cada_s,
                sem_hash,
            } => self.monitorar(&raizes, reconciliar_a_cada_s, sem_hash),
            Comando::Eventos { limite } => self.eventos(limite),
            Comando::ErrosDaVarredura {
                execucao,
                codigo,
                pasta,
                amostras,
                agrupar,
            } => self.erros_da_varredura(
                execucao,
                agrupar.unwrap_or_default(),
                codigo.as_deref(),
                pasta.as_deref(),
                amostras,
            ),
            Comando::ErrosRegras => self.erros_regras(),
            Comando::ErrosAceitar {
                padrao,
                codigo,
                motivo,
            } => {
                let padrao = padrao.trim();
                if padrao.is_empty() {
                    return Err(ErroAgente::Invalido(
                        "informe o padrão (glob de caminho)".into(),
                    ));
                }
                erros_db::adicionar_regra(
                    self.catalogo.armazem().conexao(),
                    padrao,
                    codigo.as_deref(),
                    motivo.as_deref().unwrap_or(""),
                    agora(),
                )?;
                self.erros_regras()
            }
            Comando::ErrosRemover { id } => {
                if !erros_db::remover_regra(self.catalogo.armazem().conexao(), id)? {
                    return Err(ErroAgente::Invalido(format!("não há regra {id}")));
                }
                self.erros_regras()
            }
            Comando::Excluir { padrao, motivo } => {
                let padrao = padrao.trim();
                if padrao.is_empty() {
                    return Err(ErroAgente::Invalido(
                        "informe o padrão (glob de caminho)".into(),
                    ));
                }
                erros_db::adicionar_exclusao(
                    self.catalogo.armazem().conexao(),
                    padrao,
                    motivo.as_deref().unwrap_or("excluído pelo usuário"),
                )?;
                self.erros_regras()
            }
            Comando::ExclusaoRemover { id } => {
                if !erros_db::remover_exclusao(self.catalogo.armazem().conexao(), id)? {
                    return Err(ErroAgente::Invalido(format!("não há exclusão {id}")));
                }
                self.erros_regras()
            }
            Comando::SaudeMonitor => Ok(Resposta::SaudeMonitor {
                saude: self.saude_monitor()?,
            }),
            Comando::Retomar {
                execucao: Some(id),
                threads_por_disco,
                max_discos,
            } => self.retomar_scan(
                id,
                OpcoesHash {
                    threads_por_disco,
                    max_discos,
                },
            ),
            Comando::Retomar { execucao: None, .. } => Err(ErroAgente::Invalido(
                "informe a execução a retomar (ex.: `dag scan --retomar <id>`)".into(),
            )),
            Comando::Status { execucao } => self.status(execucao),
            Comando::Explorar {
                pasta,
                limite,
                cursor,
            } => explorar(
                self.catalogo.armazem().conexao(),
                pasta.as_deref(),
                limite,
                cursor.as_deref(),
            ),
            Comando::BackupCatalogo { destino } => self.backup(Path::new(&destino)),
            Comando::LerConteineres => self.ler_conteineres(),
            Comando::StatusConteineres => self.status_conteineres(),
            Comando::ListarPedidos { incluir_encerrados } => {
                self.listar_pedidos(incluir_encerrados)
            }
            Comando::ResolverPedido { id, senha } => self.resolver_pedido(id, &senha),
            Comando::DispensarPedido { id } => self.dispensar_pedido(id),
            Comando::TentarDeNovo { id } => self.tentar_de_novo(id),
            Comando::CofreAdicionar { rotulo, senha } => self.cofre_adicionar(&rotulo, &senha),
            Comando::CofreListar => self.cofre_listar(),
            Comando::CofreRemover { id } => self.cofre_remover(id),
            Comando::ExplorarConteiner {
                conteiner,
                pai_ordem,
                pasta,
                limite,
                cursor,
            } => self.explorar_conteiner(&conteiner, pai_ordem, &pasta, limite, cursor.as_deref()),
            Comando::ListarDuplicados {
                volume,
                tamanho_minimo,
                ordem,
                limite,
                cursor,
            } => self.listar_duplicados(volume.as_deref(), tamanho_minimo, ordem, limite, cursor),
            Comando::ReconstruirDuplicados => {
                let r = self.atualizar_duplicados()?;
                Ok(Resposta::Ok {
                    mensagem: Some(format!(
                        "Grupos de Duplicados reconstruídos: {} grupo(s) com {} cópia(s) ({} recuperáveis); {} Marcação(ões) pendente(s)",
                        r.grupos,
                        r.copias,
                        dag_core::formato::bytes_humanos(r.espaco_recuperavel_bytes),
                        r.marcacoes_pendentes
                    )),
                })
            }
            Comando::DetalharGrupo { grupo } => Ok(Resposta::Grupo {
                detalhe: duplicados::detalhar_grupo(self.catalogo.armazem().conexao(), grupo)?,
            }),
            Comando::DefinirCopiaPrincipal { grupo, arquivo } => {
                duplicados::definir_copia_principal(
                    &mut self.catalogo,
                    grupo,
                    arquivo.as_ref(),
                    agora(),
                )?;
                Ok(Resposta::Grupo {
                    detalhe: duplicados::detalhar_grupo(self.catalogo.armazem().conexao(), grupo)?,
                })
            }
            Comando::MarcarBackup { alvo } => self.marcar(TipoMarcacao::Backup, &alvo),
            Comando::MarcarDelecao { alvo } => self.marcar(TipoMarcacao::Delecao, &alvo),
            Comando::Desmarcar { alvo } => Ok(Resposta::Desmarcado {
                removidas: duplicados::desmarcar(&mut self.catalogo, &alvo, self.autor, agora())?,
            }),
            Comando::ConfirmarMarcacao { marcacao } => Ok(Resposta::Marcacao {
                resultado: duplicados::confirmar(
                    &mut self.catalogo,
                    marcacao,
                    self.autor,
                    agora(),
                )?,
            }),
            Comando::DefinirPrioridadeVolume { ordem } => {
                let itens = duplicados::definir_prioridade(
                    &mut self.catalogo,
                    &ordem,
                    self.autor,
                    agora(),
                )?;
                let lista: Vec<&str> = itens.iter().map(|i| i.nome.as_str()).collect();
                Ok(Resposta::Ok {
                    mensagem: Some(format!(
                        "Prioridade de Volume definida: {}",
                        lista.join(" > ")
                    )),
                })
            }
            Comando::MoverParaLixeira {
                alvos,
                confirmacao_unica_copia,
                marcar_tudo,
                simular,
            } => self.mover_para_lixeira(&alvos, confirmacao_unica_copia, marcar_tudo, simular),
            Comando::RestaurarDaLixeira { alvos } => self.restaurar_da_lixeira(&alvos),
            Comando::ListarLixeira { limite, cursor } => {
                self.listar_lixeira(limite, cursor.as_deref())
            }
            Comando::Relatorio {
                tipo,
                nivel,
                limite,
                cursor,
                saida,
            } => self.relatorio(tipo, nivel, limite, cursor.as_deref(), saida.as_deref()),
            Comando::VerificarIntegridade {
                raizes,
                threads_por_disco,
                max_discos,
            } => self.verificar(
                &raizes,
                OpcoesHash {
                    threads_por_disco,
                    max_discos,
                },
            ),
            Comando::ConfigHash {
                threads_por_disco,
                max_discos,
            } => self.configurar_hash(OpcoesHash {
                threads_por_disco,
                max_discos,
            }),
            // Controlam um trabalho em andamento: só fazem sentido num agente em segundo plano
            // (que os atende fora da fila). Em processo, nada roda concorrentemente.
            Comando::Pausar { .. }
            | Comando::Cancelar { .. }
            | Comando::PausarJob { .. }
            | Comando::RetomarJob { .. }
            | Comando::CancelarJob { .. }
            | Comando::ReiniciarAgente => Err(ErroAgente::SemAgente),
            // Uma sessão em processo não tem fila: os trabalhos são do agente em segundo plano.
            Comando::ListarJobs => Ok(Resposta::Jobs { jobs: Vec::new() }),
            _ => Err(ErroAgente::NaoImplementado(0)),
        }
    }

    pub(crate) fn volumes_do_sistema(&self) -> Result<Vec<VolumeDescoberto>, ErroAgente> {
        match &self.volumes_fixos {
            Some(v) => Ok(v.clone()),
            None => Ok(PlataformaLinux.descobrir_volumes()?),
        }
    }

    // ------------------------------------------------------------------ volumes

    fn listar_volumes(&mut self) -> Result<Resposta, ErroAgente> {
        let descobertos = self.volumes_do_sistema()?;
        let t = agora();
        // Volumes de imagem/virtuais (snap, loop, zram...) são ruído: não entram no Catálogo por
        // aqui. Os que já estavam registrados (varridos antes, de propósito) continuam
        // sincronizados para não virarem "desmontados" por engano.
        let registrados: HashSet<String> = {
            let mut st = self
                .catalogo
                .armazem()
                .conexao()
                .prepare("SELECT platform_id FROM volumes")?;
            st.query_map([], |l| l.get(0))?.collect::<Result<_, _>>()?
        };
        let a_sincronizar: Vec<VolumeDescoberto> = descobertos
            .iter()
            .filter(|v| v.classe.eh_real() || registrados.contains(&v.platform_id))
            .cloned()
            .collect();
        self.catalogo
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroAgente> {
                // Conectados são registrados; os que sumiram ficam `mounted = 0` (última vez visto
                // preservada, nenhuma linha de arquivo alterada) com evento `desmontado`.
                reconciliacao::sincronizar_volumes(tx, &a_sincronizar, t)?;
                Ok(())
            })?;
        let conn = self.catalogo.armazem().conexao();
        let mut st = conn.prepare(
            "SELECT v.id, v.device_id, d.tipo, v.fs_type, v.rotulo, v.capacity_bytes,
                    v.free_bytes, v.mounted,
                    (SELECT path_display FROM mount_points m WHERE m.volume_id = v.id
                     ORDER BY length(m.path_raw), m.id LIMIT 1),
                    v.platform_id
             FROM volumes v LEFT JOIN dispositivos d ON d.id = v.device_id
             ORDER BY v.mounted DESC, v.id",
        )?;
        let classes: HashMap<&str, (ClasseVolume, bool)> = descobertos
            .iter()
            .map(|v| (v.platform_id.as_str(), (v.classe, v.sistema)))
            .collect();
        let mut volumes = Vec::new();
        let mut ocultos = Vec::new();
        let linhas = st.query_map([], |l| {
            let tipo: Option<String> = l.get(2)?;
            let platform_id: String = l.get(9)?;
            let (classe, sistema) = classes
                .get(platform_id.as_str())
                .copied()
                .unwrap_or_default();
            Ok(VolumeInfo {
                id: VolumeId(l.get(0)?),
                dispositivo: l.get::<_, Option<i64>>(1)?.map(Into::into),
                tipo_dispositivo: tipo.as_deref().and_then(tipo_dispositivo),
                fs_type: l.get(3)?,
                rotulo: l.get(4)?,
                capacidade_bytes: l.get::<_, Option<i64>>(5)?.map(|x| x as u64),
                livre_bytes: l.get::<_, Option<i64>>(6)?.map(|x| x as u64),
                conectado: l.get::<_, i64>(7)? != 0,
                ponto_montagem: l.get(8)?,
                classe,
                sistema,
            })
        })?;
        for v in linhas {
            let v = v?;
            if v.classe.eh_real() {
                volumes.push(v);
            } else {
                ocultos.push(v);
            }
        }
        // Ocultos ainda fora do Catálogo: descritos direto da descoberta, com id 0.
        for d in descobertos
            .iter()
            .filter(|v| !v.classe.eh_real() && !registrados.contains(&v.platform_id) && !v.pseudo)
        {
            ocultos.push(info_descoberto(d));
        }
        Ok(Resposta::Volumes { volumes, ocultos })
    }

    /// Ids de Volumes registrados que hoje são de imagem/virtuais e nunca foram varridos:
    /// ficam fora dos totais do Catálogo.
    fn volumes_ruido_registrados(&self) -> Result<HashSet<i64>, ErroAgente> {
        let ocultos: HashSet<String> = self
            .volumes_do_sistema()?
            .into_iter()
            .filter(|v| !v.classe.eh_real())
            .map(|v| v.platform_id)
            .collect();
        let conn = self.catalogo.armazem().conexao();
        let mut st = conn.prepare(
            "SELECT id, platform_id FROM volumes
             WHERE id NOT IN (SELECT volume_id FROM volume_stats)",
        )?;
        let ids = st
            .query_map([], |l| Ok((l.get::<_, i64>(0)?, l.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(_, p)| ocultos.contains(p))
            .map(|(i, _)| i)
            .collect();
        Ok(ids)
    }

    // ------------------------------------------------------------------ varredura

    /// Configuração de varredura com as exclusões do usuário (`dag excluir`).
    fn config_scan(&self) -> Result<ConfigScan, ErroAgente> {
        let home = erros_db::pasta_home();
        let exclusoes = erros_db::listar_exclusoes(self.catalogo.armazem().conexao())?
            .into_iter()
            .map(|e| Exclusao::glob(&e.padrao, e.motivo, home.as_deref()))
            .collect();
        Ok(ConfigScan {
            exclusoes,
            ..ConfigScan::default()
        })
    }

    fn novo_scan(&self, raizes: Vec<PathBuf>, elevar: bool) -> Result<FullScan, ErroAgente> {
        let elevado = crate::servico::leitor_elevado_se(&raizes, elevar);
        let mut scan = FullScan::novo(raizes, self.volumes_do_sistema()?)
            .com_config(self.config_scan()?)
            .com_controle(self.controle.clone())
            .com_classificador(Arc::new(ClassificadorPorExtensao));
        if let Some(l) = elevado {
            scan = scan.com_leitor_elevado(l);
        }
        if let Some(p) = &self.progresso {
            let p = Arc::clone(p);
            scan = scan.com_progresso(move |x| p(&Progresso::Inventario(inventario_de(x))));
        }
        Ok(scan)
    }

    fn full_scan(
        &mut self,
        raizes: &[String],
        sem_hash: bool,
        elevar: bool,
        releitura_total: bool,
        opcoes: OpcoesHash,
    ) -> Result<Resposta, ErroAgente> {
        if raizes.is_empty() {
            return Err(ErroAgente::Invalido("informe ao menos uma raiz".into()));
        }
        self.config_hash(opcoes)?;
        let raizes = self.raizes_com_lixeira(raizes.iter().map(PathBuf::from).collect())?;
        let scan = self.novo_scan(raizes, elevar)?;
        let r = scan.executar(&mut self.catalogo)?;
        self.concluir_scan(r, sem_hash, releitura_total, opcoes)
    }

    /// Fast Scan (A-12): reconcilia o Catálogo com o disco reaproveitando metadados e hash do
    /// que não mudou. Sem raízes, usa as raízes catalogadas cujo Volume está conectado.
    pub(crate) fn fast_scan(
        &mut self,
        raizes: &[String],
        sem_hash: bool,
        elevar: bool,
        opcoes: OpcoesHash,
    ) -> Result<Resposta, ErroAgente> {
        self.config_hash(opcoes)?;
        let raizes: Vec<PathBuf> = if raizes.is_empty() {
            self.raizes_padrao()?
        } else {
            raizes.iter().map(PathBuf::from).collect()
        };
        let raizes = self.raizes_com_lixeira(raizes)?;
        let elevado = crate::servico::leitor_elevado_se(&raizes, elevar);
        let mut scan = FastScan::novo(raizes, self.volumes_do_sistema()?)
            .com_config(self.config_scan()?)
            .com_controle(self.controle.clone())
            .com_classificador(Arc::new(ClassificadorPorExtensao));
        if let Some(l) = elevado {
            scan = scan.com_leitor_elevado(l);
        }
        if let Some(p) = &self.progresso {
            let p = Arc::clone(p);
            scan = scan.com_progresso(move |x| p(&Progresso::Reconciliacao(inventario_de(x))));
        }
        let r = scan.executar(&mut self.catalogo)?;
        self.concluir_scan(r, sem_hash, false, opcoes)
    }

    /// Raízes já catalogadas cujo Volume está conectado e cujo caminho ainda pertence a ele
    /// (um ponto de montagem vazio, com o volume desconectado, não conta).
    fn raizes_padrao(&mut self) -> Result<Vec<PathBuf>, ErroAgente> {
        let out: Vec<PathBuf> = self
            .raizes_disponiveis()?
            .into_iter()
            .map(|r| r.caminho)
            .collect();
        if out.is_empty() {
            return Err(ErroAgente::Invalido(
                "nenhuma raiz catalogada disponível; informe as raízes (ex.: `dag scan --fast /dados`)"
                    .into(),
            ));
        }
        Ok(out)
    }

    /// Como [`raizes_padrao`](Self::raizes_padrao), com a identidade da Raiz no Catálogo; lista
    /// vazia não é erro.
    pub(crate) fn raizes_disponiveis(&mut self) -> Result<Vec<RaizDisponivel>, ErroAgente> {
        let volumes = self.volumes_do_sistema()?;
        let t = agora();
        self.catalogo
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroAgente> {
                reconciliacao::sincronizar_volumes(tx, &volumes, t)?;
                Ok(())
            })?;
        let raizes = reconciliacao::raizes_catalogadas(self.catalogo.armazem().conexao())?;
        let mut out = Vec::new();
        for r in raizes {
            let caminho = PathBuf::from(std::ffi::OsStr::from_bytes(&r.path_raw));
            let dev = std::fs::symlink_metadata(&caminho)
                .ok()
                .map(|m| std::os::unix::fs::MetadataExt::dev(&m));
            let esperado = volumes
                .iter()
                .find(|v| v.platform_id == r.platform_id)
                .map(|v| v.st_dev);
            if r.conectado && dev.is_some() && dev == esperado {
                out.push(RaizDisponivel {
                    caminho,
                    volume_id: r.volume_id,
                    path_raw: r.path_raw,
                });
            } else {
                self.mensagem(format!(
                    "raiz ignorada (volume desconectado ou indisponível): {}",
                    caminho.display()
                ));
            }
        }
        Ok(out)
    }

    /// Monitoramento em primeiro plano até o cancelamento do [`ControleScan`] do agente
    /// (Ctrl-C). Só o escritor monitora: um segundo agente nem chega aqui (trava).
    fn monitorar(
        &mut self,
        raizes: &[String],
        reconciliar_a_cada_s: Option<u64>,
        sem_hash: bool,
    ) -> Result<Resposta, ErroAgente> {
        let raizes: Vec<PathBuf> = if raizes.is_empty() {
            self.raizes_padrao()?
        } else {
            raizes.iter().map(PathBuf::from).collect()
        };
        let scan = self.config_scan()?;
        let mut cfg = ConfigMonitor {
            exclusoes: scan.exclusoes.clone(),
            scan,
            ..ConfigMonitor::default()
        };
        match reconciliar_a_cada_s {
            Some(0) => cfg.reconciliar_a_cada = None,
            Some(s) => cfg.reconciliar_a_cada = Some(std::time::Duration::from_secs(s)),
            None => {}
        }
        let fixos = self.volumes_fixos.clone();
        let mut monitor = Monitor::novo(raizes, move || match &fixos {
            Some(v) => v.clone(),
            None => PlataformaLinux.descobrir_volumes().unwrap_or_default(),
        })
        .com_config(cfg)
        .com_classificador(Arc::new(ClassificadorPorExtensao));
        if !sem_hash {
            let autor = self.autor;
            let agente = self.opcoes_hash;
            let cofre = Arc::clone(&self.cofre);
            monitor = monitor.com_pos_reconciliacao(move |cat, execucoes| {
                pos_reconciliacao(cat, execucoes, autor, agente, &cofre).map_err(|e| e.to_string())
            });
        }
        self.mensagem("monitorando (Ctrl-C para encerrar)...");
        let inicio = Instant::now();
        let r = monitor.executar(&mut self.catalogo, &self.controle)?;
        Ok(Resposta::MonitorEncerrado {
            resumo: ResumoMonitorInfo {
                execucao: Some(ExecucaoId(r.execucao_id)),
                lotes: r.lotes,
                diretorios: r.diretorios,
                criados: r.criados,
                alterados: r.alterados,
                removidos: r.removidos,
                renomeados: r.renomeados,
                lacunas: r.lacunas,
                overflows: r.overflows,
                fast_scans: r.fast_scans,
                watches: r.watches,
                duracao_ms: u64::try_from(inicio.elapsed().as_millis()).unwrap_or(u64::MAX),
            },
        })
    }

    /// Estado final gravado da execução (`concluida*`); `padrao` se ainda não terminou.
    fn estado_gravado(&self, execucao: i64, padrao: &'static str) -> &'static str {
        let estado: Option<String> = self
            .catalogo
            .armazem()
            .conexao()
            .query_row(
                "SELECT status FROM scan_runs WHERE id = ?1",
                [execucao],
                |l| l.get(0),
            )
            .ok();
        match estado.as_deref() {
            Some("concluida") => "concluida",
            Some("concluida_com_avisos") => "concluida_com_avisos",
            Some("concluida_com_erros") => "concluida_com_erros",
            _ => padrao,
        }
    }

    /// Erros da execução (padrão: a última terminada): por desfecho, código, pasta e amostras.
    fn erros_da_varredura(
        &self,
        execucao: Option<ExecucaoId>,
        agrupar: dag_core::erros::AgruparErros,
        codigo: Option<&str>,
        pasta: Option<&str>,
        amostras: Option<u32>,
    ) -> Result<Resposta, ErroAgente> {
        let conn = self.catalogo.armazem().conexao();
        let id = match execucao {
            Some(e) => e.0,
            None => erros_db::ultima_execucao_terminada(conn)?
                .ok_or_else(|| ErroAgente::Invalido("nenhuma varredura terminada".into()))?,
        };
        let existe: bool = conn
            .query_row("SELECT COUNT(*) FROM scan_runs WHERE id = ?1", [id], |l| {
                l.get::<_, i64>(0)
            })
            .map(|n| n > 0)?;
        if !existe {
            return Err(ErroAgente::Invalido(format!("não há execução {id}")));
        }
        let mut resumo = erros_db::resumir(
            conn,
            id,
            &erros_db::OpcoesResumo {
                codigo,
                pasta,
                amostras: amostras.map(|n| n.min(50) as usize),
                pastas: None,
            },
            erros_db::pasta_home().as_deref(),
        )?;
        resumo.agrupar = agrupar;
        Ok(Resposta::ErrosDaVarredura { resumo })
    }

    /// Regras embutidas, regras aceitas e exclusões do usuário.
    fn erros_regras(&self) -> Result<Resposta, ErroAgente> {
        let conn = self.catalogo.armazem().conexao();
        Ok(Resposta::ErrosRegras {
            embutidas: dag_core::erros::REGRAS_EMBUTIDAS
                .iter()
                .map(dag_core::erros::RegraEmbutida::info)
                .collect(),
            aceitas: erros_db::listar_regras(conn)?,
            exclusoes: erros_db::listar_exclusoes(conn)?,
        })
    }

    fn eventos(&self, limite: Option<u32>) -> Result<Resposta, ErroAgente> {
        let l = limite.unwrap_or(PAGINA_PADRAO).clamp(1, PAGINA_MAXIMA);
        let eventos = reconciliacao::listar_eventos(self.catalogo.armazem().conexao(), l)?
            .into_iter()
            .map(|e| EventoInfo {
                id: e.id,
                tipo: e.tipo,
                fonte: e.fonte,
                volume: e.volume_id.map(VolumeId),
                arquivo: e.arquivo_id.map(Into::into),
                execucao: e.execucao_id.map(ExecucaoId),
                caminho: e.caminho,
                caminho_antigo: e.caminho_antigo,
                observado_em: e.observado_em,
                processado_em: e.processado_em,
                reconciliar: e.reconciliar,
                detalhe: e.detalhe,
            })
            .collect();
        Ok(Resposta::Eventos { eventos })
    }

    /// Saúde persistida do monitor (visível de qualquer processo) e reconciliação pendente.
    fn saude_monitor(&self) -> Result<SaudeMonitorInfo, ErroAgente> {
        let conn = self.catalogo.armazem().conexao();
        let s = reconciliacao::ler_saude(conn)?;
        let p = reconciliacao::pendencias(conn)?;
        let vivo = s.estado != "parado"
            && s.heartbeat
                .is_some_and(|h| agora().saturating_sub(h) <= HEARTBEAT_VALIDO_S);
        Ok(SaudeMonitorInfo {
            situacao: if vivo { "ativo" } else { "parado" }.into(),
            situacao_motivo: s.erro.clone().filter(|_| !vivo),
            estado: s.estado,
            em_execucao: vivo,
            iniciado_em: s.iniciado_em,
            heartbeat: s.heartbeat,
            backlog: s.backlog,
            ultimo_evento_em: s.ultimo_evento_em,
            ultima_reconciliacao: s.ultima_reconciliacao,
            watches: s.watches,
            watches_limite: s.watches_limite,
            overflows: s.overflows,
            eventos_processados: s.eventos_processados,
            raizes_parciais: s.raizes_parciais,
            raizes: s.raizes,
            erro: s.erro,
            lacunas_abertas: p.lacunas_abertas,
            eventos_nao_processados: p.eventos_nao_processados,
        })
    }

    /// Retoma uma execução gravada; o paralelismo do hash é o vigente agora (pedido > agente >
    /// persistido > padrão), não o da execução original.
    pub(crate) fn retomar_scan(
        &mut self,
        id: ExecucaoId,
        opcoes: OpcoesHash,
    ) -> Result<Resposta, ErroAgente> {
        self.config_hash(opcoes)?;
        let scan = self.novo_scan(Vec::new(), false)?;
        let r = scan.retomar(&mut self.catalogo, id.0)?;
        self.concluir_scan(r, false, false, opcoes)
    }

    /// Pós-inventário: classificação por caminho, Hash Completo, reconstrução dos Grupos de
    /// Duplicados e reavaliação das Marcações (estas três só sem `sem_hash`).
    ///
    /// `releitura_total`: Full Scan pedido de propósito, que lê o conteúdo de todos os arquivos
    /// de novo (os hashes guardados deixam de valer); os demais caminhos só leem o que é novo ou
    /// mudou.
    fn concluir_scan(
        &mut self,
        r: ResultadoScan,
        sem_hash: bool,
        releitura_total: bool,
        opcoes: OpcoesHash,
    ) -> Result<Resposta, ErroAgente> {
        let mut desfecho = nome_desfecho(r.desfecho);
        let mut erros = r.contadores.erros;
        let mut duracao = r.duracao;
        let (mut hash, mut dup) = (None, None);
        if r.desfecho != DesfechoScan::Cancelada {
            self.mensagem("classificando finalidade (por caminho)...");
            classificar_execucao(&mut self.catalogo, r.execucao_id, agora())?;
            self.marcar_memoria("inventário e classificação");
            // A varredura visitou a DAG-Lixeira: reconhece o que o usuário pôs, tirou ou esvaziou.
            let lixeira = self.reconciliar_lixeira()?;
            if !sem_hash {
                if releitura_total {
                    hashes::invalidar_evidencia(self.catalogo.armazem().conexao(), r.execucao_id)?;
                }
                let cfg = self.config_hash(opcoes)?;
                let mut etapa = EtapaHash::nova(ExecucaoId(r.execucao_id))
                    .com_config(cfg)
                    .com_controle(self.controle.clone());
                if let Some(p) = &self.progresso {
                    etapa = etapa.com_progresso(sink_hash(Arc::clone(p), false));
                }
                let h = etapa.executar(&mut self.catalogo)?;
                self.marcar_memoria("hash");
                duracao += h.duracao;
                erros += h.erros;
                let cancelada = h.desfecho == DesfechoScan::Cancelada;
                hash = Some(ResumoHash {
                    desfecho: nome_desfecho(h.desfecho).to_string(),
                    hasheados: h.hasheados,
                    reaproveitados: h.reaproveitados,
                    bytes_lidos: h.bytes_lidos,
                    erros: h.erros,
                    duracao_ms: u64::try_from(h.duracao.as_millis()).unwrap_or(u64::MAX),
                });
                // Conteúdo de contêineres: nova etapa depois do Hash Completo (ADR 0006).
                let mut conteineres_cancelada = false;
                if !cancelada {
                    let c = self.etapa_conteineres()?;
                    duracao += c.duracao;
                    conteineres_cancelada = c.desfecho == DesfechoScan::Cancelada;
                }
                if cancelada || conteineres_cancelada {
                    desfecho = "cancelada";
                } else {
                    // O estado final (`concluida`, `_com_avisos` ou `_com_erros`) já foi gravado
                    // pela etapa de hash, depois do reteste e da classificação.
                    desfecho = self.estado_gravado(r.execucao_id, desfecho);
                    // Passada de verificação sem mudança e sem hash novo: os grupos continuam
                    // valendo (evita reconstruí-los a cada abertura). Mas hashes que mudaram
                    // desde a última reconstrução completa (ou grupos nunca construídos) sempre
                    // reconstroem: vale para a retomada do hash e para a abertura do agente.
                    let nada_mudou = h.hasheados == 0
                        && !lixeira.mudou_copias()
                        && r.reconciliacao
                            .as_ref()
                            .map(resumo_reconciliacao)
                            .is_some_and(|x| x.sem_mudancas());
                    if !nada_mudou
                        || hashes::grupos_desatualizados(self.catalogo.armazem().conexao())?
                    {
                        dup = Some(self.atualizar_duplicados()?);
                    }
                }
            }
        }
        Ok(Resposta::ScanConcluido {
            execucao: ExecucaoId(r.execucao_id),
            desfecho: desfecho.to_string(),
            pastas: r.contadores.pastas,
            arquivos: r.contadores.arquivos,
            especiais: r.contadores.especiais,
            bytes_logicos: r.contadores.bytes_logicos,
            ignorados: r.contadores.ignorados,
            erros,
            duracao_ms: u64::try_from(duracao.as_millis()).unwrap_or(u64::MAX),
            hash,
            duplicados: dup,
            reconciliacao: r.reconciliacao.as_ref().map(resumo_reconciliacao),
        })
    }

    /// Reconstrói os Grupos de Duplicados e reavalia as Marcações.
    pub(crate) fn atualizar_duplicados(&mut self) -> Result<ResumoDuplicados, ErroAgente> {
        self.mensagem(
            "[duplicados] Agrupando duplicados (pode levar minutos em Catálogos grandes)...",
        );
        let t = agora();
        // Hashes novos podem liberar Marcações de quem está Na Lixeira; a cópia lá não conta.
        self.reconciliar_lixeira()?;
        duplicados::reconstruir_grupos(&mut self.catalogo, Escopo::Tudo, self.autor, t)?;
        self.marcar_memoria("grupos de duplicados");
        self.mensagem("[marcações] reavaliando Marcações contra o hash atual...");
        let pendentes = duplicados::reavaliar_marcacoes(&mut self.catalogo, self.autor, t)?;
        let mut r = duplicados::resumo_duplicados(self.catalogo.armazem().conexao())?;
        if pendentes > 0 {
            self.mensagem(format!(
                "[marcações] {pendentes} Marcação(ões) ficaram pendentes (conteúdo mudou); confirme com `dag confirmar`."
            ));
        }
        r.marcacoes_pendentes = r.marcacoes_pendentes.max(pendentes);
        Ok(r)
    }

    // ------------------------------------------------------------------ duplicados e marcações

    fn listar_duplicados(
        &self,
        volume: Option<&str>,
        tamanho_minimo: Option<u64>,
        ordem: Option<OrdemDuplicados>,
        limite: Option<u32>,
        cursor: Option<String>,
    ) -> Result<Resposta, ErroAgente> {
        let conn = self.catalogo.armazem().conexao();
        let filtro = FiltroGrupos {
            volume: duplicados::filtro_por_volume(conn, volume)?,
            tamanho_minimo: tamanho_minimo.unwrap_or(0),
            ordem: ordem.unwrap_or_default(),
        };
        let p = duplicados::listar_grupos(
            conn,
            &filtro,
            limite.unwrap_or(PAGINA_PADRAO).min(PAGINA_MAXIMA),
            cursor.as_deref(),
        )?;
        Ok(Resposta::Duplicados {
            grupos: p.grupos,
            total: p.total,
            proximo_cursor: p.proximo_cursor,
        })
    }

    fn marcar(&mut self, tipo: TipoMarcacao, alvo: &Alvo) -> Result<Resposta, ErroAgente> {
        Ok(Resposta::Marcacao {
            resultado: duplicados::marcar(&mut self.catalogo, tipo, alvo, self.autor, agora())?,
        })
    }

    fn relatorio(
        &self,
        tipo: TipoRelatorio,
        nivel: Option<dag_core::NivelEspaco>,
        limite: Option<u32>,
        cursor: Option<&str>,
        saida: Option<&str>,
    ) -> Result<Resposta, ErroAgente> {
        let conn = self.catalogo.armazem().conexao();
        if let Some(destino) = saida {
            let (formato, linhas) =
                duplicados::exportar_relatorio(conn, tipo, Path::new(destino), agora())?;
            return Ok(Resposta::RelatorioExportado {
                tipo,
                caminho: destino.to_string(),
                formato,
                linhas,
            });
        }
        let limite = limite.unwrap_or(PAGINA_PADRAO).min(PAGINA_MAXIMA);
        let (dados, proximo_cursor) = match tipo {
            TipoRelatorio::Cobertura => {
                duplicados::relatorio_cobertura(conn, None, limite, cursor)?
            }
            TipoRelatorio::SemCobertura => {
                duplicados::relatorio_sem_cobertura(conn, None, limite, cursor)?
            }
            TipoRelatorio::Espaco => {
                duplicados::relatorio_espaco(conn, nivel.unwrap_or_default(), limite, cursor)?
            }
            TipoRelatorio::Marcacoes => {
                let depois = cursor
                    .map(|c| {
                        c.parse::<i64>()
                            .map_err(|_| ErroAgente::Invalido(format!("cursor inválido: `{c}`")))
                    })
                    .transpose()?;
                let (marcacoes, prox) = duplicados::listar_marcacoes(conn, depois, limite)?;
                (
                    dag_core::DadosRelatorio::Marcacoes { marcacoes },
                    prox.map(|p| p.to_string()),
                )
            }
        };
        Ok(Resposta::Relatorio {
            dados,
            proximo_cursor,
        })
    }

    /// Consulta (sem campos) ou grava o paralelismo padrão do hash no Catálogo. Vale a partir
    /// da próxima execução ou retomada; a que está rodando mantém o valor com que começou.
    fn configurar_hash(&mut self, novas: OpcoesHash) -> Result<Resposta, ErroAgente> {
        novas.validar().map_err(ErroAgente::Invalido)?;
        let con = self.catalogo.armazem().conexao();
        if !novas.vazia() {
            configuracao::gravar_hash(con, &novas)?;
        }
        let p = configuracao::ler_hash(con)?
            .resolver()
            .map_err(ErroAgente::Invalido)?;
        Ok(Resposta::ConfigHash {
            threads_por_disco: p.threads_por_disco,
            max_discos: p.max_discos,
        })
    }

    /// Verificação de Integridade: relê arquivos já hasheados e compara com o hash guardado.
    fn verificar(&mut self, raizes: &[String], opcoes: OpcoesHash) -> Result<Resposta, ErroAgente> {
        let cfg = self.config_hash(opcoes)?;
        let mut v = VerificacaoIntegridade::nova()
            .com_config(cfg)
            .com_controle(self.controle.clone());
        if !raizes.is_empty() {
            v = v.raizes(raizes.iter().map(PathBuf::from));
        }
        if let Some(p) = &self.progresso {
            v = v.com_progresso(sink_hash(Arc::clone(p), true));
        }
        let r = v.executar(&mut self.catalogo)?;
        Ok(Resposta::Verificacao {
            resumo: ResumoVerificacao {
                desfecho: nome_desfecho(r.desfecho).to_string(),
                verificados: r.verificados,
                divergentes: r.divergentes.len() as u64,
                arquivos_divergentes: r.divergentes,
                alterados: r.alterados,
                bytes_lidos: r.bytes_lidos,
                erros: r.erros,
                duracao_ms: u64::try_from(r.duracao.as_millis()).unwrap_or(u64::MAX),
            },
        })
    }

    // ------------------------------------------------------------------ status

    fn status(&self, execucao: Option<ExecucaoId>) -> Result<Resposta, ErroAgente> {
        let ruido = self.volumes_ruido_registrados()?;
        let conn = self.catalogo.armazem().conexao();
        let ids: Vec<i64> = match execucao {
            Some(id) => vec![id.0],
            None => {
                let mut st = conn.prepare(
                    "SELECT id FROM scan_runs WHERE mode IN ('full','fast') ORDER BY id DESC LIMIT ?1",
                )?;
                st.query_map([STATUS_LIMITE], |l| l.get(0))?
                    .collect::<Result<_, _>>()?
            }
        };
        let mut execucoes = Vec::new();
        for id in ids {
            let e = inventario::obter_execucao(conn, id)?
                .ok_or_else(|| ErroAgente::NaoEncontrado(format!("execução {id} não existe")))?;
            let mut st = status_de(&e);
            if e.fase == FaseScan::Hash {
                aplicar_status_hash(conn, &mut st)?;
            }
            execucoes.push(st);
        }
        let (pastas, arquivos, bytes): (i64, i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(folder_count),0), COALESCE(SUM(file_count),0),
                    COALESCE(SUM(logical_bytes),0) FROM volume_stats",
            [],
            |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)),
        )?;
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM volumes", [], |l| l.get(0))?;
        let com_hash = hashes::total_com_hash(conn)?;
        let volumes = total - i64::try_from(ruido.len()).unwrap_or(0);
        let n = |x: i64| u64::try_from(x).unwrap_or(0);
        Ok(Resposta::Status {
            execucoes,
            catalogo: ResumoCatalogo {
                volumes: n(volumes),
                pastas: n(pastas),
                arquivos: n(arquivos),
                bytes_logicos: n(bytes),
                arquivos_com_hash: com_hash,
            },
            monitor: Some(self.saude_monitor()?),
            confianca: self.status_confianca()?,
        })
    }

    /// Saúde do monitor e Ponto de Confiança, para o fluxo de eventos do agente.
    pub(crate) fn monitor_e_confianca(
        &self,
    ) -> Result<(SaudeMonitorInfo, Vec<StatusConfianca>), ErroAgente> {
        Ok((self.saude_monitor()?, self.status_confianca()?))
    }

    /// Ponto de Confiança por Raiz.
    fn status_confianca(&self) -> Result<Vec<StatusConfianca>, ErroAgente> {
        Ok(confianca::listar(self.catalogo.armazem().conexao())?
            .into_iter()
            .map(|r| StatusConfianca {
                raiz: r.path_display,
                volume: VolumeId(r.volume_id),
                confiavel_em: r.confiavel_em,
                perdida_em: r.perdido_em,
                motivo_perda: r.motivo_perda,
            })
            .collect())
    }

    // ------------------------------------------------------------------ backup

    fn backup(&mut self, destino: &Path) -> Result<Resposta, ErroAgente> {
        self.catalogo.copiar_para(destino)?;
        // Dado privado também na cópia.
        std::fs::set_permissions(destino, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| ErroAgente::es(format!("permissões de {}", destino.display()), e))?;
        let resultado = {
            let c = Connection::open_with_flags(destino, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let linhas: Vec<String> = c
                .prepare("PRAGMA integrity_check")?
                .query_map([], |l| l.get(0))?
                .collect::<Result<_, _>>()?;
            linhas.join("; ")
        };
        let t = agora();
        self.catalogo.armazem().conexao().execute(
            "INSERT INTO catalog_snapshots (created_at, backup_path, integrity_result)
             VALUES (?1, ?2, ?3)",
            params![t, destino.display().to_string(), resultado],
        )?;
        if resultado != "ok" {
            return Err(ErroAgente::Invalido(format!(
                "backup gravado em {}, mas a verificação de integridade falhou: {resultado}",
                destino.display()
            )));
        }
        Ok(Resposta::Ok {
            mensagem: Some(format!(
                "backup do catálogo em {} (integridade: ok)",
                destino.display()
            )),
        })
    }
}

/// Abre o Catálogo (como escritor, ou só leitura para `Status`/`Explorar`) e executa um comando.
/// Erros de abertura (inclusive “outro processo do DAG já é o escritor”) viram [`Resposta::Erro`].
pub fn executar_comando(caminho: &Path, comando: Comando) -> Resposta {
    // O serviço não precisa do Catálogo (e pode estar rodando como escritor dele).
    if let Comando::Servico { acao } = &comando {
        return crate::servico::executar_avulso(*acao, caminho);
    }
    if let Some(n) = incremento_pendente(&comando) {
        return ErroAgente::NaoImplementado(n).para_resposta();
    }
    let agente = if somente_leitura(&comando) {
        Agente::abrir_leitura(caminho)
    } else {
        Agente::abrir(caminho)
    };
    match agente {
        Ok(mut a) => a.executar(comando),
        Err(e) => e.para_resposta(),
    }
}

/// Hash Completo das execuções que carimbaram linhas novas/alteradas (só lê o que mudou),
/// reconstrução dos Grupos de Duplicados e reavaliação das Marcações (viram Pendentes se o
/// conteúdo mudou). Usada pelo monitor quando fica quieto.
fn pos_reconciliacao(
    cat: &mut Catalogo,
    execucoes: &[i64],
    autor: Autor,
    agente: OpcoesHash,
    cofre: &Arc<CofreLazy>,
) -> Result<(), ErroAgente> {
    for id in execucoes {
        // Lê o persistido a cada passada: mudança de configuração vale na próxima.
        let cfg = config_hash_efetiva(cat.armazem().conexao(), OpcoesHash::default(), agente)?;
        EtapaHash::nova(ExecucaoId(*id))
            .com_config(cfg)
            .executar(cat)?;
    }
    // Contêineres novos ou alterados: lê o conteúdo (retomável; cifrados viram Pedido de Senha).
    let cofre: Arc<dyn dag_scanner::Cofre> = cofre.clone();
    dag_scanner::EtapaConteineres::nova()
        .com_cofre(cofre)
        .executar(cat)?;
    let t = agora();
    let lixeira = dag_store::lixeira::reconciliar(cat, autor, t)?;
    if lixeira.mudou_copias() || hashes::grupos_desatualizados(cat.armazem().conexao())? {
        duplicados::reconstruir_grupos(cat, Escopo::Tudo, autor, t)?;
    }
    duplicados::reavaliar_marcacoes(cat, autor, t)?;
    Ok(())
}

/// Precedência pedido > agente > persistido > padrão, já no formato do scanner.
fn config_hash_efetiva(
    conn: &Connection,
    pedido: OpcoesHash,
    agente: OpcoesHash,
) -> Result<ConfigHash, ErroAgente> {
    let persistido = configuracao::ler_hash(conn)?;
    let p =
        dag_core::resolver_precedencia(pedido, agente, persistido).map_err(ErroAgente::Invalido)?;
    Ok(ConfigHash {
        threads_por_disco: p.threads_por_disco as usize,
        max_discos: p.max_discos as usize,
        ..ConfigHash::default()
    })
}

fn resumo_reconciliacao(r: &dag_scanner::ResumoFast) -> ResumoReconciliacao {
    ResumoReconciliacao {
        arquivos_novos: r.arquivos_novos,
        arquivos_alterados: r.arquivos_alterados,
        arquivos_inalterados: r.arquivos_inalterados,
        pastas_novas: r.pastas_novas,
        arquivos_renomeados: r.arquivos_renomeados,
        pastas_renomeadas: r.pastas_renomeadas,
        arquivos_ausentes: r.arquivos_ausentes,
        pastas_ausentes: r.pastas_ausentes,
        bloqueios: r.bloqueios,
        remocao_aplicada: r.remocao_aplicada,
        motivo_sem_remocao: r.motivo_sem_remocao.clone(),
        eventos_omitidos: r.eventos_omitidos,
    }
}

/// Resumo de um volume descoberto que não está no Catálogo (`id` 0).
fn info_descoberto(d: &VolumeDescoberto) -> VolumeInfo {
    VolumeInfo {
        id: VolumeId(0),
        dispositivo: None,
        tipo_dispositivo: d.dispositivo.as_ref().map(|x| x.tipo),
        fs_type: Some(d.fs_type.clone()),
        rotulo: d.rotulo.clone(),
        ponto_montagem: d
            .pontos_de_montagem
            .iter()
            .min_by_key(|p| p.len())
            .map(|p| String::from_utf8_lossy(p).into_owned()),
        conectado: true,
        capacidade_bytes: d.capacidade_bytes,
        livre_bytes: d.livre_bytes,
        classe: d.classe,
        sistema: d.sistema,
    }
}

fn tipo_dispositivo(s: &str) -> Option<TipoDispositivo> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

/// Janela (segundos) sem atividade na fila de hash a partir da qual a etapa é dada como parada.
const HASH_INATIVO_SEG: i64 = 600;
/// Janela da vazão recente mostrada no status.
const HASH_VAZAO_SEG: i64 = 60;

/// Completa o status de uma execução na fase `hash` com o andamento da fila persistente.
fn aplicar_status_hash(conn: &Connection, st: &mut StatusExecucao) -> Result<(), ErroAgente> {
    let agora = agora();
    let f = hashes::andamento_fila(conn, hashes::TipoFila::Hash, agora - HASH_VAZAO_SEG)?;
    let mut h = StatusHash {
        em_andamento: false,
        arquivos_hasheados: 0,
        arquivos_pendentes: 0,
        arquivos_com_erro: 0,
        bytes_hasheados: 0,
        bytes_pendentes: 0,
        bytes_por_seg: None,
        discos: Vec::new(),
    };
    for d in &f.discos {
        h.arquivos_hasheados += d.arquivos_feitos;
        h.arquivos_pendentes += d.arquivos_pendentes;
        h.arquivos_com_erro += d.arquivos_com_erro;
        h.bytes_hasheados += d.bytes_feitos;
        h.bytes_pendentes += d.bytes_pendentes;
        h.discos.push(StatusDiscoHash {
            disco: d.nome.clone(),
            volumes: d.volume_ids.clone(),
            arquivos_hasheados: d.arquivos_feitos,
            arquivos_pendentes: d.arquivos_pendentes,
            arquivos_com_erro: d.arquivos_com_erro,
            bytes_hasheados: d.bytes_feitos,
            bytes_pendentes: d.bytes_pendentes,
            lendo: d
                .lendo
                .iter()
                .map(|a| dag_core::LeituraEmCurso {
                    caminho: a.caminho.clone(),
                    bytes_lidos: a.bytes_lidos,
                    bytes_total: a.bytes_total,
                })
                .collect(),
        });
    }
    // Atividade = arquivo concluído, reserva de lote ou batimento de bytes de um arquivo grande
    // em leitura (`updated_at` só avança quando algo mudou). Um único arquivo de centenas de GB
    // levando mais que a janela não pode parecer parado.
    let lendo_agora = h.discos.iter().any(|d| !d.lendo.is_empty());
    let recente = lendo_agora
        || f.ultima_atividade
            .is_some_and(|t| agora - t <= HASH_INATIVO_SEG);
    h.em_andamento = h.arquivos_pendentes > 0 && recente;
    if h.em_andamento {
        // Só os concluídos na janela são vazão medida; sem agente respondendo, o arquivo grande
        // em leitura não tem vazão (o batimento guarda bytes acumulados, não uma taxa).
        let por_seg = f.bytes_recentes / HASH_VAZAO_SEG as u64;
        h.bytes_por_seg = (por_seg > 0 || !lendo_agora).then_some(por_seg);
    }
    if h.arquivos_pendentes > 0 {
        if h.em_andamento {
            st.estado = EstadoExecucao::EmAndamento;
            st.estado_detalhe = "hash_em_andamento".into();
        } else {
            st.estado = EstadoExecucao::Pausada;
            st.estado_detalhe = "hash_parado".into();
        }
    } else {
        st.estado_detalhe = "hash_finalizando".into();
    }
    st.erros += h.arquivos_com_erro;
    st.hash = Some(h);
    Ok(())
}

/// Sobrepõe ao status do Catálogo o que o agente sabe de verdade em memória: com o trabalho de
/// hash da execução rodando (ou pausado) aqui, o progresso vivo manda. A heurística do Catálogo
/// (atividade recente na fila) só vale para quem não consegue perguntar ao agente.
pub(crate) fn sobrepor_status_ao_vivo(resp: &mut Resposta, jobs: &[dag_core::JobInfo]) {
    let Resposta::Status { execucoes, .. } = resp else {
        return;
    };
    for e in execucoes {
        let Some(h) = e.hash.as_mut() else { continue };
        let vivo = jobs.iter().find_map(|j| {
            let (p, verificacao) = j.hash_atual()?;
            let ativo = matches!(
                j.estado,
                dag_core::EstadoJob::Executando
                    | dag_core::EstadoJob::Pausado
                    | dag_core::EstadoJob::Cancelando
            );
            (!verificacao && ativo && p.execucao.or(j.execucao) == Some(e.id.0)).then_some((j, p))
        });
        let Some((j, p)) = vivo else { continue };
        let pausado = j.estado == dag_core::EstadoJob::Pausado;
        h.em_andamento = !pausado;
        if pausado {
            h.bytes_por_seg = None;
        } else {
            h.bytes_por_seg = Some(p.bytes_por_seg.max(0.0) as u64);
            if h.arquivos_pendentes > 0 {
                e.estado = EstadoExecucao::EmAndamento;
                e.estado_detalhe = "hash_em_andamento".into();
            }
        }
        for d in &mut h.discos {
            d.lendo = p
                .discos
                .iter()
                .filter(|pd| pd.volumes.iter().any(|v| d.volumes.contains(v)))
                .flat_map(|pd| pd.lendo.iter().cloned())
                .collect();
        }
    }
}

fn status_de(e: &inventario::Execucao) -> StatusExecucao {
    let c: serde_json::Value = serde_json::from_str(&e.contadores).unwrap_or_default();
    let n = |k: &str| c.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
    let (pastas, arquivos, especiais) = (n("pastas"), n("arquivos"), n("especiais"));
    let raizes: Vec<String> = serde_json::from_str(&e.raizes).unwrap_or_default();
    StatusExecucao {
        id: ExecucaoId(e.id),
        modo: if e.modo == "fast" {
            ModoVarredura::FastScan
        } else {
            ModoVarredura::FullScan
        },
        estado: match e.estado {
            EstadoScan::EmAndamento => EstadoExecucao::EmAndamento,
            // Interrompida (processo morto) é retomável, como a pausada.
            EstadoScan::Pausada | EstadoScan::Interrompida => EstadoExecucao::Pausada,
            EstadoScan::Concluida
            | EstadoScan::ConcluidaComAvisos
            | EstadoScan::ConcluidaComErros => EstadoExecucao::Concluida,
            EstadoScan::Cancelada => EstadoExecucao::Cancelada,
            EstadoScan::Falhou => EstadoExecucao::Falhou,
        },
        etapa: match e.fase {
            FaseScan::Inventario => Some(EtapaVarredura::Inventario),
            FaseScan::Concluida => None,
            FaseScan::Hash | FaseScan::Verificacao => Some(EtapaVarredura::Hash),
            FaseScan::Estatisticas => Some(EtapaVarredura::Agregacao),
        },
        itens_processados: pastas + arquivos + especiais,
        erros: n("erros"),
        estado_detalhe: e.estado.como_str().to_string(),
        raizes,
        iniciada_em: e.iniciada_em,
        encerrada_em: e.encerrada_em,
        pastas,
        arquivos,
        especiais,
        bytes_logicos: n("bytes_logicos"),
        ignorados: n("ignorados"),
        hash: None,
        retomando: e.retomada_automatica,
    }
}

fn nome_desfecho(d: DesfechoScan) -> &'static str {
    d.nome()
}

// ---------------------------------------------------------------------- progresso

fn inventario_de(p: &ProgressoScanner) -> ProgressoInventario {
    ProgressoInventario {
        execucao: ExecucaoId(p.execucao_id),
        fase: match p.fase {
            FaseProgresso::Inventario => FaseInventario::Inventario,
            FaseProgresso::Pausado => FaseInventario::Pausado,
            FaseProgresso::Agregacao => FaseInventario::Agregacao,
            FaseProgresso::Concluido => FaseInventario::Concluido,
        },
        pastas: p.pastas,
        arquivos: p.arquivos,
        especiais: p.especiais,
        bytes_logicos: p.bytes_logicos,
        ignorados: p.ignorados,
        erros: p.erros,
        diretorios_pendentes: p.diretorios_pendentes,
        decorrido_ms: u64::try_from(p.decorrido.as_millis()).unwrap_or(u64::MAX),
        entradas_por_segundo: p.entradas_por_segundo,
    }
}

/// `(VmRSS, VmHWM)` em KiB do próprio processo (Linux); `None` se `/proc` não responde.
pub(crate) fn memoria_do_processo() -> Option<(u64, u64)> {
    let t = std::fs::read_to_string("/proc/self/status").ok()?;
    let campo = |nome: &str| -> Option<u64> {
        t.lines()
            .find(|l| l.starts_with(nome))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    };
    Some((campo("VmRSS:")?, campo("VmHWM:")?))
}

/// Adapta o progresso da etapa de hash (ou da verificação) ao [`Progresso`] do contrato.
fn sink_hash(
    p: Arc<dyn Fn(&Progresso) + Send + Sync>,
    verificacao: bool,
) -> impl FnMut(&ProgressoHashScanner) + Send + 'static {
    move |h: &ProgressoHashScanner| {
        let ev = dag_core::ProgressoHash {
            arquivos_feitos: h.arquivos_feitos,
            arquivos_total: h.arquivos_total,
            bytes_lidos: h.bytes_lidos,
            bytes_total: h.bytes_total,
            bytes_por_seg: h.bytes_por_seg,
            reaproveitados: h.reaproveitados,
            execucao: Some(h.execucao),
            threads_por_disco: h.threads_por_disco,
            max_discos: h.max_discos,
            erros: h.erros,
            arquivos_por_seg: h.arquivos_por_seg,
            janela_ms: h.janela_ms,
            decorrido_ms: h.decorrido_ms,
            preparando: h.preparando,
            finalizado: h.finalizado,
            discos: h
                .discos
                .iter()
                .map(|d| dag_core::ProgressoDiscoHash {
                    disco: d.disco.clone(),
                    volumes: d.volumes.clone(),
                    threads: d.threads,
                    leitores_ativos: d.leitores_ativos,
                    arquivos_feitos: d.arquivos_feitos,
                    arquivos_total: d.arquivos_total,
                    bytes_lidos: d.bytes_lidos,
                    bytes_total: d.bytes_total,
                    bytes_por_seg: d.bytes_por_seg,
                    ativo: d.ativo,
                    concluido: d.concluido,
                    lendo: d.lendo.clone(),
                })
                .collect(),
        };
        p(&if verificacao {
            Progresso::Verificacao(ev)
        } else {
            Progresso::Hash(ev)
        });
    }
}

#[cfg(test)]
mod testes_status_ao_vivo {
    use super::*;
    use dag_core::{
        EstadoJob, JobInfo, LeituraEmCurso, OrigemJob, ProgressoDiscoHash, ProgressoHash as Ph,
        TipoJob,
    };

    fn status(parado: bool) -> Resposta {
        Resposta::Status {
            execucoes: vec![StatusExecucao {
                id: ExecucaoId(7),
                modo: ModoVarredura::FullScan,
                estado: EstadoExecucao::Pausada,
                etapa: Some(EtapaVarredura::Hash),
                itens_processados: 0,
                erros: 0,
                estado_detalhe: "hash_parado".into(),
                raizes: vec![],
                iniciada_em: 0,
                encerrada_em: None,
                pastas: 0,
                arquivos: 0,
                especiais: 0,
                bytes_logicos: 0,
                ignorados: 0,
                hash: Some(StatusHash {
                    em_andamento: !parado,
                    arquivos_hasheados: 0,
                    arquivos_pendentes: 1,
                    arquivos_com_erro: 0,
                    bytes_hasheados: 0,
                    bytes_pendentes: 205 << 30,
                    bytes_por_seg: None,
                    discos: vec![StatusDiscoHash {
                        disco: "Disco".into(),
                        volumes: vec![3],
                        arquivos_hasheados: 0,
                        arquivos_pendentes: 1,
                        arquivos_com_erro: 0,
                        bytes_hasheados: 0,
                        bytes_pendentes: 205 << 30,
                        lendo: vec![],
                    }],
                }),
                retomando: false,
            }],
            catalogo: ResumoCatalogo::default(),
            monitor: None,
            confianca: vec![],
        }
    }

    fn job(estado: EstadoJob) -> JobInfo {
        let mut j = JobInfo {
            id: 1,
            tipo: TipoJob::Retomada,
            origem: OrigemJob::Cli,
            raizes: vec![],
            estado,
            execucao: Some(7),
            etapa: None,
            progresso: vec![],
            criado_em: 0,
            passo: None,
            agente_pid: None,
        };
        j.registrar_progresso(&Progresso::Hash(Ph {
            execucao: Some(7),
            bytes_por_seg: 220.0 * 1_048_576.0,
            discos: vec![ProgressoDiscoHash {
                disco: "Disco".into(),
                volumes: vec![3],
                lendo: vec![LeituraEmCurso {
                    caminho: "/x/grande.img".into(),
                    bytes_lidos: 30 << 30,
                    bytes_total: 205 << 30,
                }],
                ..ProgressoDiscoHash::default()
            }],
            ..Ph::default()
        }));
        j
    }

    #[test]
    fn agente_rodando_manda_mesmo_com_o_catalogo_parecendo_parado() {
        let mut r = status(true);
        sobrepor_status_ao_vivo(&mut r, &[job(EstadoJob::Executando)]);
        let Resposta::Status { execucoes, .. } = r else {
            panic!()
        };
        let e = &execucoes[0];
        assert_eq!(e.estado, EstadoExecucao::EmAndamento);
        assert_eq!(e.estado_detalhe, "hash_em_andamento");
        let h = e.hash.as_ref().unwrap();
        assert!(h.em_andamento);
        assert_eq!(h.bytes_por_seg, Some(220 * 1_048_576));
        assert_eq!(h.discos[0].lendo[0].bytes_lidos, 30 << 30);
    }

    #[test]
    fn sem_job_vivo_ou_pausado_nao_inventa_andamento() {
        let mut r = status(true);
        sobrepor_status_ao_vivo(&mut r, &[]);
        let Resposta::Status { execucoes, .. } = &r else {
            panic!()
        };
        assert!(!execucoes[0].hash.as_ref().unwrap().em_andamento);
        assert_eq!(execucoes[0].estado, EstadoExecucao::Pausada);
        let mut r = status(false);
        sobrepor_status_ao_vivo(&mut r, &[job(EstadoJob::Pausado)]);
        let Resposta::Status { execucoes, .. } = &r else {
            panic!()
        };
        assert!(!execucoes[0].hash.as_ref().unwrap().em_andamento);
    }
}
