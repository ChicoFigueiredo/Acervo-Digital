#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Testes com executor falso: rodam sem Calibre instalado.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::fixtures::Sintetica;
use crate::calibredb::{
    AcessoServidor, Alvo, BibliotecaAClassificar, CampoPersonalizado, CamposMetadados,
    ClienteCalibredb, NovoLivro, Segredo, interpretar_lista, interpretar_versao,
};
use crate::erro::ErroCalibre;
use crate::executor::{ExecutorFalso, SaidaProcesso};
use crate::homologacao::{AprovacaoHomologacao, OpcoesHomologacao, homologar};
use crate::leitura::TipoColuna;
use crate::trava::analisar_cmdline;

const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn lib_arg(args: &[String]) -> PathBuf {
    let i = args.iter().rposition(|a| a == "--library-path").unwrap();
    PathBuf::from(&args[i + 1])
}

#[derive(Default)]
struct Estado {
    livros: Vec<(i64, String, bool)>, // id, hash, pendente
    falhar_set_metadata: bool,
}

/// Calibredb falso com estado: list/add/set_metadata/remove.
fn falso(estado: Arc<Mutex<Estado>>) -> ExecutorFalso {
    ExecutorFalso::com_funcao(move |c| {
        let a = &c.args;
        let mut st = estado.lock().unwrap();
        match a[0].as_str() {
            "list" if a.iter().any(|x| x == "--search") => {
                let i = a.iter().position(|x| x == "--search").unwrap();
                let h = a[i + 1].rsplit(':').next().unwrap().to_owned();
                let v: Vec<_> = st
                    .livros
                    .iter()
                    .filter(|l| l.1 == h)
                    .map(|l| {
                        let mut ids = json!({"dag": l.1});
                        if l.2 {
                            ids["dagpend"] = json!("1");
                        }
                        json!({"id": l.0, "identifiers": ids})
                    })
                    .collect();
                Ok(SaidaProcesso::ok(serde_json::to_string(&v).unwrap()))
            }
            "list" => {
                let p = lib_arg(a);
                if !p.join("metadata.db").exists() {
                    drop(Sintetica::nova(&p, "uuid-nova-a-classificar"));
                }
                Ok(SaidaProcesso::ok("[]"))
            }
            "add" => {
                let i = a.iter().position(|x| x.starts_with("dag:")).unwrap();
                let id = i64::try_from(st.livros.len()).unwrap() + 1;
                st.livros.push((id, a[i][4..].to_owned(), true));
                Ok(SaidaProcesso::ok(format!("Added book ids: {id}")))
            }
            "set_metadata" if st.falhar_set_metadata => Ok(SaidaProcesso::falha("boom")),
            "set_metadata" => {
                let id: i64 = a[1].parse().unwrap();
                for l in &mut st.livros {
                    if l.0 == id {
                        l.2 = false;
                    }
                }
                Ok(SaidaProcesso::ok(""))
            }
            "remove" => {
                st.livros.clear();
                Ok(SaidaProcesso::ok(""))
            }
            _ => Ok(SaidaProcesso::ok("")),
        }
    })
}

fn cenario() -> (tempfile::TempDir, PathBuf, Sintetica) {
    let t = tempfile::tempdir().unwrap();
    let h = t.path().join("homologada");
    let s = Sintetica::nova(&h, "uuid-homologada");
    (t, h, s)
}

fn arquivo(dir: &Path, nome: &str, conteudo: &[u8]) -> PathBuf {
    let p = dir.join(nome);
    std::fs::write(&p, conteudo).unwrap();
    p
}

fn cliente(exec: ExecutorFalso) -> ClienteCalibredb<ExecutorFalso> {
    ClienteCalibredb::novo(exec).com_processos(vec![])
}

