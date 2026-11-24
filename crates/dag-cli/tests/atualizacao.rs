//! `cargo install` com o agente rodando: o agente vigia o próprio executável, valida o novo e se
//! reexecuta no lugar (mesmo PID), segundo a política `--ao-atualizar`.
#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use dag_agent::{EventoObservado, Observador, OpcoesObservador};
use dag_core::EventoAgente;

struct Ambiente {
    _tmp: tempfile::TempDir,
    raiz: PathBuf,
    catalogo: PathBuf,
    socket: PathBuf,
    /// Cópia do `dag` que o agente executa e que o teste troca por rename.
    exe: PathBuf,
}

fn copiar_exe(destino: &Path) {
    std::fs::copy(env!("CARGO_BIN_EXE_dag"), destino).unwrap();
    std::fs::set_permissions(destino, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn ambiente(arquivos: usize) -> Ambiente {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let raiz = base.join("arvore");
    for d in 0..arquivos / 50 {
        for f in 0..50 {
            let p = raiz.join(format!("d{d}/f{f}.txt"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![b'x'; 100 + f]).unwrap();
        }
    }
    std::fs::create_dir_all(base.join("bin")).unwrap();
    let exe = base.join("bin/dag");
    copiar_exe(&exe);
    Ambiente {
        catalogo: base.join("dados/catalogo.sqlite"),
        socket: base.join("run/agente.sock"),
        raiz,
        exe,
        _tmp: tmp,
    }
}

fn cmd(a: &Ambiente, programa: &Path) -> Command {
    let mut c = Command::new(programa);
    c.env_remove("DAG_CATALOGO")
        .env_remove("DAG_SOCKET")
        .arg("--catalogo")
        .arg(&a.catalogo)
        .arg("--socket")
        .arg(&a.socket);
    c
}

fn dag(a: &Ambiente, args: &[&str]) -> Output {
    cmd(a, Path::new(env!("CARGO_BIN_EXE_dag")))
        .args(args)
        .output()
        .unwrap()
}

fn status(a: &Ambiente) -> serde_json::Value {
    let o = dag(a, &["--json", "status"]);
    serde_json::from_slice(&o.stdout).unwrap()
}

struct Agente(Child);

impl Agente {
    fn iniciar(a: &Ambiente, politica: &str) -> Agente {
        let filho = cmd(a, &a.exe)
            .args(["agente", "--sem-monitor", "--ao-atualizar", politica])
            .env("DAG_VIGIA_EXE_MS", "200")
            .env("DAG_ESTABILIDADE_EXE_MS", "400")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut ag = Agente(filho);
        let fim = Instant::now() + Duration::from_secs(30);
        while UnixStream::connect(&a.socket).is_err() {
            assert!(Instant::now() < fim, "o agente não abriu o socket");
            if let Some(st) = ag.0.try_wait().unwrap() {
                panic!("o agente saiu cedo: {st}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        ag
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for Agente {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Instala um executável novo como o `cargo install`: grava ao lado e renomeia sobre o antigo.
fn trocar_executavel(a: &Ambiente) -> u64 {
    let tmp = a.exe.with_file_name(".dag.novo");
    copiar_exe(&tmp);
    // Bytes diferentes (um executável ELF tolera lixo ao final), como uma compilação nova.
    let mut f = std::fs::OpenOptions::new().append(true).open(&tmp).unwrap();
    f.write_all(b"\0versao-nova").unwrap();
    drop(f);
    let inode = std::fs::metadata(&tmp).unwrap().ino();
    std::fs::rename(&tmp, &a.exe).unwrap();
    inode
}

fn esperar<T>(descricao: &str, limite: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let fim = Instant::now() + limite;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < fim, "tempo esgotado: {descricao}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn inode_do_agente(a: &Ambiente) -> Option<(u64, u64)> {
    let j = status(a);
    let ag = &j["agente"];
    (ag["servindo"] == true).then(|| {
        (
            ag["pid"].as_u64().unwrap(),
            ag["binario"]["inode"].as_u64().unwrap(),
        )
    })
}

fn observar(a: &Ambiente) -> Observador {
    Observador::iniciar(
        OpcoesObservador::novo(a.catalogo.clone(), a.socket.clone()),
        None,
    )
}

#[test]
fn binario_trocado_reexecuta_no_lugar_e_o_trabalho_interrompido_continua() {
    let a = ambiente(20_000);
    let agente = Agente::iniciar(&a, "reiniciar-agora");
    let pid = u64::from(agente.pid());
    let observador = observar(&a);
    let (pid0, inode0) = esperar("agente informa o binário", Duration::from_secs(30), || {
        inode_do_agente(&a)
    });
    assert_eq!(pid0, pid);
    // O cliente e o agente são da mesma versão: nada a avisar.
    assert!(status(&a)["agente"]["aviso_versao"].is_null());

    let scan = cmd(&a, Path::new(env!("CARGO_BIN_EXE_dag")))
        .args(["scan", "--full", a.raiz.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    esperar("varredura em andamento", Duration::from_secs(60), || {
        (status(&a)["execucoes"][0]["estado"] == "em_andamento").then_some(())
    });

    let inode_novo = trocar_executavel(&a);
    assert_ne!(inode_novo, inode0);

    // O assinante recebe o aviso, a conexão cai e volta sozinha.
    let mut avisou = false;
    let mut conexoes = 0;
    let fim = Instant::now() + Duration::from_secs(60);
    while Instant::now() < fim && !(avisou && conexoes >= 2) {
        match observador.proximo(Duration::from_millis(200)) {
            Some(EventoObservado::Conectado(_)) => conexoes += 1,
            Some(EventoObservado::Evento(EventoAgente::AgenteReiniciando { de, para })) => {
                assert!(!de.is_empty() && !para.is_empty());
                avisou = true;
            }
            _ => {}
        }
    }
    assert!(avisou, "faltou o evento AgenteReiniciando");
    assert!(conexoes >= 2, "o observador não reconectou");

    // Mesmo PID (exec), executável novo em uso e socket servindo.
    let (pid1, inode1) = esperar("agente novo no ar", Duration::from_secs(60), || {
        inode_do_agente(&a).filter(|(_, i)| *i == inode_novo)
    });
    assert_eq!(pid1, pid, "exec mantém o PID");
    assert_eq!(inode1, inode_novo);

    // A varredura interrompida é retomada sozinha pelo agente novo e conclui.
    let _ = scan.wait_with_output().unwrap();
    esperar(
        "varredura retomada e concluída",
        Duration::from_secs(120),
        || {
            let j = status(&a);
            j["execucoes"]
                .as_array()?
                .iter()
                .any(|e| e["estado"] == "concluida")
                .then_some(())
        },
    );
    observador.parar();
}

#[test]
fn politica_avisar_so_sinaliza_e_o_pedido_explicito_reinicia() {
    let a = ambiente(100);
    let agente = Agente::iniciar(&a, "avisar");
    let pid = u64::from(agente.pid());
    let (_, inode0) = esperar("agente informa o binário", Duration::from_secs(30), || {
        inode_do_agente(&a)
    });
    let inode_novo = trocar_executavel(&a);

    // Vê o binário novo, mas não reinicia: publica a atualização pendente.
    let j = esperar("atualização pendente", Duration::from_secs(30), || {
        let j = status(&a);
        j["agente"]["atualizacao_pendente"].is_string().then_some(j)
    });
    assert_eq!(j["agente"]["binario"]["inode"].as_u64().unwrap(), inode0);
    assert!(
        j["agente"]["aviso_versao"]
            .as_str()
            .unwrap()
            .contains("agente desatualizado")
    );
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        inode_do_agente(&a).unwrap().1,
        inode0,
        "avisar não reinicia"
    );

    // Botão "Reiniciar agente": `reiniciar_agente` no socket vale para qualquer política.
    let mut s = UnixStream::connect(&a.socket).unwrap();
    s.write_all(b"{\"id\":1,\"pedido\":{\"autor\":\"gui\",\"comando\":{\"comando\":\"reiniciar_agente\"}}}\n")
        .unwrap();
    let mut linha = String::new();
    BufReader::new(&s).read_line(&mut linha).unwrap();
    let r: serde_json::Value = serde_json::from_str(&linha).unwrap();
    assert_eq!(r["resposta"]["resposta"], "ok", "{linha}");

    let (pid1, inode1) = esperar("agente novo no ar", Duration::from_secs(60), || {
        inode_do_agente(&a).filter(|(_, i)| *i == inode_novo)
    });
    assert_eq!((pid1, inode1), (pid, inode_novo));
    assert!(status(&a)["agente"]["atualizacao_pendente"].is_null());
}

#[test]
fn executavel_novo_invalido_e_ignorado() {
    let a = ambiente(100);
    let agente = Agente::iniciar(&a, "reiniciar-agora");
    let pid = u64::from(agente.pid());
    let (_, inode0) = esperar("agente informa o binário", Duration::from_secs(30), || {
        inode_do_agente(&a)
    });
    // Executável que não roda (instalação quebrada): o agente segue como está.
    let tmp = a.exe.with_file_name(".dag.quebrado");
    std::fs::write(&tmp, b"nao sou um executavel").unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp, &a.exe).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let (pid1, inode1) = inode_do_agente(&a).unwrap();
    assert_eq!((pid1, inode1), (pid, inode0));
}
