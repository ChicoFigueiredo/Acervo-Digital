//! Ensaio de escala ponta a ponta (CA-A07 + CA-A29) pelo agente: Full Scan → hash → Grupos de
//! Duplicados → Fast Scan com ~0,1% de mudanças → consultas paginadas. Tudo `#[ignore]`.
//!
//! O ensaio é dividido em fases, cada uma num processo próprio, para que o pico de memória
//! (`VmHWM`) seja o da fase e não o acumulado:
//!
//! ```text
//! export DAG_ESCALA_DIR=/algum/diretorio     # fixture + catálogo ficam aqui
//! cargo test -p dag-agent --release --test escala -- --ignored --nocapture escala_gerar
//! cargo test -p dag-agent --release --test escala -- --ignored --nocapture escala_full
//! cargo test -p dag-agent --release --test escala -- --ignored --nocapture escala_consultas
//! cargo test -p dag-agent --release --test escala -- --ignored --nocapture escala_fast
//! cargo test -p dag-agent --release --test escala -- --ignored --nocapture escala_consultas
//! ```
//!
//! Tamanho: `DAG_ESCALA_ARQUIVOS` (padrão 950000) e `DAG_ESCALA_PASTAS` (padrão 50000); o total
//! de entradas é arquivos + pastas + a raiz (≥ 1.000.001 no padrão). O conteúdo é determinístico
//! por semente: ~5% dos arquivos repetem o conteúdo de outro em outra pasta, ~0,5% são hard
//! links, ~2% são vazios; profundidade até 8; tamanhos em geral pequenos (média ~5 KB).
//! `DAG_ESCALA_RODADA=N` troca os arquivos mutados na fase `escala_fast`;
//! `DAG_ESCALA_SEM_DROP=1` não tenta esvaziar o cache de páginas antes da fase `escala_full`.
#![allow(clippy::unwrap_used)]

use std::fs::{self, File};
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use dag_agent::Agente;
use dag_core::{
    Comando, DispositivoDescoberto, ModoVarredura, NivelEspaco, Progresso, Resposta,
    TipoDispositivo, TipoRelatorio, VolumeDescoberto,
};

// ------------------------------------------------------------------ utilidades

fn env_usize(nome: &str, padrao: usize) -> usize {
    std::env::var(nome)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(padrao)
}

fn proc_kb(campo: &str) -> u64 {
    fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with(campo))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn dir_ensaio() -> PathBuf {
    PathBuf::from(std::env::var("DAG_ESCALA_DIR").expect("defina DAG_ESCALA_DIR"))
}

fn raiz() -> PathBuf {
    fs::create_dir_all(dir_ensaio()).unwrap();
    fs::canonicalize(dir_ensaio()).unwrap().join("acervo")
}

fn catalogo() -> PathBuf {
    dir_ensaio().join("dados/catalogo.sqlite")
}

fn tamanho_do_catalogo(c: &Path) -> (u64, u64) {
    let db = fs::metadata(c).map_or(0, |m| m.len());
    let wal = fs::metadata(c.with_extension("sqlite-wal")).map_or(0, |m| m.len());
    (db, wal)
}

fn mib(b: u64) -> f64 {
    b as f64 / 1_048_576.0
}

fn volume(raiz: &Path) -> VolumeDescoberto {
    let dev = fs::metadata(raiz).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("escala-{dev}"),
        st_dev: dev,
        rotulo: Some("ESCALA".into()),
        fs_type: "teste".into(),
        capacidade_bytes: Some(1_000_000),
        livre_bytes: Some(400_000),
        pontos_de_montagem: vec![raiz.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: Some(DispositivoDescoberto {
            identificador: "disco-escala".into(),
            modelo: None,
            tipo: TipoDispositivo::Ssd,
            removivel: false,
        }),
    }
}

/// Mistura de 64 bits (splitmix64): determinística e sem dependências.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

// ------------------------------------------------------------------ o modelo do acervo

