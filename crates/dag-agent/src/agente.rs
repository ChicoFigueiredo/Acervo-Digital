//! O [`Agente`]: abre o Catálogo e executa [`Comando`]s.

use std::fmt;
use std::io::{IsTerminal, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use dag_core::formato::{bytes_humanos, inteiro_pt_br};
use dag_core::{
    Comando, EstadoExecucao, EtapaVarredura, ExecucaoId, ModoVarredura, Resposta, ResumoCatalogo,
    StatusExecucao, TipoDispositivo, VolumeDescoberto, VolumeId, VolumeInfo,
};
use dag_platform_linux::descobrir_volumes;
use dag_scanner::{ControleScan, DesfechoScan, FaseProgresso, FullScan, Progresso, ResultadoScan};
use dag_store::Catalogo;
use dag_store::inventario::{self, EstadoScan, FaseScan, registrar_volume};
use rusqlite::{Connection, OpenFlags, params};

use crate::caminho::preparar;
use crate::classificador::ClassificadorPorExtensao;
use crate::erro::ErroAgente;
use crate::explorar::explorar;
use crate::finalidade::classificar_execucao;
use crate::trava::GuardaEscritor;

type CallbackProgresso = Arc<dyn Fn(&Progresso) + Send + Sync>;

const STATUS_LIMITE: i64 = 10;

fn agora() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// `true` para comandos que só leem o Catálogo (não exigem ser o escritor).
#[must_use]
pub fn somente_leitura(c: &Comando) -> bool {
    matches!(c, Comando::Status { .. } | Comando::Explorar { .. })
}

/// Incremento em que o comando passa a existir, se ainda não foi implementado.
#[must_use]
pub fn incremento_pendente(c: &Comando) -> Option<u8> {
    match c {
        Comando::IniciarScan {
            modo: ModoVarredura::FastScan,
            ..
        }
        | Comando::VerificarIntegridade { .. } => Some(5),
        // Pausar/cancelar entre processos e o serviço exigem o worker com IPC.
        Comando::Pausar { .. } | Comando::Cancelar { .. } | Comando::Servico { .. } => Some(6),
        Comando::ListarDuplicados { .. }
        | Comando::MarcarBackup { .. }
        | Comando::MarcarDelecao { .. }
        | Comando::Desmarcar { .. }
        | Comando::DefinirPrioridadeVolume { .. }
        | Comando::Relatorio => Some(3),
        _ => None,
    }
}

/// O Catálogo aberto, com (ou sem) a trava de escritor.
pub struct Agente {
    catalogo: Catalogo,
    caminho: PathBuf,
    guarda: Option<GuardaEscritor>,
    controle: ControleScan,
    progresso: Option<CallbackProgresso>,
    volumes_fixos: Option<Vec<VolumeDescoberto>>,
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
        Self {
            catalogo,
            caminho,
            guarda,
            controle: ControleScan::novo(),
            progresso: Some(progresso_stderr()),
            volumes_fixos: None,
        }
    }

    /// Callback de progresso da varredura (padrão: linha em stderr).
    #[must_use]
    pub fn com_progresso(mut self, f: impl Fn(&Progresso) + Send + Sync + 'static) -> Self {
        self.progresso = Some(Arc::new(f));
        self
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

    /// Executa um comando; falhas viram [`Resposta::Erro`].
    pub fn executar(&mut self, comando: Comando) -> Resposta {
        self.tentar(comando).unwrap_or_else(|e| e.para_resposta())
    }

    fn tentar(&mut self, comando: Comando) -> Result<Resposta, ErroAgente> {
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
            Comando::IniciarScan { raizes, .. } => self.full_scan(&raizes),
            Comando::Retomar {
                execucao: Some(id), ..
            } => self.retomar_scan(id),
            Comando::Retomar { execucao: None } => Err(ErroAgente::Invalido(
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
            _ => Err(ErroAgente::NaoImplementado(0)),
        }
    }

    fn volumes_do_sistema(&self) -> Result<Vec<VolumeDescoberto>, ErroAgente> {
        match &self.volumes_fixos {
            Some(v) => Ok(v.clone()),
            None => Ok(descobrir_volumes()?),
        }
    }

    // ------------------------------------------------------------------ volumes

    fn listar_volumes(&mut self) -> Result<Resposta, ErroAgente> {
        let descobertos = self.volumes_do_sistema()?;
        let t = agora();
        self.catalogo
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroAgente> {
                // Tudo desconectado até prova em contrário; `registrar_volume` religa.
                tx.execute("UPDATE volumes SET mounted = 0", [])?;
                for v in descobertos.iter().filter(|v| !v.pseudo) {
                    registrar_volume(tx, v, t)?;
                }
                Ok(())
            })?;
        let conn = self.catalogo.armazem().conexao();
        let mut st = conn.prepare(
            "SELECT v.id, v.device_id, d.tipo, v.fs_type, v.rotulo, v.capacity_bytes,
                    v.free_bytes, v.mounted,
                    (SELECT path_display FROM mount_points m WHERE m.volume_id = v.id
                     ORDER BY length(m.path_raw), m.id LIMIT 1)
             FROM volumes v LEFT JOIN dispositivos d ON d.id = v.device_id
             ORDER BY v.mounted DESC, v.id",
        )?;
        let volumes = st
            .query_map([], |l| {
                let tipo: Option<String> = l.get(2)?;
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
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Resposta::Volumes { volumes })
    }

    // ------------------------------------------------------------------ varredura

    fn novo_scan(&self, raizes: Vec<PathBuf>) -> Result<FullScan, ErroAgente> {
        let mut scan = FullScan::novo(raizes, self.volumes_do_sistema()?)
            .com_controle(self.controle.clone())
            .com_classificador(Arc::new(ClassificadorPorExtensao));
        if let Some(p) = &self.progresso {
            let p = Arc::clone(p);
            scan = scan.com_progresso(move |x| p(x));
        }
        Ok(scan)
    }

    fn full_scan(&mut self, raizes: &[String]) -> Result<Resposta, ErroAgente> {
        if raizes.is_empty() {
            return Err(ErroAgente::Invalido("informe ao menos uma raiz".into()));
        }
        let scan = self.novo_scan(raizes.iter().map(PathBuf::from).collect())?;
        let r = scan.executar(&mut self.catalogo)?;
        self.concluir_scan(r)
    }

    fn retomar_scan(&mut self, id: ExecucaoId) -> Result<Resposta, ErroAgente> {
        let scan = self.novo_scan(Vec::new())?;
        let r = scan.retomar(&mut self.catalogo, id.0)?;
        self.concluir_scan(r)
    }

    fn concluir_scan(&mut self, r: ResultadoScan) -> Result<Resposta, ErroAgente> {
        let desfecho = match r.desfecho {
            DesfechoScan::Concluida => "concluida",
            DesfechoScan::ConcluidaComErros => "concluida_com_erros",
            DesfechoScan::Cancelada => "cancelada",
        };
        if r.desfecho != DesfechoScan::Cancelada {
            if self.progresso.is_some() {
                eprintln!("classificando finalidade (por caminho)...");
            }
            classificar_execucao(&mut self.catalogo, r.execucao_id, agora())?;
        }
        Ok(Resposta::ScanConcluido {
            execucao: ExecucaoId(r.execucao_id),
            desfecho: desfecho.to_string(),
            pastas: r.contadores.pastas,
            arquivos: r.contadores.arquivos,
            especiais: r.contadores.especiais,
            bytes_logicos: r.contadores.bytes_logicos,
            ignorados: r.contadores.ignorados,
            erros: r.contadores.erros,
            duracao_ms: u64::try_from(r.duracao.as_millis()).unwrap_or(u64::MAX),
        })
    }

    // ------------------------------------------------------------------ status

    fn status(&self, execucao: Option<ExecucaoId>) -> Result<Resposta, ErroAgente> {
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
            execucoes.push(status_de(&e));
        }
        let (pastas, arquivos, bytes): (i64, i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(folder_count),0), COALESCE(SUM(file_count),0),
                    COALESCE(SUM(logical_bytes),0) FROM volume_stats",
            [],
            |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)),
        )?;
        let volumes: i64 = conn.query_row("SELECT COUNT(*) FROM volumes", [], |l| l.get(0))?;
        let n = |x: i64| u64::try_from(x).unwrap_or(0);
        Ok(Resposta::Status {
            execucoes,
            catalogo: ResumoCatalogo {
                volumes: n(volumes),
                pastas: n(pastas),
                arquivos: n(arquivos),
                bytes_logicos: n(bytes),
            },
        })
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

