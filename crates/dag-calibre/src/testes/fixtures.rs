#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Fixtures: biblioteca sintética (SQLite direto, só como fixture) e biblioteca real (calibredb).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use rusqlite::{Connection, params};

use crate::calibredb::ClienteCalibredb;
use crate::executor::{Comando, ExecutorCalibre, ExecutorProcesso, SaidaProcesso};

const ESQUEMA: &str = "
CREATE TABLE books (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL DEFAULT 'Unknown', sort TEXT,
  timestamp TIMESTAMP, pubdate TIMESTAMP, series_index REAL NOT NULL DEFAULT 1.0, author_sort TEXT,
  isbn TEXT DEFAULT '', lccn TEXT DEFAULT '', path TEXT NOT NULL DEFAULT '', flags INTEGER NOT NULL DEFAULT 1,
  uuid TEXT, has_cover BOOL DEFAULT 0, last_modified TIMESTAMP NOT NULL DEFAULT '2000-01-01 00:00:00+00:00');
CREATE TABLE authors (id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort TEXT, link TEXT NOT NULL DEFAULT '');
CREATE TABLE books_authors_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, author INTEGER NOT NULL);
CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT NOT NULL, link TEXT NOT NULL DEFAULT '');
CREATE TABLE books_tags_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, tag INTEGER NOT NULL);
CREATE TABLE series (id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort TEXT, link TEXT NOT NULL DEFAULT '');
CREATE TABLE books_series_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, series INTEGER NOT NULL);
CREATE TABLE publishers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort TEXT, link TEXT NOT NULL DEFAULT '');
CREATE TABLE books_publishers_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, publisher INTEGER NOT NULL);
CREATE TABLE languages (id INTEGER PRIMARY KEY, lang_code TEXT NOT NULL, link TEXT NOT NULL DEFAULT '');
CREATE TABLE books_languages_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, lang_code INTEGER NOT NULL, item_order INTEGER NOT NULL DEFAULT 0);
CREATE TABLE identifiers (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, type TEXT NOT NULL DEFAULT 'isbn', val TEXT NOT NULL);
CREATE TABLE comments (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, text TEXT NOT NULL);
CREATE TABLE ratings (id INTEGER PRIMARY KEY, rating INTEGER, link TEXT NOT NULL DEFAULT '');
CREATE TABLE books_ratings_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, rating INTEGER NOT NULL);
CREATE TABLE data (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, format TEXT NOT NULL, uncompressed_size INTEGER NOT NULL, name TEXT NOT NULL);
CREATE TABLE custom_columns (id INTEGER PRIMARY KEY AUTOINCREMENT, label TEXT NOT NULL, name TEXT NOT NULL, datatype TEXT NOT NULL,
  mark_for_delete BOOL DEFAULT 0 NOT NULL, editable BOOL DEFAULT 1 NOT NULL, display TEXT DEFAULT '{}' NOT NULL,
  is_multiple BOOL DEFAULT 0 NOT NULL, normalized BOOL NOT NULL);
CREATE TABLE preferences (id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE, val TEXT NOT NULL);
CREATE TABLE library_id (id INTEGER PRIMARY KEY, uuid TEXT NOT NULL UNIQUE);
PRAGMA user_version = 26;
";

pub struct Sintetica {
    #[allow(dead_code)]
    pub raiz: PathBuf,
    pub conn: Connection,
}

pub fn cria_coluna(
    conn: &Connection,
    label: &str,
    nome: &str,
    tipo: &str,
    multipla: bool,
    display: &str,
) -> i64 {
    let normalizada = matches!(tipo, "text" | "series" | "enumeration" | "rating");
    conn.execute(
        "INSERT INTO custom_columns(label,name,datatype,display,is_multiple,normalized) VALUES (?1,?2,?3,?4,?5,?6)",
        params![label, nome, tipo, display, multipla, normalizada],
    )
    .unwrap();
    let n = conn.last_insert_rowid();
    if normalizada {
        let dt = if tipo == "rating" { "INT" } else { "TEXT" };
        let extra = if tipo == "series" { ", extra REAL" } else { "" };
        conn.execute_batch(&format!(
            "CREATE TABLE custom_column_{n}(id INTEGER PRIMARY KEY AUTOINCREMENT, value {dt} NOT NULL, UNIQUE(value));
             CREATE TABLE books_custom_column_{n}_link(id INTEGER PRIMARY KEY AUTOINCREMENT, book INTEGER NOT NULL, value INTEGER NOT NULL{extra}, UNIQUE(book,value));"
        ))
        .unwrap();
    } else {
        let dt = match tipo {
            "int" => "INT",
            "float" => "REAL",
            "bool" => "BOOL",
            "datetime" => "timestamp",
            _ => "TEXT",
        };
        conn.execute_batch(&format!(
            "CREATE TABLE custom_column_{n}(id INTEGER PRIMARY KEY AUTOINCREMENT, book INTEGER, value {dt}, UNIQUE(book));"
        ))
        .unwrap();
    }
    n
}

