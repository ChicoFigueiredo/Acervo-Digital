//! Hash Completo paralelo por Dispositivo Físico: um grupo de leitores por disco, volumes do
//! mesmo disco serializados, pausa/cancelamento em todos os discos e retomada de fila antiga.
//! Os testes de concorrência são determinísticos: usam o gancho de leitura como ponto de
//! encontro (rendezvous) ou contador de leituras ativas, nunca medem tempo.
#![allow(clippy::unwrap_used)]

mod comum;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use comum::*;
use dag_core::ExecucaoId;
use dag_scanner::{ConfigHash, ControleScan, Desfecho, EtapaHash};
use dag_store::Catalogo;
use dag_store::hashes::{self, TipoFila};

const ARQS_POR_VOLUME: usize = 4;

fn conteudo(semente: u64, tamanho: usize) -> Vec<u8> {
    (0..tamanho)
        .map(|i| ((i as u64).wrapping_mul(31).wrapping_add(semente * 7) % 251) as u8)
        .collect()
}

/// Cria `volumes` = [(nome do volume, nome do disco ou None)] sob `raiz/<volume>/`, varre tudo
/// como um único volume e depois redistribui os arquivos por volumes e dispositivos do Catálogo.
fn montar(raiz: &Path, volumes: &[(&str, Option<&str>)]) -> (Catalogo, i64) {
    for (k, (v, _)) in volumes.iter().enumerate() {
        for i in 0..ARQS_POR_VOLUME {
            let p = raiz.join(v).join(format!("f{i}.bin"));
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, conteudo((k * 100 + i) as u64 + 1, 30_000 + i * 1000)).unwrap();
        }
    }
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = scan(raiz).executar(&mut cat).unwrap().execucao_id;
    let con = cat.armazem().conexao();
    let mut discos: BTreeMap<&str, i64> = BTreeMap::new();
    for (v, disco) in volumes {
        let dev = disco.map(|d| {
            *discos.entry(d).or_insert_with(|| {
                con.execute(
                    "INSERT INTO dispositivos (identificador, rotulo, criado_em) VALUES (?1, ?1, 1)",
                    [d],
                )
                .unwrap();
                con.last_insert_rowid()
            })
        });
        con.execute(
            "INSERT INTO volumes (platform_id, device_id, rotulo) VALUES (?1, ?2, ?1)",
            rusqlite::params![v, dev],
        )
        .unwrap();
        let vid = con.last_insert_rowid();
        let padrao = format!("%/{v}/%");
        con.execute(
            "UPDATE files SET volume_id = ?1 WHERE CAST(path_raw AS TEXT) LIKE ?2",
            rusqlite::params![vid, padrao],
        )
        .unwrap();
    }
    (cat, run)
}

/// Disco de um caminho: o mapa recebe o nome do volume (`.../<volume>/fN.bin`).
fn volume_do_caminho(p: &Path) -> String {
    p.parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn mapa(volumes: &[(&str, Option<&str>)]) -> BTreeMap<String, String> {
    volumes
        .iter()
        .map(|(v, d)| {
            (
                (*v).to_owned(),
                d.map_or_else(|| (*v).to_owned(), str::to_owned),
            )
        })
        .collect()
}

fn cfg() -> ConfigHash {
    ConfigHash {
        tamanho_buffer: 4096,
        intervalo_de_progresso: Duration::ZERO,
        ..ConfigHash::default()
    }
}

fn total_hashes(cat: &Catalogo) -> i64 {
    n(cat, "SELECT COUNT(*) FROM hashes")
}

/// Ponto de encontro: cada leitor que chega espera até `partes` leitores estarem juntos.
/// Se nunca chegarem (agendamento serial), vence o limite de tempo e `furou` fica verdadeiro.
struct Encontro {
    partes: usize,
    estado: Mutex<usize>,
    cv: Condvar,
    furou: AtomicBool,
}

impl Encontro {
    fn novo(partes: usize) -> Arc<Self> {
        Arc::new(Self {
            partes,
            estado: Mutex::new(0),
            cv: Condvar::new(),
            furou: AtomicBool::new(false),
        })
    }

    fn chegar(&self) {
        let mut g = self.estado.lock().unwrap();
        if *g >= self.partes {
            return; // encontro já aconteceu
        }
        *g += 1;
        if *g >= self.partes {
            self.cv.notify_all();
            return;
        }
        let (g, r) = self
            .cv
            .wait_timeout_while(g, Duration::from_secs(20), |n| *n < self.partes)
            .unwrap();
        drop(g);
        if r.timed_out() {
            self.furou.store(true, Ordering::SeqCst);
        }
    }
}

#[test]
fn discos_diferentes_leem_ao_mesmo_tempo() {
    let tmp = tempfile::tempdir().unwrap();
    let vols = [
        ("va", Some("discoA")),
        ("vb", Some("discoB")),
        ("vc", Some("discoC")),
    ];
    let (mut cat, run) = montar(&tmp.path().join("acervo"), &vols);
    let enc = Encontro::novo(3);
    let e2 = Arc::clone(&enc);
    let r = EtapaHash::nova(ExecucaoId(run))
        .com_config(cfg())
        .com_gancho_apos_leitura(move |_| e2.chegar())
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r.desfecho, Desfecho::Concluida);
    assert!(
        !enc.furou.load(Ordering::SeqCst),
        "os 3 discos deveriam ter lido ao mesmo tempo"
    );
    assert_eq!(total_hashes(&cat), (3 * ARQS_POR_VOLUME) as i64);
}

