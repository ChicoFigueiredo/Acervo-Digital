//! Lixeira, lado Catálogo: a cópia Na Lixeira não conta como outra cópia (Única Cópia,
//! Cobertura, Grupos de Duplicados) e a reconciliação das varreduras (ADR 0005).
#![allow(clippy::unwrap_used)]

use dag_core::{Alvo, ArquivoId, Autor, TipoMarcacao};
use dag_store::Catalogo;
use dag_store::duplicados::{self, Escopo, FiltroGrupos, listar_grupos, reconstruir_grupos};
use dag_store::lixeira;
use rusqlite::params;

const T: i64 = 1_000_000;

struct Fx {
    c: Catalogo,
}

impl Fx {
    fn novo() -> Self {
        let c = Catalogo::em_memoria().unwrap();
        c.armazem()
            .conexao()
            .execute(
                "INSERT INTO scan_runs (mode, started_at) VALUES ('full', 1)",
                [],
            )
            .unwrap();
        Self { c }
    }

    fn conn(&self) -> &rusqlite::Connection {
        self.c.armazem().conexao()
    }

    fn volume(&self, rotulo: &str, disco: &str, mp: &str) -> (i64, i64) {
        let d: i64 = self
            .conn()
            .query_row(
                "INSERT INTO dispositivos (identificador, tipo, criado_em) VALUES (?1, 'ssd', 1) RETURNING id",
                [disco],
                |l| l.get(0),
            )
            .unwrap();
        let v: i64 = self
            .conn()
            .query_row(
                "INSERT INTO volumes (platform_id, device_id, rotulo, mounted, last_seen_at)
                 VALUES (?1, ?2, ?1, 1, 5000) RETURNING id",
                params![rotulo, d],
                |l| l.get(0),
            )
            .unwrap();
        self.conn()
            .execute(
                "INSERT INTO mount_points (volume_id, path_raw, path_display) VALUES (?1, CAST(?2 AS BLOB), ?2)",
                params![v, mp],
            )
            .unwrap();
        let p: i64 = self
            .conn()
            .query_row(
                "INSERT INTO folders (volume_id, path_raw, path_display, name_raw, name_display)
                 VALUES (?1, CAST(?2 AS BLOB), ?2, CAST(?2 AS BLOB), ?2) RETURNING id",
                params![v, mp],
                |l| l.get(0),
            )
            .unwrap();
        (v, p)
    }

    fn arq(&self, vol: i64, pasta: i64, caminho: &str, inode: i64, hash: u8) -> i64 {
        let id: i64 = self
            .conn()
            .query_row(
                "INSERT INTO files (volume_id, folder_id, path_raw, path_display, name_raw, name_display,
                                    native_dev, native_file_id, nlink, logical_bytes, mtime_ns, ctime_ns, seen_run_id)
                 VALUES (?1, ?2, CAST(?3 AS BLOB), ?3, CAST(?3 AS BLOB), ?3, 1, ?4, 1, 100, 1000, 1000, 1) RETURNING id",
                params![vol, pasta, caminho, inode],
                |l| l.get(0),
            )
            .unwrap();
        self.conn()
            .execute(
                "INSERT INTO hashes (file_id, hash, hashed_at, size_bytes, mtime_ns, ctime_ns, native_file_id)
                 SELECT id, ?2, 1, logical_bytes, mtime_ns, ctime_ns, native_file_id FROM files WHERE id = ?1",
                params![id, [hash; 32].as_slice()],
            )
            .unwrap();
        id
    }

    fn grupos(&self) -> usize {
        listar_grupos(self.conn(), &FiltroGrupos::default(), 100, None)
            .unwrap()
            .grupos
            .len()
    }

    fn reconstruir(&mut self) {
        reconstruir_grupos(&mut self.c, Escopo::Tudo, Autor::Cli, T).unwrap();
    }

    fn na_lixeira(&self, id: i64, sim: bool) {
        self.conn()
            .execute(
                "UPDATE files SET na_lixeira = ?2 WHERE id = ?1",
                params![id, i64::from(sim)],
            )
            .unwrap();
    }
}

