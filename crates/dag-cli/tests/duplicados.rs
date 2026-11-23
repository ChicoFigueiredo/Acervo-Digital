//! CLI de ponta a ponta: scan → duplicados → marcar → relatório (CA-A28, metade CLI).
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn dag(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dag"))
        .env_remove("DAG_CATALOGO")
        .args(args)
        .output()
        .unwrap()
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

fn texto(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn ok(o: &Output) {
    assert_eq!(
        o.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

/// `acervo/{a/foto.jpg, b/foto_copia.jpg (iguais, 6000), c/so_eu.txt (4000), d/g1, d/g2 (iguais, 100)}`
fn arvore(tmp: &Path) -> PathBuf {
    let raiz = fs::canonicalize(tmp).unwrap().join("acervo");
    for (rel, letra, n) in [
        ("a/foto.jpg", b'f', 6000),
        ("b/foto_copia.jpg", b'f', 6000),
        ("c/so_eu.txt", b's', 4000),
        ("d/g1", b'g', 100),
        ("d/g2", b'g', 100),
    ] {
        let p = raiz.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![letra; n]).unwrap();
    }
    raiz
}

fn instantaneo(raiz: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime, Vec<u8>)> {
    let mut v = Vec::new();
    for d in ["a", "b", "c", "d"] {
        for e in fs::read_dir(raiz.join(d)).unwrap() {
            let p = e.unwrap().path();
            let m = fs::metadata(&p).unwrap();
            v.push((
                p.clone(),
                m.len(),
                m.modified().unwrap(),
                fs::read(&p).unwrap(),
            ));
        }
    }
    v.sort();
    v
}

#[test]
fn scan_duplicados_marcar_relatorio_de_ponta_a_ponta() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = arvore(tmp.path());
    let cat = tmp.path().join("dados/c.sqlite");
    let (cat, r) = (cat.to_str().unwrap(), raiz.to_str().unwrap());
    let c = |extra: &[&str]| {
        let mut a = vec!["--catalogo", cat];
        a.extend_from_slice(extra);
        dag(&a)
    };

    // 1. scan completo: hash + grupos
    let o = c(&["--json", "scan", "--full", r]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["resposta"], "scan_concluido");
    assert_eq!(v["hash"]["hasheados"], 5);
    assert_eq!(v["duplicados"]["grupos"], 2);
    // saída humana do scan
    let o = c(&["scan", "--full", r]);
    ok(&o);
    let t = texto(&o);
    assert!(
        t.contains("Hash Completo") && t.contains("Duplicados: 2 grupo(s)"),
        "{t}"
    );
    assert!(
        t.contains("5 reaproveitados"),
        "segundo scan não relê nada: {t}"
    );

    // 2. duplicados (JSON e humano), ordenado pelo recuperável
    let o = c(&["--json", "duplicados"]);
    ok(&o);
    let v = json(&o);
    assert_eq!(v["total"], 2);
    let g = &v["grupos"][0];
    assert_eq!(g["tamanho"], 6000);
    assert_eq!(g["copias"], 2);
    assert_eq!(g["espaco_recuperavel_bytes"], 6000);
    let gid = g["id"].as_i64().unwrap().to_string();
    let o = c(&["duplicados"]);
    ok(&o);
    let t = texto(&o);
    assert!(
        t.contains("GRUPO") && t.contains("RECUPERÁVEL") && t.contains("2 grupo(s) no total"),
        "{t}"
    );
    // filtros e paginação
    let o = c(&["--json", "duplicados", "--min-tamanho", "1K"]);
    assert_eq!(json(&o)["total"], 1);
    let o = c(&["--json", "duplicados", "--limite", "1"]);
    let v = json(&o);
    assert_eq!(v["grupos"].as_array().unwrap().len(), 1);
    let cursor = v["proximo_cursor"].as_str().unwrap().to_string();
    let o = c(&["--json", "duplicados", "--limite", "1", "--cursor", &cursor]);
    let v = json(&o);
    assert_eq!(v["grupos"][0]["tamanho"], 100);
    assert!(v["proximo_cursor"].is_null());
    assert_eq!(
        json(&c(&["--json", "duplicados", "--volume", "9999"]))["codigo"],
        "nao_encontrado"
    );

    // 3. grupo
    let o = c(&["grupo", &gid]);
    ok(&o);
    let t = texto(&o);
    assert!(
        t.contains("foto.jpg") && t.contains("foto_copia.jpg") && t.contains("COBERTURA"),
        "{t}"
    );

    // 4. marcar (nada no acervo muda)
    let antes = instantaneo(&raiz);
    let copia = raiz.join("b/foto_copia.jpg");
    let copia = copia.to_str().unwrap();
    let o = c(&["marcar", "delecao", "--arquivo", copia]);
    ok(&o);
    let t = texto(&o);
    assert!(
        t.contains("Marcação para Deleção #1") && !t.contains("ÚNICA CÓPIA"),
        "{t}"
    );
    // Única Cópia: alerta em destaque.
    let unico = raiz.join("c/so_eu.txt");
    let o = c(&["marcar", "delecao", "--arquivo", unico.to_str().unwrap()]);
    ok(&o);
    let t = texto(&o);
    assert!(
        t.contains("⚠ ÚNICA CÓPIA") && t.contains("so_eu.txt"),
        "{t}"
    );
    let o = c(&[
        "--json",
        "marcar",
        "delecao",
        "--arquivo",
        unico.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(json(&o)["codigo"], "conflito_marcacao");
    // marcar grupo (Backup: todas as cópias) e regra de pasta
    let o = c(&[
        "--json",
        "marcar",
        "backup",
        "--pasta",
        raiz.join("d").to_str().unwrap(),
    ]);
    ok(&o);
    assert_eq!(
        json(&o)["resultado"]["marcacoes"][0]["escopo"],
        "regra_pasta"
    );
    // alvo é obrigatório e exclusivo
    assert!(!c(&["marcar", "backup"]).status.success());
    assert!(
        !c(&["marcar", "backup", "--grupo", "1", "--pasta", "/x"])
            .status
            .success()
    );
    assert_eq!(instantaneo(&raiz), antes, "o DAG nunca altera o acervo");

    // 5. relatórios
    let o = c(&["relatorio", "marcacoes"]);
    ok(&o);
    let t = texto(&o);
    assert!(
        t.contains("deleção") && t.contains("backup") && t.contains("regra de pasta"),
        "{t}"
    );
    let csv = tmp.path().join("m.csv");
    let o = c(&["relatorio", "marcacoes", "--saida", csv.to_str().unwrap()]);
    ok(&o);
    assert!(texto(&o).contains("3 linha(s)"), "{}", texto(&o));
    assert_eq!(fs::read_to_string(&csv).unwrap().lines().count(), 4);
    let json_saida = tmp.path().join("m.json");
    ok(&c(&[
        "relatorio",
        "marcacoes",
        "--saida",
        json_saida.to_str().unwrap(),
    ]));
    let doc: serde_json::Value = serde_json::from_slice(&fs::read(&json_saida).unwrap()).unwrap();
    assert_eq!(doc["linhas"].as_array().unwrap().len(), 3);
    // não sobrescreve; extensão inválida
    assert_eq!(
        c(&["relatorio", "marcacoes", "--saida", csv.to_str().unwrap()])
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        c(&["relatorio", "marcacoes", "--saida", "/tmp/x.txt"])
            .status
            .code(),
        Some(1)
    );

    let o = c(&["relatorio", "espaco"]);
    ok(&o);
    let t = texto(&o);
    assert!(
        t.contains("Espaço recuperável deste computador") && t.contains("reflink"),
        "{t}"
    );
    let v = json(&c(&["--json", "relatorio", "espaco", "--nivel", "pasta"]));
    assert_eq!(v["dados"]["total"]["bytes_marcados_delecao"], 6000 + 4000);
    let o = c(&["relatorio", "sem-cobertura"]);
    ok(&o);
    assert!(texto(&o).contains("ÚNICA CÓPIA"), "{}", texto(&o));
    ok(&c(&["relatorio", "cobertura"]));

    // 6. principal, prioridade, desmarcar, confirmar
    let foto = raiz.join("b/foto_copia.jpg");
    let o = c(&["principal", &gid, foto.to_str().unwrap()]);
    ok(&o);
    assert!(texto(&o).contains("manual"), "{}", texto(&o));
    ok(&c(&["principal", &gid, "--auto"]));
    let o = c(&["prioridade", raiz.to_str().unwrap()]);
    ok(&o);
    assert!(texto(&o).contains("Prioridade"), "{}", texto(&o));
    let o = c(&["desmarcar", "--arquivo", copia]);
    ok(&o);
    assert!(texto(&o).contains("removida"));
    assert_eq!(c(&["desmarcar", "--arquivo", copia]).status.code(), Some(1));
    assert_eq!(c(&["--json", "confirmar", "999"]).status.code(), Some(1));

    // 7. conteúdo muda e o Full Scan torna a marcação pendente (CA-A24 pela CLI)
    let o = c(&["marcar", "delecao", "--arquivo", copia]);
    ok(&o);
    fs::write(&foto, vec![b'X'; 6000]).unwrap();
    let o = c(&["--json", "scan", "--full", r]);
    ok(&o);
    assert_eq!(json(&o)["duplicados"]["marcacoes_pendentes"], 1);
    let o = c(&["relatorio", "marcacoes"]);
    assert!(texto(&o).contains("PENDENTE"), "{}", texto(&o));
    let mid = json(&c(&["--json", "relatorio", "marcacoes"]))["dados"]["marcacoes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["estado"] == "pendente")
        .unwrap()["id"]
        .as_i64()
        .unwrap()
        .to_string();
    let o = c(&["confirmar", &mid]);
    ok(&o);

    // 8. verificar e --sem-hash
    let o = c(&["verificar"]);
    ok(&o);
    assert!(
        texto(&o).contains("Verificação de Integridade concluída"),
        "{}",
        texto(&o)
    );
    let o = c(&["--json", "scan", "--full", "--sem-hash", r]);
    ok(&o);
    let v = json(&o);
    assert!(v["hash"].is_null() && v["duplicados"].is_null());
}

#[test]
fn help_lista_os_novos_subcomandos_e_leitura_nao_cria_catalogo() {
    let o = dag(&["--help"]);
    let t = texto(&o);
    for s in [
        "duplicados",
        "grupo",
        "principal",
        "confirmar",
        "relatorio",
        "verificar",
    ] {
        assert!(t.contains(s), "{s}: {t}");
    }
    let o = dag(&["scan", "--help"]);
    assert!(texto(&o).contains("--sem-hash"));
    let tmp = tempfile::tempdir().unwrap();
    let cat = tmp.path().join("nada/c.sqlite");
    for args in [
        vec!["duplicados"],
        vec!["grupo", "1"],
        vec!["relatorio", "espaco"],
    ] {
        let mut a = vec!["--catalogo", cat.to_str().unwrap(), "--json"];
        a.extend(args);
        let o = dag(&a);
        assert_eq!(o.status.code(), Some(1));
        assert_eq!(json(&o)["codigo"], "sem_catalogo");
    }
    assert!(!cat.parent().unwrap().exists());
}
