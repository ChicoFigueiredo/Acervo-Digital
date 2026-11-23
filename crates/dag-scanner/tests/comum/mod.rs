//! Utilitários compartilhados pelos testes do scanner.
#![allow(dead_code, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use dag_core::VolumeDescoberto;
use dag_scanner::{ConfigScan, FullScan};
use dag_store::Catalogo;

pub fn volume_de(p: &Path) -> VolumeDescoberto {
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

pub fn scan(raiz: &Path) -> FullScan {
    FullScan::novo(vec![raiz.to_path_buf()], vec![volume_de(raiz)])
}

/// Configuração que força muitos lotes pequenos (exercita pausa, cancelamento e retomada).
pub fn config_miuda() -> ConfigScan {
    ConfigScan {
        concorrencia: 2,
        linhas_por_transacao: 50,
        entradas_por_mensagem: 20,
        fila_de_resultados: 4,
        reservas_por_vez: 4,
        intervalo_de_progresso: std::time::Duration::ZERO,
        exclusoes: Vec::new(),
    }
}

pub fn n(c: &Catalogo, sql: &str) -> i64 {
    c.armazem()
        .conexao()
        .query_row(sql, [], |l| l.get(0))
        .unwrap()
}

pub fn escrever(p: &Path, bytes: usize) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut f = File::create(p).unwrap();
    f.write_all(&vec![b'x'; bytes]).unwrap();
}

/// Árvore determinística: 6 topos × 10 médios × 5 folhas (366 pastas + raiz), ~2.700 arquivos.
/// Inclui pastas reais chamadas `proc`, `sys` e `dev`, que devem ser varridas.
pub fn criar_arvore(raiz: &Path) {
    let mut k = 0usize;
    let mut tamanho = || {
        k += 1;
        (k * 37) % 5000
    };
    for i in 0..5 {
        escrever(&raiz.join(format!("raiz{i}.dat")), tamanho());
    }
    for t in 0..6 {
        let topo = raiz.join(format!("t{t}"));
        for i in 0..3 {
            escrever(&topo.join(format!("a{i}.bin")), tamanho());
        }
        for m in 0..10 {
            let medio = topo.join(format!("m{m}"));
            for i in 0..4 {
                escrever(&medio.join(format!("b{i}.txt")), tamanho());
            }
            for l in 0..5 {
                let folha = medio.join(format!("l{l}"));
                fs::create_dir_all(&folha).unwrap();
                for i in 0..8 {
                    escrever(&folha.join(format!("f{i}.log")), tamanho());
                }
            }
        }
    }
    for nome in ["proc", "sys", "dev"] {
        escrever(&raiz.join(nome).join("arquivo.txt"), 11);
    }
    // Arquivo vazio e pasta vazia.
    escrever(&raiz.join("vazio.txt"), 0);
    fs::create_dir_all(raiz.join("pasta_vazia")).unwrap();
    // Datas conhecidas em alguns arquivos.
    for i in 0..5 {
        let f = File::options()
            .write(true)
            .open(raiz.join(format!("raiz{i}.dat")))
            .unwrap();
        f.set_modified(
            std::time::UNIX_EPOCH + std::time::Duration::new(1_600_000_000 + i, 123_456_789),
        )
        .unwrap();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaFs {
    pub kind: &'static str,
    pub tamanho: i64,
    pub alocado: i64,
    pub nlink: i64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
    pub dev: u64,
    pub ino: u64,
}

pub fn kind_de(t: std::fs::FileType) -> &'static str {
    use std::os::unix::fs::FileTypeExt;
    if t.is_dir() {
        "pasta"
    } else if t.is_file() {
        "arquivo"
    } else if t.is_symlink() {
        "symlink"
    } else if t.is_fifo() {
        "fifo"
    } else if t.is_socket() {
        "socket"
    } else if t.is_block_device() {
        "dispositivo_bloco"
    } else if t.is_char_device() {
        "dispositivo_char"
    } else {
        "outro"
    }
}

pub fn meta_de(m: &fs::Metadata) -> MetaFs {
    MetaFs {
        kind: kind_de(m.file_type()),
        tamanho: m.size() as i64,
        alocado: (m.blocks() * 512) as i64,
        nlink: m.nlink() as i64,
        mtime_ns: m.mtime() * 1_000_000_000 + m.mtime_nsec(),
        ctime_ns: m.ctime() * 1_000_000_000 + m.ctime_nsec(),
        dev: m.dev(),
        ino: m.ino(),
    }
}

/// Percorre o FS sem seguir symlinks e sem montar nada: caminho bruto → metadados.
pub fn percorrer_fs(raiz: &Path) -> BTreeMap<Vec<u8>, MetaFs> {
    let mut saida = BTreeMap::new();
    let m = fs::symlink_metadata(raiz).unwrap();
    saida.insert(raiz.as_os_str().as_bytes().to_vec(), meta_de(&m));
    let dev = m.dev();
    let mut pilha: Vec<PathBuf> = vec![raiz.to_path_buf()];
    while let Some(d) = pilha.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let m = e.metadata().unwrap();
            let p = e.path();
            saida.insert(p.as_os_str().as_bytes().to_vec(), meta_de(&m));
            if m.is_dir() && m.dev() == dev {
                pilha.push(p);
            }
        }
    }
    saida
}

