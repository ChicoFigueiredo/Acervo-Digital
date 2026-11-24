#![allow(clippy::unwrap_used)]
use dag_core::{
    AcaoServico, Alvo, Autor, Comando, GrupoId, ModoVarredura, Pedido, Progresso, Resposta,
};
use dag_mcp::{Encaminhador, servir, todas};
use serde_json::{Value, json};
use std::io::Cursor;

#[derive(Default)]
struct Falso {
    pedidos: Vec<Pedido>,
    resposta: Option<Resposta>,
}

impl Encaminhador for &mut Falso {
    fn enviar(
        &mut self,
        p: Pedido,
        progresso: &mut dyn FnMut(&Progresso),
    ) -> Result<Resposta, String> {
        progresso(&Progresso::Mensagem {
            texto: "lendo".into(),
        });
        self.pedidos.push(p);
        Ok(self
            .resposta
            .clone()
            .unwrap_or(Resposta::Ok { mensagem: None }))
    }
}

fn rodar(f: &mut Falso, linhas: &[Value]) -> Vec<Value> {
    let entrada: String = linhas.iter().map(|l| format!("{l}\n")).collect();
    rodar_bruto(f, &entrada)
}

fn rodar_bruto(f: &mut Falso, entrada: &str) -> Vec<Value> {
    let mut saida = Vec::new();
    servir(f, Cursor::new(entrada.to_string()), &mut saida).unwrap();
    let txt = String::from_utf8(saida).unwrap();
    // stdout só pode ter um JSON por linha.
    txt.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn chamar(nome: &str, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":nome,"arguments":args}})
}

#[test]
fn handshake_e_ping() {
    let mut f = Falso::default();
    let r = rodar(
        &mut f,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":"a","method":"ping"}),
        ],
    );
    assert_eq!(r.len(), 2, "notificação não responde");
    assert_eq!(r[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(r[0]["result"]["serverInfo"]["name"], "dag");
    assert!(r[0]["result"]["capabilities"]["tools"].is_object());
    assert_eq!(r[1]["id"], "a");
    assert_eq!(r[1]["result"], json!({}));
}

#[test]
fn lista_todas_as_ferramentas_com_schema_valido() {
    let mut f = Falso::default();
    let r = rodar(
        &mut f,
        &[json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    let ts = r[0]["result"]["tools"].as_array().unwrap();
    let nomes: Vec<&str> = ts.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for esperado in [
        "listar_volumes",
        "scan_full",
        "scan_fast",
        "retomar",
        "pausar",
        "cancelar",
        "status",
        "explorar",
        "listar_duplicados",
        "detalhar_grupo",
        "definir_copia_principal",
        "marcar_backup",
        "marcar_delecao",
        "desmarcar",
        "confirmar_marcacao",
        "definir_prioridade",
        "relatorio",
        "verificar_integridade",
        "eventos",
        "saude_monitor",
        "servico",
        "backup_catalogo",
    ] {
        assert!(nomes.contains(&esperado), "falta {esperado}");
    }
    assert_eq!(nomes.len(), todas().len());
    for t in ts {
        let s = &t["inputSchema"];
        assert_eq!(s["type"], "object");
        assert!(s["properties"].is_object());
        for req in s["required"].as_array().unwrap() {
            assert!(s["properties"].get(req.as_str().unwrap()).is_some());
        }
        assert!(!t["description"].as_str().unwrap().is_empty());
    }
    let marcar = ts.iter().find(|t| t["name"] == "marcar_delecao").unwrap();
    assert!(
        marcar["description"]
            .as_str()
            .unwrap()
            .contains("NADA é apagado")
    );
    let serv = ts.iter().find(|t| t["name"] == "servico").unwrap();
    assert!(serv["description"].as_str().unwrap().contains("senha"));
}

fn comando_de(nome: &str, args: Value) -> (Comando, Autor) {
    let mut f = Falso::default();
    let r = rodar(&mut f, &[chamar(nome, args)]);
    assert_eq!(r[0]["result"]["isError"], false, "{r:?}");
    let p = f.pedidos.remove(0);
    (p.comando, p.autor)
}

#[test]
fn chamadas_montam_comando_com_autor_mcp() {
    let (c, a) = comando_de("scan_full", json!({"raizes":["/x"],"sem_hash":true}));
    assert_eq!(a, Autor::Mcp);
    assert_eq!(
        c,
        Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec!["/x".into()],
            sem_hash: true,
            elevar: false,
            releitura_total: false,
            threads_por_disco: None,
            max_discos: None,
        }
    );
    let (c, _) = comando_de("scan_fast", json!({}));
    assert_eq!(
        c,
        Comando::IniciarScan {
            modo: ModoVarredura::FastScan,
            raizes: vec![],
            sem_hash: false,
            elevar: false,
            releitura_total: false,
            threads_por_disco: None,
            max_discos: None,
        }
    );
    let (c, _) = comando_de("marcar_delecao", json!({"alvo":{"tipo":"grupo","valor":3}}));
    assert_eq!(
        c,
        Comando::MarcarDelecao {
            alvo: Alvo::Grupo(GrupoId(3))
        }
    );
    let (c, _) = comando_de(
        "marcar_backup",
        json!({"alvo":{"tipo":"volume","valor":"HD"}}),
    );
    assert_eq!(
        c,
        Comando::MarcarBackup {
            alvo: Alvo::Volume("HD".into())
        }
    );
    let (c, _) = comando_de("definir_copia_principal", json!({"grupo":2}));
    assert_eq!(
        c,
        Comando::DefinirCopiaPrincipal {
            grupo: GrupoId(2),
            arquivo: None
        }
    );
    let (c, _) = comando_de("servico", json!({"acao":"autostart","ativo":false}));
    assert_eq!(
        c,
        Comando::Servico {
            acao: AcaoServico::Autostart { ativo: false }
        }
    );
    let (c, _) = comando_de("servico", json!({"acao":"instalar"}));
    assert_eq!(
        c,
        Comando::Servico {
            acao: AcaoServico::Instalar
        }
    );
    let (c, _) = comando_de(
        "listar_duplicados",
        json!({"volume":"SSD","tamanho_minimo":10,"ordem":"tamanho","limite":5}),
    );
    assert!(matches!(
        c,
        Comando::ListarDuplicados {
            tamanho_minimo: Some(10),
            limite: Some(5),
            ..
        }
    ));
    let (c, _) = comando_de(
        "relatorio",
        json!({"tipo":"espaco","nivel":"pasta","saida":"/t/r.csv"}),
    );
    assert!(matches!(
        c,
        Comando::Relatorio {
            saida: Some(_),
            nivel: Some(_),
            ..
        }
    ));
    let (c, _) = comando_de("backup_catalogo", json!({"destino":"/b"}));
    assert_eq!(
        c,
        Comando::BackupCatalogo {
            destino: "/b".into()
        }
    );
    let (c, _) = comando_de("verificar_integridade", json!({}));
    assert_eq!(
        c,
        Comando::VerificarIntegridade {
            raizes: vec![],
            threads_por_disco: None,
            max_discos: None
        }
    );
    let (c, _) = comando_de("status", json!({"execucao":4}));
    assert!(matches!(c, Comando::Status { execucao: Some(_) }));
    let (c, _) = comando_de("saude_monitor", json!({}));
    assert_eq!(c, Comando::SaudeMonitor);
}

#[test]
fn resultado_tem_texto_e_estruturado_e_erro_vira_is_error() {
    let mut f = Falso {
        resposta: Some(Resposta::Erro {
            codigo: "x".into(),
            mensagem: "ruim".into(),
        }),
        ..Default::default()
    };
    let r = rodar(&mut f, &[chamar("listar_volumes", json!({}))]);
    let res = &r[0]["result"];
    assert_eq!(res["isError"], true);
    assert!(res["content"][0]["text"].as_str().unwrap().contains("ruim"));
    assert_eq!(res["structuredContent"]["resposta"], "erro");
}

#[test]
fn argumentos_invalidos_viram_is_error() {
    let mut f = Falso::default();
    let r = rodar(
        &mut f,
        &[
            chamar("scan_full", json!({})),
            chamar("servico", json!({"acao":"autostart"})),
            chamar("status", json!({"lixo":1})),
            chamar("marcar_backup", json!({"alvo":{"tipo":"zzz","valor":1}})),
        ],
    );
    for x in &r {
        assert_eq!(x["result"]["isError"], true, "{x}");
    }
    assert!(f.pedidos.is_empty());
}

#[test]
fn ferramenta_desconhecida_e_metodo_desconhecido() {
    let mut f = Falso::default();
    let r = rodar(
        &mut f,
        &[
            chamar("apagar_tudo", json!({})),
            json!({"jsonrpc":"2.0","id":2,"method":"resources/list"}),
        ],
    );
    assert_eq!(r[0]["error"]["code"], -32602);
    assert_eq!(r[1]["error"]["code"], -32601);
    assert!(f.pedidos.is_empty());
}

#[test]
fn json_malformado_e_requisicao_invalida() {
    let mut f = Falso::default();
    let r = rodar_bruto(&mut f, "{nao e json\n[1]\n{\"id\":3}\n");
    assert_eq!(r[0]["error"]["code"], -32700);
    assert_eq!(r[0]["id"], Value::Null);
    assert_eq!(r[1]["error"]["code"], -32600);
    assert_eq!(r[2]["error"]["code"], -32600);
    assert_eq!(r[2]["id"], 3);
}

#[test]
fn progresso_so_com_token() {
    let mut f = Falso::default();
    let com = json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"scan_fast","arguments":{},"_meta":{"progressToken":"t1"}}});
    let r = rodar(&mut f, &[com]);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["method"], "notifications/progress");
    assert_eq!(r[0]["params"]["progressToken"], "t1");
    assert_eq!(r[1]["id"], 9);
    let r = rodar(&mut f, &[chamar("scan_fast", json!({}))]);
    assert_eq!(r.len(), 1);
}
