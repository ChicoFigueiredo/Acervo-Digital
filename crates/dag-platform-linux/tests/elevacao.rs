#![allow(clippy::unwrap_used)]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use dag_platform_linux::elevacao::{ClienteHelper, Diagnostico, ErroElevacao, diagnosticar};
use dag_platform_linux::servico::{ExecutorSistema, SaidaProcesso};

type Chamada = (String, Vec<String>, Vec<u8>);

struct Falso {
    resposta: Mutex<Option<io::Result<SaidaProcesso>>>,
    vistas: Mutex<Vec<Chamada>>,
}

impl Falso {
    fn novo(r: io::Result<SaidaProcesso>) -> Arc<Self> {
        Arc::new(Self {
            resposta: Mutex::new(Some(r)),
            vistas: Mutex::default(),
        })
    }
}

impl ExecutorSistema for Falso {
    fn executar(&self, p: &str, a: &[&str], s: Option<&[u8]>) -> io::Result<SaidaProcesso> {
        self.vistas.lock().unwrap().push((
            p.into(),
            a.iter().map(ToString::to_string).collect(),
            s.unwrap_or_default().to_vec(),
        ));
        self.resposta.lock().unwrap().take().unwrap()
    }
}

fn saida(codigo: i32, stdout: &str, stderr: &str) -> io::Result<SaidaProcesso> {
    Ok(SaidaProcesso {
        codigo: Some(codigo),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    })
}

fn meta(tipo: &str, ino: u64) -> String {
    format!(
        "\"tipo_arquivo\":\"{tipo}\",\"dev\":1,\"ino\":{ino},\"nlink\":1,\"mode\":33188,\"uid\":0,\"gid\":0,\"size\":10,\"blocks\":8,\"mtime_ns\":5,\"ctime_ns\":6"
    )
}

fn cliente(f: Arc<Falso>) -> ClienteHelper {
    ClienteHelper::com_executor(
        f,
        "pkexec".into(),
        PathBuf::from("/usr/libexec/dag/dag-helper"),
        false,
    )
}

#[test]
fn le_listagem_do_helper() {
    let corpo = format!(
        "{{\"tipo\":\"dir\",{}}}\n{{\"tipo\":\"entrada\",\"nome\":\"a.txt\",{}}}\n{{\"tipo\":\"entrada\",\"nome_hex\":\"6eff\",\"alvo\":\"x\",{}}}\n{{\"tipo\":\"fim\",\"entradas\":2}}\n",
        meta("dir", 1),
        meta("file", 2),
        meta("symlink", 3)
    );
    let f = Falso::novo(saida(0, &corpo, ""));
    let l = cliente(f.clone())
        .listar(&["/mnt".into()], "/mnt/privado")
        .unwrap();
    assert_eq!(l.entradas.len(), 2);
    assert_eq!(l.entradas[0].nome, b"a.txt");
    assert_eq!(l.entradas[1].nome, vec![0x6e, 0xff]);
    assert_eq!(l.entradas[1].alvo.as_deref(), Some(&b"x"[..]));
    let v = f.vistas.lock().unwrap();
    assert_eq!(v[0].0, "pkexec");
    assert_eq!(v[0].1, vec!["/usr/libexec/dag/dag-helper"]);
    let req: serde_json::Value = serde_json::from_slice(&v[0].2).unwrap();
    assert_eq!(req["caminho"], "/mnt/privado");
    assert_eq!(req["raizes"][0], "/mnt");
    assert_eq!(req.as_object().unwrap().len(), 2, "só raizes e caminho");
}

#[test]
fn sem_pkexec_diagnostica() {
    let f = Falso::novo(Err(io::Error::from(io::ErrorKind::NotFound)));
    let e = cliente(f).listar(&["/".into()], "/x").unwrap_err();
    assert!(matches!(e, ErroElevacao::SemPkexec));
    assert!(e.to_string().contains("Alternativas"));
}

#[test]
fn pkexec_126_e_127_viram_nao_autorizado() {
    for c in [126, 127] {
        let f = Falso::novo(saida(c, "", "Error getting authority"));
        let e = cliente(f).listar(&["/".into()], "/x").unwrap_err();
        match &e {
            ErroElevacao::NaoAutorizado { codigo, .. } => assert_eq!(*codigo, c),
            o => panic!("{o:?}"),
        }
        let t = e.to_string();
        assert!(t.contains("agente Polkit") && t.contains("setfacl"), "{t}");
    }
}

#[test]
fn recusa_do_helper_e_resposta_truncada() {
    let f = Falso::novo(saida(
        2,
        "{\"tipo\":\"erro\",\"codigo\":\"fora_das_raizes\",\"mensagem\":\"m\"}\n",
        "",
    ));
    assert!(matches!(
        cliente(f).listar(&["/a".into()], "/b").unwrap_err(),
        ErroElevacao::Recusado { .. }
    ));
    let f = Falso::novo(saida(
        0,
        &format!("{{\"tipo\":\"dir\",{}}}\n", meta("dir", 1)),
        "",
    ));
    assert!(matches!(
        cliente(f).listar(&["/".into()], "/x").unwrap_err(),
        ErroElevacao::RespostaInvalida(_)
    ));
    let f = Falso::novo(saida(0, "lixo\n", ""));
    assert!(cliente(f).listar(&["/".into()], "/x").is_err());
}

#[test]
fn helper_ausente_e_detectado_antes_do_pkexec() {
    let f = Falso::novo(saida(0, "", ""));
    let c = ClienteHelper::com_executor(
        f.clone(),
        "pkexec".into(),
        "/nao/existe/dag-helper".into(),
        true,
    );
    assert!(matches!(
        c.listar(&["/".into()], "/x").unwrap_err(),
        ErroElevacao::SemHelper(_)
    ));
    assert!(f.vistas.lock().unwrap().is_empty());
}

#[test]
fn diagnostico_com_proc_falso() {
    let t = tempfile::tempdir().unwrap();
    let proc_ = t.path().join("proc");
    std::fs::create_dir_all(proc_.join("10")).unwrap();
    std::fs::write(proc_.join("10/comm"), "bash\n").unwrap();
    let helper = t.path().join("dag-helper");
    let pk = t.path().join("pkexec");
    let d = diagnosticar(&helper, &proc_, &[pk.as_path()]);
    assert_eq!(
        d,
        Diagnostico {
            pkexec: false,
            helper: false,
            agente_polkit: false
        }
    );
    assert!(!d.pronto());
    assert!(d.mensagem().contains("Alternativas"));
    std::fs::write(&helper, "").unwrap();
    std::fs::write(&pk, "").unwrap();
    std::fs::create_dir_all(proc_.join("11")).unwrap();
    std::fs::write(proc_.join("11/comm"), "polkit-gnome-au\n").unwrap();
    let d = diagnosticar(&helper, &proc_, &[Path::new(&pk)]);
    assert!(d.pronto() && d.agente_polkit);
    assert!(d.mensagem().contains("disponível"));
}
