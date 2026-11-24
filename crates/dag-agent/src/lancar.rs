//! Sobe o agente em segundo plano quando nenhum atende (ADR 0004).
//!
//! GUI, CLI e MCP são clientes do agente. Se não há agente, o cliente o lança: pela unidade
//! systemd de usuário, se instalada (`systemctl --user start dag-agente`), senão como processo
//! destacado (`dag agente`). Dois clientes que lançam ao mesmo tempo não criam dois agentes: a
//! trava de lançamento deixa um só lançar e, de qualquer forma, a trava de escritor do agente
//! decide (quem perde sai e o cliente apenas conecta ao vencedor).

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rustix::fs::{FlockOperation, flock};

use crate::caminho::{caminho_do_lock, preparar_diretorio};
use crate::cliente::{Cliente, tentar_agente};
use crate::erro::ErroAgente;

/// Executável do `dag` (substitui a busca ao lado do executável atual e no `PATH`).
pub const VAR_BIN: &str = "DAG_BIN";
/// Qualquer valor não vazio e diferente de `0` desliga o uso do systemd ao lançar.
pub const VAR_SEM_SYSTEMD: &str = "DAG_SEM_SYSTEMD";
/// Argumentos extras do `dag agente` lançado, separados por espaço (testes: `--sem-monitor`).
pub const VAR_ARGS_AGENTE: &str = "DAG_AGENTE_ARGS";
/// Qualquer valor não vazio e diferente de `0` liga o modo sem agente (sessão em processo).
pub const VAR_SEM_AGENTE: &str = "DAG_SEM_AGENTE";

/// Quanto o cliente espera o agente aparecer, ao todo.
const ESPERA_PADRAO: Duration = Duration::from_secs(20);
/// Quanto a unidade systemd tem para atender antes de o cliente lançar o processo ele mesmo.
const ESPERA_SYSTEMD: Duration = Duration::from_secs(4);

fn verdadeiro(v: Option<OsString>) -> bool {
    v.is_some_and(|v| !v.is_empty() && v != "0")
}

/// O modo sem agente (sessão em processo) foi pedido pelo ambiente?
#[must_use]
pub fn sem_agente_do_ambiente() -> bool {
    verdadeiro(std::env::var_os(VAR_SEM_AGENTE))
}

/// Como lançar o agente.
#[derive(Debug, Clone)]
pub struct OpcoesLancamento {
    pub catalogo: PathBuf,
    pub socket: PathBuf,
    /// Executável do `dag`; `None` = procura ao lado do processo atual e no `PATH`.
    pub executavel: Option<PathBuf>,
    /// Usa a unidade systemd de usuário, se estiver instalada.
    pub usar_systemd: bool,
    /// Quanto esperar o agente aparecer.
    pub espera: Duration,
    /// Argumentos a mais para `dag agente`.
    pub argumentos_agente: Vec<OsString>,
}

impl OpcoesLancamento {
    /// Valores do ambiente do processo (`DAG_BIN`, `DAG_SEM_SYSTEMD`, `DAG_AGENTE_ARGS`).
    #[must_use]
    pub fn do_ambiente(catalogo: &Path, socket: &Path) -> Self {
        Self {
            catalogo: catalogo.to_path_buf(),
            socket: socket.to_path_buf(),
            executavel: std::env::var_os(VAR_BIN)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            usar_systemd: !verdadeiro(std::env::var_os(VAR_SEM_SYSTEMD)),
            espera: ESPERA_PADRAO,
            argumentos_agente: std::env::var(VAR_ARGS_AGENTE)
                .map(|s| s.split_whitespace().map(OsString::from).collect())
                .unwrap_or_default(),
        }
    }
}