const EXTENSOES: [&str; 8] = ["txt", "jpg", "pdf", "dat", "mp3", "rs", "json", "log"];

struct Modelo {
    arquivos: usize,
    /// Caminho relativo de cada pasta (índice 0 = a própria raiz, caminho vazio).
    pastas: Vec<String>,
}

/// Tamanho do conteúdo de uma semente: ~2% vazios, maioria < 4 KiB, cauda até ~1 MiB.
fn tamanho_da_semente(semente: u64) -> usize {
    let r = mix(semente ^ 0xA5A5) % 10_000;
    let u = mix(semente ^ 0x5A5A);
    match r {
        0..=199 => 0,
        200..=7_899 => 100 + (u % 3_996) as usize,
        7_900..=9_899 => 4_096 + (u % 28_672) as usize,
        9_900..=9_989 => 32_768 + (u % 229_376) as usize,
        _ => 262_144 + (u % 786_432) as usize,
    }
}

fn conteudo(semente: u64) -> Vec<u8> {
    let n = tamanho_da_semente(semente);
    let mut v = Vec::with_capacity(n + 8);
    let mut s = semente;
    while v.len() < n {
        s = mix(s);
        v.extend_from_slice(&s.to_le_bytes());
    }
    v.truncate(n);
    v
}

impl Modelo {
    fn novo(arquivos: usize, n_pastas: usize) -> Self {
        let mut pastas = vec![String::new()];
        let mut prof = vec![0usize];
        for i in 1..n_pastas {
            // Pai: costuma ser uma pasta recente (aprofunda a árvore) ou qualquer uma antes.
            let mut pai = 0;
            for t in 0..6 {
                let r = mix(i as u64 * 31 + t);
                let cand = if r.is_multiple_of(3) {
                    i - 1 - (r >> 8) as usize % i.min(40)
                } else {
                    (r >> 8) as usize % i
                };
                if prof[cand] < 8 {
                    pai = cand;
                    break;
                }
            }
            let nome = format!("d{i:05}");
            let rel = if pastas[pai].is_empty() {
                nome
            } else {
                format!("{}/{nome}", pastas[pai])
            };
            prof.push(prof[pai] + 1);
            pastas.push(rel);
        }
        Self { arquivos, pastas }
    }

    fn profundidade_maxima(&self) -> usize {
        self.pastas
            .iter()
            .map(|p| p.matches('/').count() + usize::from(!p.is_empty()))
            .max()
            .unwrap_or(0)
    }

    /// Pasta do arquivo `i`: viés para as primeiras (pastas grandes e pequenas).
    fn pasta_de(&self, i: usize) -> usize {
        let r = mix(i as u64 ^ 0xD1D1);
        let f = (r >> 11) as f64 / (1u64 << 53) as f64;
        ((f * f) * self.pastas.len() as f64) as usize % self.pastas.len()
    }

    fn eh_dup(i: usize) -> bool {
        i >= 40 && i % 20 == 7
    }

    fn eh_hardlink(i: usize) -> bool {
        i >= 40 && i % 200 == 3
    }

    /// Semente do conteúdo: duplicados repetem a de um arquivo anterior (único, de outra pasta).
    fn semente(i: usize) -> u64 {
        if Self::eh_dup(i) {
            let mut j = (mix(i as u64) as usize) % i;
            while Self::eh_dup(j) || Self::eh_hardlink(j) {
                j = (j + 1) % i;
            }
            return mix(j as u64 + 1);
        }
        mix(i as u64 + 1)
    }

    fn caminho(&self, raiz: &Path, i: usize) -> PathBuf {
        let ext = EXTENSOES[mix(i as u64 ^ 0xE0) as usize % EXTENSOES.len()];
        let p = &self.pastas[self.pasta_de(i)];
        let mut c = raiz.to_path_buf();
        if !p.is_empty() {
            c.push(p);
        }
        c.push(format!("f{i:07}.{ext}"));
        c
    }

