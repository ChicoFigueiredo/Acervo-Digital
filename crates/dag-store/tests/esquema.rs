//! Testes do esquema do Catálogo.
#![allow(clippy::unwrap_used)]

use bancada_armazem::esta_protegida;
use dag_store::{Catalogo, versao_do_esquema};
use rusqlite::Connection;
use std::os::unix::ffi::OsStrExt;

fn cat() -> Catalogo {
    Catalogo::em_memoria().expect("catálogo em memória")
}

fn volume(c: &Connection) -> i64 {
    c.execute(
        "INSERT INTO volumes (platform_id) VALUES (?1)",
        [format!("v{}", c.last_insert_rowid())],
    )
    .expect("volume");
    c.last_insert_rowid()
}

fn pasta(c: &Connection, vol: i64, parent: Option<i64>, caminho: &[u8]) -> i64 {
    c.execute(
        "INSERT INTO folders (volume_id, parent_folder_id, path_raw, path_display, name_raw, name_display)
         VALUES (?1, ?2, ?3, 'p', ?3, 'p')",
        rusqlite::params![vol, parent, caminho],
    )
    .expect("pasta");
    c.last_insert_rowid()
}

fn arquivo(c: &Connection, vol: i64, pasta: i64, caminho: &[u8], bytes: i64) -> i64 {
    c.execute(
        "INSERT INTO files (volume_id, folder_id, path_raw, path_display, name_raw, name_display, logical_bytes)
         VALUES (?1, ?2, ?3, 'a', ?3, 'a', ?4)",
        rusqlite::params![vol, pasta, caminho, bytes],
    )
    .expect("arquivo");
    c.last_insert_rowid()
}

fn contar(c: &Connection, tabela: &str) -> i64 {
    c.query_row(&format!("SELECT COUNT(*) FROM {tabela}"), [], |l| l.get(0))
        .expect("count")
}

#[test]
fn migracao_em_banco_vazio_cria_todas_as_tabelas() {
    let catalogo = cat();
    assert_eq!(catalogo.versao().expect("versão"), versao_do_esquema());
    assert_eq!(versao_do_esquema(), 10);
    let c = catalogo.armazem().conexao();
    for t in [
        "dispositivos",
        "volumes",
        "mount_points",
        "folders",
        "files",
        "file_types",
        "classifications",
        "folder_stats",
        "scan_runs",
        "scan_queue",
        "scan_events",
        "scan_errors",
        "scan_exclusions",
        "service_state",
        "catalog_snapshots",
        "hashes",
        "fila_hash",
        "grupos_duplicados",
        "grupo_membros",
        "prioridades",
        "marcacoes",
        "marcacoes_historico",
        "descartados",
    ] {
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [t],
                |l| l.get(0),
            )
            .expect("master");
        assert_eq!(n, 1, "tabela {t} ausente");
    }
}

#[test]
fn reabrir_e_idempotente() {
    let dir = tempfile::tempdir().expect("tmp");
    let caminho = dir.path().join("catalogo.sqlite");
    {
        let c = Catalogo::abrir(&caminho).expect("abrir");
        c.armazem()
            .conexao()
            .execute("INSERT INTO volumes (platform_id) VALUES ('x')", [])
            .expect("ins");
    }
    let c = Catalogo::abrir(&caminho).expect("reabrir");
    assert_eq!(c.versao().expect("v"), versao_do_esquema());
    assert_eq!(contar(c.armazem().conexao(), "volumes"), 1);
    assert_eq!(contar(c.armazem().conexao(), "service_state"), 1);
}

#[test]
fn arquivos_e_pastas_sao_tabelas_separadas() {
    let catalogo = cat();
    let c = catalogo.armazem().conexao();
    let v = volume(c);
    let p = pasta(c, v, None, b"/a");
    arquivo(c, v, p, b"/a/f", 10);
    assert_eq!(contar(c, "folders"), 1);
    assert_eq!(contar(c, "files"), 1);
    // arquivo não pode apontar para pasta inexistente
    let err = c.execute(
        "INSERT INTO files (volume_id, folder_id, path_raw, path_display, name_raw, name_display, logical_bytes)
         VALUES (?1, 9999, x'01', 'a', x'01', 'a', 1)",
        [v],
    );
    assert!(err.is_err());
}