#[test]
fn volumes_do_mesmo_disco_sao_serializados() {
    let tmp = tempfile::tempdir().unwrap();
    // v1 e v2 no mesmo disco; v3 em outro.
    let vols = [
        ("v1", Some("disco1")),
        ("v2", Some("disco1")),
        ("v3", Some("disco2")),
    ];
    let disco_de = mapa(&vols);
    let (mut cat, run) = montar(&tmp.path().join("acervo"), &vols);
    let ativos: Arc<Mutex<BTreeMap<String, usize>>> = Arc::default();
    let maximo: Arc<Mutex<BTreeMap<String, usize>>> = Arc::default();
    let (a2, m2) = (Arc::clone(&ativos), Arc::clone(&maximo));
    let r = EtapaHash::nova(ExecucaoId(run))
        .com_config(cfg())
        .com_gancho_apos_leitura(move |p| {
            let disco = disco_de[&volume_do_caminho(p)].clone();
            {
                let mut a = a2.lock().unwrap();
                let c = a.entry(disco.clone()).or_default();
                *c += 1;
                let mut m = m2.lock().unwrap();
                let mx = m.entry(disco.clone()).or_default();
                *mx = (*mx).max(*c);
            }
            // Segura a leitura "aberta" para dar chance a uma leitura concorrente (se existisse).
            std::thread::sleep(Duration::from_millis(15));
            *a2.lock().unwrap().get_mut(&disco).unwrap() -= 1;
        })
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r.desfecho, Desfecho::Concluida);
    let m = maximo.lock().unwrap();
    assert_eq!(m["disco1"], 1, "volumes do mesmo disco nunca leem juntos");
    assert_eq!(m["disco2"], 1);
    assert_eq!(total_hashes(&cat), (3 * ARQS_POR_VOLUME) as i64);
}

#[test]
fn threads_por_disco_permite_mais_de_uma_leitura_no_mesmo_disco() {
    let tmp = tempfile::tempdir().unwrap();
    let vols = [("v1", Some("disco1")), ("v2", Some("disco1"))];
    let (mut cat, run) = montar(&tmp.path().join("acervo"), &vols);
    let enc = Encontro::novo(2);
    let e2 = Arc::clone(&enc);
    EtapaHash::nova(ExecucaoId(run))
        .com_config(ConfigHash {
            threads_por_disco: 2,
            ..cfg()
        })
        .com_gancho_apos_leitura(move |_| e2.chegar())
        .executar(&mut cat)
        .unwrap();
    assert!(!enc.furou.load(Ordering::SeqCst));
}

#[test]
fn volumes_sem_dispositivo_formam_um_grupo_cada() {
    let tmp = tempfile::tempdir().unwrap();
    let vols = [("v1", None), ("v2", None)];
    let (mut cat, run) = montar(&tmp.path().join("acervo"), &vols);
    let enc = Encontro::novo(2);
    let e2 = Arc::clone(&enc);
    EtapaHash::nova(ExecucaoId(run))
        .com_config(cfg())
        .com_gancho_apos_leitura(move |_| e2.chegar())
        .executar(&mut cat)
        .unwrap();
    assert!(
        !enc.furou.load(Ordering::SeqCst),
        "sem disco conhecido, 1 grupo por volume"
    );
}

#[test]
fn max_discos_limita_discos_simultaneos() {
    let tmp = tempfile::tempdir().unwrap();
    let vols = [("va", Some("A")), ("vb", Some("B")), ("vc", Some("C"))];
    let (mut cat, run) = montar(&tmp.path().join("acervo"), &vols);
    let ativos = Arc::new(AtomicUsize::new(0));
    let maximo = Arc::new(AtomicUsize::new(0));
    let (a2, m2) = (Arc::clone(&ativos), Arc::clone(&maximo));
    let r = EtapaHash::nova(ExecucaoId(run))
        .com_config(ConfigHash {
            max_discos: 2,
            ..cfg()
        })
        .com_gancho_apos_leitura(move |_| {
            let c = a2.fetch_add(1, Ordering::SeqCst) + 1;
            m2.fetch_max(c, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(10));
            a2.fetch_sub(1, Ordering::SeqCst);
        })
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r.desfecho, Desfecho::Concluida);
    assert!(maximo.load(Ordering::SeqCst) <= 2);
    assert_eq!(
        total_hashes(&cat),
        (3 * ARQS_POR_VOLUME) as i64,
        "o 3º disco também termina"
    );
}