    /// Origem do hard link `i`: um arquivo comum anterior.
    fn origem_do_link(i: usize) -> usize {
        let mut j = (mix(i as u64 ^ 0x77) as usize) % i;
        while Self::eh_dup(j) || Self::eh_hardlink(j) {
            j = (j + 1) % i;
        }
        j
    }
}

// ------------------------------------------------------------------ fase: gerar

#[test]
#[ignore = "gera ≥ 1 milhão de entradas (~5 GB); rode com --ignored --nocapture"]
fn escala_gerar() {
    let arquivos = env_usize("DAG_ESCALA_ARQUIVOS", 950_000);
    let n_pastas = env_usize("DAG_ESCALA_PASTAS", 50_000);
    let m = Arc::new(Modelo::novo(arquivos, n_pastas));
    let raiz = raiz();
    assert!(!raiz.exists(), "{} já existe", raiz.display());
    let t0 = Instant::now();
    fs::create_dir_all(&raiz).unwrap();
    for p in m.pastas.iter().filter(|p| !p.is_empty()) {
        fs::create_dir(raiz.join(p)).unwrap();
    }
    println!(
        "fixture: {} pastas (profundidade máxima {}) em {:.1}s",
        m.pastas.len(),
        m.profundidade_maxima(),
        t0.elapsed().as_secs_f64()
    );
    let threads = 4;
    let bytes = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut hs = Vec::new();
    for t in 0..threads {
        let (m, raiz, bytes) = (Arc::clone(&m), raiz.clone(), Arc::clone(&bytes));
        hs.push(std::thread::spawn(move || {
            let mut i = t;
            while i < m.arquivos {
                if !Modelo::eh_hardlink(i) {
                    let dados = conteudo(Modelo::semente(i));
                    bytes.fetch_add(dados.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    let mut f = File::create(m.caminho(&raiz, i)).unwrap();
                    f.write_all(&dados).unwrap();
                }
                i += threads;
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    let mut links = 0;
    for i in (0..m.arquivos).filter(|i| Modelo::eh_hardlink(*i)) {
        let origem = m.caminho(&raiz, Modelo::origem_do_link(i));
        fs::hard_link(origem, m.caminho(&raiz, i)).unwrap();
        links += 1;
    }
    let total = 1 + m.pastas.len() - 1 + m.arquivos;
    let b = bytes.load(std::sync::atomic::Ordering::Relaxed);
    println!(
        "fixture: {total} entradas ({} arquivos, {links} hard links) e {:.2} GiB de conteúdo em {:.1}s",
        m.arquivos,
        b as f64 / 1_073_741_824.0,
        t0.elapsed().as_secs_f64()
    );
}

// ------------------------------------------------------------------ agente com telemetria

struct Marcas {
    ini: Instant,
    /// (segundos desde o início, texto)
    etapas: Mutex<Vec<(f64, String)>>,
}

fn agente(marcas: &Arc<Marcas>) -> Agente {
    let raiz = raiz();
    let c = catalogo();
    fs::create_dir_all(c.parent().unwrap()).unwrap();
    let m = Arc::clone(marcas);
    let ultimo = Mutex::new(String::new());
    Agente::abrir(c)
        .unwrap()
        .com_volumes_fixos(vec![volume(&raiz)])
        .com_progresso(move |p| {
            let nome = match p {
                Progresso::Inventario(_) => "inventário".to_string(),
                Progresso::Reconciliacao(_) => "reconciliação".to_string(),
                Progresso::Hash(_) => "hash".to_string(),
                Progresso::Verificacao(_) => "verificação".to_string(),
                Progresso::Mensagem { texto } => texto.clone(),
            };
            let mut u = ultimo.lock().unwrap();
            if *u != nome {
                m.etapas
                    .lock()
                    .unwrap()
                    .push((m.ini.elapsed().as_secs_f64(), nome.clone()));
                *u = nome;
            }
        })
}

fn tentar_esvaziar_cache() {
    if std::env::var("DAG_ESCALA_SEM_DROP").is_ok() {
        println!("cache de páginas: mantido (DAG_ESCALA_SEM_DROP)");
        return;
    }
    let _ = std::process::Command::new("sync").status();
    match fs::write("/proc/sys/vm/drop_caches", "3") {
        Ok(()) => println!("cache de páginas: esvaziado (drop_caches=3) antes da fase"),
        Err(e) => println!(
            "cache de páginas: NÃO foi possível esvaziar ({e}); leituras podem vir do cache"
        ),
    }
}

fn relatar_scan(rotulo: &str, r: &Resposta, wall: f64, marcas: &Marcas) {
    let Resposta::ScanConcluido {
        desfecho,
        pastas,
        arquivos,
        especiais,
        bytes_logicos,
        erros,
        duracao_ms,
        hash,
        duplicados,
        reconciliacao,
        ..
    } = r
    else {
        panic!("{r:?}")
    };
    let entradas = pastas + arquivos + especiais;
    let hash_s = hash.as_ref().map_or(0.0, |h| h.duracao_ms as f64 / 1000.0);
    let inv_s = *duracao_ms as f64 / 1000.0 - hash_s;
    let pos_s = wall - *duracao_ms as f64 / 1000.0;
    println!("== {rotulo}: desfecho={desfecho} erros={erros}");
    println!(
        "  entradas={entradas} (pastas={pastas} arquivos={arquivos} especiais={especiais}) bytes lógicos={:.2} GiB",
        *bytes_logicos as f64 / 1_073_741_824.0
    );
    println!("  tempo total (parede): {wall:.1}s");
    println!(
        "  inventário/reconciliação: {inv_s:.1}s => {:.0} entradas/s",
        entradas as f64 / inv_s.max(1e-9)
    );
    if let Some(h) = hash {
        println!(
            "  hash: {hash_s:.1}s, hasheados={} reaproveitados={} bytes lidos={:.2} MiB => {:.1} MB/s ({:.0} arquivos/s)",
            h.hasheados,
            h.reaproveitados,
            h.bytes_lidos as f64 / 1_048_576.0,
            h.bytes_lidos as f64 / 1e6 / hash_s.max(1e-9),
            h.hasheados as f64 / hash_s.max(1e-9)
        );
    }
    if let Some(d) = duplicados {
        println!(
            "  duplicados: grupos={} cópias={} recuperável={:.1} MiB (criados={} removidos={}); classificação+grupos+reavaliação+resumo: {pos_s:.1}s",
            d.grupos,
            d.copias,
            d.espaco_recuperavel_bytes as f64 / 1_048_576.0,
            d.grupos_criados,
            d.grupos_removidos
        );
    }
    if let Some(rc) = reconciliacao {
        println!("  reconciliação: {rc:?}");
    }
    println!(
        "  etapas (início em s): {:?}",
        marcas.etapas.lock().unwrap()
    );
    let (db, wal) = tamanho_do_catalogo(&catalogo());
    println!(
        "  pico de memória (VmHWM): {:.0} MiB; catálogo: {:.0} MiB (+WAL {:.0} MiB)",
        proc_kb("VmHWM:") as f64 / 1024.0,
        mib(db),
        mib(wal)
    );
}

// ------------------------------------------------------------------ fase: Full Scan

#[test]
#[ignore = "fase 2 do ensaio de escala"]
fn escala_full() {
    tentar_esvaziar_cache();
    let marcas = Arc::new(Marcas {
        ini: Instant::now(),
        etapas: Mutex::new(Vec::new()),
    });
    let mut a = agente(&marcas);
    let t = Instant::now();
    let r = a.executar(Comando::IniciarScan {
        modo: ModoVarredura::FullScan,
        raizes: vec![raiz().to_str().unwrap().into()],
        sem_hash: false,
        elevar: false,
        releitura_total: false,
    });
    relatar_scan(
        "Full Scan + hash + duplicados",
        &r,
        t.elapsed().as_secs_f64(),
        &marcas,
    );
    assert!(matches!(r, Resposta::ScanConcluido { .. }));
}

// ------------------------------------------------------------------ fase: Fast Scan

/// ~0,1% de mudanças: conteúdo alterado, removidos, novos e renomeados.
#[test]
#[ignore = "fase 4 do ensaio de escala"]
fn escala_fast() {
    let arquivos = env_usize("DAG_ESCALA_ARQUIVOS", 950_000);
    let n_pastas = env_usize("DAG_ESCALA_PASTAS", 50_000);
    let m = Modelo::novo(arquivos, n_pastas);
    let raiz = raiz();
    let total = arquivos + n_pastas;
    let n = (total / 1000).max(8);
    // `DAG_ESCALA_RODADA` muda quais arquivos mudam (repetir a fase sobre o mesmo acervo).
    let rodada = env_usize("DAG_ESCALA_RODADA", 0) as u64;
    let (mut alt, mut rem, mut ren, mut novos) = (0, 0, 0, 0);
    let candidatos: Vec<usize> = (0..arquivos)
        .filter(|i| {
            !Modelo::eh_hardlink(*i)
                && !Modelo::eh_dup(*i)
                && tamanho_da_semente(Modelo::semente(*i)) > 0
        })
        .collect();
    for k in 0..n {
        let i = candidatos[(mix(k as u64 ^ 0xFA57 ^ (rodada << 32)) as usize) % candidatos.len()];
        let p = m.caminho(&raiz, i);
        if !p.exists() {
            continue;
        }
        match k % 4 {
            0 | 1 => {
                // Altera o conteúdo (mesmo tamanho + 1 byte) e o mtime.
                let mut d = fs::read(&p).unwrap();
                d.push(0x42);
                fs::write(&p, d).unwrap();
                alt += 1;
            }
            2 => {
                fs::remove_file(&p).unwrap();
                rem += 1;
            }
            _ => {
                let novo = p.with_file_name(format!("renomeado-{i:07}.bin"));
                fs::rename(&p, novo).unwrap();
                ren += 1;
            }
        }
    }
    for k in 0..n / 4 {
        let dir = if m.pastas[k % m.pastas.len()].is_empty() {
            raiz.clone()
        } else {
            raiz.join(&m.pastas[k % m.pastas.len()])
        };
        fs::write(
            dir.join(format!("novo-{k:05}.dat")),
            conteudo(0xBEEF + k as u64),
        )
        .unwrap();
        novos += 1;
    }
    println!(
        "mutações: {alt} alterados, {rem} removidos, {ren} renomeados, {novos} novos = {} (~{:.2}% de {total})",
        alt + rem + ren + novos,
        (alt + rem + ren + novos) as f64 * 100.0 / total as f64
    );
    let marcas = Arc::new(Marcas {
        ini: Instant::now(),
        etapas: Mutex::new(Vec::new()),
    });
    let mut a = agente(&marcas);
    let t = Instant::now();
    let r = a.executar(Comando::IniciarScan {
        modo: ModoVarredura::FastScan,
        raizes: vec![raiz.to_str().unwrap().into()],
        sem_hash: false,
        elevar: false,
        releitura_total: false,
    });
    relatar_scan(
        "Fast Scan (~0,1% de mudanças)",
        &r,
        t.elapsed().as_secs_f64(),
        &marcas,
    );
    let Resposta::ScanConcluido {
        hash: Some(h),
        reconciliacao: Some(rc),
        ..
    } = &r
    else {
        panic!("{r:?}")
    };
    // Só o que mudou é relido (CA-A25): bem abaixo de todo o acervo.
    assert!(h.hasheados <= (alt + novos + ren) as u64 + 8, "{h:?}");
    assert_eq!(rc.arquivos_ausentes as usize, rem);
}

// ------------------------------------------------------------------ fase: consultas

/// Mede `f` `vezes` vezes e devolve (mínimo, mediana, máximo) em ms, mais a primeira (fria).
fn medir(vezes: usize, mut f: impl FnMut()) -> (f64, f64, f64, f64) {
    let mut v = Vec::new();
    for _ in 0..vezes {
        let t = Instant::now();
        f();
        v.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let primeira = v[0];
    v.sort_by(f64::total_cmp);
    (primeira, v[0], v[v.len() / 2], v[v.len() - 1])
}

#[test]
#[ignore = "fase 3/5 do ensaio de escala"]
fn escala_consultas() {
    let marcas = Arc::new(Marcas {
        ini: Instant::now(),
        etapas: Mutex::new(Vec::new()),
    });
    let mut a = agente(&marcas).silencioso();
    let raiz = raiz();
    let m = Modelo::novo(
        env_usize("DAG_ESCALA_ARQUIVOS", 950_000),
        env_usize("DAG_ESCALA_PASTAS", 50_000),
    );
    // Pasta profunda: a de maior profundidade.
    let funda = m
        .pastas
        .iter()
        .max_by_key(|p| p.matches('/').count())
        .unwrap()
        .clone();
    let rss0 = proc_kb("VmRSS:");
    let hwm0 = proc_kb("VmHWM:");
    println!(
        "consultas: RSS inicial {:.0} MiB, VmHWM {:.0} MiB; catálogo {:.0} MiB",
        rss0 as f64 / 1024.0,
        hwm0 as f64 / 1024.0,
        mib(tamanho_do_catalogo(&catalogo()).0)
    );
    let mostrar = |rotulo: &str, vezes: usize, f: &mut dyn FnMut()| {
        let (fria, min, med, max) = medir(vezes, f);
        println!(
            "  {rotulo}: 1ª(fria)={fria:.2} ms  min={min:.2}  mediana={med:.2}  max={max:.2} ms  | RSS={:.0} MiB VmHWM={:.0} MiB",
            proc_kb("VmRSS:") as f64 / 1024.0,
            proc_kb("VmHWM:") as f64 / 1024.0
        );
    };
    let explorar = |a: &mut Agente, pasta: Option<String>, cursor: Option<String>| {
        let r = a.executar(Comando::Explorar {
            pasta,
            limite: Some(100),
            cursor,
        });
        let Resposta::Explorar {
            entradas,
            proximo_cursor,
            ..
        } = r
        else {
            panic!("{r:?}")
        };
        (entradas.len(), proximo_cursor)
    };
    mostrar("explorar: Raízes", 5, &mut || {
        explorar(&mut a, None, None);
    });
    let rs = raiz.to_str().unwrap().to_string();
    mostrar("explorar: raiz (1ª página de 100)", 5, &mut || {
        let (n, _) = explorar(&mut a, Some(rs.clone()), None);
        assert!(n > 0);
    });
    let (_, cur) = explorar(&mut a, Some(rs.clone()), None);
    mostrar("explorar: raiz (2ª página, keyset)", 5, &mut || {
        explorar(&mut a, Some(rs.clone()), cur.clone());
    });
    let fp = raiz.join(&funda).to_str().unwrap().to_string();
    mostrar(
        &format!(
            "explorar: pasta profunda (nível {})",
            funda.matches('/').count() + 1
        ),
        5,
        &mut || {
            explorar(&mut a, Some(fp.clone()), None);
        },
    );

    let dup = |a: &mut Agente, cursor: Option<String>| {
        let r = a.executar(Comando::ListarDuplicados {
            volume: None,
            tamanho_minimo: None,
            ordem: None,
            limite: Some(50),
            cursor,
        });
        let Resposta::Duplicados {
            grupos,
            total,
            proximo_cursor,
        } = r
        else {
            panic!("{r:?}")
        };
        (grupos.len(), total, proximo_cursor)
    };
    let (_, total, cur1) = dup(&mut a, None);
    println!("  grupos de duplicados no catálogo: {total}");
    mostrar("listar_duplicados: 1ª página (50)", 3, &mut || {
        dup(&mut a, None);
    });
    mostrar(
        "listar_duplicados: 2ª página (50, cursor)",
        3,
        &mut || {
            let (n, _, _) = dup(&mut a, cur1.clone());
            assert!(n > 0);
        },
    );
    mostrar(
        "listar_duplicados: filtro por volume + tamanho mínimo",
        3,
        &mut || {
            let r = a.executar(Comando::ListarDuplicados {
                volume: Some("ESCALA".into()),
                tamanho_minimo: Some(4096),
                ordem: None,
                limite: Some(50),
                cursor: None,
            });
            assert!(matches!(r, Resposta::Duplicados { .. }), "{r:?}");
        },
    );
    mostrar("relatório de espaço por volume", 3, &mut || {
        let r = a.executar(Comando::Relatorio {
            tipo: TipoRelatorio::Espaco,
            nivel: Some(NivelEspaco::Volume),
            limite: Some(50),
            cursor: None,
            saida: None,
        });
        assert!(matches!(r, Resposta::Relatorio { .. }), "{r:?}");
    });
    mostrar(
        "relatório de espaço por pasta (1ª página)",
        3,
        &mut || {
            let r = a.executar(Comando::Relatorio {
                tipo: TipoRelatorio::Espaco,
                nivel: Some(NivelEspaco::Pasta),
                limite: Some(50),
                cursor: None,
                saida: None,
            });
            assert!(matches!(r, Resposta::Relatorio { .. }), "{r:?}");
        },
    );
    mostrar("status (totais do catálogo)", 3, &mut || {
        let r = a.executar(Comando::Status { execucao: None });
        assert!(matches!(r, Resposta::Status { .. }), "{r:?}");
    });
    mostrar(
        "resumo_duplicados (totais de todos os grupos)",
        3,
        &mut || {
            dag_store::duplicados::resumo_duplicados(a.catalogo().armazem().conexao()).unwrap();
        },
    );
    println!(
        "consultas: crescimento de VmHWM durante as consultas = {:.0} MiB",
        (proc_kb("VmHWM:") - hwm0) as f64 / 1024.0
    );
    // Comparação: as versões de referência (todos os grupos na memória) são as do código
    // anterior à consulta em SQL. `DAG_ESCALA_SEM_REFERENCIA=1` pula (podem levar minutos).
    if std::env::var("DAG_ESCALA_SEM_REFERENCIA").is_err() {
        use dag_store::duplicados::{FiltroGrupos, listar_grupos_referencia};
        let hwm1 = proc_kb("VmHWM:");
        let conn = a.catalogo().armazem().conexao();
        mostrar(
            "[referência em memória] listar_duplicados 1ª página",
            1,
            &mut || {
                listar_grupos_referencia(conn, &FiltroGrupos::default(), 50, None).unwrap();
            },
        );
        mostrar(
            "[referência em memória] resumo_duplicados",
            1,
            &mut || {
                dag_store::duplicados::resumo_duplicados_referencia(conn).unwrap();
            },
        );
        mostrar(
            "[referência em memória] espaço por volume",
            1,
            &mut || {
                dag_store::duplicados::espaco_de_grupos_referencia(conn).unwrap();
            },
        );
        println!(
            "[referência] crescimento de VmHWM = {:.0} MiB",
            (proc_kb("VmHWM:") - hwm1) as f64 / 1024.0
        );
    }
    println!(
        "consultas: VmHWM final = {:.0} MiB; crescimento total = {:.0} MiB",
        proc_kb("VmHWM:") as f64 / 1024.0,
        (proc_kb("VmHWM:") - hwm0) as f64 / 1024.0
    );
}
