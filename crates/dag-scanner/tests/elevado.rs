//! Gancho `LeitorElevado`: pasta que nega acesso é preenchida com metadados do leitor elevado.
#![allow(clippy::unwrap_used)]

mod comum;

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use comum::*;
use dag_scanner::{DesfechoScan, EntradaElevada, LeitorElevado, TipoElevado};
use dag_store::Catalogo;
use dag_store::inventario::consulta::listar_problemas;

fn entrada(dev: u64, nome: &str, tipo: TipoElevado, ino: u64, tamanho: u64) -> EntradaElevada {
    EntradaElevada {
        nome: nome.as_bytes().to_vec(),
        tipo,
        dev,
        ino,
        nlink: 1,
        mode: 0o644,
        uid: 0,
        gid: 0,
        tamanho,
        blocos: 8,
        mtime_ns: 1_000,
        ctime_ns: 2_000,
        alvo_symlink: None,
    }
}

/// Leitor falso: responde por caminho e registra o que foi pedido.
struct Falso {
    por_pasta: HashMap<Vec<u8>, Vec<EntradaElevada>>,
    pedidos: Mutex<Vec<Vec<u8>>>,
}

impl LeitorElevado for Falso {
    fn listar(&self, pasta: &[u8]) -> io::Result<Vec<EntradaElevada>> {
        self.pedidos.lock().unwrap().push(pasta.to_vec());
        self.por_pasta
            .get(pasta)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "não autorizado"))
    }
}

fn uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0)
}

fn logica() {
    let tmp = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
    let raiz = tmp.path().join("raiz");
    escrever(&raiz.join("ok/a.txt"), 1);
    escrever(&raiz.join("trancada/b.txt"), 1);
    escrever(&raiz.join("trancada/c/d.txt"), 1);
    fs::set_permissions(raiz.join("trancada"), fs::Permissions::from_mode(0o000)).unwrap();
    let dev = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&raiz).unwrap().dev()
    };
    let trancada = raiz.join("trancada").as_os_str().as_bytes().to_vec();
    let c = raiz.join("trancada/c").as_os_str().as_bytes().to_vec();

    // Sem leitor: comportamento de sempre.
    let mut cat = Catalogo::em_memoria().unwrap();
    let r = scan(&raiz).executar(&mut cat).unwrap();
    assert_eq!(r.desfecho, DesfechoScan::ConcluidaComErros);
    let probs =
        listar_problemas(cat.armazem().conexao(), r.execucao_id, 0, 10, Some(false)).unwrap();
    assert_eq!(probs.len(), 1);
    assert_eq!(probs[0].codigo, "permissao_negada");

    // Com leitor: a subárvore negada é preenchida só com metadados.
    let falso = Arc::new(Falso {
        por_pasta: HashMap::from([
            (
                trancada.clone(),
                vec![
                    entrada(dev, "b.txt", TipoElevado::Arquivo, 10, 123),
                    entrada(dev, "c", TipoElevado::Pasta, 11, 4096),
                ],
            ),
            (
                c.clone(),
                vec![entrada(dev, "d.txt", TipoElevado::Arquivo, 12, 7)],
            ),
        ]),
        pedidos: Mutex::default(),
    });
    let mut cat = Catalogo::em_memoria().unwrap();
    let r = scan(&raiz)
        .com_leitor_elevado(falso.clone())
        .executar(&mut cat)
        .unwrap();
    let pedidos = falso.pedidos.lock().unwrap().clone();
    assert!(
        pedidos.contains(&trancada) && pedidos.contains(&c),
        "{pedidos:?}"
    );
    assert!(
        !pedidos.contains(&raiz.as_os_str().as_bytes().to_vec()),
        "só pastas negadas"
    );
    let f = foto(&cat);
    for rel in ["ok/a.txt", "trancada/b.txt", "trancada/c/d.txt"] {
        assert!(
            f.arquivos
                .contains_key(raiz.join(rel).as_os_str().as_bytes()),
            "falta {rel}"
        );
    }
    assert!(f.pastas.contains_key(&c));
    assert_eq!(
        n(
            &cat,
            "SELECT logical_bytes FROM files WHERE name_raw = CAST('b.txt' AS BLOB)"
        ),
        123
    );
    // O acesso negado fica registrado como aviso; nada vira erro grave.
    let probs = listar_problemas(cat.armazem().conexao(), r.execucao_id, 0, 10, None).unwrap();
    assert_eq!(probs.len(), 2);
    assert!(probs.iter().all(|p| p.codigo == "permissao_negada_elevada"));
    // O aviso mantém o desfecho "com erros", como no acesso negado comum (fica auditável).
    assert_eq!(r.desfecho, DesfechoScan::ConcluidaComErros);

    // Leitor que falha: volta a registrar como inacessível.
    let mudo = Arc::new(Falso {
        por_pasta: HashMap::new(),
        pedidos: Mutex::default(),
    });
    let mut cat = Catalogo::em_memoria().unwrap();
    let r = scan(&raiz)
        .com_leitor_elevado(mudo)
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r.desfecho, DesfechoScan::ConcluidaComErros);
    let probs =
        listar_problemas(cat.armazem().conexao(), r.execucao_id, 0, 10, Some(false)).unwrap();
    assert_eq!(probs[0].codigo, "permissao_negada");
    assert!(
        probs[0]
            .motivo
            .as_deref()
            .unwrap()
            .contains("leitura elevada falhou")
    );

    fs::set_permissions(raiz.join("trancada"), fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn leitor_elevado_preenche_subarvore_negada() {
    if uid() != 0 {
        logica();
        return;
    }
    // Como root o kernel ignora o modo 000: repete o teste como uid 65534 via `setpriv`.
    let tmp = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let copia = tmp.path().join("teste");
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    fs::copy(exe, &copia).unwrap();
    fs::set_permissions(&copia, fs::Permissions::from_mode(0o755)).unwrap();
    let r = Command::new("setpriv")
        .args(["--reuid=65534", "--regid=65534", "--clear-groups"])
        .arg(&copia)
        .args([
            "--exact",
            "leitor_elevado_preenche_subarvore_negada",
            "--nocapture",
        ])
        .env("TMPDIR", "/tmp")
        .output();
    match r {
        Ok(o) if o.status.success() => {}
        Ok(o) if String::from_utf8_lossy(&o.stderr).contains("setpriv:") => {
            eprintln!(
                "PULADO: setpriv não pôde rebaixar: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        }
        Ok(o) => panic!(
            "filho falhou:\n{}\n{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => eprintln!("PULADO: sem setpriv ({e})"),
    }
    let _ = Path::new("/");
}