/// Acha o executável `dag`: `DAG_BIN` (ou `explicito`), o próprio executável se for o `dag`, um
/// `dag` ao lado do executável atual (a GUI e o CLI são instalados juntos) e o `PATH`.
///
/// # Errors
/// [`ErroAgente::Invalido`] com a lista do que foi procurado e a saída (`--sem-agente`).
pub fn localizar_dag(explicito: Option<&Path>) -> Result<PathBuf, ErroAgente> {
    let mut procurados: Vec<String> = Vec::new();
    if let Some(p) = explicito {
        if p.is_file() {
            return Ok(p.to_path_buf());
        }
        procurados.push(format!("{} ({VAR_BIN})", p.display()));
    }
    if let Ok(atual) = std::env::current_exe() {
        if atual.file_name().is_some_and(|n| n == "dag") {
            return Ok(atual);
        }
        if let Some(ao_lado) = atual.parent().map(|d| d.join("dag")) {
            if ao_lado.is_file() {
                return Ok(ao_lado);
            }
            procurados.push(ao_lado.display().to_string());
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            let c = d.join("dag");
            if c.is_file() {
                return Ok(c);
            }
        }
        procurados.push("o PATH".into());
    }
    Err(ErroAgente::Invalido(format!(
        "não encontrei o executável `dag` para subir o agente em segundo plano (procurei em: {}). \
         Instale o `dag` junto do aplicativo, ponha-o no PATH, aponte {VAR_BIN} para ele ou use \
         --sem-agente (modo em processo, sem acompanhar nem controlar o que outros processos fazem)",
        procurados.join(", ")
    )))
}

/// Há uma unidade `dag-agente.service` de usuário instalada?
fn unidade_instalada() -> bool {
    dag_platform_linux::servico::diretorio_unidades(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .is_some_and(|d| d.join(dag_platform_linux::servico::NOME_UNIDADE).is_file())
}

fn arquivo_de_lancamento(catalogo: &Path) -> PathBuf {
    let mut s = caminho_do_lock(catalogo).into_os_string();
    s.push(".lancar");
    PathBuf::from(s)
}

/// Trava de lançamento: só quem a tem sobe o agente; solta ao descartar.
fn trava_de_lancamento(catalogo: &Path) -> Option<File> {
    let f = arquivo_de_lancamento(catalogo);
    preparar_diretorio(&f).ok()?;
    let arq = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&f)
        .ok()?;
    flock(&arq, FlockOperation::NonBlockingLockExclusive)
        .ok()
        .map(|()| arq)
}

fn arquivo_de_log(catalogo: &Path) -> PathBuf {
    let mut s = catalogo.as_os_str().to_owned();
    s.push(".agente.log");
    PathBuf::from(s)
}

/// Últimas linhas do log do agente lançado (por que ele saiu).
fn cauda_do_log(catalogo: &Path) -> String {
    let t = std::fs::read_to_string(arquivo_de_log(catalogo)).unwrap_or_default();
    let linhas: Vec<&str> = t.lines().rev().take(6).collect();
    linhas.into_iter().rev().collect::<Vec<_>>().join("\n")
}

/// Processo destacado: grupo próprio (Ctrl-C do terminal e fechamento da janela não o atingem),
/// sem stdin, e a saída no log ao lado do Catálogo.
fn lancar_processo(o: &OpcoesLancamento) -> Result<Child, ErroAgente> {
    let dag = localizar_dag(o.executavel.as_deref())?;
    let log = arquivo_de_log(&o.catalogo);
    preparar_diretorio(&log)?;
    let saida = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&log)
        .map_err(|e| ErroAgente::es(format!("abrir {}", log.display()), e))?;
    let erro = saida
        .try_clone()
        .map_err(|e| ErroAgente::es(format!("abrir {}", log.display()), e))?;
    let filho = Command::new(&dag)
        .arg("--catalogo")
        .arg(&o.catalogo)
        .arg("--socket")
        .arg(&o.socket)
        .arg("agente")
        .args(&o.argumentos_agente)
        .stdin(Stdio::null())
        .stdout(saida)
        .stderr(erro)
        .process_group(0)
        .spawn()
        .map_err(|e| ErroAgente::es(format!("executar {}", dag.display()), e))?;
    Ok(filho)
}

