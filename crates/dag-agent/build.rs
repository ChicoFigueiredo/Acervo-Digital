//! Grava o commit curto do git em `DAG_GIT_COMMIT` (opcional: sem git ou fora de um repositório
//! o agente só informa a versão do pacote).

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let o = Command::new("git").args(args).output().ok()?;
    if !o.status.success() {
        return None;
    }
    let s = String::from_utf8(o.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DAG_GIT_COMMIT");
    if let Some(head) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }
    if std::env::var_os("DAG_GIT_COMMIT").is_none()
        && let Some(c) = git(&["rev-parse", "--short", "HEAD"])
    {
        println!("cargo:rustc-env=DAG_GIT_COMMIT={c}");
    }
}
