//! Laço JSON-RPC 2.0 do MCP (uma mensagem JSON por linha, sem quebras internas).

use crate::ferramentas::{montar, todas};
use crate::resumo::{resumir, texto_progresso};
use dag_agent::cliente::{Conexao, OpcoesConexao};
use dag_core::{Autor, Pedido, Progresso, Resposta};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

/// Versão do protocolo MCP implementada.
pub const VERSAO_PROTOCOLO: &str = "2025-06-18";

const PARSE: i64 = -32700;
const REQ_INVALIDA: i64 = -32600;
const SEM_METODO: i64 = -32601;
const PARAMS: i64 = -32602;

/// Ponte até o worker. `progresso` recebe linhas de andamento (varreduras longas).
pub trait Encaminhador {
    fn enviar(
        &mut self,
        pedido: Pedido,
        progresso: &mut dyn FnMut(&Progresso),
    ) -> Result<Resposta, String>;
}

/// Estado do servidor MCP.
#[derive(Debug)]
pub struct Servidor<E> {
    enc: E,
}

fn erro(id: &Value, codigo: i64, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": codigo, "message": msg}})
}

fn ok(id: &Value, resultado: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": resultado})
}

fn escrever(w: &mut dyn Write, v: &Value) -> io::Result<()> {
    // `to_string` nunca emite quebras de linha: um quadro por linha.
    writeln!(w, "{v}")?;
    w.flush()
}

fn resultado_ferramenta(texto: String, estruturado: Option<Value>, e: bool) -> Value {
    let mut r = json!({"content": [{"type": "text", "text": texto}], "isError": e});
    if let Some(s) = estruturado {
        r["structuredContent"] = s;
    }
    r
}

impl<E: Encaminhador> Servidor<E> {
    pub fn novo(enc: E) -> Self {
        Self { enc }
    }

    /// Trata uma linha de entrada, escrevendo zero ou mais quadros em `saida`.
    pub fn tratar_linha(&mut self, linha: &str, saida: &mut dyn Write) -> io::Result<()> {
        let msg: Value = match serde_json::from_str(linha) {
            Ok(v) => v,
            Err(e) => {
                return escrever(
                    saida,
                    &erro(&Value::Null, PARSE, &format!("JSON inválido: {e}")),
                );
            }
        };
        let Some(obj) = msg.as_object() else {
            return escrever(
                saida,
                &erro(&Value::Null, REQ_INVALIDA, "esperado um objeto JSON-RPC"),
            );
        };
        let id = obj.get("id").cloned();
        let Some(metodo) = obj.get("method").and_then(Value::as_str) else {
            // Resposta do cliente a algo nosso (não enviamos requisições): ignora.
            if id.is_some() && (obj.contains_key("result") || obj.contains_key("error")) {
                return Ok(());
            }
            return escrever(
                saida,
                &erro(
                    &id.unwrap_or(Value::Null),
                    REQ_INVALIDA,
                    "requisição sem `method`",
                ),
            );
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            // Notificação: nunca responde (inclui notifications/initialized e cancelled).
            return Ok(());
        };
        match metodo {
            "initialize" => escrever(saida, &ok(&id, inicializar())),
            "ping" => escrever(saida, &ok(&id, json!({}))),
            "tools/list" => {
                let ts: Vec<Value> = todas()
                    .into_iter()
                    .map(|t| {
                        json!({"name": t.nome, "description": t.descricao, "inputSchema": t.schema})
                    })
                    .collect();
                escrever(saida, &ok(&id, json!({"tools": ts})))
            }
            "tools/call" => self.chamar(&id, &params, saida),
            outro => escrever(
                saida,
                &erro(&id, SEM_METODO, &format!("método desconhecido: {outro}")),
            ),
        }
    }

