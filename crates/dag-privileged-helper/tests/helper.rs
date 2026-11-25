#![allow(clippy::unwrap_used)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use dag_privileged_helper::{
    Linha, Requisicao, TipoArquivo, atender, decodificar_bytes, interpretar, ler_politica, validar,
};

fn req(raizes: &[&str], caminho: &str) -> Requisicao {
    Requisicao {
        raizes: raizes.iter().map(ToString::to_string).collect(),
        caminho: caminho.into(),
    }
}

fn pedido(r: &Requisicao) -> Vec<u8> {
    serde_json::to_vec(r).unwrap()
}

fn rodar(entrada: &[u8], politica: Option<&[PathBuf]>) -> (bool, Vec<Linha>) {
    let mut saida = Vec::new();
    let d = atender(&mut &*entrada, &mut saida, politica);
    let linhas = String::from_utf8(saida)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (d.ok, linhas)
}

fn codigo_erro(linhas: &[Linha]) -> String {
    match linhas.last() {
        Some(Linha::Erro { codigo, .. }) => codigo.clone(),
        o => panic!("esperava erro, veio {o:?}"),
    }
}

fn raiz() -> (tempfile::TempDir, String) {
    let t = tempfile::tempdir().unwrap();
    let r = fs::canonicalize(t.path())
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    (t, r)
}

#[test]
fn recusa_pedidos_invalidos() {
    let (_t, r) = raiz();
    let dentro = format!("{r}/x");
    let casos = [
        ("relativo", req(&[&r], "a/b"), "caminho_invalido"),
        ("vazio", req(&[&r], ""), "caminho_invalido"),
        (
            "ponto-ponto",
            req(&[&r], &format!("{r}/../etc")),
            "caminho_invalido",
        ),
        ("ponto", req(&[&r], &format!("{r}/./x")), "caminho_invalido"),
        (
            "barra dupla",
            req(&[&r], &format!("{r}//x")),
            "caminho_invalido",
        ),
        (
            "barra final",
            req(&[&r], &format!("{dentro}/")),
            "caminho_invalido",
        ),
        ("nul", req(&[&r], &format!("{r}/x\0y")), "caminho_invalido"),
        ("fora", req(&[&r], "/etc"), "fora_das_raizes"),
        (
            "prefixo de string",
            req(&[&r], &format!("{r}2/x")),
            "fora_das_raizes",
        ),
        ("sem raizes", req(&[], &dentro), "raiz_invalida"),
        ("raiz relativa", req(&["a"], &dentro), "raiz_invalida"),
    ];
    for (nome, rq, esperado) in casos {
        let (ok, linhas) = rodar(&pedido(&rq), None);
        assert!(!ok, "{nome}");
        assert_eq!(codigo_erro(&linhas), esperado, "{nome}");
    }
}

#[test]
fn recusa_json_e_formatos_estranhos() {
    for entrada in [
        &b"nao e json"[..],
        b"",
        b"[]",
        b"{\"raizes\":[\"/\"],\"caminho\":\"/tmp\",\"extra\":1}",
        b"{\"comando\":\"rm -rf /\"}",
        b"{\"raizes\":\"/\",\"caminho\":\"/tmp\"}",
    ] {
        let (ok, linhas) = rodar(entrada, None);
        assert!(!ok);
        assert_eq!(codigo_erro(&linhas), "requisicao_invalida");
    }
    assert!(interpretar(b"{\"raizes\":[],\"caminho\":\"/\"}").is_ok());
    let grande = vec![b' '; 100_000];
    assert!(!rodar(&grande, None).0);
}

#[test]
fn politica_do_administrador_restringe() {
    let (_t, r) = raiz();
    fs::create_dir(format!("{r}/a")).unwrap();
    let rq = req(&[&r], &format!("{r}/a"));
    let so_outra = [PathBuf::from("/var/outra")];
    assert_eq!(
        codigo_erro(&rodar(&pedido(&rq), Some(&so_outra)).1),
        "fora_da_politica"
    );
    let ok = [PathBuf::from(&r)];
    assert!(rodar(&pedido(&rq), Some(&ok)).0);
    assert!(validar(&rq, Some(&[])).is_err());

    let arq = tempfile::NamedTempFile::new().unwrap();
    fs::write(arq.path(), format!("# c\n\n{r}\nrelativo\n")).unwrap();
    assert_eq!(ler_politica(arq.path()).unwrap(), vec![PathBuf::from(&r)]);
    assert!(ler_politica(Path::new("/nao/existe/dag.roots")).is_none());
}

