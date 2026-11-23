//! Ensaio de escala (CA-A07): gera ~1 milhão de entradas e mede tempo, memória e latência de
//! consulta. Rode com:
//! `cargo test -p dag-scanner --release --test escala -- --ignored --nocapture`
//! (`DAG_BENCH_PASTAS` e `DAG_BENCH_ARQUIVOS_POR_PASTA` mudam o tamanho; padrão 1000 × 999).
#![allow(clippy::unwrap_used)]

mod comum;

use std::fs::{self, File};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use comum::*;
use dag_scanner::ConfigScan;
use dag_store::Catalogo;
use dag_store::inventario::consulta::filhos_por_tamanho;

fn proc_kb(campo: &str) -> u64 {
    fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with(campo))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn env_usize(nome: &str, padrao: usize) -> usize {
    std::env::var(nome)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(padrao)
}

#[test]
#[ignore = "gera ~1 milhão de entradas; rode com --ignored --nocapture"]
fn escala_de_um_milhao_de_entradas() {
    let pastas = env_usize("DAG_BENCH_PASTAS", 1000);
    let por_pasta = env_usize("DAG_BENCH_ARQUIVOS_POR_PASTA", 999);
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    let t0 = Instant::now();
    for d in 0..pastas {
        let dir = raiz.join(format!("d{d:04}"));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..por_pasta {
            File::create(dir.join(format!("f{f:04}.dat"))).unwrap();
        }
    }
    let total = 1 + pastas + pastas * por_pasta;
    println!(
        "fixture: {total} entradas criadas em {:.1}s",
        t0.elapsed().as_secs_f64()
    );

    let catalogo_path = tmp.path().join("catalogo.sqlite");
    let mut cat = Catalogo::abrir(&catalogo_path).unwrap();
    let rss_antes = proc_kb("VmRSS:");
    let hwm_antes = proc_kb("VmHWM:");
    let pico_rss = Arc::new(AtomicU64::new(0));
    let pico = Arc::clone(&pico_rss);
    let amostras = Arc::new(std::sync::Mutex::new(Vec::<(u64, u64)>::new()));
    let am = Arc::clone(&amostras);
    let t1 = Instant::now();
    let r = scan(&raiz)
        .com_config(ConfigScan {
            concorrencia: 4,
            ..ConfigScan::default()
        })
        .com_progresso(move |p| {
            let rss = proc_kb("VmRSS:");
            pico.fetch_max(rss, Ordering::Relaxed);
            let mut a = am.lock().unwrap();
            if a.len() < 4 || p.pastas + p.arquivos >= a.last().map_or(0, |x| x.0) + 200_000 {
                a.push((p.pastas + p.arquivos, rss));
            }
        })
        .executar(&mut cat)
        .unwrap();
    let dur = t1.elapsed();
    let hwm = proc_kb("VmHWM:");
    println!(
        "scan: {:?} desfecho, {:.1}s total",
        r.desfecho,
        dur.as_secs_f64()
    );
    println!(
        "  entradas={} pastas={} arquivos={} => {:.0} entradas/s",
        r.contadores.pastas + r.contadores.arquivos,
        r.contadores.pastas,
        r.contadores.arquivos,
        (r.contadores.pastas + r.contadores.arquivos) as f64 / dur.as_secs_f64()
    );
    println!(
        "  RSS antes={} MiB, VmHWM antes={} MiB, VmHWM depois={} MiB, pico RSS (amostrado)={} MiB",
        rss_antes / 1024,
        hwm_antes / 1024,
        hwm / 1024,
        pico_rss.load(Ordering::Relaxed) / 1024
    );
    println!(
        "  RSS por volume de entradas (amostras): {:?}",
        amostras.lock().unwrap()
    );
    let db = fs::metadata(&catalogo_path).unwrap().len();
    println!(
        "  catálogo: {} MiB (+WAL {} MiB)",
        db / 1_048_576,
        fs::metadata(catalogo_path.with_extension("sqlite-wal")).map_or(0, |m| m.len()) / 1_048_576
    );
    assert_eq!(r.contadores.pastas as usize, 1 + pastas);
    assert_eq!(r.contadores.arquivos as usize, pastas * por_pasta);

    let con = cat.armazem().conexao();
    let raiz_id = n(
        &cat,
        "SELECT id FROM folders WHERE parent_folder_id IS NULL",
    );
    let primeira = n(&cat, "SELECT id FROM folders WHERE name_display = 'd0000'");
    let medir = |rotulo: &str, f: &mut dyn FnMut()| {
        let t = Instant::now();
        for _ in 0..20 {
            f();
        }
        println!(
            "  consulta {rotulo}: {:.2} ms/chamada",
            t.elapsed().as_secs_f64() * 1000.0 / 20.0
        );
    };
    medir(
        "filhos da raiz (1ª página de 100, por tamanho)",
        &mut || {
            filhos_por_tamanho(con, raiz_id, 100, None).unwrap();
        },
    );
    medir("filhos de d0000 (1ª página de 100)", &mut || {
        filhos_por_tamanho(con, primeira, 100, None).unwrap();
    });
    let p1 = filhos_por_tamanho(con, primeira, 500, None).unwrap();
    let cursor = p1.proximo;
    medir("filhos de d0000 (2ª página, keyset)", &mut || {
        filhos_por_tamanho(con, primeira, 500, cursor).unwrap();
    });
    medir("folder_stats da raiz", &mut || {
        dag_store::inventario::consulta::estatisticas_da_pasta(con, raiz_id).unwrap();
    });
    let t = Instant::now();
    let c = n(&cat, "SELECT COUNT(*) FROM files WHERE logical_bytes = 0");
    println!(
        "  COUNT(*) com filtro em {} arquivos: {:.0} ms",
        c,
        t.elapsed().as_secs_f64() * 1000.0
    );
}

