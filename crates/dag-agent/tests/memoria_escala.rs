//! Medição de memória (VmHWM) por fase em escala: Full Scan + hash + grupos, passadas de Fast
//! Scan (rotina de abertura) e reconstrução de grupos. `#[ignore]`: rode em release.
//!
//! ```text
//! DAG_MEM_DIR=/algum/dir DAG_MEM_ARQUIVOS=1000000 \
//!   cargo test -p dag-agent --release --test memoria_escala -- --ignored --nocapture
//! ```
#![allow(clippy::unwrap_used)]

use std::fs::{self, File};
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use dag_agent::{Agente, PoliticaAbertura};
use dag_core::{Comando, DispositivoDescoberto, ModoVarredura, TipoDispositivo, VolumeDescoberto};

fn kb(campo: &str) -> u64 {
    fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with(campo))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Zera o pico (`VmHWM`) para medir a fase seguinte isoladamente.
fn zerar_pico() {
    let _ = fs::write("/proc/self/clear_refs", "5");
}

fn fase(nome: &str, t: std::time::Instant) {
    println!(
        "MEM {nome}: VmHWM {:.1} MiB, VmRSS {:.1} MiB, {:.1}s",
        kb("VmHWM:") as f64 / 1024.0,
        kb("VmRSS:") as f64 / 1024.0,
        t.elapsed().as_secs_f64()
    );
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

#[test]
#[ignore = "medição de escala"]
fn memoria_por_fase() {
    let n: usize = std::env::var("DAG_MEM_ARQUIVOS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);
    let base = PathBuf::from(std::env::var("DAG_MEM_DIR").unwrap());
    let raiz = fs::canonicalize({
        fs::create_dir_all(&base).unwrap();
        &base
    })
    .unwrap()
    .join("acervo");
    if !raiz.exists() {
        let t = std::time::Instant::now();
        fs::create_dir_all(&raiz).unwrap();
        let por_pasta: usize = std::env::var("DAG_MEM_POR_PASTA")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);
        for i in 0..n {
            if i % por_pasta == 0 {
                fs::create_dir_all(raiz.join(format!("d{:07}", i / por_pasta))).unwrap();
            }
            let p = raiz
                .join(format!("d{:07}", i / por_pasta))
                .join(format!("f{i:08}.dat"));
            let mut f = File::create(p).unwrap();
            // 1 em 10 repete o conteúdo (duplicados); o resto é único.
            let sem = if i % 10 == 7 { i / 10 } else { i };
            f.write_all(format!("conteudo-{sem:012}").as_bytes())
                .unwrap();
        }
        println!("fixture {n} arquivos em {:.1}s", t.elapsed().as_secs_f64());
    }
    let cat = base.join("dados/catalogo.sqlite");
    fs::create_dir_all(cat.parent().unwrap()).unwrap();
    let novo = !cat.exists();
    let mut a = Agente::abrir(&cat)
        .unwrap()
        .com_volumes_fixos(vec![volume(&raiz)])
        .com_progresso(|p| {
            use dag_core::Progresso;
            // Marca o RSS na primeira ocorrência de cada tipo de quadro e nas mensagens.
            static ULT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
            let nome = match p {
                Progresso::Inventario(_) => "inventario".to_string(),
                Progresso::Reconciliacao(_) => "reconciliacao".to_string(),
                Progresso::Hash(h) if h.preparando => "hash-preparando".to_string(),
                Progresso::Hash(_) => "hash".to_string(),
                Progresso::Verificacao(_) => "verificacao".to_string(),
                Progresso::Conteineres(_) => "conteineres".to_string(),
                Progresso::Mensagem { texto } => texto.chars().take(60).collect(),
                Progresso::Passo(_) => "passo".to_string(),
            };
            let mut u = ULT.lock().unwrap();
            if *u != nome {
                println!(
                    "  RSS {:.0} MiB (HWM {:.0}) em: {nome}",
                    kb("VmRSS:") as f64 / 1024.0,
                    kb("VmHWM:") as f64 / 1024.0
                );
                *u = nome;
            }
        });
    fase("base (agente aberto)", std::time::Instant::now());
    if novo {
        zerar_pico();
        let t = std::time::Instant::now();
        let r = a.executar(Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec![raiz.to_str().unwrap().into()],
            sem_hash: false,
            elevar: false,
            releitura_total: false,
            threads_por_disco: None,
            max_discos: None,
        });
        println!("{:?}", std::mem::discriminant(&r));
        fase("full scan + hash + grupos", t);
    }
    zerar_pico();
    let t = std::time::Instant::now();
    let pol = PoliticaAbertura {
        max_passadas: 3,
        ..PoliticaAbertura::default()
    };
    let rel = a.recuperar_ao_abrir(&pol).unwrap();
    println!("{rel}");
    fase("abertura (3 passadas fast + grupos se preciso)", t);
    zerar_pico();
    let t = std::time::Instant::now();
    let r = a.executar(Comando::ReconstruirDuplicados);
    println!("{:?}", std::mem::discriminant(&r));
    fase("reconstruir duplicados", t);
}
