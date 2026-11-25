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
        "reconstruir_duplicados",
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
        "mover_para_lixeira",
        "restaurar_da_lixeira",
        "listar_lixeira",
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

// ---------------------------------------------------------------------- Lixeira (ADR 0005)

#[test]
fn mover_para_lixeira_exige_as_duas_confirmacoes_explicitas() {
    let alvos = json!([{"tipo":"caminho","valor":"/mnt/d/a.bin"}]);
    for args in [
        json!({"alvos": alvos}),
        json!({"alvos": alvos, "confirmacao_1": true}),
        json!({"alvos": alvos, "confirmacao_2": true}),
        json!({"alvos": alvos, "confirmacao_1": true, "confirmacao_2": false}),
        json!({"alvos": alvos, "confirmacao_1": "true", "confirmacao_2": "true"}),
        json!({"alvos": alvos, "confirmacao_unica_copia": true}),
    ] {
        let mut f = Falso::default();
        let r = rodar(&mut f, &[chamar("mover_para_lixeira", args.clone())]);
        assert_eq!(r[0]["result"]["isError"], true, "{args}");
        let t = r[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(t.contains("duas confirmações"), "{t}");
        assert!(f.pedidos.is_empty(), "nada pode chegar ao agente: {args}");
    }
    // Alvos vazios: recusado mesmo com as confirmações.
    let mut f = Falso::default();
    let r = rodar(
        &mut f,
        &[chamar(
            "mover_para_lixeira",
            json!({"alvos": [], "confirmacao_1": true, "confirmacao_2": true}),
        )],
    );
    assert_eq!(r[0]["result"]["isError"], true);
    assert!(f.pedidos.is_empty());
    // Com as duas, o comando segue (as confirmações não vão para o agente).
    let (c, a) = comando_de(
        "mover_para_lixeira",
        json!({"alvos": alvos, "confirmacao_1": true, "confirmacao_2": true,
               "confirmacao_unica_copia": true}),
    );
    assert_eq!(a, Autor::Mcp);
    assert_eq!(
        c,
        Comando::MoverParaLixeira {
            alvos: vec![Alvo::Caminho("/mnt/d/a.bin".into())],
            confirmacao_unica_copia: true,
            marcar_tudo: false,
            simular: false,
        }
    );
    // Simulação não exige confirmação: só planeja.
    let (c, _) = comando_de(
        "mover_para_lixeira",
        json!({"alvos": alvos, "simular": true}),
    );
    assert!(matches!(c, Comando::MoverParaLixeira { simular: true, .. }));
}

#[test]
fn restaurar_e_listar_lixeira_montam_comandos() {
    let (c, _) = comando_de(
        "restaurar_da_lixeira",
        json!({"alvos":[{"tipo":"arquivo","valor":4},{"tipo":"volume","valor":"HD"}]}),
    );
    assert_eq!(
        c,
        Comando::RestaurarDaLixeira {
            alvos: vec![
                Alvo::Arquivo(dag_core::ArquivoId(4)),
                Alvo::Volume("HD".into())
            ]
        }
    );
    let (c, _) = comando_de("listar_lixeira", json!({"limite": 5}));
    assert_eq!(
        c,
        Comando::ListarLixeira {
            limite: Some(5),
            cursor: None
        }
    );
    let mut f = Falso::default();
    let r = rodar(&mut f, &[chamar("restaurar_da_lixeira", json!({}))]);
    assert_eq!(r[0]["result"]["isError"], true);
}

#[test]
fn descricao_do_mover_avisa_das_confirmacoes_e_so_o_usuario_esvazia() {
    let mut f = Falso::default();
    let r = rodar(
        &mut f,
        &[json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    let ts = r[0]["result"]["tools"].as_array().unwrap();
    let m = ts
        .iter()
        .find(|t| t["name"] == "mover_para_lixeira")
        .unwrap();
    let d = m["description"].as_str().unwrap();
    assert!(
        d.contains("DUAS confirmações") && d.contains("esvaziar"),
        "{d}"
    );
    let req: Vec<_> = m["inputSchema"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    assert_eq!(req, [&json!("alvos")]);
    assert!(m["inputSchema"]["properties"]["confirmacao_1"].is_object());
    assert!(m["inputSchema"]["properties"]["confirmacao_2"].is_object());
}

// ---------------------------------------------------------------------------------------------
// Curadoria Bibliográfica
// ---------------------------------------------------------------------------------------------

const FERRAMENTAS_BIBLIOTECA: [&str; 19] = [
    "biblioteca_listar",
    "biblioteca_perfil",
    "biblioteca_duplicatas",
    "biblioteca_espaco",
    "biblioteca_fila",
    "biblioteca_revisao",
    "biblioteca_alertas",
    "biblioteca_resumo",
    "biblioteca_config_obter",
    "biblioteca_registrar",
    "biblioteca_sincronizar",
    "biblioteca_enfileirar",
    "biblioteca_processar",
    "biblioteca_marcar_visto",
    "biblioteca_decidir",
    "biblioteca_desfazer",
    "biblioteca_cadastrar",
    "biblioteca_propor_homologacao",
    "biblioteca_config_definir",
];

fn recusa_de(nome: &str, args: Value) -> String {
    let mut f = Falso::default();
    let r = rodar(&mut f, &[chamar(nome, args)]);
    assert_eq!(r[0]["result"]["isError"], true, "{r:?}");
    assert!(f.pedidos.is_empty(), "nada pode ir ao worker");
    r[0]["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn biblioteca_tools_list_com_schema_estrito() {
    let mut f = Falso::default();
    let r = rodar(
        &mut f,
        &[json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    let ts = r[0]["result"]["tools"].as_array().unwrap();
    for nome in FERRAMENTAS_BIBLIOTECA {
        let t = ts
            .iter()
            .find(|t| t["name"] == nome)
            .unwrap_or_else(|| panic!("falta {nome}"));
        let s = &t["inputSchema"];
        assert_eq!(s["type"], "object");
        assert_eq!(s["additionalProperties"], false, "{nome}");
        for req in s["required"].as_array().unwrap() {
            assert!(s["properties"].get(req.as_str().unwrap()).is_some());
        }
        assert!(!t["description"].as_str().unwrap().is_empty());
    }
    // Nenhuma ferramenta de descarte de livro nem de aprovação.
    for t in ts {
        let n = t["name"].as_str().unwrap();
        if n.starts_with("biblioteca_") {
            assert!(FERRAMENTAS_BIBLIOTECA.contains(&n), "{n}");
            assert!(!n.contains("descart") && !n.contains("aprovar"), "{n}");
            let props = t["inputSchema"]["properties"].as_object().unwrap();
            assert!(!props.contains_key("aprovacao"), "{n}");
        }
    }
    let d = |n: &str| {
        ts.iter().find(|t| t["name"] == n).unwrap()["description"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert!(d("biblioteca_revisao").contains("NÃO CONFIÁVEL"));
    assert!(d("biblioteca_duplicatas").contains("não iniciam descarte"));
    assert!(d("biblioteca_propor_homologacao").contains("--aprovado-por"));
    assert!(d("biblioteca_decidir").contains("autor `mcp`"));
}

#[test]
fn biblioteca_traduz_argumentos_para_comandos() {
    use dag_core::{AcaoRevisao, AlvoRevisao, ItemDecisao, MotivoFila, PapelRegistro};
    let (c, a) = comando_de("biblioteca_listar", json!({}));
    assert_eq!((c, a), (Comando::BibliotecaListar, Autor::Mcp));
    let (c, _) = comando_de("biblioteca_perfil", json!({"biblioteca": 3}));
    assert_eq!(
        c,
        Comando::BibliotecaPerfil {
            biblioteca: "3".into()
        }
    );
    let (c, _) = comando_de(
        "biblioteca_registrar",
        json!({"caminho": "/l", "papel": "a_classificar"}),
    );
    assert_eq!(
        c,
        Comando::BibliotecaRegistrar {
            caminho: "/l".into(),
            papel: PapelRegistro::AClassificar,
            nome: None
        }
    );
    let (c, _) = comando_de(
        "biblioteca_enfileirar",
        json!({"raizes": ["/a"], "motivo": "reprocessar"}),
    );
    assert_eq!(
        c,
        Comando::BibliotecaEnfileirar {
            raizes: vec!["/a".into()],
            motivo: Some(MotivoFila::Reprocessar)
        }
    );
    let (c, _) = comando_de("biblioteca_processar", json!({"limite": 5, "ia": true}));
    assert_eq!(
        c,
        Comando::BibliotecaProcessar {
            limite: Some(5),
            ia: true
        }
    );
    let (c, _) = comando_de("biblioteca_espaco", json!({"volume": "HD1"}));
    assert_eq!(
        c,
        Comando::BibliotecaRelatorioEspaco {
            volume: Some("HD1".into())
        }
    );
    let (c, _) = comando_de(
        "biblioteca_decidir",
        json!({"lote": "L", "itens": [
            {"alvo": {"tipo": "arquivo", "id": 9}, "campo": "titulo", "acao": "corrigir", "valor": "Dom Casmurro"}]}),
    );
    assert_eq!(
        c,
        Comando::BibliotecaDecidir {
            itens: vec![ItemDecisao {
                alvo: AlvoRevisao::Arquivo(9),
                campo: "titulo".into(),
                acao: AcaoRevisao::Corrigir,
                valor: Some("Dom Casmurro".into())
            }],
            lote: Some("L".into())
        }
    );
    let (c, _) = comando_de("biblioteca_desfazer", json!({"lote": "L"}));
    assert_eq!(c, Comando::BibliotecaDesfazer { lote: "L".into() });
    let (c, _) = comando_de("biblioteca_cadastrar", json!({"itens": [1, 2]}));
    assert_eq!(c, Comando::BibliotecaCadastrar { itens: vec![1, 2] });
    let (c, _) = comando_de("biblioteca_marcar_visto", json!({"ids": [4]}));
    assert_eq!(c, Comando::BibliotecaMarcarVisto { ids: vec![4] });
    let (c, _) = comando_de(
        "biblioteca_alertas",
        json!({"nao_vistos": true, "limite": 10}),
    );
    assert_eq!(
        c,
        Comando::BibliotecaAlertas {
            nao_vistos: true,
            limite: Some(10),
            cursor: None
        }
    );
    let (c, _) = comando_de("biblioteca_config_obter", json!({}));
    assert_eq!(c, Comando::BibliotecaConfigObter);
}

#[test]
fn biblioteca_valida_argumentos_obrigatorios() {
    assert!(recusa_de("biblioteca_decidir", json!({"itens": []})).contains("itens"));
    let t = recusa_de(
        "biblioteca_decidir",
        json!({"itens": [{"alvo": {"tipo": "obra", "id": 1}, "campo": "titulo", "acao": "corrigir"}]}),
    );
    assert!(t.contains("valor"), "{t}");
    assert!(recusa_de("biblioteca_enfileirar", json!({"raizes": []})).contains("raizes"));
    assert!(recusa_de("biblioteca_perfil", json!({"x": 1})).contains("desconhecido"));
}

#[test]
fn propor_homologacao_nunca_envia_aprovacao() {
    let (c, a) = comando_de("biblioteca_propor_homologacao", json!({"item": 4}));
    assert_eq!(a, Autor::Mcp);
    assert_eq!(
        c,
        Comando::BibliotecaHomologar {
            item: 4,
            aprovacao: None
        }
    );
    for args in [
        json!({"item": 4, "aprovacao": {"quem": "Chico"}}),
        json!({"item": 4, "aprovado_por": "Chico"}),
        json!({"item": 4, "quem": "Chico"}),
    ] {
        let t = recusa_de("biblioteca_propor_homologacao", args);
        assert!(t.contains("só propõe") && t.contains("pessoa"), "{t}");
    }
}

#[test]
fn aprovacao_humana_exigida_vira_explicacao_clara() {
    let mut f = Falso {
        resposta: Some(Resposta::Erro {
            codigo: "aprovacao_humana_exigida".into(),
            mensagem: "x".into(),
        }),
        ..Default::default()
    };
    let r = rodar(
        &mut f,
        &[chamar("biblioteca_propor_homologacao", json!({"item": 4}))],
    );
    let t = r[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(t.contains("GUI") && t.contains("CLI") && t.contains("--aprovado-por"));
}

#[test]
fn config_definir_recusa_chaves_sensiveis() {
    for (alt, trecho) in [
        (json!({"usar_unshare": true}), "usar_unshare"),
        (json!({"homologacao": {"senha_env": "X"}}), "homologacao"),
        (
            json!({"ia": {"endpoint": "http://10.0.0.5:11434"}}),
            "loopback",
        ),
        (
            json!({"ia": {"endpoint": "http://127.0.0.1@evil.com:1"}}),
            "loopback",
        ),
        (
            json!({"homologacao": {"url": "http://exemplo.com:17790"}}),
            "homologacao",
        ),
    ] {
        let t = recusa_de("biblioteca_config_definir", json!({"alteracoes": alt}));
        assert!(t.contains(trecho) && t.contains("Nada foi alterado"), "{t}");
    }
    assert!(
        recusa_de(
            "biblioteca_config_definir",
            json!({"alteracoes": {"inexistente": 1}})
        )
        .contains("desconhecido")
    );
    for alt in [
        json!({"homologacao": {"modo": "servidor", "url": "http://127.0.0.1:17790"}}),
        json!({"politica_trava": "apenas_servidor"}),
    ] {
        let t = recusa_de("biblioteca_config_definir", json!({"alteracoes": alt}));
        assert!(t.contains("só mudam pela GUI ou pela CLI"), "{t}");
    }
    // O resto passa, inclusive loopback.
    let (c, a) = comando_de(
        "biblioteca_config_definir",
        json!({"alteracoes": {"ia": {"ativa": true, "endpoint": "http://localhost:11434"},
            "usar_unshare": false, "debounce_s": 10}}),
    );
    assert_eq!(a, Autor::Mcp);
    assert!(matches!(c, Comando::BibliotecaConfigDefinir { .. }));
}
