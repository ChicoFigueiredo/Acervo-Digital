//! `dag biblioteca` e `dag modelos` de ponta a ponta, sem agente em segundo plano (sessão em
//! processo), com Catálogo temporário. Nada aqui precisa do Calibre nem de servidor de IA.
#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

fn dag(cat: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_dag"))
        .env("DAG_SEM_AGENTE", "1")
        .env_remove("DAG_CATALOGO")
        .arg("--catalogo")
        .arg(cat)
        .args(args)
        .output()
        .unwrap()
}

/// Cria o Catálogo vazio (como `dag volumes`, que é quem o cria na primeira vez).
fn criar(cat: &Path) {
    let o = dag(cat, &["volumes"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

fn json(o: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout não é JSON ({e}): {}",
            String::from_utf8_lossy(&o.stdout)
        )
    })
}

fn texto(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn help_lista_biblioteca_e_modelos() {
    let t = tempfile::tempdir().unwrap();
    let o = dag(&t.path().join("c.sqlite"), &["--help"]);
    let h = texto(&o.stdout);
    assert!(h.contains("biblioteca") && h.contains("modelos"), "{h}");
    let o = dag(&t.path().join("c.sqlite"), &["biblioteca", "--help"]);
    let h = texto(&o.stdout);
    for s in [
        "registrar",
        "listar",
        "perfil",
        "sincronizar",
        "duplicatas",
        "espaco",
        "enfileirar",
        "fila",
        "processar",
        "revisar",
        "decidir",
        "desfazer",
        "cadastrar",
        "homologar",
        "alertas",
        "visto",
        "resumo",
        "config",
        "ouro",
    ] {
        assert!(h.contains(s), "help sem `{s}`:\n{h}");
    }
}

#[test]
fn listar_e_config_pelo_agente_de_sessao() {
    let t = tempfile::tempdir().unwrap();
    let cat = t.path().join("c.sqlite");
    criar(&cat);
    let o = dag(&cat, &["--json", "biblioteca", "listar"]);
    assert!(o.status.success(), "{}", texto(&o.stderr));
    let v = json(&o);
    assert_eq!(v["resposta"], "biblioteca");
    assert_eq!(v["dados"]["tipo"], "bibliotecas");
    let o = dag(&cat, &["biblioteca", "listar"]);
    assert!(texto(&o.stdout).contains("Nenhuma biblioteca registrada"));

    let o = dag(&cat, &["--json", "biblioteca", "config"]);
    assert_eq!(json(&o)["dados"]["config"]["ia"]["ativa"], false);
    let o = dag(
        &cat,
        &[
            "--json",
            "biblioteca",
            "config",
            "definir",
            "ia.ativa=true",
            "ocr.dpi=200",
        ],
    );
    assert!(o.status.success(), "{}", texto(&o.stderr));
    let o = dag(&cat, &["--json", "biblioteca", "config", "obter"]);
    let c = &json(&o)["dados"]["config"];
    assert_eq!(c["ia"]["ativa"], true);
    assert_eq!(c["ocr"]["dpi"], 200);
    let o = dag(&cat, &["biblioteca", "config"]);
    assert!(texto(&o.stdout).contains("ia.ativa = true"));
    // Endpoint de IA fora do loopback é recusado pelo agente, com dica.
    let o = dag(
        &cat,
        &[
            "biblioteca",
            "config",
            "definir",
            "ia.endpoint=http://exemplo.com:11434",
        ],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(texto(&o.stderr).contains("erro:"), "{}", texto(&o.stderr));
}

#[test]
fn homologar_cadastro_inexistente_nao_pergunta_nada_e_orienta() {
    let t = tempfile::tempdir().unwrap();
    let cat = t.path().join("c.sqlite");
    criar(&cat);
    let o = dag(
        &cat,
        &["biblioteca", "homologar", "99", "--aprovado-por", "Chico"],
    );
    assert_eq!(o.status.code(), Some(1));
    let e = texto(&o.stderr);
    assert!(e.contains("erro:") && e.contains("dica:"), "{e}");
    assert!(e.contains("cadastrar"), "{e}");
}

#[test]
fn decidir_e_resumo_validam_argumentos_antes_de_chamar_o_agente() {
    let t = tempfile::tempdir().unwrap();
    let cat = t.path().join("c.sqlite");
    let o = dag(
        &cat,
        &[
            "--json",
            "biblioteca",
            "decidir",
            "--alvo",
            "1",
            "--campo",
            "titulo",
        ],
    );
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(json(&o)["codigo"], "argumento_invalido");
    let o = dag(
        &cat,
        &["--json", "biblioteca", "resumo", "--desde", "ontem"],
    );
    assert_eq!(json(&o)["codigo"], "argumento_invalido");
    assert!(!cat.exists(), "argumento inválido não abre o Catálogo");
    criar(&cat);
    // Resumo válido responde.
    let o = dag(&cat, &["--json", "biblioteca", "resumo", "--desde", "7d"]);
    assert!(o.status.success(), "{}", texto(&o.stderr));
    assert_eq!(json(&o)["dados"]["tipo"], "resumo");
}

#[test]
fn ouro_exportar_e_avaliar_de_ponta_a_ponta_sem_itens() {
    let t = tempfile::tempdir().unwrap();
    let cat = t.path().join("c.sqlite");
    let saida = t.path().join("ouro");
    criar(&cat);
    // A pasta precisa existir.
    let o = dag(
        &cat,
        &[
            "biblioteca",
            "ouro",
            "exportar",
            "--n",
            "5",
            "--saida",
            saida.to_str().unwrap(),
        ],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(
        texto(&o.stderr).contains("crie-a antes"),
        "{}",
        texto(&o.stderr)
    );
    std::fs::create_dir(&saida).unwrap();
    let o = dag(
        &cat,
        &[
            "biblioteca",
            "ouro",
            "exportar",
            "--n",
            "5",
            "--saida",
            saida.to_str().unwrap(),
        ],
    );
    assert!(o.status.success(), "{}", texto(&o.stderr));
    assert!(
        texto(&o.stdout).contains("0 caso(s) novo(s)"),
        "{}",
        texto(&o.stdout)
    );
    // Avaliar uma pasta com um caso preenchido à mão (sem agente).
    let caso = serde_json::json!({
        "id": "ouro-000001",
        "file_id": 1,
        "entrada": {"nome_arquivo": "a.epub", "extracao": {"formato": "epub", "estado": {"estado": "lido"}}},
        "gabarito": {"titulo": "O Jogo da Vida"},
        "gabarito_extra": {},
        "sugestao_dag": {"titulo": {"valor": "O Jogo da Vida", "lista": [], "confianca": 0.97}},
        "vinculo_dag": {"obra_id": 1, "edicao_id": 1},
        "duplicata_exata_dag": false
    });
    std::fs::write(saida.join("ouro-000001.json"), caso.to_string()).unwrap();
    let o = dag(
        &cat,
        &[
            "biblioteca",
            "ouro",
            "avaliar",
            "--casos",
            saida.to_str().unwrap(),
        ],
    );
    assert!(o.status.success(), "{}", texto(&o.stderr));
    let s = texto(&o.stdout);
    assert!(
        s.contains("1 com gabarito preenchido") && s.contains("100,0%"),
        "{s}"
    );
    let o = dag(
        &cat,
        &[
            "--json",
            "biblioteca",
            "ouro",
            "avaliar",
            "--casos",
            saida.to_str().unwrap(),
        ],
    );
    let v = json(&o);
    assert_eq!(v["com_gabarito"], 1);
    assert_eq!(v["campos"][0]["campo"], "titulo");
    let o = dag(
        &cat,
        &["biblioteca", "ouro", "avaliar", "--casos", "/nao/existe"],
    );
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn modelos_listar_com_servidor_inalcancavel_e_benchmark_sem_casos() {
    let t = tempfile::tempdir().unwrap();
    let cat = t.path().join("c.sqlite");
    let perfis = t.path().join("p.json");
    std::fs::write(
        &perfis,
        r#"{"modelos":[{"nome":"m1","endpoint":"http://127.0.0.1:1","backend":"ollama","papel":"primario"}]}"#,
    )
    .unwrap();
    let o = dag(
        &cat,
        &["modelos", "listar", "--perfis", perfis.to_str().unwrap()],
    );
    assert!(o.status.success());
    let s = texto(&o.stdout);
    assert!(s.contains("m1") && s.contains("inalcançável"), "{s}");
    let o = dag(
        &cat,
        &[
            "--json",
            "modelos",
            "listar",
            "--perfis",
            perfis.to_str().unwrap(),
        ],
    );
    assert_eq!(json(&o)[0]["nome"], "m1");
    let vazio = t.path().join("vazio");
    std::fs::create_dir(&vazio).unwrap();
    let o = dag(
        &cat,
        &[
            "modelos",
            "benchmark",
            "--casos",
            vazio.to_str().unwrap(),
            "--modelo",
            "m1",
        ],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(texto(&o.stderr).contains("nenhum caso com gabarito"));
    assert!(!cat.exists(), "modelos não abre o Catálogo");
}