impl Sintetica {
    pub fn nova(raiz: &Path, library_id: &str) -> Self {
        std::fs::create_dir_all(raiz).unwrap();
        let conn = Connection::open(raiz.join("metadata.db")).unwrap();
        conn.execute_batch(ESQUEMA).unwrap();
        conn.execute("INSERT INTO library_id(uuid) VALUES (?1)", [library_id])
            .unwrap();
        Self {
            raiz: raiz.to_path_buf(),
            conn,
        }
    }

    pub fn pref(&self, k: &str, v: &str) {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO preferences(key,val) VALUES (?1,?2)",
                params![k, v],
            )
            .unwrap();
    }

    pub fn livro(&self, id: i64, titulo: &str, path: &str) {
        self.conn
            .execute(
                "INSERT INTO books(id,title,sort,pubdate,series_index,author_sort,path,uuid,has_cover,last_modified)
                 VALUES (?1,?2,?2,'0101-01-01 00:00:00+00:00',1.0,'x',?3,?4,0,'2024-01-01 00:00:00+00:00')",
                params![id, titulo, path, format!("uuid-{id}")],
            )
            .unwrap();
    }

    /// Valor normalizado para colunas text/enumeration/rating/series.
    pub fn valor_norm(
        &self,
        col: i64,
        livro: i64,
        valor: rusqlite::types::Value,
        extra: Option<f64>,
    ) {
        self.conn
            .execute(
                &format!("INSERT OR IGNORE INTO custom_column_{col}(value) VALUES (?1)"),
                [&valor],
            )
            .unwrap();
        let vid: i64 = self
            .conn
            .query_row(
                &format!("SELECT id FROM custom_column_{col} WHERE value=?1"),
                [&valor],
                |r| r.get(0),
            )
            .unwrap();
        let sql = if extra.is_some() {
            format!(
                "INSERT INTO books_custom_column_{col}_link(book,value,extra) VALUES (?1,?2,?3)"
            )
        } else {
            format!("INSERT INTO books_custom_column_{col}_link(book,value) VALUES (?1,?2)")
        };
        if let Some(e) = extra {
            self.conn.execute(&sql, params![livro, vid, e]).unwrap();
        } else {
            self.conn.execute(&sql, params![livro, vid]).unwrap();
        }
    }

    pub fn valor_direto(&self, col: i64, livro: i64, valor: rusqlite::types::Value) {
        self.conn
            .execute(
                &format!("INSERT INTO custom_column_{col}(book,value) VALUES (?1,?2)"),
                params![livro, valor],
            )
            .unwrap();
    }
}

// ---------- calibre real ----------

static TRAVA_CALIBRE: Mutex<()> = Mutex::new(());
static CONFIG: OnceLock<tempfile::TempDir> = OnceLock::new();

/// Serializa os testes que usam o `calibredb` (o lock `db` é global por usuário) e devolve o
/// cliente real; `None` (com aviso) se o `calibredb` não existir.
pub fn calibre() -> Option<(MutexGuard<'static, ()>, ClienteCalibredb<ExecutorProcesso>)> {
    let g = TRAVA_CALIBRE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(exec) = executor_real() else {
        eprintln!("AVISO: calibredb não encontrado; teste pulado");
        return None;
    };
    let cli = ClienteCalibredb::novo(exec)
        .com_tentativas_lock(60)
        .com_timeout(Duration::from_secs(120));
    Some((g, cli))
}

pub fn executor_real() -> Option<ExecutorProcesso> {
    let cfg = CONFIG.get_or_init(|| tempfile::tempdir().unwrap());
    let e = ExecutorProcesso::default()
        .com_ambiente("CALIBRE_CONFIG_DIRECTORY", cfg.path().to_string_lossy());
    let ok = e
        .executar(&Comando {
            args: vec!["--version".into()],
            stdin: None,
            timeout: Duration::from_secs(60),
        })
        .is_ok_and(|s| s.sucesso());
    ok.then_some(e)
}