pub type LinhaArquivo = (String, i64, Option<i64>, i64, Option<i64>);
#[allow(clippy::type_complexity)]
pub type LinhaStats = (i64, i64, i64, i64, i64, Option<i64>, Option<i64>, String);

#[derive(Debug, PartialEq, Eq)]
pub struct Foto {
    pub pastas: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    pub arquivos: BTreeMap<Vec<u8>, LinhaArquivo>,
    pub stats: BTreeMap<Vec<u8>, LinhaStats>,
}

pub fn foto(c: &Catalogo) -> Foto {
    let con = c.armazem().conexao();
    let mut pastas = BTreeMap::new();
    let mut stats = BTreeMap::new();
    let mut st = con
        .prepare(
            "SELECT f.path_raw, p.path_raw, s.child_count, s.recursive_file_count,
                    s.recursive_folder_count, s.recursive_special_count, s.logical_bytes,
                    s.allocated_entry_bytes, s.physical_unique_bytes, s.aggregate_state
             FROM folders f LEFT JOIN folders p ON p.id = f.parent_folder_id
             LEFT JOIN folder_stats s ON s.folder_id = f.id",
        )
        .unwrap();
    let mut rows = st.query([]).unwrap();
    while let Some(l) = rows.next().unwrap() {
        let caminho: Vec<u8> = l.get(0).unwrap();
        pastas.insert(caminho.clone(), l.get::<_, Option<Vec<u8>>>(1).unwrap());
        if let Some(estado) = l.get::<_, Option<String>>(9).unwrap() {
            stats.insert(
                caminho,
                (
                    l.get(2).unwrap(),
                    l.get(3).unwrap(),
                    l.get(4).unwrap(),
                    l.get(5).unwrap(),
                    l.get(6).unwrap(),
                    l.get(7).unwrap(),
                    l.get(8).unwrap(),
                    estado,
                ),
            );
        }
    }
    let mut arquivos = BTreeMap::new();
    let mut st = con
        .prepare(
            "SELECT path_raw, kind, logical_bytes, allocated_bytes, nlink, mtime_ns FROM files",
        )
        .unwrap();
    let mut rows = st.query([]).unwrap();
    while let Some(l) = rows.next().unwrap() {
        arquivos.insert(
            l.get::<_, Vec<u8>>(0).unwrap(),
            (
                l.get(1).unwrap(),
                l.get(2).unwrap(),
                l.get(3).unwrap(),
                l.get(4).unwrap(),
                l.get(5).unwrap(),
            ),
        );
    }
    Foto {
        pastas,
        arquivos,
        stats,
    }
}

pub fn nome_bruto(p: &[u8]) -> &[u8] {
    Path::new(OsStr::from_bytes(p))
        .file_name()
        .map_or(p, OsStr::as_bytes)
}

pub fn pais_de(p: &[u8]) -> Vec<u8> {
    Path::new(OsStr::from_bytes(p))
        .parent()
        .unwrap()
        .as_os_str()
        .as_bytes()
        .to_vec()
}

pub fn conjunto<T: Ord + Clone>(it: impl IntoIterator<Item = T>) -> BTreeSet<T> {
    it.into_iter().collect()
}