#[test]
fn migracao_8_e_aditiva_e_comeca_sem_nada_na_lixeira() {
    let f = Fx::novo();
    let c = f.conn();
    let cols: Vec<String> = c
        .prepare("SELECT name FROM pragma_table_info('files')")
        .unwrap()
        .query_map([], |l| l.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for col in [
        "na_lixeira",
        "lixeira_origem_raw",
        "lixeira_origem_display",
        "lixeira_movido_em",
    ] {
        assert!(cols.iter().any(|x| x == col), "falta {col}");
    }
    let (v, p) = f.volume("A", "d1", "/a");
    let id = f.arq(v, p, "/a/x", 1, 1);
    let na: i64 = c
        .query_row("SELECT na_lixeira FROM files WHERE id = ?1", [id], |l| {
            l.get(0)
        })
        .unwrap();
    assert_eq!(na, 0);
    assert_eq!(lixeira::totais(c).unwrap().itens, 0);
}

#[test]
fn copia_na_lixeira_nao_forma_grupo_nem_evita_unica_copia() {
    let mut f = Fx::novo();
    let (v1, p1) = f.volume("A", "d1", "/a");
    let (v2, p2) = f.volume("B", "d2", "/b");
    let x = f.arq(v1, p1, "/a/x", 1, 7);
    let y = f.arq(v2, p2, "/b/x", 2, 7);
    f.reconstruir();
    assert_eq!(f.grupos(), 1);
    assert!(!duplicados::eh_unica_copia(f.conn(), x).unwrap());
    f.na_lixeira(y, true);
    f.reconstruir();
    assert_eq!(f.grupos(), 0, "a cópia Na Lixeira sai do grupo");
    assert!(duplicados::eh_unica_copia(f.conn(), x).unwrap());
    // E marcar a restante para deleção dá o alerta de Única Cópia.
    let r = duplicados::marcar(
        &mut f.c,
        TipoMarcacao::Delecao,
        &Alvo::Arquivo(ArquivoId(x)),
        Autor::Cli,
        T,
    )
    .unwrap();
    assert_eq!(r.unicas_copias_total, 1);
}

#[test]
fn copia_na_lixeira_nao_da_cobertura() {
    let mut f = Fx::novo();
    let (v1, p1) = f.volume("A", "d1", "/a");
    let (v2, p2) = f.volume("B", "d2", "/b");
    let x = f.arq(v1, p1, "/a/x", 1, 7);
    let y = f.arq(v2, p2, "/b/x", 2, 7);
    f.reconstruir();
    duplicados::marcar(
        &mut f.c,
        TipoMarcacao::Backup,
        &Alvo::Arquivo(ArquivoId(y)),
        Autor::Cli,
        T,
    )
    .unwrap();
    assert!(
        duplicados::cobertura_do_arquivo(f.conn(), x)
            .unwrap()
            .coberto
    );
    f.na_lixeira(y, true);
    f.reconstruir();
    assert!(
        !duplicados::cobertura_do_arquivo(f.conn(), x)
            .unwrap()
            .coberto,
        "o backup Na Lixeira não cobre"
    );
    let (d, _) = duplicados::relatorio_sem_cobertura(f.conn(), None, 100, None).unwrap();
    let dag_core::DadosRelatorio::SemCobertura { arquivos } = d else {
        panic!()
    };
    assert!(
        arquivos.iter().all(|a| a.caminho != "/b/x"),
        "Na Lixeira não entra no relatório"
    );
}

#[test]
fn reconciliar_poe_tira_e_descarta() {
    let mut f = Fx::novo();
    let (v, p) = f.volume("A", "d1", "/a");
    let pl = f.arq(v, p, "/a/DAG-Lixeira/sub/f.txt", 1, 3);
    let normal = f.arq(v, p, "/a/g.txt", 2, 4);
    // Posto à mão: passa a Na Lixeira, com origem e Marcação para Deleção.
    let r = lixeira::reconciliar(&mut f.c, Autor::Cli, T).unwrap();
    assert_eq!((r.postos_na_lixeira, r.marcados), (1, 1));
    let (itens, _, t) = lixeira::listar(f.conn(), None, 10).unwrap();
    assert_eq!(t.itens, 1);
    assert_eq!(itens[0].origem, "/a/sub/f.txt");
    let marcas: i64 = f
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM marcacoes WHERE tipo = 'delecao' AND file_id = ?1",
            [pl],
            |l| l.get(0),
        )
        .unwrap();
    assert_eq!(marcas, 1);
    // Idempotente.
    let r = lixeira::reconciliar(&mut f.c, Autor::Cli, T).unwrap();
    assert_eq!(
        (r.postos_na_lixeira, r.retirados, r.descartados, r.marcados),
        (0, 0, 0, 0)
    );
    // Tirado à mão: o caminho sai da Lixeira; vira comum e perde a marcação.
    f.conn()
        .execute(
            "UPDATE files SET path_raw = CAST('/a/f.txt' AS BLOB), path_display = '/a/f.txt' WHERE id = ?1",
            [pl],
        )
        .unwrap();
    let r = lixeira::reconciliar(&mut f.c, Autor::Cli, T).unwrap();
    assert_eq!(r.retirados, 1);
    assert_eq!(lixeira::totais(f.conn()).unwrap().itens, 0);
    let marcas: i64 = f
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM marcacoes WHERE file_id = ?1",
            [pl],
            |l| l.get(0),
        )
        .unwrap();
    assert_eq!(marcas, 0);
    // Some da Lixeira: vira Descartado; arquivo comum que some não vira.
    f.conn()
        .execute(
            "UPDATE files SET path_raw = CAST('/a/DAG-Lixeira/f.txt' AS BLOB), path_display = '/a/DAG-Lixeira/f.txt' WHERE id = ?1",
            [pl],
        )
        .unwrap();
    lixeira::reconciliar(&mut f.c, Autor::Cli, T).unwrap();
    f.conn()
        .execute(
            "UPDATE files SET state = 'ausente' WHERE id IN (?1, ?2)",
            [pl, normal],
        )
        .unwrap();
    let r = lixeira::reconciliar(&mut f.c, Autor::Cli, T + 5).unwrap();
    assert_eq!(r.descartados, 1);
    let d = lixeira::ultimos_descartados(f.conn(), 10).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].caminho, "/a/DAG-Lixeira/f.txt");
    assert_eq!(d[0].hash.as_deref(), Some("03".repeat(32).as_str()));
    assert_eq!(lixeira::totais(f.conn()).unwrap().descartados, 1);
}
