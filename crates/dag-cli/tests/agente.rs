//! `dag agente` + CLI de ponta a ponta: o CLI usa o agente em segundo plano quando há um, e
//! `dag pausar|cancelar` agem entre processos.
#![allow(clippy::unwrap_used)]

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Ambiente {
    _tmp: tempfile::TempDir,
    raiz: PathBuf,
    catalogo: PathBuf,
    socket: PathBuf,
}

fn ambiente() -> Ambiente {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let raiz = base.join("arvore");
    // Árvore grande o bastante para a varredura durar enquanto pausamos e cancelamos.
    for d in 0..150 {
        for f in 0..60 {
            let p = raiz.join(format!("d{d}/f{f}.txt"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![b'x'; 100 + f]).unwrap();
        }
    }
    Ambiente {
        catalogo: base.join("dados/catalogo.sqlite"),
        socket: base.join("run/agente.sock"),
        raiz,
        _tmp: tmp,
    }
}

fn dag_cmd(a: &Ambiente) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_dag"));
    c.env_remove("DAG_CATALOGO")
        .env_remove("DAG_SOCKET")
        .arg("--catalogo")
        .arg(&a.catalogo)
        .arg("--socket")
        .arg(&a.socket);
    c
}

fn dag(a: &Ambiente, args: &[&str]) -> Output {
    dag_cmd(a).args(args).output().unwrap()
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout não é JSON ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    })
}

/// O processo `dag agente`, encerrado ao sair do escopo.
struct Agente(Child);

impl Agente {
    fn iniciar(a: &Ambiente, extra: &[&str]) -> Agente {
        let filho = dag_cmd(a)
            .arg("agente")
            .args(extra)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut ag = Agente(filho);
        let fim = Instant::now() + Duration::from_secs(30);
        while UnixStream::connect(&a.socket).is_err() {
            assert!(Instant::now() < fim, "o agente não abriu o socket");
            if let Some(st) = ag.0.try_wait().unwrap() {
                panic!("o agente saiu cedo: {st}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        ag
    }

    /// SIGTERM e espera o encerramento limpo.
    fn terminar(mut self) -> std::process::ExitStatus {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(self.0.id().to_string())
            .status()
            .unwrap();
        let fim = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(st) = self.0.try_wait().unwrap() {
                return st;
            }
            assert!(Instant::now() < fim, "o agente não encerrou com SIGTERM");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Agente {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn estado_da_primeira(a: &Ambiente) -> Option<String> {
    let o = dag(a, &["--json", "status"]);
    json(&o)["execucoes"][0]["estado"]
        .as_str()
        .map(str::to_string)
}

fn esperar_estado(a: &Ambiente, quer: impl Fn(&str) -> bool, descricao: &str) {
    let fim = Instant::now() + Duration::from_secs(60);
    loop {
        if estado_da_primeira(a).is_some_and(|e| quer(&e)) {
            return;
        }
        assert!(Instant::now() < fim, "tempo esgotado: {descricao}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn existe(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

#[test]
fn status_mostra_se_ha_agente_e_pausar_cancelar_funcionam_entre_processos() {
    let a = ambiente();

    // Sem agente: sessão em processo; pausar/cancelar dão mensagem clara e nada é criado.
    let o = dag(&a, &["pausar"]);
    assert_eq!(o.status.code(), Some(1));
    let msg = String::from_utf8_lossy(&o.stderr).into_owned() + &String::from_utf8_lossy(&o.stdout);
    assert!(
        msg.contains("nenhum agente do DAG está em execução"),
        "{msg}"
    );
    let o = dag(&a, &["--json", "cancelar"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(json(&o)["codigo"], "sem_agente");
    assert!(
        !existe(&a.catalogo),
        "controlar sem agente não cria Catálogo"
    );

    let agente = Agente::iniciar(&a, &[]);

    let o = dag(&a, &["--json", "status"]);
    assert_eq!(o.status.code(), Some(0));
    let j = json(&o);
    assert_eq!(j["resposta"], "status");
    assert_eq!(j["agente"]["servindo"], true);
    assert_eq!(j["agente"]["monitor"], true);
    let texto = dag(&a, &["status"]);
    assert!(
        String::from_utf8_lossy(&texto.stdout).contains("Agente: servindo"),
        "{}",
        String::from_utf8_lossy(&texto.stdout)
    );

    // Full scan pelo CLI (cliente do agente) em segundo plano do teste.
    let scan = dag_cmd(&a)
        .args(["scan", "--full", a.raiz.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    esperar_estado(&a, |e| e == "em_andamento", "varredura em andamento");

    // Pausar de outro processo.
    let o = dag(&a, &["pausar"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    esperar_estado(&a, |e| e == "pausada", "varredura pausada");
    // Retomar de outro processo.
    let o = dag(&a, &["retomar"]);
    assert_eq!(o.status.code(), Some(0));
    esperar_estado(&a, |e| e != "pausada", "varredura retomada");
    // Cancelar de outro processo: o `dag scan` termina com 130 (cancelada, retomável).
    let o = dag(&a, &["cancelar"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let saida = scan.wait_with_output().unwrap();
    assert_eq!(
        saida.status.code(),
        Some(130),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&saida.stdout),
        String::from_utf8_lossy(&saida.stderr)
    );
    assert!(String::from_utf8_lossy(&saida.stdout).contains("cancelada"));
    assert_eq!(estado_da_primeira(&a).as_deref(), Some("cancelada"));

    // Sem nada em andamento, cancelar é erro claro.
    let o = dag(&a, &["--json", "cancelar"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(json(&o)["codigo"], "nada_em_execucao");

    // Um scan completo pelo agente conclui, com o hash visível no progresso do stderr.
    let o = dag(&a, &["--json", "scan", "--full", a.raiz.to_str().unwrap()]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(json(&o)["desfecho"], "concluida");

    // O CLI não consegue ser um segundo escritor: o agente tem a trava.
    let o = dag(&a, &["--json", "volumes"]);
    assert_eq!(o.status.code(), Some(0), "volumes passa pelo agente");

    // SIGTERM: encerramento limpo, socket removido.
    let st = agente.terminar();
    assert_eq!(st.code(), Some(0));
    assert!(!existe(&a.socket));

    // Sem o agente, o CLI volta à sessão em processo e vê o mesmo Catálogo.
    let o = dag(&a, &["status"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&o.stdout).contains("Agente: nenhum"));
}

#[test]
fn dag_agente_recusa_segundo_agente_no_mesmo_catalogo() {
    let a = ambiente();
    let _agente = Agente::iniciar(&a, &["--sem-monitor"]);
    let mut outro = a.socket.clone();
    outro.set_file_name("outro.sock");
    let o = Command::new(env!("CARGO_BIN_EXE_dag"))
        .env_remove("DAG_CATALOGO")
        .arg("--catalogo")
        .arg(&a.catalogo)
        .arg("--socket")
        .arg(&outro)
        .arg("agente")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("escritor"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!existe(&outro));
}
