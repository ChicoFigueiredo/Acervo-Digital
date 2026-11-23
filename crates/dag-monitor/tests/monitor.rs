//! Monitor inotify (A-13; CA-A10, CA-A11): eventos persistidos após reconciliação, lacunas
//! corrigidas por Fast Scan, limites de watches e de fila, raiz que some.
#![allow(clippy::unwrap_used)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dag_core::{ExecucaoId, VolumeDescoberto};
use dag_monitor::{ConfigMonitor, ErroMonitor, InjetorMonitor, Monitor, ResumoMonitor};
use dag_scanner::{ControleScan, EtapaHash, FullScan};
use dag_store::Catalogo;
use dag_store::reconciliacao::ler_saude;
use rusqlite::{Connection, OpenFlags};

fn volume_de(p: &Path) -> VolumeDescoberto {
    let dev = fs::metadata(p).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("teste-dev-{dev}"),
        st_dev: dev,
        rotulo: None,
        fs_type: "teste".into(),
        capacidade_bytes: None,
        livre_bytes: None,
        pontos_de_montagem: vec![p.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        dispositivo: None,
    }
}

fn gravar(p: &Path, dados: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, dados).unwrap();
}

fn cfg() -> ConfigMonitor {
    ConfigMonitor {
        debounce: Duration::from_millis(150),
        debounce_maximo: Duration::from_millis(600),
        estabilidade: Duration::from_millis(100),
        heartbeat: Duration::from_millis(100),
        nova_tentativa: Duration::from_millis(400),
        reconciliar_a_cada: None,
        reconciliar_ao_iniciar: false,
        ..ConfigMonitor::default()
    }
}

struct Sessao {
    handle: Option<JoinHandle<Result<ResumoMonitor, ErroMonitor>>>,
    parar: ControleScan,
    injetor: InjetorMonitor,
    leitura: Connection,
    _tmp: tempfile::TempDir,
    #[allow(dead_code)]
    db: PathBuf,
    raiz: PathBuf,
}

/// Catálogo com Full Scan (e hash) feito; depois o monitor sobe numa thread.
fn iniciar(montar: impl FnOnce(&Path), config: ConfigMonitor) -> Sessao {
    iniciar_com(montar, config, |m| m)
}

fn iniciar_com(
    montar: impl FnOnce(&Path),
    config: ConfigMonitor,
    ajustar: impl FnOnce(Monitor) -> Monitor,
) -> Sessao {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    fs::create_dir_all(&raiz).unwrap();
    montar(&raiz);
    let db = tmp.path().join("catalogo.sqlite");
    {
        let mut cat = Catalogo::abrir(&db).unwrap();
        let r = FullScan::novo(vec![raiz.clone()], vec![volume_de(&raiz)])
            .executar(&mut cat)
            .unwrap();
        EtapaHash::nova(ExecucaoId(r.execucao_id))
            .executar(&mut cat)
            .unwrap();
    }
    let parar = ControleScan::novo();
    let vol = volume_de(&raiz);
    let mon =
        ajustar(Monitor::novo(vec![raiz.clone()], move || vec![vol.clone()]).com_config(config));
    let injetor = mon.injetor();
    let mut mon = mon;
    let (db2, p2) = (db.clone(), parar.clone());
    let handle = thread::spawn(move || {
        let mut cat = Catalogo::abrir(&db2).unwrap();
        mon.executar(&mut cat, &p2)
    });
    let leitura = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let s = Sessao {
        handle: Some(handle),
        parar,
        injetor,
        leitura,
        _tmp: tmp,
        db,
        raiz,
    };
    s.esperar("monitor ativo", |s| {
        ler_saude(&s.leitura).is_ok_and(|h| h.estado == "ativo" || h.estado == "reconciliando")
            && s.n("SELECT COUNT(*) FROM scan_runs WHERE mode = 'monitor'") == 1
    });
    s
}

