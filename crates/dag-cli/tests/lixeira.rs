//! `dag lixeira` de ponta a ponta com o binário: dupla confirmação, `--sim --sim`, Única Cópia
//! e restaurar. O Catálogo é preparado em processo com o Volume apontando para um diretório
//! temporário (nenhum teste toca fora dele).
#![allow(clippy::unwrap_used)]

use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use dag_agent::Agente;
use dag_core::{
    Alvo, Comando, DispositivoDescoberto, ModoVarredura, Resposta, TipoDispositivo,
    VolumeDescoberto,
};

fn volume(raiz: &Path) -> VolumeDescoberto {
    let dev = fs::metadata(raiz).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("T-{dev}"),
        st_dev: dev,
        rotulo: Some("T".into()),
        fs_type: "teste".into(),
        capacidade_bytes: Some(1_000_000),
        livre_bytes: Some(400_000),
        pontos_de_montagem: vec![raiz.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: Some(DispositivoDescoberto {
            identificador: "d".into(),
            modelo: None,
            tipo: TipoDispositivo::Ssd,
            removivel: false,
        }),
    }
}

struct Amb {
    _tmp: tempfile::TempDir,
    raiz: PathBuf,
    cat: String,
}

fn ambiente() -> Amb {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("acervo");
    for (rel, letra, n) in [
        ("a/dup.bin", b'd', 500),
        ("b/dup.bin", b'd', 500),
        ("c/so.bin", b's', 300),
    ] {
        let p = raiz.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![letra; n]).unwrap();
    }
    let cat = tmp.path().join("dados/c.sqlite");
    {
        let mut a = Agente::abrir(&cat)
            .unwrap()
            .silencioso()
            .com_volumes_fixos(vec![volume(&raiz)]);
        let r = a.executar(Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec![raiz.to_str().unwrap().into()],
            sem_hash: false,
            elevar: false,
            releitura_total: false,
            threads_por_disco: None,
            max_discos: None,
        });
        assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
        for rel in ["b/dup.bin", "c/so.bin"] {
            let r = a.executar(Comando::MarcarDelecao {
                alvo: Alvo::Caminho(raiz.join(rel).to_str().unwrap().into()),
            });
            assert!(matches!(r, Resposta::Marcacao { .. }), "{r:?}");
        }
    }
    Amb {
        _tmp: tmp,
        raiz,
        cat: cat.to_str().unwrap().into(),
    }
}

/// Roda `dag` com `stdin` (ou fechado).
fn dag(a: &Amb, args: &[&str], stdin: Option<&str>) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_dag"));
    c.env("DAG_SEM_AGENTE", "1")
        .env_remove("DAG_CATALOGO")
        .args(["--catalogo", &a.cat])
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut f = c.spawn().unwrap();
    if let Some(t) = stdin {
        f.stdin.take().unwrap().write_all(t.as_bytes()).unwrap();
    }
    f.wait_with_output().unwrap()
}

