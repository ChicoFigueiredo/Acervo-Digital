//! `dag mcp` de ponta a ponta: processo real, Catálogo temporário, sem agente (modo sessão).
#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn mcp_scan_full_e_listar_duplicados() {
    let tmp = tempfile::tempdir().unwrap();
    let arvore = tmp.path().join("acervo");
    std::fs::create_dir_all(arvore.join("a")).unwrap();
    std::fs::create_dir_all(arvore.join("b")).unwrap();
    let conteudo = vec![7u8; 4096];
    std::fs::write(arvore.join("a/x.bin"), &conteudo).unwrap();
    std::fs::write(arvore.join("b/y.bin"), &conteudo).unwrap();
    std::fs::write(arvore.join("b/unico.txt"), b"so um").unwrap();

    let mut filho = Command::new(env!("CARGO_BIN_EXE_dag"))
        .arg("mcp")
        .env("DAG_CATALOGO", tmp.path().join("cat.sqlite"))
        .env("DAG_SOCKET", tmp.path().join("sem-agente.sock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut entrada = filho.stdin.take().unwrap();
    let mut saida = BufReader::new(filho.stdout.take().unwrap());

    let mut pedir = |v: Value| -> Value {
        writeln!(entrada, "{v}").unwrap();
        entrada.flush().unwrap();
        if v.get("id").is_none() {
            return Value::Null; // notificação: sem resposta
        }
        let mut l = String::new();
        saida.read_line(&mut l).unwrap();
        // Todo byte do stdout é um quadro JSON-RPC.
        serde_json::from_str(&l).unwrap_or_else(|e| panic!("stdout não-JSON ({e}): {l:?}"))
    };

    let r = pedir(
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
    );
    assert_eq!(r["result"]["serverInfo"]["name"], "dag");
    pedir(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    let r = pedir(
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
        "name":"scan_full","arguments":{"raizes":[arvore.to_str().unwrap()]}}}),
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(
        r["result"]["structuredContent"]["resposta"],
        "scan_concluido"
    );

    let r = pedir(
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
        "name":"listar_duplicados","arguments":{}}}),
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    let sc = &r["result"]["structuredContent"];
    assert_eq!(sc["resposta"], "duplicados");
    assert_eq!(sc["total"], 1, "{sc}");

    // CA-A28: marcação e relatório pelo MCP; o arquivo marcado não muda (conteúdo e mtime).
    let alvo = arvore.join("b/y.bin");
    let antes = (
        std::fs::read(&alvo).unwrap(),
        std::fs::metadata(&alvo).unwrap().modified().unwrap(),
    );
    let r = pedir(
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
        "name":"marcar_delecao","arguments":{"alvo":{"tipo":"caminho","valor":alvo.to_str().unwrap()}}}}),
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(r["result"]["structuredContent"]["resposta"], "marcacao");
    let r = pedir(
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{
        "name":"relatorio","arguments":{"tipo":"marcacoes"}}}),
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    let dados = &r["result"]["structuredContent"]["dados"];
    let marcacoes = dados["marcacoes"]
        .as_array()
        .unwrap_or_else(|| panic!("{r}"));
    assert_eq!(marcacoes.len(), 1, "{r}");
    assert!(marcacoes[0].to_string().contains("y.bin"), "{r}");
    let depois = (
        std::fs::read(&alvo).unwrap(),
        std::fs::metadata(&alvo).unwrap().modified().unwrap(),
    );
    assert_eq!(antes, depois, "o MCP só altera o Catálogo");

    drop(entrada);
    assert!(filho.wait().unwrap().success());
}
