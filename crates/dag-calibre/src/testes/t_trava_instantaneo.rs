#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::time::Duration;

use crate::erro::ErroCalibre;
use crate::executor::{Comando, ExecutorCalibre, ExecutorProcesso};
use crate::instantaneo::{Diferenca, Escopo, comparar, tirar};
use crate::trava::{PoliticaTrava, TipoProcessoCalibre, analisar_cmdline, verificar_escrita_local};

fn v(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn cmdline_do_servidor_do_chico() {
    let p = analisar_cmdline(
        42,
        &v(&[
            "/usr/bin/python3",
            "/usr/bin/calibre-server",
            "--port=17790",
            "--listen-on",
            "127.0.0.1",
            "--enable-auth",
            "--auth-mode=basic",
            "--disable-local-write",
            "/mnt/e/Livros",
        ]),
    )
    .unwrap();
    assert_eq!(p.tipo, TipoProcessoCalibre::Servidor);
    assert_eq!(p.porta, Some(17790));
    assert_eq!(p.escuta.as_deref(), Some("127.0.0.1"));
    assert!(p.desabilita_escrita_local);
    assert!(p.bibliotecas.iter().any(|b| b.ends_with("Livros")));
    assert!(analisar_cmdline(1, &v(&["/usr/bin/calibredb", "list"])).is_none());
    assert_eq!(
        analisar_cmdline(2, &v(&["/usr/bin/calibre"])).unwrap().tipo,
        TipoProcessoCalibre::Gui
    );
}

#[test]
fn trava_recusa_servidor_servindo_e_calibre_ocupado() {
    let t = tempfile::tempdir().unwrap();
    let lib = t.path().canonicalize().unwrap();
    let serve = analisar_cmdline(7, &v(&["calibre-server", lib.to_str().unwrap()])).unwrap();
    let e = verificar_escrita_local(
        &lib,
        PoliticaTrava::ApenasServidor,
        std::slice::from_ref(&serve),
    )
    .unwrap_err();
    assert!(matches!(e, ErroCalibre::ServidorServindo { pid: 7, .. }));
    // servidor de outra biblioteca: Estrita recusa (lock por usuário), ApenasServidor deixa
    let outro = analisar_cmdline(8, &v(&["calibre-server", "/nao/existe"])).unwrap();
    let e = verificar_escrita_local(&lib, PoliticaTrava::Estrita, std::slice::from_ref(&outro))
        .unwrap_err();
    assert!(matches!(e, ErroCalibre::CalibreOcupado { .. }));
    verificar_escrita_local(&lib, PoliticaTrava::ApenasServidor, &[outro]).unwrap();
    let gui = analisar_cmdline(9, &v(&["/usr/bin/calibre"])).unwrap();
    assert!(verificar_escrita_local(&lib, PoliticaTrava::Estrita, &[gui]).is_err());
    verificar_escrita_local(&lib, PoliticaTrava::Estrita, &[]).unwrap();
}

#[test]
fn executor_respeita_timeout_e_binario_ausente() {
    let e = ExecutorProcesso::novo("sleep");
    let r = e.executar(&Comando {
        args: v(&["5"]),
        stdin: None,
        timeout: Duration::from_millis(200),
    });
    assert!(matches!(r, Err(ErroCalibre::Timeout { .. })));
    let e = ExecutorProcesso::novo("nao-existe-calibredb-xyz");
    let r = e.executar(&Comando {
        args: v(&["--version"]),
        stdin: None,
        timeout: Duration::from_secs(5),
    });
    assert!(matches!(r, Err(ErroCalibre::CalibredbAusente(_))));
}

#[test]
fn timeout_mata_tambem_os_netos() {
    // Um neto em segundo plano segura o stdout: sem matar o grupo, a leitura esperaria 30 s.
    let e = ExecutorProcesso::novo("sh");
    let inicio = std::time::Instant::now();
    let r = e.executar(&Comando {
        args: v(&["-c", "sleep 30 & sleep 30"]),
        stdin: None,
        timeout: Duration::from_millis(300),
    });
    assert!(matches!(r, Err(ErroCalibre::Timeout { .. })));
    assert!(
        inicio.elapsed() < Duration::from_secs(10),
        "{:?}",
        inicio.elapsed()
    );
}

#[test]
fn saida_enorme_e_truncada_sem_travar() {
    let e = ExecutorProcesso::novo("sh");
    let s = e
        .executar(&Comando {
            args: v(&["-c", "head -c 70000000 /dev/zero; echo fim >&2"]),
            stdin: None,
            timeout: Duration::from_secs(60),
        })
        .unwrap();
    assert_eq!(s.status, Some(0));
    assert_eq!(s.stdout.len(), 64 * 1024 * 1024);
    assert_eq!(s.stderr.trim(), "fim");
}

#[test]
fn executor_captura_stdout_stderr_e_stdin() {
    let e = ExecutorProcesso::novo("sh");
    let s = e
        .executar(&Comando {
            args: v(&["-c", "cat; echo erro >&2; exit 3"]),
            stdin: Some(b"segredo".to_vec()),
            timeout: Duration::from_secs(5),
        })
        .unwrap();
    assert_eq!(s.status, Some(3));
    assert_eq!(s.stdout, "segredo");
    assert_eq!(s.stderr.trim(), "erro");
    let c = Comando {
        args: v(&["x"]),
        stdin: Some(b"segredo".to_vec()),
        timeout: Duration::from_secs(1),
    };
    assert!(!format!("{c:?}").contains("segredo"));
}

#[test]
fn prefixo_do_executor_e_aplicado() {
    // `env` como prefixo: `env sh -c ...` roda igual; prova que o prefixo vem antes do binário.
    let e = ExecutorProcesso::novo("sh").com_prefixo(v(&["env", "FOO=bar"]));
    let s = e
        .executar(&Comando {
            args: v(&["-c", "echo $FOO"]),
            stdin: None,
            timeout: Duration::from_secs(5),
        })
        .unwrap();
    assert_eq!(s.stdout.trim(), "bar");
}

#[test]
fn instantaneo_detecta_alteracao_criacao_e_remocao() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("metadata.db"), b"db").unwrap();
    std::fs::create_dir_all(t.path().join("A/B")).unwrap();
    std::fs::write(t.path().join("A/B/x.epub"), b"1234").unwrap();
    std::fs::write(t.path().join("A/B/y.pdf"), b"abcd").unwrap();
    let a = tirar(t.path(), Escopo::Completo, None).unwrap();
    assert!(a.arquivos["A/B/x.epub"].blake3.is_some());
    assert_eq!(
        comparar(&a, &tirar(t.path(), Escopo::Completo, Some(&a)).unwrap()),
        vec![]
    );
    // mesmo tamanho, conteúdo diferente
    std::fs::write(t.path().join("A/B/x.epub"), b"1235").unwrap();
    std::fs::remove_file(t.path().join("A/B/y.pdf")).unwrap();
    std::fs::write(t.path().join("novo.txt"), b"n").unwrap();
    let d = comparar(&a, &tirar(t.path(), Escopo::Completo, None).unwrap());
    assert!(d.iter().any(|x| matches!(x, Diferenca::Alterado { caminho, campos } if caminho == "A/B/x.epub" && campos.contains(&"blake3".to_owned()))));
    assert!(d.contains(&Diferenca::Removido {
        caminho: "A/B/y.pdf".into()
    }));
    assert!(d.contains(&Diferenca::Adicionado {
        caminho: "novo.txt".into()
    }));
    // escopo mínimo só vê os metadados
    let m = tirar(t.path(), Escopo::Minimo, None).unwrap();
    assert_eq!(m.arquivos.keys().collect::<Vec<_>>(), ["metadata.db"]);
}