/// Quem detém a trava de escritor (pid gravado nela), para a mensagem de erro.
fn dono_da_trava(catalogo: &Path) -> Option<String> {
    let t = std::fs::read_to_string(caminho_do_lock(catalogo)).ok()?;
    let pid = t.trim();
    (!pid.is_empty()).then(|| pid.to_string())
}

/// Conecta ao agente que serve `o.catalogo`, lançando-o se for preciso.
///
/// # Errors
/// [`ErroAgente::Invalido`] se o `dag` não for encontrado ou se o Catálogo tem outro escritor que
/// não é um agente (versão antiga da GUI, `dag --sem-agente`); [`ErroAgente::AgenteIndisponivel`]
/// se o agente não atendeu a tempo.
pub fn garantir_agente(o: &OpcoesLancamento) -> Result<Cliente, ErroAgente> {
    if let Some(c) = tentar_agente(&o.catalogo, &o.socket) {
        return Ok(c);
    }
    let fim = Instant::now() + o.espera;
    let trava = trava_de_lancamento(&o.catalogo);
    let mut lancou = false;
    let mut systemd_ate: Option<Instant> = None;
    let mut filho: Option<Child> = None;
    let mut saida_do_filho: Option<(String, Instant)> = None;
    loop {
        if let Some(c) = tentar_agente(&o.catalogo, &o.socket) {
            reaper(filho);
            return Ok(c);
        }
        let agora = Instant::now();
        if trava.is_some() && !lancou {
            lancou = true;
            let systemctl = o.usar_systemd
                && unidade_instalada()
                && Command::new("systemctl")
                    .args(["--user", "start", dag_platform_linux::servico::NOME_UNIDADE])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success());
            if systemctl {
                systemd_ate = Some(agora + ESPERA_SYSTEMD);
            } else {
                filho = Some(lancar_processo(o)?);
            }
        }
        if trava.is_some()
            && filho.is_none()
            && saida_do_filho.is_none()
            && systemd_ate.is_some_and(|t| agora >= t)
        {
            // A unidade subiu mas não serve este Catálogo/socket: sobe o processo.
            systemd_ate = None;
            filho = Some(lancar_processo(o)?);
        }
        if let Some(f) = filho.as_mut()
            && let Ok(Some(status)) = f.try_wait()
        {
            // Perdeu a corrida pela trava de escritor (o vencedor já deve estar subindo) ou
            // falhou; espera um pouco o vencedor aparecer antes de desistir.
            saida_do_filho = Some((status.to_string(), agora + Duration::from_secs(4)));
            filho = None;
        }
        if let Some((_, ate)) = &saida_do_filho
            && agora >= *ate
        {
            break;
        }
        if agora >= fim {
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    reaper(filho);
    let mut motivo = format!(
        "nenhum agente atendeu em {} (esperei {}s)",
        o.socket.display(),
        o.espera.as_secs()
    );
    if let Some((status, _)) = saida_do_filho {
        motivo = format!("o agente lançado saiu ({status})");
        if let Some(pid) = dono_da_trava(&o.catalogo) {
            motivo.push_str(&format!(
                "; o Catálogo tem outro escritor sem agente (pid {pid}), provavelmente uma versão antiga do aplicativo ou `dag --sem-agente`: feche-o e tente de novo"
            ));
        }
        let cauda = cauda_do_log(&o.catalogo);
        if !cauda.is_empty() {
            motivo.push_str(&format!(
                "\nlog do agente ({}):\n{cauda}",
                arquivo_de_log(&o.catalogo).display()
            ));
        }
    }
    Err(ErroAgente::AgenteIndisponivel {
        socket: o.socket.clone(),
        motivo,
    })
}

/// O agente lançado deve continuar depois que o cliente sai: só se colhe o processo (evita
/// zumbi enquanto o cliente vive) numa thread à parte.
fn reaper(filho: Option<Child>) {
    if let Some(mut f) = filho {
        let _ = std::thread::Builder::new()
            .name("dag-reaper".into())
            .spawn(move || {
                let _ = f.wait();
            });
    }
}