#[test]
fn recusa_criar_a_classificar_na_homologada_ou_perto_dela() {
    let (t, h, _s) = cenario();
    let cli = cliente(ExecutorFalso::com_fila(vec![]));
    for destino in [
        h.clone(),
        h.join("filha"),
        h.join("../homologada/./"),
        t.path().to_path_buf(),
    ] {
        let e = cli.criar_a_classificar(&destino, &h).unwrap_err();
        assert!(
            matches!(e, ErroCalibre::CaminhoProibido(_)),
            "{destino:?}: {e}"
        );
    }
    // symlink para a Homologada
    let ln = t.path().join("atalho");
    std::os::unix::fs::symlink(&h, &ln).unwrap();
    assert!(matches!(
        cli.criar_a_classificar(&ln, &h).unwrap_err(),
        ErroCalibre::CaminhoProibido(_)
    ));
    // diretório com lixo
    let sujo = t.path().join("sujo");
    std::fs::create_dir_all(&sujo).unwrap();
    std::fs::write(sujo.join("a"), b"x").unwrap();
    assert!(matches!(
        cli.criar_a_classificar(&sujo, &h).unwrap_err(),
        ErroCalibre::DiretorioNaoVazio(_)
    ));
    // cópia da Homologada: caminho distinto, mas mesmo library_id
    let copia = t.path().join("copia");
    std::fs::create_dir_all(&copia).unwrap();
    std::fs::copy(h.join("metadata.db"), copia.join("metadata.db")).unwrap();
    assert!(matches!(
        BibliotecaAClassificar::abrir(&copia, &h).unwrap_err(),
        ErroCalibre::CaminhoProibido(_)
    ));
    assert!(cli.criar_a_classificar(&h, &h).is_err());
    assert!(cli.exec_chamadas().is_empty(), "nada foi executado");
}

impl ClienteCalibredb<ExecutorFalso> {
    fn exec_chamadas(&self) -> Vec<Vec<String>> {
        self.executor().chamadas()
    }
}

#[test]
fn criar_a_classificar_e_reabrir() {
    let (t, h, _s) = cenario();
    let cli = cliente(falso(Arc::default()));
    let dest = t.path().join("a-classificar");
    let ac = cli.criar_a_classificar(&dest, &h).unwrap();
    assert_ne!(ac.library_id(), "uuid-homologada");
    let de_novo = BibliotecaAClassificar::abrir(&dest, &h).unwrap();
    assert_eq!(ac, de_novo);
    let chamadas = cli.exec_chamadas();
    assert_eq!(chamadas.len(), 1);
    assert!(chamadas[0].contains(&"--library-path".to_owned()));
}

#[test]
fn adicionar_tres_vezes_nao_duplica() {
    let (t, h, _s) = cenario();
    let estado = Arc::new(Mutex::new(Estado::default()));
    let cli = cliente(falso(estado.clone()));
    let ac = cli.criar_a_classificar(&t.path().join("ac"), &h).unwrap();
    let f = arquivo(t.path(), "livro.pdf", b"conteudo");
    let novo = NovoLivro {
        arquivo: f,
        hash_blake3: HASH.into(),
        capa: None,
        confianca: Some(0.9),
        formatos_extras: vec![],
        metadados: CamposMetadados {
            titulo: Some("Título Ünico".into()),
            titulo_ordenacao: Some("Titulo Unico".into()),
            autor_ordenacao: Some("Souza, Ana".into()),
            autores: Some(vec!["Ana Souza".into()]),
            ..CamposMetadados::default()
        },
    };
    let r: Vec<_> = (0..3).map(|_| cli.adicionar(&ac, &novo).unwrap()).collect();
    assert!(r[0].criado);
    assert!(!r[1].criado && !r[2].criado);
    assert!(r.iter().all(|x| x.id == 1));
    assert_eq!(estado.lock().unwrap().livros.len(), 1);
    let adds = cli.exec_chamadas().iter().filter(|a| a[0] == "add").count();
    assert_eq!(adds, 1);
    // o add leva o identificador e --duplicates; sort explícitos no set_metadata
    let ch = cli.exec_chamadas();
    let add = ch.iter().find(|a| a[0] == "add").unwrap();
    assert!(add.contains(&"--duplicates".to_owned()));
    assert!(add.contains(&format!("dag:{HASH}")));
    let sm = ch.iter().find(|a| a[0] == "set_metadata").unwrap();
    assert!(sm.contains(&"title_sort:Titulo Unico".to_owned()));
    assert!(sm.contains(&"author_sort:Souza, Ana".to_owned()));
    assert!(sm.contains(&format!("identifiers:dag:{HASH}")));
}