impl Sessao {
    fn n(&self, sql: &str) -> i64 {
        self.leitura
            .query_row(sql, [], |l| l.get::<_, Option<i64>>(0))
            .unwrap()
            .unwrap_or(0)
    }

    fn esperar(&self, o_que: &str, mut cond: impl FnMut(&Self) -> bool) {
        let limite = Instant::now() + Duration::from_secs(20);
        while Instant::now() < limite {
            if cond(self) {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("tempo esgotado esperando: {o_que}");
    }

    fn sem_mudar(&self, dur: Duration, mut cond: impl FnMut(&Self) -> bool) -> bool {
        let fim = Instant::now() + dur;
        while Instant::now() < fim {
            if !cond(self) {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
        true
    }

    fn id(&self, rel: &str) -> Option<i64> {
        self.leitura
            .query_row(
                "SELECT id FROM files WHERE path_raw = ?1",
                [self.raiz.join(rel).as_os_str().as_bytes()],
                |l| l.get(0),
            )
            .ok()
    }

    fn estado(&self, rel: &str) -> Option<String> {
        self.leitura
            .query_row(
                "SELECT state FROM files WHERE path_raw = ?1",
                [self.raiz.join(rel).as_os_str().as_bytes()],
                |l| l.get(0),
            )
            .ok()
    }

    fn tamanho(&self, rel: &str) -> Option<i64> {
        self.leitura
            .query_row(
                "SELECT logical_bytes FROM files WHERE path_raw = ?1 AND state = 'ativo'",
                [self.raiz.join(rel).as_os_str().as_bytes()],
                |l| l.get(0),
            )
            .ok()
    }

    fn encerrar(mut self) -> ResumoMonitor {
        self.parar.cancelar();
        self.handle.take().unwrap().join().unwrap().unwrap()
    }
}

impl Drop for Sessao {
    fn drop(&mut self) {
        self.parar.cancelar();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn arvore(raiz: &Path) {
    gravar(&raiz.join("a.txt"), &[b'a'; 100]);
    gravar(&raiz.join("sub/b.txt"), &[b'b'; 200]);
    gravar(&raiz.join("sub/c.bin"), &[b'c'; 300]);
    gravar(&raiz.join("d/e.txt"), &[b'e'; 400]);
    gravar(&raiz.join("d/fundo/g.txt"), &[b'g'; 500]);
}

#[test]
fn ca_a10_criacao_edicao_renomeacao_e_remocao_sao_persistidas() {
    let s = iniciar(arvore, cfg());
    let saude = ler_saude(&s.leitura).unwrap();
    assert_eq!(saude.watches, 4, "raiz + sub + d + d/fundo");
    let id_b = s.id("sub/b.txt").unwrap();
    let id_a = s.id("a.txt").unwrap();

    // Criação.
    gravar(&s.raiz.join("novo.txt"), &[b'n'; 77]);
    s.esperar("criação", |s| s.tamanho("novo.txt") == Some(77));
    // Edição.
    gravar(&s.raiz.join("a.txt"), &[b'A'; 150]);
    s.esperar("edição", |s| s.tamanho("a.txt") == Some(150));
    assert_eq!(s.id("a.txt"), Some(id_a), "a edição mantém o id");
    // Renomeação: mesmo id.
    fs::rename(s.raiz.join("sub/b.txt"), s.raiz.join("sub/b2.txt")).unwrap();
    s.esperar("renomeação", |s| s.id("sub/b2.txt") == Some(id_b));
    assert_eq!(s.id("sub/b.txt"), None);
    // Remoção.
    fs::remove_file(s.raiz.join("sub/c.bin")).unwrap();
    s.esperar("remoção", |s| {
        s.estado("sub/c.bin").as_deref() == Some("ausente")
    });
    // Pasta nova com subpasta e arquivo (e o watch dela passa a valer).
    gravar(&s.raiz.join("nova/x/y.txt"), &[b'y'; 9]);
    s.esperar("pasta nova", |s| s.tamanho("nova/x/y.txt") == Some(9));
    gravar(&s.raiz.join("nova/x/z.txt"), &[b'z'; 11]);
    s.esperar("arquivo na pasta nova", |s| {
        s.tamanho("nova/x/z.txt") == Some(11)
    });
    // Pasta removida inteira.
    fs::remove_dir_all(s.raiz.join("d")).unwrap();
    s.esperar("pasta removida", |s| {
        s.estado("d/e.txt").as_deref() == Some("ausente")
            && s.estado("d/fundo/g.txt").as_deref() == Some("ausente")
    });
    assert_eq!(
        s.n("SELECT COUNT(*) FROM folders WHERE state = 'ausente'"),
        2,
        "d e d/fundo"
    );

    // Eventos persistidos com observed_at/processed_at.
    s.esperar("eventos processados", |s| {
        s.n("SELECT COUNT(*) FROM scan_events WHERE source = 'inotify' AND event_type = 'renomeado'")
            == 1
    });
    for tipo in ["criado", "alterado", "removido"] {
        assert!(
            s.n(&format!(
                "SELECT COUNT(*) FROM scan_events WHERE source = 'inotify' AND event_type = '{tipo}'"
            )) >= 1,
            "{tipo}"
        );
    }
    assert_eq!(
        s.n("SELECT COUNT(*) FROM scan_events WHERE source = 'inotify' AND (processed_at IS NULL OR observed_at IS NULL OR processed_at < observed_at)"),
        0
    );
    assert_eq!(
        s.n(&format!(
            "SELECT COUNT(*) FROM scan_events WHERE event_type = 'renomeado' AND file_id = {id_b} AND old_path_raw IS NOT NULL"
        )),
        1
    );
    // Saúde publicada.
    let h = ler_saude(&s.leitura).unwrap();
    assert!(
        h.ultimo_evento_em.is_some() && h.eventos_processados >= 6,
        "{h:?}"
    );
    assert_eq!(h.overflows, 0);
    assert!(h.heartbeat.is_some());
    let resumo = s.encerrar();
    assert!(resumo.renomeados >= 1 && resumo.removidos >= 1 && resumo.criados >= 1);
}

#[test]
fn renomear_pasta_pelo_monitor_preserva_ids() {
    let s = iniciar(arvore, cfg());
    let ids: Vec<i64> = ["d/e.txt", "d/fundo/g.txt"]
        .iter()
        .map(|p| s.id(p).unwrap())
        .collect();
    fs::rename(s.raiz.join("d"), s.raiz.join("d2")).unwrap();
    s.esperar("pasta renomeada", |s| {
        s.id("d2/e.txt") == Some(ids[0]) && s.id("d2/fundo/g.txt") == Some(ids[1])
    });
    s.esperar("sem pendências de ausente", |s| {
        s.n("SELECT COUNT(*) FROM files WHERE state <> 'ativo'") == 0
    });
    assert_eq!(
        s.n("SELECT COUNT(*) FROM folders WHERE state <> 'ativo'"),
        0
    );
    // O watch acompanhou: mexer dentro da pasta nova ainda é visto.
    gravar(&s.raiz.join("d2/fundo/h.txt"), &[b'h'; 5]);
    s.esperar("arquivo depois da renomeação", |s| {
        s.tamanho("d2/fundo/h.txt") == Some(5)
    });
}

#[test]
fn ca_a11_overflow_sintetico_registra_lacuna_e_reconcilia() {
    // Só a raiz tem watch: o que muda em subpastas é um "evento perdido".
    let s = iniciar(
        arvore,
        ConfigMonitor {
            max_watches: Some(1),
            ..cfg()
        },
    );
    // A lacuna de limite do início já foi corrigida pelo Fast Scan inicial.
    s.esperar("lacuna de limite fechada", |s| {
        s.n("SELECT COUNT(*) FROM scan_events WHERE detail LIKE 'limite_de_watches%' AND processed_at IS NOT NULL")
            == 1
    });
    gravar(&s.raiz.join("sub/perdido.txt"), &[b'p'; 33]);
    fs::remove_file(s.raiz.join("sub/c.bin")).unwrap();
    gravar(&s.raiz.join("sub/b.txt"), &[b'B'; 222]);
    assert!(
        s.sem_mudar(Duration::from_millis(1200), |s| s
            .tamanho("sub/perdido.txt")
            .is_none()
            && s.estado("sub/c.bin").as_deref() == Some("ativo")),
        "sem evento, o catálogo ainda não sabe"
    );
    // Overflow injetado: lacuna gravada com reconcile_required, corrigida por Fast Scan.
    s.injetor.injetar_overflow();
    s.esperar("lacuna de overflow fechada", |s| {
        s.n("SELECT COUNT(*) FROM scan_events WHERE event_type = 'overflow' AND detail LIKE 'overflow:%' AND reconcile_required = 1 AND processed_at IS NOT NULL") == 1
    });
    // Quando a lacuna foi declarada processada, a correção já estava no catálogo.
    assert_eq!(s.tamanho("sub/perdido.txt"), Some(33));
    assert_eq!(s.estado("sub/c.bin").as_deref(), Some("ausente"));
    assert_eq!(s.tamanho("sub/b.txt"), Some(222));
    let h = ler_saude(&s.leitura).unwrap();
    assert_eq!(
        h.overflows, 1,
        "o overflow (aqui injetado) é contado na saúde"
    );
    assert!(h.ultima_reconciliacao.is_some());
    assert_eq!(
        s.n(
            "SELECT COUNT(*) FROM scan_events WHERE reconcile_required = 1 AND processed_at IS NULL"
        ),
        0
    );
    let resumo = s.encerrar();
    assert!(resumo.fast_scans >= 2 && resumo.lacunas >= 2);
}

#[test]
fn limite_de_watches_registra_lacuna_e_a_reconciliacao_periodica_cobre() {
    let s = iniciar(
        arvore,
        ConfigMonitor {
            max_watches: Some(2),
            reconciliar_a_cada: Some(Duration::from_millis(1500)),
            ..cfg()
        },
    );
    let h = ler_saude(&s.leitura).unwrap();
    assert!(h.watches <= 2 && h.watches_limite == 2, "{h:?}");
    assert_eq!(h.raizes_parciais, 1);
    assert!(s.n("SELECT COUNT(*) FROM scan_events WHERE detail LIKE 'limite_de_watches%' AND reconcile_required = 1") >= 1);
    // Um diretório sem watch: só a reconciliação periódica (Fast Scan) enxerga.
    gravar(&s.raiz.join("d/fundo/tarde.txt"), &[b't'; 3]);
    s.esperar("periódico enxerga", |s| {
        s.tamanho("d/fundo/tarde.txt") == Some(3)
    });
}

#[test]
fn fila_saturada_vira_lacuna_e_fast_scan() {
    let s = iniciar(
        arvore,
        ConfigMonitor {
            max_dirs_pendentes: 2,
            debounce: Duration::from_millis(800),
            debounce_maximo: Duration::from_secs(3),
            ..cfg()
        },
    );
    for i in 0..12 {
        gravar(&s.raiz.join(format!("rajada/p{i}/f.txt")), &[b'r'; 4]);
    }
    s.esperar("tudo catalogado apesar da fila pequena", |s| {
        (0..12).all(|i| s.tamanho(&format!("rajada/p{i}/f.txt")) == Some(4))
    });
    s.esperar("lacuna de fila fechada", |s| {
        s.n("SELECT COUNT(*) FROM scan_events WHERE detail LIKE 'fila_saturada%' AND processed_at IS NOT NULL") >= 1
    });
}

#[test]
fn raiz_que_some_nao_remove_nada_e_ao_voltar_tudo_se_recupera() {
    let s = iniciar(arvore, cfg());
    let fora = s.raiz.with_file_name("acervo_fora");
    fs::rename(&s.raiz, &fora).unwrap();
    s.esperar("lacuna de raiz desmontada", |s| {
        s.n("SELECT COUNT(*) FROM scan_events WHERE detail LIKE 'raiz_desmontada%' AND reconcile_required = 1 AND processed_at IS NULL") >= 1
    });
    assert!(
        s.sem_mudar(Duration::from_millis(1200), |s| s
            .n("SELECT COUNT(*) FROM files WHERE state <> 'ativo'")
            == 0
            && s.n("SELECT COUNT(*) FROM folders WHERE state <> 'ativo'") == 0),
        "raiz indisponível nunca vira remoção"
    );
    // A raiz volta (com uma mudança no meio): lacuna corrigida e watches refeitos.
    gravar(&fora.join("a.txt"), &[b'X'; 111]);
    fs::rename(&fora, &s.raiz).unwrap();
    s.esperar("lacuna corrigida", |s| {
        s.n("SELECT COUNT(*) FROM scan_events WHERE detail LIKE 'raiz_desmontada%' AND processed_at IS NOT NULL") >= 1
            && s.tamanho("a.txt") == Some(111)
    });
    gravar(&s.raiz.join("sub/depois.txt"), &[b'd'; 8]);
    s.esperar("watches refeitos", |s| {
        s.tamanho("sub/depois.txt") == Some(8)
    });
    assert_eq!(s.n("SELECT COUNT(*) FROM files WHERE state = 'ausente'"), 0);
}

#[test]
fn arquivo_em_escrita_e_relido_depois_e_o_hash_vem_por_ultimo() {
    let chamadas: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let c2 = Arc::clone(&chamadas);
    let alvo: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let a2 = Arc::clone(&alvo);
    let s = iniciar_com(
        arvore,
        ConfigMonitor {
            estabilidade: Duration::from_millis(500),
            ..cfg()
        },
        move |m| {
            m.com_pos_reconciliacao(move |_cat, ids| {
                let tam = a2
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|p| fs::metadata(p).ok())
                    .map_or(0, |m| m.len());
                c2.lock().unwrap().push(tam);
                assert!(!ids.is_empty());
                Ok(())
            })
        },
    );
    let arq = s.raiz.join("grande.log");
    *alvo.lock().unwrap() = Some(arq.clone());
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&arq)
        .unwrap();
    for _ in 0..12 {
        f.write_all(&[b'L'; 1000]).unwrap();
        f.flush().unwrap();
        thread::sleep(Duration::from_millis(100));
    }
    drop(f);
    s.esperar("tamanho final catalogado", |s| {
        s.tamanho("grande.log") == Some(12_000)
    });
    s.esperar("etapa de hash chamada", |s| {
        !chamadas.lock().unwrap().is_empty()
            && s.n("SELECT COUNT(*) FROM scan_runs WHERE mode = 'monitor'") == 1
    });
    // O pós-processamento (hash etc.) só roda com o arquivo parado: sempre com o tamanho final.
    assert!(
        chamadas.lock().unwrap().iter().all(|t| *t == 12_000),
        "{:?}",
        chamadas.lock().unwrap()
    );
}

#[test]
fn raiz_invalida_e_recusada() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("c.sqlite");
    let mut cat = Catalogo::abrir(&db).unwrap();
    let vol = volume_de(tmp.path());
    let mut m = Monitor::novo(vec![tmp.path().join("nao_existe")], move || {
        vec![vol.clone()]
    });
    let e = m.executar(&mut cat, &ControleScan::novo()).unwrap_err();
    assert!(matches!(e, ErroMonitor::SemRaizes), "{e:?}");
}