fn saida(o: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn movido(a: &Amb, rel: &str) -> bool {
    a.raiz.join("DAG-Lixeira").join(rel).exists() && !a.raiz.join(rel).exists()
}

fn no_lugar(a: &Amb, rel: &str) -> bool {
    a.raiz.join(rel).exists() && !a.raiz.join("DAG-Lixeira").join(rel).exists()
}

fn arq(a: &Amb, rel: &str) -> String {
    a.raiz.join(rel).to_str().unwrap().to_string()
}

#[test]
fn dupla_confirmacao_interativa() {
    let a = ambiente();
    let alvo = arq(&a, "b/dup.bin");
    // Sem terminal e sem --sim --sim: nada acontece.
    let o = dag(&a, &["lixeira", "mover", "--arquivo", &alvo], None);
    assert_eq!(o.status.code(), Some(1), "{}", saida(&o));
    assert!(no_lugar(&a, "b/dup.bin"));
    // Só a primeira confirmação: não basta.
    let o = dag(&a, &["lixeira", "mover", "--arquivo", &alvo], Some("s\n"));
    assert_eq!(o.status.code(), Some(1));
    assert!(no_lugar(&a, "b/dup.bin"));
    // Segunda palavra errada: cancela.
    let o = dag(
        &a,
        &["lixeira", "mover", "--arquivo", &alvo],
        Some("s\nmover\n"),
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(no_lugar(&a, "b/dup.bin"));
    // Resumo mostrado antes de perguntar.
    let t = saida(&o);
    assert!(
        t.contains("Seriam movidos: 1 arquivo(s)") && t.contains("DAG-Lixeira"),
        "{t}"
    );
    // As duas confirmações.
    let o = dag(
        &a,
        &["lixeira", "mover", "--arquivo", &alvo],
        Some("s\nMOVER\n"),
    );
    assert_eq!(o.status.code(), Some(0), "{}", saida(&o));
    assert!(movido(&a, "b/dup.bin"));
    assert_eq!(
        fs::read(a.raiz.join("DAG-Lixeira/b/dup.bin")).unwrap(),
        vec![b'd'; 500]
    );
}

#[test]
fn sim_sim_e_confirmar_duas_vezes_dispensam_as_perguntas() {
    let a = ambiente();
    let alvo = arq(&a, "b/dup.bin");
    // Um --sim só dispensa a primeira: sem a palavra, cancela.
    let o = dag(&a, &["lixeira", "mover", "--sim", "--arquivo", &alvo], None);
    assert_eq!(o.status.code(), Some(1));
    assert!(no_lugar(&a, "b/dup.bin"));
    let o = dag(
        &a,
        &["lixeira", "mover", "--sim", "--sim", "--arquivo", &alvo],
        None,
    );
    assert_eq!(o.status.code(), Some(0), "{}", saida(&o));
    assert!(movido(&a, "b/dup.bin"));
    // Listar e restaurar.
    let o = dag(&a, &["--json", "lixeira", "listar"], None);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["total"], 1);
    assert!(
        v["itens"][0]["origem"]
            .as_str()
            .unwrap()
            .ends_with("/b/dup.bin")
    );
    let o = dag(
        &a,
        &[
            "lixeira",
            "restaurar",
            "--arquivo",
            &arq(&a, "DAG-Lixeira/b/dup.bin"),
        ],
        None,
    );
    assert_eq!(o.status.code(), Some(0), "{}", saida(&o));
    assert!(no_lugar(&a, "b/dup.bin"));
    // Restaurar encerra a candidatura a descarte: sem marcar de novo, não vai.
    let o = dag(
        &a,
        &[
            "lixeira",
            "mover",
            "--confirmar-duas-vezes",
            "--arquivo",
            &alvo,
        ],
        None,
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(saida(&o).contains("sem_marcacao"), "{}", saida(&o));
    let o = dag(&a, &["marcar", "delecao", "--arquivo", &alvo], None);
    assert_eq!(o.status.code(), Some(0), "{}", saida(&o));
    // --confirmar-duas-vezes
    let o = dag(
        &a,
        &[
            "lixeira",
            "mover",
            "--confirmar-duas-vezes",
            "--arquivo",
            &alvo,
        ],
        None,
    );
    assert_eq!(o.status.code(), Some(0), "{}", saida(&o));
    assert!(movido(&a, "b/dup.bin"));
}

#[test]
fn unica_copia_exige_ultima_copia_e_simular_nao_move() {
    let a = ambiente();
    let alvo = arq(&a, "c/so.bin");
    let o = dag(
        &a,
        &["lixeira", "mover", "--sim", "--sim", "--arquivo", &alvo],
        None,
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(saida(&o).contains("unica_copia"), "{}", saida(&o));
    assert!(no_lugar(&a, "c/so.bin"));
    // --simular: só mostra.
    let o = dag(
        &a,
        &[
            "lixeira",
            "mover",
            "--simular",
            "--ultima-copia",
            "--arquivo",
            &alvo,
        ],
        None,
    );
    assert_eq!(o.status.code(), Some(0), "{}", saida(&o));
    assert!(saida(&o).contains("Única Cópia"));
    assert!(no_lugar(&a, "c/so.bin"));
    let o = dag(
        &a,
        &[
            "lixeira",
            "mover",
            "--sim",
            "--sim",
            "--ultima-copia",
            "--arquivo",
            &alvo,
        ],
        None,
    );
    assert_eq!(o.status.code(), Some(0), "{}", saida(&o));
    assert!(movido(&a, "c/so.bin"));
}
