#![allow(clippy::unwrap_used, clippy::expect_used)]
//! O agente lançado por `lancar_destacado` sobrevive à saída do processo que o lançou e fica em
//! sessão própria (sem terminal de controle).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use dag_agent::lancar::{OpcoesLancamento, lancar_destacado};

const VAR_AJUDANTE: &str = "DAG_TESTE_AJUDANTE_DIR";

/// Campo `session` (6º) de `/proc/<pid>/stat`.
fn sessao(pid: u32) -> Option<u32> {
    let t = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let depois = &t[t.rfind(')')? + 2..]; // começa em `state` (campo 3)
    depois.split_whitespace().nth(3)?.parse().ok()
}

fn tem_setsid() -> bool {
    Command::new("setsid")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Processo "pai": lança o agente falso e sai sem esperar nem colher o filho.
#[test]
fn ajudante_lanca_e_sai() {
    let Some(dir) = std::env::var_os(VAR_AJUDANTE).map(PathBuf::from) else {
        return; // só faz algo quando chamado pelo teste de verdade
    };
    let o = OpcoesLancamento {
        catalogo: dir.join("c.sqlite"),
        socket: dir.join("s.sock"),
        executavel: Some(dir.join("dag")),
        usar_systemd: false,
        espera: Duration::from_secs(1),
        argumentos_agente: vec![],
    };
    let filho = lancar_destacado(&o).expect("lançar");
    std::mem::forget(filho);
    std::process::exit(0);
}

fn esperar(p: &Path) -> Option<u32> {
    let fim = Instant::now() + Duration::from_secs(10);
    while Instant::now() < fim {
        if let Some(n) = std::fs::read_to_string(p)
            .ok()
            .and_then(|t| t.trim().parse().ok())
        {
            return Some(n);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

#[test]
fn agente_destacado_sobrevive_ao_pai_e_tem_sessao_propria() {
    if !tem_setsid() {
        eprintln!("sem setsid (util-linux): teste ignorado");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("pid");
    let falso = dir.path().join("dag");
    std::fs::write(
        &falso,
        format!(
            "#!/bin/sh\necho $$ > {}\nexec sleep 60\n",
            pidfile.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&falso, std::fs::Permissions::from_mode(0o755)).unwrap();

    let st = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "ajudante_lanca_e_sai", "--test-threads=1"])
        .env(VAR_AJUDANTE, dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(st.success());

    let pid = esperar(&pidfile).expect("o agente falso não subiu");
    std::thread::sleep(Duration::from_millis(300));
    let dele = sessao(pid);
    let minha = sessao(std::process::id());
    let _ = Command::new("kill").arg(pid.to_string()).status();
    assert!(dele.is_some(), "o agente morreu junto com o pai");
    assert_ne!(dele, minha, "o agente deve ter sessão própria");
    assert_eq!(dele, Some(pid), "o agente deve ser líder da sua sessão");
}