    fn chamar(&mut self, id: &Value, params: &Value, saida: &mut dyn Write) -> io::Result<()> {
        let Some(nome) = params.get("name").and_then(Value::as_str) else {
            return escrever(saida, &erro(id, PARAMS, "faltou `name`"));
        };
        if !todas().iter().any(|t| t.nome == nome) {
            return escrever(
                saida,
                &erro(id, PARAMS, &format!("ferramenta desconhecida: {nome}")),
            );
        }
        let args = params.get("arguments").cloned().unwrap_or(Value::Null);
        let comando = match montar(nome, &args) {
            Ok(c) => c,
            Err(m) => {
                return escrever(saida, &ok(id, resultado_ferramenta(m, None, true)));
            }
        };
        let token = params
            .get("_meta")
            .and_then(|m| m.get("progressToken"))
            .cloned();
        let pedido = Pedido {
            autor: Autor::Mcp,
            comando,
        };
        let mut falha_io: Option<io::Error> = None;
        let mut passo = 0u64;
        let r = {
            let mut cb = |p: &Progresso| {
                let Some(tok) = &token else { return };
                passo += 1;
                let n = json!({"jsonrpc": "2.0", "method": "notifications/progress",
                    "params": {"progressToken": tok, "progress": passo, "message": texto_progresso(p)}});
                if let Err(e) = escrever(saida, &n) {
                    falha_io.get_or_insert(e);
                }
            };
            self.enc.enviar(pedido, &mut cb)
        };
        if let Some(e) = falha_io {
            return Err(e);
        }
        let res = match r {
            Err(m) => resultado_ferramenta(format!("Falha ao falar com o worker: {m}"), None, true),
            Ok(resp) => {
                let e = matches!(resp, Resposta::Erro { .. });
                let est = serde_json::to_value(&resp).ok();
                resultado_ferramenta(resumir(&resp), est, e)
            }
        };
        escrever(saida, &ok(id, res))
    }
}

fn inicializar() -> Value {
    json!({
        "protocolVersion": VERSAO_PROTOCOLO,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "dag", "title": "DAG: catalogador de acervo digital",
            "version": env!("CARGO_PKG_VERSION")},
        "instructions": "DAG cataloga discos e acha duplicados. Somente leitura sobre o acervo: \
    nada é apagado nem movido no disco; marcações só mudam o Catálogo. Ações de serviço podem abrir \
    um diálogo do sistema onde o usuário digita a senha; o MCP só fornece senhas de contêineres (`fornecer_senha`) e nunca recebe nenhuma de volta."
    })
}

/// Servidor stdio completo: conecta ao agente sob demanda (a cada chamada de ferramenta) e
/// atende stdin/stdout até o fim da entrada. Sobe o agente se não houver (ADR 0004).
///
/// # Errors
/// Falha de E/S no stdin/stdout.
pub fn servir_stdio(catalogo: &Path, socket: &Path) -> io::Result<()> {
    servir_stdio_com(catalogo, socket, OpcoesConexao::novo(false))
}

/// Como [`servir_stdio`], com as opções de conexão (`--sem-agente`, lançamento do agente).
///
/// # Errors
/// Falha de E/S no stdin/stdout.
pub fn servir_stdio_com(catalogo: &Path, socket: &Path, opcoes: OpcoesConexao) -> io::Result<()> {
    let enc = ConexaoPreguicosa {
        catalogo: catalogo.to_path_buf(),
        socket: socket.to_path_buf(),
        opcoes,
    };
    servir(enc, io::stdin().lock(), io::stdout().lock())
}

/// Liga-se ao agente só quando preciso: `initialize` e `tools/list` funcionam mesmo sem
/// Catálogo. Cada chamada decide a conexão pelo comando (o agente pode ter reiniciado desde a
/// anterior): escrita e varredura usam o agente (lançado se faltar), leitura usa o agente ou o
/// Catálogo somente leitura, e o controle de trabalhos exige o agente.
#[derive(Debug)]
struct ConexaoPreguicosa {
    catalogo: PathBuf,
    socket: PathBuf,
    opcoes: OpcoesConexao,
}

impl Encaminhador for ConexaoPreguicosa {
    fn enviar(
        &mut self,
        pedido: Pedido,
        progresso: &mut dyn FnMut(&Progresso),
    ) -> Result<Resposta, String> {
        let mut c =
            Conexao::abrir_para_com(&self.catalogo, &self.socket, &pedido.comando, &self.opcoes)
                .map_err(|e| e.to_string())?;
        c.enviar(pedido, progresso).map_err(|e| e.to_string())
    }
}

/// Lê quadros de `entrada` até o fim e responde em `saida`.
pub fn servir<E: Encaminhador, R: BufRead, W: Write>(
    enc: E,
    entrada: R,
    mut saida: W,
) -> io::Result<()> {
    let mut s = Servidor::novo(enc);
    for linha in entrada.lines() {
        let linha = linha?;
        if linha.trim().is_empty() {
            continue;
        }
        s.tratar_linha(&linha, &mut saida)?;
    }
    Ok(())
}
