//! ADR 0004 de ponta a ponta, com processos reais: o agente é o único escritor, os clientes (CLI,
//! MCP e uma "GUI" que assina eventos) o sobem sozinhos e enxergam e controlam os trabalhos uns
//! dos outros.
#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use dag_agent::{Cliente, Conexao, OpcoesConexao, OpcoesLancamento, garantir_agente};
use dag_core::{Autor, Comando, EventoAgente, OrigemJob, Pedido, Resposta, TipoJob};
use serde_json::{Value, json};

const DAG: &str = env!("CARGO_BIN_EXE_dag");

struct Ambiente {
    _tmp: tempfile::TempDir,
    raiz: PathBuf,
    catalogo: PathBuf,
    socket: PathBuf,
}

fn ambiente(grande: bool) -> Ambiente {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let raiz = base.join("arvore");
    let (pastas, arquivos) = if grande { (150, 60) } else { (3, 4) };
    for d in 0..pastas {
        for f in 0..arquivos {
            let p = raiz.join(format!("d{d}/f{f}.txt"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            // Conteúdo repetido: gera duplicados para as ferramentas de marcação.
            std::fs::write(p, vec![b'x'; 100 + f]).unwrap();
        }
    }
    Ambiente {
        catalogo: base.join("dados/catalogo.sqlite"),
        socket: base.join("run/agente.sock"),
        raiz,
        _tmp: tmp,
    }
}

/// `dag` com o ambiente de um cliente que precisa subir o agente: sem systemd, `dag agente` sem
/// monitor, e o próprio binário como o `dag` a lançar.
fn dag_cmd(a: &Ambiente) -> Command {
    let mut c = Command::new(DAG);
    c.env_remove("DAG_CATALOGO")
        .env_remove("DAG_SEM_AGENTE")
        .env("DAG_SOCKET", &a.socket)
        .env("DAG_BIN", DAG)
        .env("DAG_SEM_SYSTEMD", "1")
        .env("DAG_AGENTE_ARGS", "--sem-monitor")
        .arg("--catalogo")
        .arg(&a.catalogo);
    c
}

fn dag(a: &Ambiente, args: &[&str]) -> Output {
    dag_cmd(a).args(args).output().unwrap()
}

fn json_de(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout não é JSON ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    })
}

/// Garante que o agente lançado pelos clientes não sobra depois do teste.
struct Limpeza(PathBuf, PathBuf);