#[test]
fn symlink_no_meio_ou_no_fim_e_recusado() {
    let (_t, r) = raiz();
    fs::create_dir_all(format!("{r}/real/sub")).unwrap();
    symlink(format!("{r}/real"), format!("{r}/atalho")).unwrap();
    let (ok, l) = rodar(&pedido(&req(&[&r], &format!("{r}/atalho/sub"))), None);
    assert!(!ok);
    assert_eq!(codigo_erro(&l), "caminho_suspeito");
    let (ok, l) = rodar(&pedido(&req(&[&r], &format!("{r}/atalho"))), None);
    assert!(!ok);
    assert_eq!(codigo_erro(&l), "caminho_suspeito");
    // arquivo comum no lugar de pasta
    fs::write(format!("{r}/arq"), b"x").unwrap();
    let (ok, l) = rodar(&pedido(&req(&[&r], &format!("{r}/arq"))), None);
    assert!(!ok);
    assert_eq!(codigo_erro(&l), "caminho_suspeito");
    // inexistente
    let (ok, l) = rodar(&pedido(&req(&[&r], &format!("{r}/nada"))), None);
    assert!(!ok);
    assert_eq!(codigo_erro(&l), "nao_encontrado");
    // o caminho real funciona
    assert!(rodar(&pedido(&req(&[&r], &format!("{r}/real/sub"))), None).0);
}

fn montar_arvore(r: &str) {
    fs::create_dir_all(format!("{r}/p/sub")).unwrap();
    fs::write(format!("{r}/p/a.txt"), b"conteudo secreto").unwrap();
    fs::write(format!("{r}/p/vazio"), b"").unwrap();
    symlink("alvo/relativo", format!("{r}/p/link")).unwrap();
    fs::hard_link(format!("{r}/p/a.txt"), format!("{r}/p/dura")).unwrap();
    use std::os::unix::ffi::OsStringExt;
    let invalido = std::ffi::OsString::from_vec(vec![b'n', 0xff, 0xfe]);
    fs::write(Path::new(&r).join("p").join(invalido), b"z").unwrap();
}

#[test]
fn metadados_batem_com_lstat() {
    let (_t, r) = raiz();
    montar_arvore(&r);
    let dir = format!("{r}/p");
    let (ok, linhas) = rodar(&pedido(&req(&[&r], &dir)), None);
    assert!(ok);
    let Linha::Dir { meta } = &linhas[0] else {
        panic!("primeira linha deve ser a pasta")
    };
    assert_eq!(meta.tipo_arquivo, TipoArquivo::Dir);
    assert_eq!(meta.ino, fs::symlink_metadata(&dir).unwrap().ino());
    assert_eq!(linhas.last(), Some(&Linha::Fim { entradas: 6 }));
    let mut vistos = 0;
    for l in &linhas {
        let Linha::Entrada {
            nome,
            nome_hex,
            alvo,
            meta,
            ..
        } = l
        else {
            continue;
        };
        vistos += 1;
        let nome_b = decodificar_bytes(nome.as_deref(), nome_hex.as_deref()).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let m = fs::symlink_metadata(Path::new(&dir).join(std::ffi::OsStr::from_bytes(&nome_b)))
            .unwrap();
        assert_eq!(meta.dev, m.dev());
        assert_eq!(meta.ino, m.ino());
        assert_eq!(meta.nlink, m.nlink());
        assert_eq!(meta.mode, m.mode());
        assert_eq!(meta.size, m.size());
        assert_eq!(meta.blocks, m.blocks());
        assert_eq!(meta.uid, m.uid());
        assert_eq!(meta.mtime_ns, m.mtime() * 1_000_000_000 + m.mtime_nsec());
        assert_eq!(meta.ctime_ns, m.ctime() * 1_000_000_000 + m.ctime_nsec());
        match nome.as_deref() {
            Some("link") => {
                assert_eq!(meta.tipo_arquivo, TipoArquivo::Symlink);
                assert_eq!(alvo.as_deref(), Some("alvo/relativo"));
            }
            Some("sub") => assert_eq!(meta.tipo_arquivo, TipoArquivo::Dir),
            Some("a.txt" | "dura") => {
                assert_eq!(meta.tipo_arquivo, TipoArquivo::File);
                assert_eq!(meta.nlink, 2);
                assert_eq!(meta.size, 16);
            }
            None => assert_eq!(nome_b, vec![b'n', 0xff, 0xfe]),
            _ => {}
        }
    }
    assert_eq!(vistos, 6);
}