/// Vazão do hash BLAKE3 em ~1 GiB (`DAG_BENCH_HASH_MIB` muda o tamanho). Rode com
/// `cargo test -p dag-scanner --release --test escala -- --ignored --nocapture vazao_do_hash`.
/// Em arquivos recém-gravados o cache de página domina: o número mede CPU + leitura do cache.
#[test]
#[ignore = "grava ~1 GiB; rode com --ignored --nocapture"]
fn vazao_do_hash() {
    use dag_core::ExecucaoId;
    use dag_scanner::{ConfigHash, EtapaHash};
    use std::io::Write as _;

    let mib = env_usize("DAG_BENCH_HASH_MIB", 1024);
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    fs::create_dir_all(&raiz).unwrap();
    let pedaco: Vec<u8> = (0..(1usize << 20))
        .map(|i| (i * 31 + i / 7) as u8)
        .collect();
    let arquivos = 16usize;
    let por_arquivo = mib / arquivos;
    for a in 0..arquivos {
        let mut f = File::create(raiz.join(format!("g{a:02}.bin"))).unwrap();
        for k in 0..por_arquivo {
            let mut p = pedaco.clone();
            p[0] = a as u8;
            p[1] = k as u8;
            f.write_all(&p).unwrap();
        }
    }
    let mut cat = Catalogo::abrir(tmp.path().join("catalogo.sqlite")).unwrap();
    let run = scan(&raiz).executar(&mut cat).unwrap().execucao_id;
    for leitores in [1usize, 2, 4] {
        let t = Instant::now();
        let r = EtapaHash::nova(ExecucaoId(run))
            .com_config(ConfigHash {
                concorrencia: leitores,
                ..ConfigHash::default()
            })
            .executar(&mut cat)
            .unwrap();
        let s = t.elapsed().as_secs_f64();
        println!(
            "hash: {leitores} leitor(es): {} MiB em {s:.2}s = {:.0} MB/s (hasheados {}, reaproveitados {})",
            r.bytes_lidos >> 20,
            r.bytes_lidos as f64 / 1e6 / s,
            r.hasheados,
            r.reaproveitados
        );
        // Zera os hashes para a próxima rodada medir leitura de novo.
        cat.armazem()
            .conexao()
            .execute("DELETE FROM hashes", [])
            .unwrap();
    }
}