#[test]
fn progresso_agrega_e_informa_cada_disco() {
    let tmp = tempfile::tempdir().unwrap();
    let vols = [("va", Some("A")), ("vb", Some("B")), ("vc", Some("B"))];
    let (mut cat, run) = montar(&tmp.path().join("acervo"), &vols);
    let ultimo: Arc<Mutex<Option<dag_scanner::ProgressoHash>>> = Arc::default();
    let u2 = Arc::clone(&ultimo);
    EtapaHash::nova(ExecucaoId(run))
        .com_config(cfg())
        .com_progresso(move |p| *u2.lock().unwrap() = Some(p.clone()))
        .executar(&mut cat)
        .unwrap();
    let p = ultimo.lock().unwrap().clone().unwrap();
    assert_eq!(p.arquivos_feitos, p.arquivos_total);
    assert_eq!(p.arquivos_total, (3 * ARQS_POR_VOLUME) as u64);
    assert_eq!(p.discos.len(), 2, "A e B (vb e vc agrupados)");
    let b = p.discos.iter().find(|d| d.disco == "B").unwrap();
    assert_eq!(b.volumes.len(), 2);
    assert_eq!(b.arquivos_total, (2 * ARQS_POR_VOLUME) as u64);
    assert!(p.discos.iter().all(|d| d.concluido));
    assert_eq!(
        p.discos.iter().map(|d| d.bytes_lidos).sum::<u64>(),
        p.bytes_lidos
    );
    assert_eq!(
        p.discos.iter().map(|d| d.arquivos_feitos).sum::<u64>(),
        p.arquivos_total
    );
}

#[test]
fn cancelar_com_varios_discos_para_todos_e_retoma_sem_reler() {
    let tmp = tempfile::tempdir().unwrap();
    let vols = [
        ("va", Some("A")),
        ("vb", Some("B")),
        ("vc", Some("C")),
        ("vd", Some("C")),
    ];
    let (mut cat, run) = montar(&tmp.path().join("acervo"), &vols);
    let total = (4 * ARQS_POR_VOLUME) as u64;
    let ctl = ControleScan::novo();
    let c2 = ctl.clone();
    let r1 = EtapaHash::nova(ExecucaoId(run))
        .com_config(cfg())
        .com_controle(ctl)
        .com_progresso(move |p| {
            if p.arquivos_feitos >= 3 {
                c2.cancelar();
            }
        })
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r1.desfecho, Desfecho::Cancelada);
    let feitos = total_hashes(&cat) as u64;
    assert!((3..total).contains(&feitos), "feitos = {feitos}");
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM fila_hash WHERE state = 'em_andamento'"
        ),
        0
    );
    let r2 = EtapaHash::nova(ExecucaoId(run))
        .com_config(cfg())
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r2.desfecho, Desfecho::Concluida);
    assert_eq!(r2.reaproveitados, feitos);
    assert_eq!(r2.hasheados, total - feitos);
    assert_eq!(total_hashes(&cat) as u64, total);
}

#[test]
fn pausar_para_todos_os_discos_e_continuar_termina() {
    let tmp = tempfile::tempdir().unwrap();
    let vols = [("va", Some("A")), ("vb", Some("B")), ("vc", Some("C"))];
    let (mut cat, run) = montar(&tmp.path().join("acervo"), &vols);
    let total = (3 * ARQS_POR_VOLUME) as u64;
    let ctl = ControleScan::novo();
    let c2 = ctl.clone();
    // Fases: 0 = rodando, 1 = pausado (contando callbacks parados), 2 = retomado.
    let fase = Arc::new(AtomicUsize::new(0));
    let parados = Arc::new(AtomicUsize::new(0));
    let bytes_na_pausa = Arc::new(Mutex::new(Vec::<u64>::new()));
    let (f2, p2, b2) = (
        Arc::clone(&fase),
        Arc::clone(&parados),
        Arc::clone(&bytes_na_pausa),
    );
    let r = EtapaHash::nova(ExecucaoId(run))
        .com_config(cfg())
        .com_controle(ctl)
        .com_progresso(move |p| match f2.load(Ordering::SeqCst) {
            0 if p.arquivos_feitos >= 2 => {
                c2.pausar();
                f2.store(1, Ordering::SeqCst);
            }
            1 => {
                // Já pausado: só chega aqui com tudo em voo drenado; os bytes não podem crescer.
                b2.lock().unwrap().push(p.bytes_lidos);
                if p2.fetch_add(1, Ordering::SeqCst) >= 4 {
                    c2.continuar();
                    f2.store(2, Ordering::SeqCst);
                }
            }
            _ => {}
        })
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r.desfecho, Desfecho::Concluida);
    assert_eq!(r.hasheados + r.reaproveitados, total);
    assert_eq!(total_hashes(&cat) as u64, total);
    assert_eq!(
        fase.load(Ordering::SeqCst),
        2,
        "a pausa aconteceu e foi liberada"
    );
    let v = bytes_na_pausa.lock().unwrap();
    // Depois de drenar o que estava em voo, a leitura parada não avança.
    let estaveis = v.windows(2).filter(|w| w[0] == w[1]).count();
    assert!(estaveis >= 1, "bytes parados durante a pausa: {v:?}");
}