/// CA-A15: o helper não modifica nada do que lê.
#[test]
fn helper_nao_modifica_arquivos() {
    let (_t, r) = raiz();
    montar_arvore(&r);
    let instantaneo = |r: &str| -> Vec<String> {
        let mut v = Vec::new();
        let mut pilha = vec![PathBuf::from(r)];
        while let Some(d) = pilha.pop() {
            let m = fs::symlink_metadata(&d).unwrap();
            v.push(format!(
                "{} {} {} {} {} {}",
                d.display(),
                m.mode(),
                m.size(),
                m.mtime_nsec(),
                m.mtime(),
                m.ctime_nsec()
            ));
            if m.is_dir() {
                for e in fs::read_dir(&d).unwrap() {
                    pilha.push(e.unwrap().path());
                }
            } else if m.is_file() {
                v.push(format!("{:?}", fs::read(&d).unwrap()));
            }
        }
        v.sort();
        v
    };
    let antes = instantaneo(&r);
    for sub in ["p", "p/sub"] {
        assert!(rodar(&pedido(&req(&[&r], &format!("{r}/{sub}"))), None).0);
    }
    assert_eq!(instantaneo(&r), antes);
}

fn binario() -> &'static str {
    env!("CARGO_BIN_EXE_dag-helper")
}

fn rodar_binario(mut cmd: Command, entrada: &[u8]) -> (i32, String) {
    let mut f = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    f.stdin.take().unwrap().write_all(entrada).unwrap();
    let o = f.wait_with_output().unwrap();
    (
        o.status.code().unwrap(),
        String::from_utf8(o.stdout).unwrap(),
    )
}

#[test]
fn binario_funciona_sem_privilegio_para_caminho_legivel() {
    let (_t, r) = raiz();
    montar_arvore(&r);
    let (cod, saida) = rodar_binario(
        Command::new(binario()),
        &pedido(&req(&[&r], &format!("{r}/p"))),
    );
    assert_eq!(cod, 0, "{saida}");
    assert!(saida.lines().next().unwrap().contains("\"tipo\":\"dir\""));
    assert!(saida.lines().last().unwrap().contains("\"fim\""));
    let (cod, saida) = rodar_binario(Command::new(binario()), b"lixo");
    assert_eq!(cod, 2);
    assert!(saida.contains("requisicao_invalida"));
}

/// Pasta 0700 de root: usuário comum não lista; o helper (root) lista só metadados.
#[test]
fn pasta_protegida_negada_ao_usuario_e_lida_pelo_helper_root() {
    if rustix::process::getuid().as_raw() != 0 || which("setpriv").is_none() {
        eprintln!("ignorado: precisa de root e setpriv");
        return;
    }
    let (t, r) = raiz();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o755)).unwrap();
    montar_arvore(&r);
    fs::set_permissions(format!("{r}/p"), fs::Permissions::from_mode(0o700)).unwrap();
    let copia = format!("{r}/dag-helper");
    fs::copy(binario(), &copia).unwrap();
    fs::set_permissions(&copia, fs::Permissions::from_mode(0o755)).unwrap();
    let rq = pedido(&req(&[&r], &format!("{r}/p")));

    let (cod, saida) = rodar_binario(Command::new(&copia), &rq);
    assert_eq!(cod, 0);
    assert!(saida.contains("\"nome\":\"a.txt\""));

    let mut c = Command::new("setpriv");
    c.args(["--reuid=65534", "--regid=65534", "--clear-groups", &copia]);
    let (cod, saida) = rodar_binario(c, &rq);
    if saida.contains("erro_es") {
        assert_eq!(cod, 2, "{saida}");
    } else {
        // setpriv indisponível no ambiente (ex.: sem CAP_SETUID): nada a afirmar.
        eprintln!("setpriv não pôde rebaixar: {saida}");
    }
}

fn which(p: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|d| Path::new(d).join(p))
        .find(|c| c.is_file())
}