#[test]
fn falha_no_meio_deixa_pendencia_e_a_retomada_nao_duplica() {
    let (t, h, _s) = cenario();
    let estado = Arc::new(Mutex::new(Estado::default()));
    let cli = cliente(falso(estado.clone()));
    let ac = cli.criar_a_classificar(&t.path().join("ac"), &h).unwrap();
    let novo = NovoLivro {
        arquivo: arquivo(t.path(), "l.pdf", b"x"),
        hash_blake3: HASH.into(),
        capa: None,
        confianca: None,
        formatos_extras: vec![],
        metadados: CamposMetadados {
            titulo: Some("T".into()),
            ..CamposMetadados::default()
        },
    };
    estado.lock().unwrap().falhar_set_metadata = true;
    assert!(cli.adicionar(&ac, &novo).is_err());
    assert!(estado.lock().unwrap().livros[0].2, "marcado como pendente");
    estado.lock().unwrap().falhar_set_metadata = false;
    let r = cli.adicionar(&ac, &novo).unwrap();
    assert!(!r.criado && r.retomado);
    assert_eq!(estado.lock().unwrap().livros.len(), 1);
    assert!(!estado.lock().unwrap().livros[0].2);
}

#[test]
fn caminhos_invalidos_nao_chegam_ao_calibredb() {
    let (t, h, _s) = cenario();
    let cli = cliente(falso(Arc::default()));
    let ac = cli.criar_a_classificar(&t.path().join("ac"), &h).unwrap();
    let antes = cli.exec_chamadas().len();
    let vazio = arquivo(t.path(), "vazio.pdf", b"");
    let dir = t.path().join("pasta");
    std::fs::create_dir_all(&dir).unwrap();
    for arq in [
        PathBuf::new(),
        PathBuf::from("relativo.pdf"),
        vazio,
        dir,
        t.path().join("nao-existe.pdf"),
    ] {
        let novo = NovoLivro {
            arquivo: arq.clone(),
            hash_blake3: HASH.into(),
            capa: None,
            confianca: None,
            formatos_extras: vec![],
            metadados: CamposMetadados::default(),
        };
        assert!(
            matches!(
                cli.adicionar(&ac, &novo).unwrap_err(),
                ErroCalibre::ValorInvalido(_)
            ),
            "{arq:?}"
        );
    }
    let mut novo = NovoLivro {
        arquivo: arquivo(t.path(), "ok.pdf", b"1"),
        hash_blake3: "ZZ".into(),
        capa: None,
        confianca: None,
        formatos_extras: vec![],
        metadados: CamposMetadados::default(),
    };
    assert!(cli.adicionar(&ac, &novo).is_err(), "hash malformado");
    novo.hash_blake3 = HASH.to_uppercase();
    assert!(cli.adicionar(&ac, &novo).is_err(), "hash em maiúsculas");
    assert_eq!(cli.exec_chamadas().len(), antes);
}

