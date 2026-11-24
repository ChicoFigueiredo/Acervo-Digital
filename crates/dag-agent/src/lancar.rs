//! Sobe o agente em segundo plano quando nenhum atende (ADR 0004).
//!
//! GUI, CLI e MCP são clientes do agente. Se não há agente, o cliente o lança: pela unidade
//! systemd de usuário, se instalada (`systemctl --user start dag-agente`); senão numa unidade
//! transitória (`systemd-run --user`), fora do escopo/cgroup do cliente (o KDE põe os aplicativos
//! num escopo `app-*.scope` que cai junto com a janela); senão com `setsid` (sessão própria, sem
//! terminal de controle); em último caso, como processo em grupo próprio (`dag agente`). Dois clientes que lançam ao mesmo tempo não criam dois agentes: a
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

/// Executável do agente a partir do `dag` e das opções: argumentos de `dag ... agente`.
#[must_use]
pub fn argumentos_do_agente(o: &OpcoesLancamento) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec![
        "--catalogo".into(),
        o.catalogo.clone().into_os_string(),
        "--socket".into(),
        o.socket.clone().into_os_string(),
        "agente".into(),
    ];
    a.extend(o.argumentos_agente.iter().cloned());
    a
}

/// Hash estável (FNV-1a de 64 bits) do caminho do Catálogo, para nomear a unidade transitória.
#[must_use]
pub fn hash_do_catalogo(catalogo: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in catalogo.as_os_str().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Nome (sem `.service`) da unidade transitória do agente deste Catálogo.
#[must_use]
pub fn nome_da_unidade_transitoria(catalogo: &Path) -> String {
    format!("dag-agente-{}", hash_do_catalogo(catalogo))
}

/// Argumentos do `systemd-run` que lança o agente numa unidade de serviço de usuário própria.
/// `log` = `Some` redireciona a saída com `append:` (systemd >= 240).
#[must_use]
pub fn argumentos_systemd_run(
    unidade: &str,
    log: Option<&Path>,
    dag: &Path,
    agente: &[OsString],
) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec![
        "--user".into(),
        format!("--unit={unidade}").into(),
        "--collect".into(),
        "--quiet".into(),
        "--property=Type=exec".into(),
    ];
    if let Some(l) = log {
        for fluxo in ["StandardOutput", "StandardError"] {
            let mut p = OsString::from(format!("--property={fluxo}=append:"));
            p.push(l.as_os_str());
            a.push(p);
        }
    }
    a.push("--".into());
    a.push(dag.as_os_str().to_owned());
    a.extend(agente.iter().cloned());
    a
}

/// Argumentos do `setsid` (util-linux). Sem `-f`: o filho do cliente não é líder de grupo, então
/// o `setsid` apenas chama `setsid(2)` e faz `exec`, e o cliente ainda vê o agente sair.
#[must_use]
pub fn argumentos_setsid(dag: &Path, agente: &[OsString]) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec![dag.as_os_str().to_owned()];
    a.extend(agente.iter().cloned());
    a
}

/// A versão do systemd (primeira linha de `systemd-run --version`, "systemd 255 (...)") aceita
/// `StandardOutput=append:`?
#[must_use]
pub fn systemd_aceita_append(saida_version: &str) -> bool {
    saida_version
        .split_whitespace()
        .nth(1)
        .and_then(|n| n.parse::<u32>().ok())
        .is_some_and(|n| n >= 240)
}

fn comando_mudo(prog: &str) -> Command {
    let mut c = Command::new(prog);
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    c
}

/// Há um gerenciador de usuário do systemd ao alcance (socket privado em `$XDG_RUNTIME_DIR`)?
fn gerenciador_de_usuario() -> bool {
    std::env::var_os("XDG_RUNTIME_DIR")
        .is_some_and(|d| Path::new(&d).join("systemd/private").exists())
}

/// Lança numa unidade transitória (`systemd-run --user`), fora do cgroup do cliente. `true` se o
/// agente subiu ou já havia a unidade ativa (o cliente apenas conecta); `false` para cair no
/// próximo método.
fn lancar_via_systemd_run(o: &OpcoesLancamento) -> bool {
    if !gerenciador_de_usuario() {
        return false;
    }
    let Ok(dag) = localizar_dag(o.executavel.as_deref()) else {
        return false;
    };
    let versao = Command::new("systemd-run")
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(versao) = versao else {
        return false; // sem systemd-run
    };
    let versao = String::from_utf8_lossy(&versao.stdout).into_owned();
    if !systemd_aceita_append(versao.lines().next().unwrap_or("")) {
        return false;
    }
    let log = arquivo_de_log(&o.catalogo);
    if preparar_diretorio(&log).is_err() {
        return false;
    }
    let unidade = nome_da_unidade_transitoria(&o.catalogo);
    let args = argumentos_systemd_run(&unidade, Some(&log), &dag, &argumentos_do_agente(o));
    let ok = comando_mudo("systemd-run")
        .args(&args)
        .status()
        .is_ok_and(|s| s.success());
    ok || comando_mudo("systemctl")
        .args([
            "--user",
            "is-active",
            "--quiet",
            &format!("{unidade}.service"),
        ])
        .status()
        .is_ok_and(|s| s.success())
}