#[test]
fn chaves_estrangeiras_e_checks_recusam_linhas_ruins() {
    let catalogo = cat();
    let c = catalogo.armazem().conexao();
    let v = volume(c);
    let p = pasta(c, v, None, b"/a");
    // FK: volume inexistente
    assert!(
        c.execute(
            "INSERT INTO mount_points (volume_id, path_raw, path_display) VALUES (999, x'2f', '/')",
            []
        )
        .is_err()
    );
    // CHECK de enum
    assert!(
        c.execute("UPDATE folders SET state = 'quebrado' WHERE id = ?1", [p])
            .is_err()
    );
    // CHECK de valor
    let f = arquivo(c, v, p, b"/a/f", 5);
    assert!(
        c.execute("UPDATE files SET logical_bytes = -1 WHERE id = ?1", [f])
            .is_err()
    );
    assert!(
        c.execute("UPDATE files SET allocated_bytes = -1 WHERE id = ?1", [f])
            .is_err()
    );
    // hash precisa de 32 bytes
    assert!(
        c.execute(
            "INSERT INTO hashes (file_id, hash, hashed_at, size_bytes) VALUES (?1, x'00', 1, 5)",
            [f]
        )
        .is_err()
    );
    assert!(c.execute("INSERT INTO hashes (file_id, hash, hashed_at, size_bytes) VALUES (?1, zeroblob(32), 1, 5)", [f]).is_ok());
    // só BLAKE3
    assert!(
        c.execute("UPDATE hashes SET algoritmo = 'md5'", [])
            .is_err()
    );
    // volume_id duplicado de caminho
    assert!(c.execute(
        "INSERT INTO folders (volume_id, path_raw, path_display, name_raw, name_display) VALUES (?1, x'2f61', 'p', x'61', 'p')",
        [v]
    ).is_err());
    // linha única de service_state
    assert!(
        c.execute("INSERT INTO service_state (id) VALUES (2)", [])
            .is_err()
    );
    // device_id FK
    assert!(
        c.execute(
            "INSERT INTO volumes (platform_id, device_id) VALUES ('z', 777)",
            []
        )
        .is_err()
    );
}

#[test]
fn caminho_nao_utf8_faz_ida_e_volta() {
    let catalogo = cat();
    let c = catalogo.armazem().conexao();
    let v = volume(c);
    let p = pasta(c, v, None, b"/d");
    let cru: &[u8] = b"/d/arq\xff\xfe\x80.txt";
    assert!(String::from_utf8_lossy(cru).contains('\u{FFFD}'));
    let f = arquivo(c, v, p, cru, 1);
    let lido: Vec<u8> = c
        .query_row("SELECT path_raw FROM files WHERE id=?1", [f], |l| l.get(0))
        .expect("lê");
    assert_eq!(lido, cru);
    let os = std::ffi::OsStr::from_bytes(&lido);
    assert_eq!(os.as_bytes(), cru);
}

#[test]
fn campos_indisponiveis_ficam_nulos() {
    let catalogo = cat();
    let c = catalogo.armazem().conexao();
    let v = volume(c);
    let p = pasta(c, v, None, b"/a");
    let f = arquivo(c, v, p, b"/a/f", 7);
    let (alo, nasc): (Option<i64>, Option<i64>) = c
        .query_row(
            "SELECT allocated_bytes, birthtime_ns FROM files WHERE id=?1",
            [f],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )
        .expect("lê");
    assert_eq!((alo, nasc), (None, None));
    // sem linha em hashes = sem hash
    assert_eq!(contar(c, "hashes"), 0);
}

#[test]
fn historico_de_marcacoes_e_somente_acrescimo() {
    let catalogo = cat();
    let c = catalogo.armazem().conexao();
    assert!(esta_protegida(c, "marcacoes_historico").expect("protegida"));
    c.execute(
        "INSERT INTO marcacoes_historico (marcacao_id, evento, tipo, escopo, estado_novo, autor, em)
         VALUES (1, 'criada', 'backup', 'manual_arquivo', 'ativa', 'cli', 100)",
        [],
    )
    .expect("insert");
    let upd = c
        .execute("UPDATE marcacoes_historico SET autor = 'gui'", [])
        .expect_err("update");
    assert!(upd.to_string().contains("somente de acréscimo"));
    let del = c
        .execute("DELETE FROM marcacoes_historico", [])
        .expect_err("delete");
    assert!(del.to_string().contains("somente de acréscimo"));
    assert_eq!(contar(c, "marcacoes_historico"), 1);
    assert!(c.execute("INSERT INTO marcacoes_historico (evento, tipo, escopo, autor, em) VALUES ('criada','backup','manual_arquivo','robo',1)", []).is_err());
}