impl Drop for Limpeza {
    fn drop(&mut self) {
        if let Ok(mut c) = Cliente::conectar(&self.1)
            && let Ok(i) = c.info()
        {
            let _ = Command::new("kill")
                .args(["-TERM", &i.pid.to_string()])
                .status();
            let fim = Instant::now() + Duration::from_secs(10);
            while Cliente::conectar(&self.1).is_ok() && Instant::now() < fim {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let _ = &self.0;
    }
}

fn opcoes(a: &Ambiente) -> OpcoesLancamento {
    OpcoesLancamento {
        catalogo: a.catalogo.clone(),
        socket: a.socket.clone(),
        executavel: Some(PathBuf::from(DAG)),
        usar_systemd: false,
        espera: Duration::from_secs(30),
        argumentos_agente: vec!["--sem-monitor".into()],
    }
}

fn esperar(descricao: &str, mut pronto: impl FnMut() -> bool) {
    let fim = Instant::now() + Duration::from_secs(60);
    while !pronto() {
        assert!(Instant::now() < fim, "tempo esgotado: {descricao}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn pid_do_agente(socket: &Path, catalogo: &Path) -> Option<u32> {
    let mut c = Cliente::conectar(socket).ok()?;
    let i = c.info().ok()?;
    (Path::new(&i.catalogo) == catalogo).then_some(i.pid)
}

#[test]
fn a_cli_sobe_o_agente_sozinha_e_ele_continua_depois_do_comando() {
    let a = ambiente(false);
    let _l = Limpeza(a.catalogo.clone(), a.socket.clone());
    assert!(pid_do_agente(&a.socket, &a.catalogo).is_none());

    let o = dag(&a, &["--json", "scan", "--full", a.raiz.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(json_de(&o)["resposta"], "scan_concluido");
    // O agente que a CLI lançou segue vivo e é o escritor único: o status vem dele.
    let pid = pid_do_agente(&a.socket, &a.catalogo).expect("o agente devia estar de pé");
    let o = dag(&a, &["--json", "status"]);
    let v = json_de(&o);
    assert_eq!(v["agente"]["servindo"], true, "{v}");
    assert_eq!(v["agente"]["pid"], pid);
    assert_eq!(
        v["agente"]["monitor"], false,
        "DAG_AGENTE_ARGS=--sem-monitor"
    );
    assert!(v["jobs"].as_array().unwrap().is_empty(), "{v}");
    // Um segundo comando de escrita usa o mesmo agente (mesmo pid), sem lançar outro.
    let o = dag(&a, &["--json", "volumes"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(pid_do_agente(&a.socket, &a.catalogo), Some(pid));
    // O log do agente lançado fica ao lado do Catálogo.
    let log = PathBuf::from(format!("{}.agente.log", a.catalogo.display()));
    assert!(log.is_file(), "{}", log.display());
}

#[test]
fn sem_agente_a_cli_roda_em_processo_e_nao_deixa_agente() {
    let a = ambiente(false);
    let _l = Limpeza(a.catalogo.clone(), a.socket.clone());
    let o = dag(
        &a,
        &[
            "--sem-agente",
            "--json",
            "scan",
            "--full",
            a.raiz.to_str().unwrap(),
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(pid_do_agente(&a.socket, &a.catalogo).is_none());
    // Leitura sem agente: o Catálogo é aberto só para leitura, sem lançar nada.
    let o = dag(&a, &["--json", "status"]);
    assert_eq!(json_de(&o)["agente"]["servindo"], false);
    assert!(pid_do_agente(&a.socket, &a.catalogo).is_none());
}

#[test]
fn dois_clientes_lancando_ao_mesmo_tempo_acabam_com_um_so_agente() {
    let a = ambiente(false);
    let _l = Limpeza(a.catalogo.clone(), a.socket.clone());
    let pids: Vec<u32> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..4)
            .map(|_| {
                s.spawn(|| {
                    let mut c = garantir_agente(&opcoes(&a)).unwrap();
                    c.info().unwrap().pid
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(pids.windows(2).all(|p| p[0] == p[1]), "{pids:?}");
    assert_eq!(pid_do_agente(&a.socket, &a.catalogo), Some(pids[0]));
}

#[test]
fn dois_agentes_subindo_juntos_a_trava_de_escritor_deixa_um_so() {
    let a = ambiente(false);
    let _l = Limpeza(a.catalogo.clone(), a.socket.clone());
    let subir = || -> Child {
        dag_cmd(&a)
            .args(["agente", "--sem-monitor"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    };
    let (mut x, mut y) = (subir(), subir());
    // Quem perde a corrida pela trava sai sozinho com erro; o outro segue servindo.
    esperar("um dos agentes sair", || {
        x.try_wait().unwrap().is_some() || y.try_wait().unwrap().is_some()
    });
    let (sx, sy) = (x.try_wait().unwrap(), y.try_wait().unwrap());
    assert!(
        sx.is_some() != sy.is_some(),
        "só um devia ter saído: {sx:?} {sy:?}"
    );
    assert!(!sx.or(sy).unwrap().success());
    let vivo = if sx.is_none() { &mut x } else { &mut y };
    // O vencedor ainda pode estar publicando o pid: espera o estado, não o relógio.
    let id = vivo.id();
    esperar("o agente vivo publicar o pid", || {
        pid_do_agente(&a.socket, &a.catalogo) == Some(id)
    });
    let _ = vivo.kill();
    let _ = vivo.wait();
}

#[test]
fn sem_o_executavel_dag_o_erro_e_claro_e_aponta_o_sem_agente() {
    let a = ambiente(false);
    let o = OpcoesLancamento {
        executavel: Some(PathBuf::from("/nao/existe/dag")),
        espera: Duration::from_millis(300),
        ..opcoes(&a)
    };
    let e = garantir_agente(&o).unwrap_err().to_string();
    assert!(e.contains("não encontrei o executável `dag`"), "{e}");
    assert!(e.contains("--sem-agente"), "{e}");
    // O mesmo erro chega a quem abre uma conexão para escrever, e o modo sem agente funciona.
    let comando = Comando::ListarVolumes;
    let com = OpcoesConexao {
        sem_agente: false,
        lancamento: Some(o),
    };
    let e = Conexao::abrir_para_com(&a.catalogo, &a.socket, &comando, &com).unwrap_err();
    assert!(e.to_string().contains("--sem-agente"), "{e}");
    let sem = OpcoesConexao::novo(true);
    let c = Conexao::abrir_para_com(&a.catalogo, &a.socket, &comando, &sem).unwrap();
    assert!(!c.eh_daemon());
}

/// Lê linhas do stdout de um processo numa thread, para esperar uma delas sem travar.
fn linhas_de(filho: &mut Child) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let saida = filho.stdout.take().unwrap();
    std::thread::spawn(move || {
        for l in BufReader::new(saida).lines().map_while(Result::ok) {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    rx
}

fn ate_linha(rx: &std::sync::mpsc::Receiver<String>, trecho: &str) -> String {
    let fim = Instant::now() + Duration::from_secs(60);
    loop {
        let resto = fim.saturating_duration_since(Instant::now());
        assert!(!resto.is_zero(), "linha com `{trecho}` não chegou");
        if let Ok(l) = rx.recv_timeout(resto)
            && l.contains(trecho)
        {
            return l;
        }
    }
}

#[test]
fn cli_acompanha_e_controla_o_trabalho_de_outro_processo_pelo_id_do_job() {
    let a = ambiente(true);
    let _l = Limpeza(a.catalogo.clone(), a.socket.clone());
    // Uma varredura qualquer sobe o agente.
    let o = dag(&a, &["--json", "volumes"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let mut seguir = dag_cmd(&a)
        .args(["status", "--seguir"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let linhas = linhas_de(&mut seguir);
    ate_linha(&linhas, "conectado ao agente");

    // Outro processo (uma "CLI") inicia a varredura; quem acompanha a vê, de qualquer origem.
    let mut scan = dag_cmd(&a)
        .args(["scan", "--full", a.raiz.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let l = ate_linha(&linhas, "iniciado");
    assert!(l.contains("Full Scan") && l.contains("cli"), "{l}");

    // `dag jobs` lista o trabalho com quem o iniciou.
    let mut job = None;
    esperar("job na lista", || {
        let v = json_de(&dag(&a, &["--json", "jobs"]));
        job = v["jobs"]
            .as_array()
            .and_then(|j| j.iter().find(|j| j["tipo"] == "full_scan").cloned());
        job.is_some()
    });
    let job = job.unwrap();
    assert_eq!(job["origem"], "cli");
    let id = job["id"].as_u64().unwrap().to_string();
    // Pausar, retomar e cancelar por id de job, de outro processo.
    let o = dag(&a, &["pausar", "--job", &id]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    ate_linha(&linhas, "pausado");
    let o = dag(&a, &["retomar", "--job", &id]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    ate_linha(&linhas, "executando");
    let o = dag(&a, &["cancelar", "--job", &id]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let l = ate_linha(&linhas, "terminou");
    assert!(l.contains("cancelada"), "{l}");
    // A CLI que iniciou sai como varredura cancelada (130).
    esperar("a CLI da varredura sair", || {
        scan.try_wait().unwrap().is_some()
    });
    assert_eq!(scan.wait().unwrap().code(), Some(130));
    let _ = seguir.kill();
    let _ = seguir.wait();
}

#[test]
fn mcp_escreve_pelo_agente_com_a_gui_conectada_e_lista_os_jobs() {
    let a = ambiente(false);
    let _l = Limpeza(a.catalogo.clone(), a.socket.clone());
    let mut mcp = dag_cmd(&a)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut entrada = mcp.stdin.take().unwrap();
    let mut saida = BufReader::new(mcp.stdout.take().unwrap());
    let mut pedir = |v: Value| -> Value {
        writeln!(entrada, "{v}").unwrap();
        entrada.flush().unwrap();
        let mut l = String::new();
        saida.read_line(&mut l).unwrap();
        serde_json::from_str(&l).unwrap_or_else(|e| panic!("stdout não-JSON ({e}): {l:?}"))
    };
    let chamar = |id: u64, nome: &str, args: Value| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":nome,"arguments":args}});
    // A primeira ferramenta de escrita sobe o agente.
    let r = pedir(chamar(
        1,
        "scan_full",
        json!({"raizes":[a.raiz.to_str().unwrap()]}),
    ));
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert!(pid_do_agente(&a.socket, &a.catalogo).is_some());

    // A "GUI" fica conectada ao agente, assinando eventos.
    let mut gui = Cliente::conectar(&a.socket).unwrap().assinar().unwrap();
    let primeiro = gui.proximo(Duration::from_secs(10)).unwrap().unwrap();
    assert!(matches!(primeiro, EventoAgente::Instantaneo { .. }));

    // Escrita do MCP com a GUI conectada: funciona (um só escritor, o agente).
    let alvo = a.raiz.join("d0/f0.txt");
    let r = pedir(chamar(
        2,
        "marcar_delecao",
        json!({"alvo":{"tipo":"caminho","valor":alvo.to_str().unwrap()}}),
    ));
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(r["result"]["structuredContent"]["resposta"], "marcacao");
    // A ferramenta `jobs` mostra o que o agente está fazendo (nada, agora).
    let r = pedir(chamar(3, "jobs", json!({})));
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(r["result"]["structuredContent"]["resposta"], "jobs");

    // E um scan iniciado pelo MCP aparece, com a origem `mcp`, para a GUI conectada.
    let (socket, raiz) = (a.socket.clone(), a.raiz.clone());
    let _ = (socket, raiz);
    let r = pedir(chamar(4, "scan_fast", json!({})));
    assert_eq!(r["result"]["isError"], false, "{r}");
    let mut visto = false;
    let fim = Instant::now() + Duration::from_secs(10);
    while Instant::now() < fim && !visto {
        if let Ok(Some(EventoAgente::JobFinalizado { job, .. })) =
            gui.proximo(Duration::from_millis(200))
            && job.origem == OrigemJob::Mcp
            && job.tipo == TipoJob::FastScan
        {
            visto = true;
        }
    }
    assert!(visto, "a GUI devia ver o Fast Scan iniciado pelo MCP");
    drop(entrada);
    assert!(mcp.wait().unwrap().success());
    // O agente é de quem o lançou, não do MCP: continua de pé.
    assert!(pid_do_agente(&a.socket, &a.catalogo).is_some());
    let _ = Pedido {
        autor: Autor::Gui,
        comando: Comando::ListarJobs,
    };
    let _ = Resposta::Ok { mensagem: None };
}