#[test]
fn argumentos_de_metadados() {
    let c = CamposMetadados {
        titulo: Some("T".into()),
        autores: Some(vec!["A".into(), "B".into()]),
        tags: Some(vec!["x".into(), "y".into()]),
        serie: Some("S".into()),
        indice_serie: Some(2.0),
        avaliacao: Some(8),
        idiomas: Some(vec!["por".into()]),
        identificadores: Some(vec![("isbn".into(), "1".into())]),
        personalizados: vec![
            CampoPersonalizado {
                rotulo: "s".into(),
                tipo: TipoColuna::Series,
                valor: json!({"serie":"C","indice":2.5}),
            },
            CampoPersonalizado {
                rotulo: "b".into(),
                tipo: TipoColuna::Bool,
                valor: json!(true),
            },
            CampoPersonalizado {
                rotulo: "r".into(),
                tipo: TipoColuna::Rating,
                valor: json!(6),
            },
            CampoPersonalizado {
                rotulo: "m".into(),
                tipo: TipoColuna::Text,
                valor: json!(["u", "v"]),
            },
        ],
        ..CamposMetadados::default()
    };
    let a = c.argumentos().unwrap().join("|");
    for esperado in [
        "-f|authors:A&B",
        "-f|tags:x,y",
        "-f|series:S",
        "-f|series_index:2",
        "-f|rating:4",
        "-f|languages:por",
        "-f|identifiers:isbn:1",
        "-f|#s:C",
        "-f|#s_index:2.5",
        "-f|#b:true",
        "-f|#r:3",
        "-f|#m:u,v",
    ] {
        assert!(a.contains(esperado), "falta {esperado} em {a}");
    }
    let ruim = |c: CamposMetadados| c.argumentos().is_err();
    assert!(ruim(CamposMetadados {
        tags: Some(vec!["a,b".into()]),
        ..Default::default()
    }));
    assert!(ruim(CamposMetadados {
        autores: Some(vec!["A & B".into()]),
        ..Default::default()
    }));
    assert!(ruim(CamposMetadados {
        avaliacao: Some(7),
        ..Default::default()
    }));
    assert!(ruim(CamposMetadados {
        personalizados: vec![CampoPersonalizado {
            rotulo: "c".into(),
            tipo: TipoColuna::Composite,
            valor: json!("x")
        }],
        ..Default::default()
    }));
}

#[test]
fn versao_e_lista() {
    assert_eq!(
        interpretar_versao("calibredb (calibre 7.6)").unwrap().maior,
        7
    );
    let v = interpretar_versao("calibredb (calibre 9.14.0)\n").unwrap();
    assert_eq!((v.maior, v.menor, v.patch), (9, 14, 0));
    assert!(interpretar_versao("lixo").is_err());
    let l = interpretar_lista("aviso\n[{\"id\": 3, \"title\": \"x\"}]\n").unwrap();
    assert_eq!(l[0].id, 3);
    assert!(interpretar_lista("nada").is_err());
}

#[test]
fn lock_do_calibre_vira_erro_tipado_com_orientacao() {
    let (t, h, _s) = cenario();
    let cli = cliente(ExecutorFalso::com_funcao(|c| {
        if c.args[0] == "list" {
            Ok(SaidaProcesso::falha(
                "Another calibre program such as calibre-server or the main calibre program is running.",
            ))
        } else {
            Ok(SaidaProcesso::ok(""))
        }
    }));
    let e = cli
        .criar_a_classificar(&t.path().join("ac"), &h)
        .unwrap_err();
    assert!(matches!(e, ErroCalibre::CalibreOcupado { .. }));
    assert!(e.to_string().contains("por usuário"));
    assert!(e.to_string().contains("--with-library"));
}

#[test]
fn servidor_servindo_a_biblioteca_alvo_bloqueia_escrita_local() {
    let (t, h, _s) = cenario();
    let dest = t.path().join("ac");
    let cli = cliente(falso(Arc::default()));
    let ac = cli.criar_a_classificar(&dest, &h).unwrap();
    let srv = analisar_cmdline(
        77,
        &["calibre-server".into(), dest.to_string_lossy().into_owned()],
    )
    .unwrap();
    let cli2 = ClienteCalibredb::novo(falso(Arc::default())).com_processos(vec![srv]);
    let novo = NovoLivro {
        arquivo: arquivo(t.path(), "a.pdf", b"1"),
        hash_blake3: HASH.into(),
        capa: None,
        confianca: None,
        formatos_extras: vec![],
        metadados: CamposMetadados::default(),
    };
    let e = cli2.adicionar(&ac, &novo).unwrap_err();
    assert!(
        matches!(e, ErroCalibre::ServidorServindo { pid: 77, .. })
            || matches!(e, ErroCalibre::CalibreOcupado { .. })
    );
    assert!(!cli2.exec_chamadas().iter().any(|a| a[0] == "add"));
}