#[test]
fn marcacoes_regras_de_escopo_e_unicidade() {
    let catalogo = cat();
    let c = catalogo.armazem().conexao();
    let v = volume(c);
    let p = pasta(c, v, None, b"/a");
    let f = arquivo(c, v, p, b"/a/f", 7);
    // regra de volume sem hash é válida
    c.execute("INSERT INTO marcacoes (tipo, escopo, volume_id, autor, criada_em, atualizada_em) VALUES ('backup','regra_volume',?1,'gui',1,1)", [v]).expect("regra");
    // regra de volume repetida: recusada
    assert!(c.execute("INSERT INTO marcacoes (tipo, escopo, volume_id, autor, criada_em, atualizada_em) VALUES ('delecao','regra_volume',?1,'gui',1,1)", [v]).is_err());
    // manual sem hash: recusada
    assert!(c.execute("INSERT INTO marcacoes (tipo, escopo, file_id, autor, criada_em, atualizada_em) VALUES ('backup','manual_arquivo',?1,'cli',1,1)", [f]).is_err());
    // escopo incoerente com colunas: recusada
    assert!(c.execute("INSERT INTO marcacoes (tipo, escopo, volume_id, file_id, hash_na_marcacao, autor, criada_em, atualizada_em) VALUES ('backup','manual_arquivo',?1,?2,zeroblob(32),'cli',1,1)", [v, f]).is_err());
    c.execute("INSERT INTO marcacoes (tipo, escopo, file_id, hash_na_marcacao, autor, criada_em, atualizada_em) VALUES ('backup','manual_arquivo',?1,zeroblob(32),'mcp',1,1)", [f]).expect("manual");
    // no máximo uma marcação por cópia
    assert!(c.execute("INSERT INTO marcacoes (tipo, escopo, file_id, hash_na_marcacao, autor, criada_em, atualizada_em) VALUES ('delecao','manual_arquivo',?1,zeroblob(32),'gui',1,1)", [f]).is_err());
    assert!(
        c.execute("UPDATE marcacoes SET estado='pendente'", [])
            .is_ok()
    );
    assert!(
        c.execute("UPDATE marcacoes SET estado='talvez'", [])
            .is_err()
    );
}

#[test]
fn grupo_duplicado_exige_principal_quando_manual() {
    let catalogo = cat();
    let c = catalogo.armazem().conexao();
    c.execute("INSERT INTO grupos_duplicados (logical_bytes, hash, atualizado_em) VALUES (5, zeroblob(32), 1)", []).expect("auto");
    assert!(c.execute("INSERT INTO grupos_duplicados (logical_bytes, hash, copia_principal_modo, atualizado_em) VALUES (6, zeroblob(32), 'manual', 1)", []).is_err());
    assert!(c.execute("INSERT INTO grupos_duplicados (logical_bytes, hash, atualizado_em) VALUES (5, zeroblob(32), 1)", []).is_err());
    assert!(c.execute("INSERT INTO grupos_duplicados (logical_bytes, hash, atualizado_em) VALUES (0, zeroblob(32), 1)", []).is_err());
}

#[test]
fn copia_de_banco_wal_tem_mesmas_contagens_e_passa_na_integridade() {
    let dir = tempfile::tempdir().expect("tmp");
    let origem = dir.path().join("catalogo.sqlite");
    let destino = dir.path().join("copia.sqlite");
    let catalogo = Catalogo::abrir(&origem).expect("abrir");
    let modo: String = catalogo
        .armazem()
        .conexao()
        .query_row("PRAGMA journal_mode", [], |l| l.get(0))
        .expect("modo");
    assert_eq!(modo.to_lowercase(), "wal");
    {
        let c = catalogo.armazem().conexao();
        let v = volume(c);
        let p = pasta(c, v, None, b"/a");
        for i in 0..50_u8 {
            arquivo(c, v, p, &[b'/', b'a', b'/', i, 0xff], i64::from(i));
        }
    }
    catalogo.verificar_integridade().expect("íntegro");
    catalogo.copiar_para(&destino).expect("copiar");
    assert!(catalogo.copiar_para(&destino).is_err(), "não sobrescreve");
    bancada_armazem::verificar_integridade_de_arquivo(&destino).expect("cópia íntegra");

    let copia = Catalogo::abrir(&destino).expect("abre cópia");
    assert_eq!(copia.versao().expect("v"), versao_do_esquema());
    for t in ["volumes", "folders", "files"] {
        assert_eq!(
            contar(copia.armazem().conexao(), t),
            contar(catalogo.armazem().conexao(), t),
            "{t}"
        );
    }
    assert_eq!(contar(copia.armazem().conexao(), "files"), 50);
}

#[test]
fn configuracao_de_hash_persiste_entre_aberturas() {
    use dag_core::OpcoesHash;
    use dag_store::configuracao;
    let dir = tempfile::tempdir().expect("tmp");
    let caminho = dir.path().join("catalogo.sqlite");
    {
        let c = Catalogo::abrir(&caminho).expect("abrir");
        let con = c.armazem().conexao();
        assert_eq!(configuracao::ler_hash(con).unwrap(), OpcoesHash::default());
        configuracao::gravar_hash(
            con,
            &OpcoesHash {
                threads_por_disco: Some(3),
                max_discos: None,
            },
        )
        .unwrap();
        configuracao::gravar_hash(
            con,
            &OpcoesHash {
                threads_por_disco: None,
                max_discos: Some(5),
            },
        )
        .unwrap();
    }
    let c = Catalogo::abrir(&caminho).expect("reabrir");
    let o = configuracao::ler_hash(c.armazem().conexao()).unwrap();
    assert_eq!((o.threads_por_disco, o.max_discos), (Some(3), Some(5)));
}