/// Roda o calibredb cru (só para montar fixtures) e exige sucesso.
pub fn cal(e: &ExecutorProcesso, lib: &Path, args: &[&str]) -> SaidaProcesso {
    for _ in 0..80 {
        let mut a: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        a.push("--library-path".into());
        a.push(lib.to_string_lossy().into_owned());
        let s = e
            .executar(&Comando {
                args: a,
                stdin: None,
                timeout: Duration::from_secs(120),
            })
            .unwrap();
        if s.sucesso() {
            return s;
        }
        if s.stderr.contains("Another calibre program") {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        panic!("calibredb {args:?} falhou: {}", s.stderr);
    }
    panic!("lock do calibredb nunca liberou");
}

/// Biblioteca "homologada" de verdade, criada pelo calibredb, com Unicode, múltiplos autores,
/// série, uma coluna de cada tipo, dois formatos num livro e duas bibliotecas virtuais.
pub fn construir_real(e: &ExecutorProcesso, lib: &Path, arquivos: &Path) {
    std::fs::create_dir_all(lib).unwrap();
    std::fs::create_dir_all(arquivos).unwrap();
    cal(e, lib, &["list", "--fields", "id"]);
    let cols: [(&str, &str, &str, &[&str]); 11] = [
        ("pasta", "!1.Pasta", "text", &[]),
        ("tagsx", "Tags X", "text", &["--is-multiple"]),
        ("serie2", "Coleção", "series", &[]),
        (
            "enu",
            "Nível",
            "enumeration",
            &["--display", r#"{"enum_values":["a","b","c"]}"#],
        ),
        ("inteiro", "Páginas", "int", &[]),
        ("flt", "Peso", "float", &[]),
        ("flag", "É Scan", "bool", &[]),
        ("nota", "Nota", "rating", &[]),
        ("quando", "Lido em", "datetime", &[]),
        ("texto", "Resenha", "comments", &[]),
        (
            "calc",
            "Calculada",
            "composite",
            &[
                "--display",
                r##"{"composite_template":"{title} ({#pasta})"}"##,
            ],
        ),
    ];
    for (l, n, t, extra) in cols {
        let mut a = vec!["add_custom_column", l, n, t];
        a.extend_from_slice(extra);
        cal(e, lib, &a);
    }
    let f1 = arquivos.join("calc.txt");
    std::fs::write(&f1, "Cálculo — conteúdo do livro um\n").unwrap();
    let f1b = arquivos.join("calc.html");
    std::fs::write(&f1b, "<html><body>Cálculo</body></html>").unwrap();
    let f2 = arquivos.join("xadrez.txt");
    std::fs::write(&f2, "Xadrez básico\n").unwrap();
    let f3 = arquivos.join("sem-tags.txt");
    std::fs::write(&f3, "Sem tags\n").unwrap();
    cal(
        e,
        lib,
        &[
            "add",
            "-d",
            "-t",
            "Cálculo — Vol. 1: 微分",
            "-a",
            "José Álvaro Pereira & Maria Çelik",
            "-T",
            "Matemática,!Catalogar,Análise",
            "-s",
            "Série Única",
            "-S",
            "2",
            "-l",
            "por,eng",
            "-I",
            "isbn:9788535902778",
            f1.to_str().unwrap(),
        ],
    );
    cal(e, lib, &["add_format", "1", f1b.to_str().unwrap()]);
    cal(
        e,
        lib,
        &[
            "set_metadata",
            "1",
            "-f",
            "publisher:Editora São Jorge",
            "-f",
            "comments:Resenha <b>boa</b>",
            "-f",
            "rating:4",
            "-f",
            "pubdate:2020-05-06",
            "-f",
            "#pasta:Matemática",
            "-f",
            "#tagsx:um,dois",
            "-f",
            "#serie2:Coleção Azul",
            "-f",
            "#serie2_index:2.5",
            "-f",
            "#enu:b",
            "-f",
            "#inteiro:300",
            "-f",
            "#flt:1.5",
            "-f",
            "#flag:true",
            "-f",
            "#nota:3",
            "-f",
            "#quando:2021-03-04",
            "-f",
            "#texto:<p>oi</p>",
        ],
    );
    cal(
        e,
        lib,
        &[
            "add",
            "-d",
            "-t",
            "Xadrez Básico",
            "-a",
            "Ana Souza",
            "-T",
            "Xadrez",
            f2.to_str().unwrap(),
        ],
    );
    cal(
        e,
        lib,
        &[
            "set_metadata",
            "2",
            "-f",
            "#pasta:Xadrez",
            "-f",
            "#flag:false",
        ],
    );
    cal(
        e,
        lib,
        &[
            "add",
            "-d",
            "-t",
            "Sem Tags",
            "-a",
            "Beto",
            f3.to_str().unwrap(),
        ],
    );
    cal(
        e,
        lib,
        &["saved_searches", "add", "so-xadrez", "tags:=Xadrez"],
    );
    cal(e, lib, &["backup_metadata", "--all"]);
    // Bibliotecas virtuais não têm comando no calibredb: fixture grava a preferência.
    let c = Connection::open(lib.join("metadata.db")).unwrap();
    c.execute(
        "INSERT OR REPLACE INTO preferences(key,val) VALUES ('virtual_libraries', ?1)",
        [r##"{"Matemática": "#pasta:\"=Matemática\"", "Xadrez": "#pasta:\"=Xadrez\"", "Inglês": "languages:=eng"}"##],
    )
    .unwrap();
}