#[test]
fn modo_servidor_manda_a_senha_por_stdin_e_nunca_em_argv() {
    let capt: Arc<Mutex<Vec<crate::executor::Comando>>> = Arc::default();
    let c2 = capt.clone();
    let cli = ClienteCalibredb::novo(ExecutorFalso::com_funcao(move |c| {
        c2.lock().unwrap().push(c.clone());
        Ok(SaidaProcesso::ok("[]"))
    }))
    .com_processos(vec![]);
    let acesso = AcessoServidor {
        url: "http://127.0.0.1:17790/".into(),
        id_biblioteca: "Livros".into(),
        usuario: "dag".into(),
        senha: Segredo::novo("s3gr3d0-ultra"),
    };
    assert!(!format!("{acesso:?}").contains("s3gr3d0"));
    cli.rodar(
        &Alvo::Servidor(acesso),
        "list",
        &["--for-machine".into()],
        true,
    )
    .unwrap();
    let c = capt.lock().unwrap()[0].clone();
    let args = c.args.join(" ");
    assert!(args.contains("--library-path http://127.0.0.1:17790/#Livros"));
    assert!(args.contains("--username dag"));
    assert!(args.contains("--password <f:/dev/stdin>"));
    assert!(!args.contains("s3gr3d0"));
    assert_eq!(c.stdin.as_deref(), Some(b"s3gr3d0-ultra".as_slice()));
    assert!(!format!("{c:?}").contains("s3gr3d0"));
}

#[test]
fn homologada_protegida_nunca_recebe_calibredb() {
    let (t, h, _s) = cenario();
    let cli = ClienteCalibredb::novo(ExecutorFalso::com_fila(vec![]))
        .com_processos(vec![])
        .proteger_homologada(&h)
        .unwrap();
    let copia = t.path().join("copia");
    std::fs::create_dir_all(&copia).unwrap();
    std::fs::copy(h.join("metadata.db"), copia.join("metadata.db")).unwrap();
    for p in [h.clone(), h.join("sub"), copia, t.path().to_path_buf()] {
        for sub in ["list", "clone", "backup_metadata", "check_library"] {
            let e = cli
                .rodar(&Alvo::Local(p.clone()), sub, &[], false)
                .unwrap_err();
            assert!(
                matches!(e, ErroCalibre::CaminhoProibido(_)),
                "{sub} em {p:?}"
            );
        }
    }
    assert!(cli.exec_chamadas().is_empty());
    // biblioteca não relacionada passa
    let (_t2, h2, _s2) = cenario_outra();
    cli.rodar(&Alvo::Local(h2), "list", &[], false).unwrap();
}

fn cenario_outra() -> (tempfile::TempDir, PathBuf, Sintetica) {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("outra");
    let s = Sintetica::nova(&p, "uuid-outra");
    (t, p, s)
}

// ---------- homologação ----------

struct CenarioHomolog {
    _t: tempfile::TempDir,
    h: PathBuf,
    _sh: Sintetica,
    ac: BibliotecaAClassificar,
    _sac: Sintetica,
}

fn cenario_homolog() -> CenarioHomolog {
    let t = tempfile::tempdir().unwrap();
    let h = t.path().join("h");
    let sh = Sintetica::nova(&h, "uuid-h");
    let acp = t.path().join("ac");
    let sac = Sintetica::nova(&acp, "uuid-ac");
    sac.livro(1, "T", "Au/T (1)");
    sac.conn
        .execute_batch(&format!(
            "INSERT INTO identifiers(book,type,val) VALUES (1,'dag','{HASH}');
             INSERT INTO data(book,format,uncompressed_size,name) VALUES (1,'TXT',3,'f');"
        ))
        .unwrap();
    std::fs::create_dir_all(acp.join("Au/T (1)")).unwrap();
    std::fs::write(acp.join("Au/T (1)/f.txt"), b"abc").unwrap();
    let ac = BibliotecaAClassificar::abrir(&acp, &h).unwrap();
    CenarioHomolog {
        _t: t,
        h,
        _sh: sh,
        ac,
        _sac: sac,
    }
}