#[test]
fn fila_criada_antes_da_mudanca_retoma_no_agendador_paralelo() {
    // Estado que o binário antigo deixa: run `concluida` na fase `hash`, parte dos hashes
    // gravados, fila com itens `concluido`, `pendente` e `em_andamento` (reserva de um processo
    // morto), volumes sem dispositivo registrado.
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    let vols = [("v1", None), ("v2", None)];
    let (mut cat, run) = montar(&raiz, &vols);
    EtapaHash::nova(ExecucaoId(run))
        .com_config(cfg())
        .executar(&mut cat)
        .unwrap();
    let total = total_hashes(&cat);
    {
        let con = cat.armazem().conexao();
        // "Perde" metade dos hashes e deixa a fila como um processo morto no meio a deixaria.
        con.execute(
            "DELETE FROM hashes WHERE file_id IN (SELECT id FROM files ORDER BY id LIMIT 4)",
            [],
        )
        .unwrap();
        con.execute(
            "UPDATE fila_hash SET state = 'em_andamento' WHERE file_id IN (SELECT id FROM files ORDER BY id LIMIT 2)",
            [],
        )
        .unwrap();
        con.execute(
            "UPDATE fila_hash SET state = 'pendente' WHERE file_id IN (SELECT id FROM files ORDER BY id LIMIT 4 OFFSET 2)",
            [],
        )
        .unwrap();
        con.execute("UPDATE scan_runs SET phase = 'hash' WHERE id = ?1", [run])
            .unwrap();
    }
    // O `retomar` da execução na fase `hash` agora é aceito (antes dava NaoRetomavel).
    let r = scan(&raiz).retomar(&mut cat, run).unwrap();
    assert_eq!(r.execucao_id, run);
    let h = EtapaHash::nova(ExecucaoId(run))
        .com_config(cfg())
        .executar(&mut cat)
        .unwrap();
    assert_eq!(h.desfecho, Desfecho::Concluida);
    assert_eq!(
        h.reaproveitados as i64,
        total - 4,
        "o que já tinha hash não é relido"
    );
    assert_eq!(h.hasheados, 4);
    assert_eq!(total_hashes(&cat), total);
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM fila_hash WHERE state NOT IN ('concluido')"
        ),
        0
    );
    let fase: String = cat
        .armazem()
        .conexao()
        .query_row("SELECT phase FROM scan_runs WHERE id = ?1", [run], |l| {
            l.get(0)
        })
        .unwrap();
    assert_eq!(fase, "concluida");
}

#[test]
fn andamento_da_fila_agrupa_por_dispositivo() {
    let tmp = tempfile::tempdir().unwrap();
    let vols = [
        ("va", Some("A")),
        ("vb", Some("B")),
        ("vc", Some("B")),
        ("vd", None),
    ];
    let (mut cat, run) = montar(&tmp.path().join("acervo"), &vols);
    // Só prepara a fila (nada hasheado ainda).
    cat.armazem_mut()
        .transacao_imediata(|tx| -> Result<(), dag_scanner::ErroScan> {
            hashes::preparar_fila_hash(tx, run, 1)?;
            Ok(())
        })
        .unwrap();
    let a = hashes::andamento_fila(cat.armazem().conexao(), TipoFila::Hash, 0).unwrap();
    assert_eq!(
        a.discos.len(),
        3,
        "A, B (2 volumes) e o volume sem dispositivo"
    );
    let b = a.discos.iter().find(|d| d.nome == "B").unwrap();
    assert_eq!(b.volume_ids.len(), 2);
    assert_eq!(b.arquivos_pendentes, (2 * ARQS_POR_VOLUME) as u64);
    assert!(
        a.discos
            .iter()
            .all(|d| d.arquivos_feitos == 0 && d.bytes_pendentes > 0)
    );
}
