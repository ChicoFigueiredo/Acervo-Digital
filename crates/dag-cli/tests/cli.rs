#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn dag(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_dag"))
        .env_remove("DAG_CATALOGO")
        .args(args)
        .output()
        .unwrap()
}

/// Cria `<tmp>/arvore/{grande.pdf 8000, sub/{a.txt 3000, b.bin 1000}, pequeno.txt 10}`.
fn arvore(tmp: &Path) -> PathBuf {
    let raiz = std::fs::canonicalize(tmp).unwrap().join("arvore");
    for (rel, n) in [
        ("grande.pdf", 8000),
        ("sub/a.txt", 3000),
        ("sub/b.bin", 1000),
        ("pequeno.txt", 10),
    ] {
        let p = raiz.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![b'x'; n]).unwrap();
    }
    raiz
}

fn json(o: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout não é JSON ({e}): {}",
            String::from_utf8_lossy(&o.stdout)
        )
    })
}

#[test]
fn help_lista_todos_os_subcomandos() {
    let o = dag(&["--help"]);
    assert!(o.status.success());
    let out = String::from_utf8(o.stdout).unwrap();
    for s in [
        "volumes",
        "scan",
        "explorar",
        "pausar",
        "retomar",
        "cancelar",
        "status",
        "duplicados",
        "monitorar",
        "eventos",
        "marcar",
        "desmarcar",
        "prioridade",
        "relatorio",
        "verificar",
        "servico",
        "catalogo",
        "mcp",
        "--json",
        "--catalogo",
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
fn scan_exige_modo() {
    assert!(!dag(&["scan", "/tmp"]).status.success());
    assert!(!dag(&["scan", "--full", "--fast", "/tmp"]).status.success());
}

#[test]
fn scan_retomar_e_exclusivo() {
    assert!(!dag(&["scan", "--retomar", "1", "--full"]).status.success());
    assert!(!dag(&["scan", "--retomar", "1", "/tmp"]).status.success());
    assert!(!dag(&["scan", "--retomar", "x"]).status.success());
}

#[test]
fn subcomandos_futuros_saem_com_2_sem_criar_catalogo() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = tmp.path().join("nao-criar/c.sqlite");
    let cat = cat.to_str().unwrap();
    let casos: [(&[&str], &str); 2] = [
        (&["servico", "autostart", "on"], "incremento 6"),
        (&["pausar", "--execucao", "3"], "incremento 6"),
    ];
    for (args, esperado) in casos {
        let mut a = vec!["--catalogo", cat, "--json"];
        a.extend_from_slice(args);
        let o = dag(&a);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert_eq!(json(&o)["codigo"], "nao_implementado");
        assert!(String::from_utf8(o.stderr).unwrap().contains(esperado));
    }
    assert!(!Path::new(cat).exists());
    assert!(!Path::new(cat).parent().unwrap().exists());
}

#[test]
fn scan_full_e_explorar_json() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = arvore(tmp.path());
    let cat = tmp.path().join("dados/c.sqlite");
    let (cat, r) = (cat.to_str().unwrap(), raiz.to_str().unwrap());

    let o = dag(&["--catalogo", cat, "--json", "scan", "--full", r]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v = json(&o);
    assert_eq!(v["resposta"], "scan_concluido");
    assert_eq!(v["desfecho"], "concluida");
    assert_eq!(v["arquivos"], 4);
    assert_eq!(v["pastas"], 2);
    assert_eq!(v["bytes_logicos"], 12_010);

    let o = dag(&["--catalogo", cat, "--json", "explorar", r]);
    assert_eq!(o.status.code(), Some(0));
    let v = json(&o);
    assert_eq!(v["resposta"], "explorar");
    assert_eq!(v["pasta"], r);
    let e = v["entradas"].as_array().unwrap();
    let nomes: Vec<&str> = e.iter().map(|x| x["nome"].as_str().unwrap()).collect();
    assert_eq!(nomes, ["grande.pdf", "sub", "pequeno.txt"]);
    assert_eq!(e[0]["bytes_logicos"], 8000);
    assert_eq!(e[0]["mime"], "application/pdf");
    assert_eq!(e[1]["tipo"], "pasta");
    assert_eq!(e[1]["bytes_logicos"], 4000);
    assert_eq!(e[1]["arquivos"], 2);

    // --limite e paginação por cursor.
    let o = dag(&["--catalogo", cat, "--json", "explorar", r, "--limite", "2"]);
    let v = json(&o);
    assert_eq!(v["entradas"].as_array().unwrap().len(), 2);
    let cursor = v["proximo_cursor"].as_str().unwrap().to_string();
    let o = dag(&[
        "--catalogo",
        cat,
        "--json",
        "explorar",
        r,
        "--limite",
        "2",
        "--cursor",
        &cursor,
    ]);
    let v = json(&o);
    assert_eq!(v["entradas"][0]["nome"], "pequeno.txt");
    assert!(v["proximo_cursor"].is_null());

    // Sem pasta: as Raízes catalogadas.
    let o = dag(&["--catalogo", cat, "--json", "explorar"]);
    let v = json(&o);
    assert_eq!(v["entradas"].as_array().unwrap().len(), 1);
    assert_eq!(v["entradas"][0]["bytes_logicos"], 12_010);

    // Pasta fora do catálogo: erro 1.
    let o = dag(&["--catalogo", cat, "--json", "explorar", "/nao/catalogada"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(json(&o)["codigo"], "nao_encontrado");

    // Status.
    let o = dag(&["--catalogo", cat, "--json", "status"]);
    let v = json(&o);
    assert_eq!(v["execucoes"][0]["estado"], "concluida");
    assert_eq!(v["execucoes"][0]["arquivos"], 4);
    assert_eq!(v["catalogo"]["arquivos"], 4);
}

#[test]
fn saida_humana_do_scan_e_do_explorar() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = arvore(tmp.path());
    let cat = tmp.path().join("c.sqlite");
    let (cat, r) = (cat.to_str().unwrap(), raiz.to_str().unwrap());
    let o = dag(&["--catalogo", cat, "scan", "--full", r]);
    assert_eq!(o.status.code(), Some(0));
    let out = String::from_utf8(o.stdout).unwrap();
    assert!(out.contains("Varredura 1 concluída"), "{out}");
    assert!(
        out.contains("4 arquivos") && out.contains("11,7 KiB"),
        "{out}"
    );
    let o = dag(&["--catalogo", cat, "explorar", r]);
    let out = String::from_utf8(o.stdout).unwrap();
    assert!(
        out.contains("grande.pdf") && out.contains("7,8 KiB"),
        "{out}"
    );
    assert!(out.contains("sub/") && out.contains("LÓGICO"), "{out}");
    let o = dag(&["--catalogo", cat, "status"]);
    let out = String::from_utf8(o.stdout).unwrap();
    assert!(
        out.contains("Full Scan") && out.contains("concluída"),
        "{out}"
    );
}

#[test]
fn volumes_json_lista_o_volume_raiz() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = tmp.path().join("c.sqlite");
    let o = dag(&["--catalogo", cat.to_str().unwrap(), "--json", "volumes"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v = json(&o);
    assert_eq!(v["resposta"], "volumes");
    let vols = v["volumes"].as_array().unwrap();
    assert!(
        vols.iter()
            .any(|x| x["ponto_montagem"] == "/" && x["conectado"] == true),
        "sem volume raiz: {v}"
    );
}

#[test]
fn catalogo_backup_gera_arquivo_integro() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = arvore(tmp.path());
    let cat = tmp.path().join("c.sqlite");
    let copia = tmp.path().join("copia.sqlite");
    let (cat, copia) = (cat.to_str().unwrap(), copia.to_str().unwrap());
    assert!(
        dag(&["--catalogo", cat, "scan", "--full", raiz.to_str().unwrap()])
            .status
            .success()
    );
    let o = dag(&["--catalogo", cat, "catalogo", "backup", copia]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        String::from_utf8(o.stdout)
            .unwrap()
            .contains("integridade: ok")
    );
    // A cópia abre sozinha e o `explorar` dela enxerga o mesmo inventário.
    let o = dag(&[
        "--catalogo",
        copia,
        "--json",
        "explorar",
        raiz.to_str().unwrap(),
    ]);
    assert_eq!(json(&o)["entradas"].as_array().unwrap().len(), 3);
    // Não sobrescreve.
    let o = dag(&["--catalogo", cat, "--json", "catalogo", "backup", copia]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(json(&o)["codigo"], "destino_existe");
}

#[test]
fn catalogo_por_variavel_de_ambiente_e_precedencia_da_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let env_cat = tmp.path().join("env.sqlite");
    let flag_cat = tmp.path().join("flag.sqlite");
    let status = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_dag"))
            .env("DAG_CATALOGO", &env_cat)
            .args(extra)
            .arg("--json")
            .arg("volumes")
            .output()
            .unwrap()
    };
    assert!(status(&[]).status.success());
    assert!(env_cat.exists() && !flag_cat.exists());
    assert!(
        status(&["--catalogo", flag_cat.to_str().unwrap()])
            .status
            .success()
    );
    assert!(flag_cat.exists());
}

#[test]
fn retomar_execucao_inexistente_da_erro() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = tmp.path().join("c.sqlite");
    let o = dag(&[
        "--catalogo",
        cat.to_str().unwrap(),
        "--json",
        "scan",
        "--retomar",
        "42",
    ]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(json(&o)["codigo"], "execucao_inexistente");
}

#[test]
fn mcp_placeholder() {
    let o = dag(&["mcp"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8(o.stderr).unwrap().contains("MCP"));
}

#[test]
fn scan_fast_eventos_e_status_pelo_cli() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = arvore(tmp.path());
    let cat = tmp.path().join("c.sqlite");
    let cat = cat.to_str().unwrap();
    let r = raiz.to_str().unwrap();
    assert!(
        dag(&["--catalogo", cat, "scan", "--full", r])
            .status
            .success()
    );
    std::fs::rename(raiz.join("sub/a.txt"), raiz.join("sub/z.txt")).unwrap();
    std::fs::remove_file(raiz.join("pequeno.txt")).unwrap();
    // Sem raízes: reconcilia todas as raízes já varridas.
    let o = dag(&["--catalogo", cat, "--json", "scan", "--fast"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v = json(&o);
    assert_eq!(v["reconciliacao"]["arquivos_renomeados"], 1);
    assert_eq!(v["reconciliacao"]["arquivos_ausentes"], 1);
    assert_eq!(v["hash"]["bytes_lidos"], 0);
    let o = dag(&["--catalogo", cat, "scan", "--fast"]);
    let out = String::from_utf8(o.stdout).unwrap();
    assert!(out.contains("Reconciliação:"), "{out}");
    let o = dag(&["--catalogo", cat, "eventos", "--limite", "10"]);
    assert!(o.status.success());
    let out = String::from_utf8(o.stdout).unwrap();
    assert!(
        out.contains("renomeado") && out.contains("removido"),
        "{out}"
    );
    let o = dag(&["--catalogo", cat, "status"]);
    let out = String::from_utf8(o.stdout).unwrap();
    assert!(out.contains("Monitor: parado"), "{out}");
}

#[test]
fn monitorar_roda_ate_sinal_e_resume() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = arvore(tmp.path());
    let cat = tmp.path().join("c.sqlite");
    let cat = cat.to_str().unwrap();
    let r = raiz.to_str().unwrap();
    assert!(
        dag(&["--catalogo", cat, "scan", "--full", r])
            .status
            .success()
    );
    let filho = Command::new(env!("CARGO_BIN_EXE_dag"))
        .env_remove("DAG_CATALOGO")
        .args([
            "--catalogo",
            cat,
            "--json",
            "monitorar",
            "--reconciliar-a-cada",
            "0",
            r,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // Aguarda o monitor ficar ativo (saúde persistida) e envia SIGINT.
    let inicio = std::time::Instant::now();
    loop {
        let o = dag(&["--catalogo", cat, "--json", "status"]);
        let v = json(&o);
        if v["monitor"]["em_execucao"] == true && v["monitor"]["watches"].as_u64() > Some(0) {
            break;
        }
        assert!(inicio.elapsed().as_secs() < 15, "monitor não subiu: {v}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // Um segundo escritor é recusado.
    let o = dag(&["--catalogo", cat, "--json", "monitorar", r]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(json(&o)["codigo"], "escritor_ocupado");
    let pid = filho.id().to_string();
    assert!(
        Command::new("kill")
            .args(["-INT", &pid])
            .status()
            .unwrap()
            .success()
    );
    let o = filho.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(json(&o)["resposta"], "monitor_encerrado");
}

#[test]
fn intervalo_invalido_e_rejeitado() {
    assert!(
        !dag(&["monitorar", "--reconciliar-a-cada", "abc", "/tmp"])
            .status
            .success()
    );
}