fn tipo_dispositivo(s: &str) -> Option<TipoDispositivo> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
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
            EstadoScan::Concluida | EstadoScan::ConcluidaComErros => EstadoExecucao::Concluida,
            EstadoScan::Cancelada => EstadoExecucao::Cancelada,
            EstadoScan::Falhou => EstadoExecucao::Falhou,
        },
        etapa: match e.fase {
            FaseScan::Inventario => Some(EtapaVarredura::Inventario),
            FaseScan::Concluida => None,
            _ => Some(EtapaVarredura::Agregacao),
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
    }
}

// ---------------------------------------------------------------------- progresso em stderr

fn progresso_stderr() -> CallbackProgresso {
    let tty = std::io::stderr().is_terminal();
    let inicio = Instant::now();
    let ultimo_ms = AtomicU64::new(0);
    Arc::new(move |p: &Progresso| {
        let fase = match p.fase {
            FaseProgresso::Inventario => "inventário",
            FaseProgresso::Pausado => "pausado",
            FaseProgresso::Agregacao => "agregando",
            FaseProgresso::Concluido => "concluído",
        };
        let agora_ms = u64::try_from(inicio.elapsed().as_millis()).unwrap_or(u64::MAX);
        let fim = p.fase == FaseProgresso::Concluido;
        // Sem terminal: no máximo uma linha a cada 2 s (mais a final).
        if !tty && !fim && agora_ms < ultimo_ms.load(Ordering::Relaxed) + 2000 {
            return;
        }
        ultimo_ms.store(agora_ms, Ordering::Relaxed);
        let linha = format!(
            "[{fase}] {} pastas, {} arquivos, {} | {} pendentes | {} itens/s",
            inteiro_pt_br(p.pastas),
            inteiro_pt_br(p.arquivos),
            bytes_humanos(p.bytes_logicos),
            inteiro_pt_br(p.diretorios_pendentes),
            inteiro_pt_br(p.entradas_por_segundo as u64),
        );
        let mut e = std::io::stderr().lock();
        let _ = if tty {
            write!(e, "\r\x1b[2K{linha}{}", if fim { "\n" } else { "" })
        } else {
            writeln!(e, "{linha}")
        };
    })
}
