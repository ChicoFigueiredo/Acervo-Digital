//! A memória do agente não cresce com o número de arquivos (bug de campo: 28,7 GB de RSS em
//! 19,9 milhões de arquivos durante a rotina de abertura). Cada tamanho roda num processo
//! filho (o pico `VmHWM` é por processo) com Full Scan + hash, uma passada de Fast Scan
//! (reaproveitando todos os hashes, a fila de hash com tudo reaproveitado) e a reconstrução dos
//! Grupos de Duplicados. Crescimento por arquivo acima de ~0,4 KiB reprova: o vazamento de
//! campo era de ~1,4 KiB por arquivo.
//!
//! Escala real (milhões de arquivos, fases separadas): `memoria_escala.rs` e `memoria_sql.rs`
//! (`#[ignore]`).
#![allow(clippy::unwrap_used)]

use std::fs::{self, File};
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

use dag_agent::{Agente, PoliticaAbertura};
use dag_core::{
    Comando, DispositivoDescoberto, ModoVarredura, Resposta, TipoDispositivo, VolumeDescoberto,
};

const PEQUENO: usize = 6_000;
const GRANDE: usize = 36_000;
/// Crescimento máximo do pico entre `PEQUENO` e `GRANDE` (30.000 arquivos a mais).
const MAX_CRESCIMENTO_KIB: u64 = 12 * 1024;

fn pico_kib() -> u64 {
    fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap()
}

fn volume(raiz: &Path) -> VolumeDescoberto {
    let dev = fs::metadata(raiz).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("mem-{dev}"),
        st_dev: dev,
        rotulo: Some("MEM".into()),
        fs_type: "teste".into(),
        capacidade_bytes: Some(1_000_000),
        livre_bytes: Some(400_000),
        pontos_de_montagem: vec![raiz.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: Some(DispositivoDescoberto {
            identificador: "disco-mem".into(),
            modelo: None,
            tipo: TipoDispositivo::Ssd,
            removivel: false,
        }),
    }
}

/// Filho: só trabalha se `DAG_MEM_FILHO` define o número de arquivos; imprime `HWM_KIB=`.
#[test]
fn memoria_filho() {
    let Some(n) = std::env::var("DAG_MEM_FILHO")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("acervo");
    // Poucos arquivos por pasta (muitas pastas) e 10% de conteúdo repetido (grupos).
    for i in 0..n {
        if i % 8 == 0 {
            fs::create_dir_all(raiz.join(format!("d{:06}", i / 8))).unwrap();
        }
        let sem = if i % 10 == 7 { i / 10 } else { i };
        let mut f = File::create(raiz.join(format!("d{:06}/f{i:07}.dat", i / 8))).unwrap();
        f.write_all(format!("conteudo-{sem:012}").as_bytes())
            .unwrap();
    }
    let mut a = Agente::abrir(tmp.path().join("catalogo.sqlite"))
        .unwrap()
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
    // Rotina de abertura: passada de Fast Scan com tudo reaproveitado + grupos.
    let pol = PoliticaAbertura {
        max_passadas: 2,
        ..PoliticaAbertura::default()
    };
    let rel = a.recuperar_ao_abrir(&pol).unwrap();
    assert!(!rel.passadas.is_empty(), "{rel}");
    let r = a.executar(Comando::ReconstruirDuplicados);
    assert!(!matches!(r, Resposta::Erro { .. }), "{r:?}");
    println!("HWM_KIB={}", pico_kib());
}

fn pico_do_filho(n: usize) -> u64 {
    let exe = std::env::current_exe().unwrap();
    let saida = Command::new(exe)
        .args([
            "--exact",
            "memoria_filho",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("DAG_MEM_FILHO", n.to_string())
        .output()
        .unwrap();
    let texto = String::from_utf8_lossy(&saida.stdout);
    assert!(
        saida.status.success(),
        "filho falhou: {texto}\n{}",
        String::from_utf8_lossy(&saida.stderr)
    );
    texto
        .split("HWM_KIB=")
        .nth(1)
        .unwrap_or_else(|| panic!("sem HWM_KIB: {texto}"))
        .lines()
        .next()
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn pico_de_memoria_nao_cresce_com_o_numero_de_arquivos() {
    let pequeno = pico_do_filho(PEQUENO);
    let grande = pico_do_filho(GRANDE);
    let delta = grande.saturating_sub(pequeno);
    println!(
        "VmHWM com {PEQUENO} arquivos: {} KiB; com {GRANDE}: {} KiB; crescimento {} KiB ({:.0} B/arquivo)",
        pequeno,
        grande,
        delta,
        delta as f64 * 1024.0 / (GRANDE - PEQUENO) as f64
    );
    assert!(
        delta <= MAX_CRESCIMENTO_KIB,
        "o pico cresceu {delta} KiB para {} arquivos a mais: memória por arquivo no caminho \
         Full Scan + Fast Scan + hash reaproveitado + grupos",
        GRANDE - PEQUENO
    );
}