/// Processo destacado, sem stdin e com a saída no log ao lado do Catálogo. Preferência: `setsid`
/// (sessão nova, sem terminal de controle: nem fechar o terminal nem desfazer a sessão da GUI o
/// atingem); senão grupo de processos próprio. O `Child` devolvido só serve para observar a saída
/// (descartá-lo não mata o agente).
///
/// # Errors
/// [`ErroAgente::Invalido`] se o `dag` não for encontrado; [`ErroAgente::Es`]-like se não der para
/// abrir o log ou executar.
pub fn lancar_destacado(o: &OpcoesLancamento) -> Result<Child, ErroAgente> {
    let dag = localizar_dag(o.executavel.as_deref())?;
    let log = arquivo_de_log(&o.catalogo);
    preparar_diretorio(&log)?;
    let abrir = || {
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&log)
            .map_err(|e| ErroAgente::es(format!("abrir {}", log.display()), e))
    };
    let agente = argumentos_do_agente(o);
    let com_setsid = Command::new("setsid")
        .args(argumentos_setsid(&dag, &agente))
        .stdin(Stdio::null())
        .stdout(abrir()?)
        .stderr(abrir()?)
        .spawn();
    if let Ok(f) = com_setsid {
        return Ok(f);
    }
    Command::new(&dag)
        .args(&agente)
        .stdin(Stdio::null())
        .stdout(abrir()?)
        .stderr(abrir()?)
        .process_group(0)
        .spawn()
        .map_err(|e| ErroAgente::es(format!("executar {}", dag.display()), e))
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
                && ((unidade_instalada()
                    && comando_mudo("systemctl")
                        .args(["--user", "start", dag_platform_linux::servico::NOME_UNIDADE])
                        .status()
                        .is_ok_and(|s| s.success()))
                    || lancar_via_systemd_run(o));
            if systemctl {
                systemd_ate = Some(agora + ESPERA_SYSTEMD);
            } else {
                filho = Some(lancar_destacado(o)?);
            }
        }
        if trava.is_some()
            && filho.is_none()
            && saida_do_filho.is_none()
            && systemd_ate.is_some_and(|t| agora >= t)
        {
            // A unidade subiu mas não serve este Catálogo/socket: sobe o processo.
            systemd_ate = None;
            filho = Some(lancar_destacado(o)?);
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

#[cfg(test)]
mod testes {
    use super::*;

    fn opcoes() -> OpcoesLancamento {
        OpcoesLancamento {
            catalogo: "/d/c.sqlite".into(),
            socket: "/run/s.sock".into(),
            executavel: None,
            usar_systemd: true,
            espera: Duration::from_secs(1),
            argumentos_agente: vec!["--sem-monitor".into()],
        }
    }

    fn texto(v: &[OsString]) -> Vec<String> {
        v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn argumentos_do_agente_levam_catalogo_socket_e_extras() {
        assert_eq!(
            texto(&argumentos_do_agente(&opcoes())),
            [
                "--catalogo",
                "/d/c.sqlite",
                "--socket",
                "/run/s.sock",
                "agente",
                "--sem-monitor"
            ]
        );
    }

    #[test]
    fn unidade_transitoria_e_estavel_e_distinta_por_catalogo() {
        let a = nome_da_unidade_transitoria(Path::new("/d/a.sqlite"));
        assert_eq!(a, nome_da_unidade_transitoria(Path::new("/d/a.sqlite")));
        assert_ne!(a, nome_da_unidade_transitoria(Path::new("/d/b.sqlite")));
        assert!(a.starts_with("dag-agente-") && a.len() == "dag-agente-".len() + 16);
    }

    #[test]
    fn systemd_run_com_log_append_e_sem_log() {
        let ag = argumentos_do_agente(&opcoes());
        let com = texto(&argumentos_systemd_run(
            "dag-agente-x",
            Some(Path::new("/d/c.sqlite.agente.log")),
            Path::new("/bin/dag"),
            &ag,
        ));
        assert_eq!(com[0], "--user");
        assert!(com.contains(&"--unit=dag-agente-x".to_string()));
        assert!(com.contains(&"--collect".to_string()));
        assert!(com.contains(&"--property=StandardOutput=append:/d/c.sqlite.agente.log".into()));
        assert!(com.contains(&"--property=StandardError=append:/d/c.sqlite.agente.log".into()));
        let sep = com.iter().position(|a| a == "--").unwrap();
        assert_eq!(com[sep + 1], "/bin/dag");
        assert_eq!(com[sep + 2], "--catalogo");
        let sem = texto(&argumentos_systemd_run(
            "u",
            None,
            Path::new("/bin/dag"),
            &ag,
        ));
        assert!(!sem.iter().any(|a| a.contains("append:")));
    }

    #[test]
    fn setsid_executa_o_dag_com_os_argumentos_do_agente() {
        let ag = argumentos_do_agente(&opcoes());
        let a = texto(&argumentos_setsid(Path::new("/bin/dag"), &ag));
        assert_eq!(a[0], "/bin/dag");
        assert_eq!(a[1..], texto(&ag)[..]);
    }

    #[test]
    fn versao_do_systemd_para_append() {
        assert!(systemd_aceita_append("systemd 255 (255.4-1ubuntu8)"));
        assert!(systemd_aceita_append("systemd 240"));
        assert!(!systemd_aceita_append("systemd 239 (239-58.el8)"));
        assert!(!systemd_aceita_append(""));
    }
}
