#![allow(clippy::unwrap_used)]

use std::process::Command;

fn dag(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_dag"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_lista_todos_os_subcomandos() {
    let o = dag(&["--help"]);
    assert!(o.status.success());
    let out = String::from_utf8(o.stdout).unwrap();
    for s in [
        "volumes",
        "scan",
        "pausar",
        "retomar",
        "cancelar",
        "status",
        "duplicados",
        "marcar",
        "desmarcar",
        "prioridade",
        "relatorio",
        "verificar",
        "servico",
        "catalogo",
        "mcp",
        "--json",
    ] {
        assert!(out.contains(s), "help sem `{s}`:\n{out}");
    }
}

#[test]
fn version_funciona() {
    let o = dag(&["--version"]);
    assert!(o.status.success());
    assert!(String::from_utf8(o.stdout).unwrap().starts_with("dag "));
}

#[test]
fn scan_fast_json() {
    let o = dag(&["--json", "scan", "--fast", "/tmp"]);
    assert_eq!(o.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["comando"], "iniciar_scan");
    assert_eq!(v["modo"], "fast_scan");
    assert_eq!(v["raizes"][0], "/tmp");
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.contains("worker ainda não implementado (incremento 1)"));
}

#[test]
fn scan_exige_modo() {
    assert!(!dag(&["scan", "/tmp"]).status.success());
    assert!(!dag(&["scan", "--full", "--fast", "/tmp"]).status.success());
}

#[test]
fn outros_subcomandos_json() {
    let casos: [(&[&str], &str); 5] = [
        (&["--json", "volumes"], "listar_volumes"),
        (&["--json", "marcar", "backup", "/a"], "marcar_backup"),
        (&["--json", "servico", "autostart", "on"], "servico"),
        (&["--json", "catalogo", "backup", "/d"], "backup_catalogo"),
        (&["--json", "pausar", "--execucao", "3"], "pausar"),
    ];
    for (args, esperado) in casos {
        let o = dag(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["comando"], esperado);
    }
}

#[test]
fn mcp_placeholder() {
    let o = dag(&["mcp"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8(o.stdout).unwrap().contains("MCP"));
}