fn aprovacao(hash: &str, quando: u64) -> AprovacaoHomologacao {
    AprovacaoHomologacao {
        aprovado_por: "Chico".into(),
        aprovado_em_unix: quando,
        hash_blake3: hash.into(),
        motivo: "teste".into(),
    }
}

const AGORA: u64 = 1_800_000_000;

fn opcoes() -> OpcoesHomologacao<'static> {
    OpcoesHomologacao {
        agora_unix: Some(AGORA),
        ..OpcoesHomologacao::default()
    }
}

#[test]
fn homologar_exige_aprovacao_valida_antes_de_qualquer_escrita() {
    let c = cenario_homolog();
    let cli = cliente(ExecutorFalso::com_fila(vec![]));
    let alvo = Alvo::Local(c.h.clone());
    let casos = [
        (
            "sem nome",
            AprovacaoHomologacao {
                aprovado_por: "  ".into(),
                ..aprovacao(HASH, AGORA)
            },
        ),
        ("outro hash", aprovacao(&"b".repeat(64), AGORA)),
        ("expirada", aprovacao(HASH, AGORA - 8 * 24 * 3600)),
        ("futuro", aprovacao(HASH, AGORA + 3600)),
        ("sem data", aprovacao(HASH, 0)),
    ];
    for (nome, ap) in casos {
        let e = homologar(&cli, &c.ac, 1, &c.h, &alvo, &ap, &opcoes()).unwrap_err();
        assert!(
            matches!(e, ErroCalibre::AprovacaoInvalida(_)),
            "{nome}: {e}"
        );
    }
    assert!(cli.exec_chamadas().is_empty());
}

#[test]
fn homologar_recusa_livro_que_ja_existe_na_homologada() {
    let c = cenario_homolog();
    c._sh.livro(1, "Já", "X/Já (1)");
    c._sh
        .conn
        .execute(
            "INSERT INTO identifiers(book,type,val) VALUES (1,'dag',?1)",
            [HASH],
        )
        .unwrap();
    let cli = cliente(ExecutorFalso::com_fila(vec![]));
    let e = homologar(
        &cli,
        &c.ac,
        1,
        &c.h,
        &Alvo::Local(c.h.clone()),
        &aprovacao(HASH, AGORA),
        &opcoes(),
    )
    .unwrap_err();
    assert!(matches!(e, ErroCalibre::JaExisteNaHomologada(_)));
    assert!(cli.exec_chamadas().is_empty());
    // e por hash de formato conhecido pelo Catálogo
    let c = cenario_homolog();
    let f = |h: &str| (h == blake3::hash(b"abc").to_hex().as_str()).then(|| "Calc.pdf".to_owned());
    let op = OpcoesHomologacao {
        hash_ja_homologado: Some(&f),
        ..opcoes()
    };
    let e = homologar(
        &cli,
        &c.ac,
        1,
        &c.h,
        &Alvo::Local(c.h.clone()),
        &aprovacao(HASH, AGORA),
        &op,
    )
    .unwrap_err();
    assert!(matches!(e, ErroCalibre::JaExisteNaHomologada(_)));
}

#[test]
fn homologar_com_servidor_servindo_recusa_local() {
    let c = cenario_homolog();
    let srv = analisar_cmdline(
        5,
        &["calibre-server".into(), c.h.to_string_lossy().into_owned()],
    )
    .unwrap();
    let cli = ClienteCalibredb::novo(ExecutorFalso::com_fila(vec![])).com_processos(vec![srv]);
    let e = homologar(
        &cli,
        &c.ac,
        1,
        &c.h,
        &Alvo::Local(c.h.clone()),
        &aprovacao(HASH, AGORA),
        &opcoes(),
    )
    .unwrap_err();
    assert!(matches!(e, ErroCalibre::ServidorServindo { .. }), "{e}");
    assert!(!cli.exec_chamadas().iter().any(|a| a[0] == "add"));
}

