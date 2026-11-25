//! Escala da fila de hash: a reserva por volume usa índice (não varre a fila toda), nenhum disco
//! passa fome e o escritor acompanha centenas de milhares de itens.
#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use dag_core::ExecucaoId;
use dag_scanner::{ConfigHash, EtapaHash};
use dag_store::Catalogo;
use dag_store::hashes::{self, TipoFila};
use rusqlite::Connection;

/// Catálogo sintético: `discos` = (volume, dispositivo, arquivos), arquivos que não existem no
/// disco (a leitura falha rápido: o que se mede é o agendador e o escritor).
fn montar(discos: &[(i64, i64, i64)]) -> (Catalogo, i64) {
    let cat = Catalogo::em_memoria().unwrap();
    let con = cat.armazem().conexao();
    con.execute(
        "INSERT INTO scan_runs (mode, roots, started_at, status, phase)
         VALUES ('full', '[]', 1, 'concluida', 'hash')",
        [],
    )
    .unwrap();
    let run = con.last_insert_rowid();
    for (vol, dev, n) in discos {
        con.execute(
            "INSERT OR IGNORE INTO dispositivos (id, identificador, rotulo, criado_em)
             VALUES (?1, 'd' || ?1, 'd' || ?1, 1)",
            [dev],
        )
        .unwrap();
        con.execute(
            "INSERT INTO volumes (id, platform_id, device_id) VALUES (?1, 'v' || ?1, ?2)",
            [vol, dev],
        )
        .unwrap();
        con.execute(
            "INSERT INTO folders (id, volume_id, path_raw, path_display, name_raw, name_display,
                                  seen_run_id)
             VALUES (?1, ?1, CAST('/v' || ?1 AS BLOB), '/v' || ?1, CAST('v' AS BLOB), 'v', ?2)",
            [*vol, run],
        )
        .unwrap();
        con.execute(
            "WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM s WHERE i < ?3)
             INSERT INTO files (volume_id, folder_id, path_raw, path_display, name_raw,
                                name_display, logical_bytes, seen_run_id)
             SELECT ?1, ?1, CAST('/inexistente/v' || ?1 || '/f' || i AS BLOB),
                    '/inexistente/v' || ?1 || '/f' || i, CAST('f' || i AS BLOB), 'f' || i,
                    10, ?2 FROM s",
            [*vol, run, *n],
        )
        .unwrap();
    }
    (cat, run)
}

fn plano(con: &Connection, sql: &str) -> String {
    let mut st = con.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    st.query_map([], |l| l.get::<_, String>(3))
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>()
        .join(" | ")
}

#[test]
fn reserva_por_volume_usa_indice_e_nao_varre_a_fila() {
    // O volume 3 fica no fim da fila: sem índice por volume, cada reserva varreria 600 mil itens.
    let (mut cat, run) = montar(&[(1, 1, 300_000), (2, 2, 300_000), (3, 3, 2_000)]);
    let con = cat.armazem().conexao();
    hashes::preparar_fila_hash(con, run, 1).unwrap();
    let p = plano(
        con,
        "SELECT q.id FROM fila_hash q JOIN files f ON f.id = q.file_id
         WHERE q.tipo = 'hash' AND q.state = 'pendente' AND q.volume_id = 3
         ORDER BY q.priority DESC, q.id LIMIT 64",
    );
    assert!(p.contains("idx_fila_hash_volume"), "{p}");
    assert!(!p.contains("TEMP B-TREE"), "{p}");

    let t = Instant::now();
    let mut total = 0;
    for _ in 0..30 {
        let lote = cat
            .armazem_mut()
            .transacao_imediata(|tx| hashes::reservar_volumes(tx, TipoFila::Hash, &[3], 64, 2))
            .unwrap();
        assert_eq!(lote.len(), 64);
        assert!(lote.iter().all(|i| i.volume_id == 3));
        total += lote.len();
    }
    assert_eq!(total, 30 * 64);
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "30 reservas levaram {:?}",
        t.elapsed()
    );
}

#[test]
fn escritor_acompanha_200_mil_itens_em_3_discos_sem_deixar_disco_com_fome() {
    let (mut cat, run) = montar(&[(1, 1, 90_000), (2, 2, 90_000), (25, 3, 20_000)]);
    let t = Instant::now();
    let r = EtapaHash::nova(ExecucaoId(run))
        .com_config(ConfigHash {
            threads_por_disco: 1,
            ..ConfigHash::default()
        })
        .executar(&mut cat)
        .unwrap();
    let dur = t.elapsed();
    // Todos os 200 mil itens foram tentados (leitura falha: arquivo inexistente).
    assert_eq!(r.erros, 200_000);
    let por_volume: Vec<(i64, i64)> = {
        let con = cat.armazem().conexao();
        let mut st = con
            .prepare("SELECT volume_id, COUNT(*) FROM fila_hash WHERE state = 'erro' GROUP BY 1 ORDER BY 1")
            .unwrap();
        st.query_map([], |l| Ok((l.get(0)?, l.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(por_volume, vec![(1, 90_000), (2, 90_000), (25, 20_000)]);
    assert!(
        dur < Duration::from_secs(120),
        "200 mil itens levaram {dur:?}"
    );
}