#[test]
fn homologar_falha_no_meio_reverte_o_livro_criado() {
    let c = cenario_homolog();
    let cli = cliente(ExecutorFalso::com_funcao(|cmd| {
        match cmd.args[0].as_str() {
            "add" => Ok(SaidaProcesso::ok("Added book ids: 9")),
            "set_metadata" => Ok(SaidaProcesso::falha("boom")),
            _ => Ok(SaidaProcesso::ok("")),
        }
    }));
    let e = homologar(
        &cli,
        &c.ac,
        1,
        &c.h,
        &Alvo::Local(c.h.clone()),
        &aprovacao(HASH, AGORA),
        &opcoes(),
    )
    .unwrap_err();
    match e {
        ErroCalibre::HomologacaoParcial {
            id_criado,
            situacao,
            ..
        } => {
            assert_eq!(id_criado, 9);
            assert!(situacao.contains("revertido"), "{situacao}");
        }
        outro => panic!("{outro}"),
    }
    let ch = cli.exec_chamadas();
    let rm = ch.iter().find(|a| a[0] == "remove").unwrap();
    assert!(rm.contains(&"--permanent".to_owned()) && rm.contains(&"9".to_owned()));
}

#[test]
fn homologar_add_interrompido_desfaz_o_livro_orfao() {
    // O `add` cria o livro na Homologada e estoura o tempo antes de dizer o id.
    let c = cenario_homolog();
    let db = c.h.join("metadata.db");
    let cli = cliente(ExecutorFalso::com_funcao(move |cmd| {
        if cmd.args[0] == "add" {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(&format!(
                "INSERT INTO books(id,title,sort,path) VALUES (7,'T','T','Au/T (7)');
                 INSERT INTO identifiers(book,type,val) VALUES (7,'dag','{HASH}');
                 INSERT INTO identifiers(book,type,val) VALUES (7,'dagpend','1');"
            ))
            .unwrap();
            return Err(ErroCalibre::Timeout {
                comando: "add".into(),
                segundos: 1,
            });
        }
        Ok(SaidaProcesso::ok(""))
    }));
    let e = homologar(
        &cli,
        &c.ac,
        1,
        &c.h,
        &Alvo::Local(c.h.clone()),
        &aprovacao(HASH, AGORA),
        &opcoes(),
    )
    .unwrap_err();
    match e {
        ErroCalibre::HomologacaoParcial {
            id_criado,
            situacao,
            ..
        } => {
            assert_eq!(id_criado, 7);
            assert!(situacao.contains("revertido"), "{situacao}");
        }
        outro => panic!("{outro}"),
    }
    let ch = cli.exec_chamadas();
    let rm = ch.iter().find(|a| a[0] == "remove").unwrap();
    assert!(rm.contains(&"7".to_owned()));
}

#[test]
fn homologar_add_que_falha_sem_criar_nada_nao_remove() {
    let c = cenario_homolog();
    let cli = cliente(ExecutorFalso::com_funcao(|cmd| {
        if cmd.args[0] == "add" {
            return Err(ErroCalibre::Timeout {
                comando: "add".into(),
                segundos: 1,
            });
        }
        Ok(SaidaProcesso::ok(""))
    }));
    let e = homologar(
        &cli,
        &c.ac,
        1,
        &c.h,
        &Alvo::Local(c.h.clone()),
        &aprovacao(HASH, AGORA),
        &opcoes(),
    )
    .unwrap_err();
    assert!(matches!(e, ErroCalibre::Timeout { .. }), "{e}");
    assert!(!cli.exec_chamadas().iter().any(|a| a[0] == "remove"));
}
