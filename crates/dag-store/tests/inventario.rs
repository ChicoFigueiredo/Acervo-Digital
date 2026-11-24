//! Testes do módulo de inventário: gravação idempotente, fila persistente, agregação e consulta.
#![allow(clippy::unwrap_used)]

use dag_core::{DispositivoDescoberto, TipoDispositivo, VolumeDescoberto};
use dag_store::Catalogo;
use dag_store::inventario::agregacao::{agregar_execucao, marcar_grupos_hardlink};
use dag_store::inventario::consulta::{
    TipoFilho, estatisticas_da_pasta, estatisticas_do_volume, filhos_por_tamanho, listar_problemas,
};
use dag_store::inventario::{
    self, EstadoScan, FimDiretorio, NovaPasta, NovoArquivo, NovoProblema, Severidade, TipoEntrada,
    exibir, registrar_volume,
};

fn volume_desc() -> VolumeDescoberto {
    VolumeDescoberto {
        platform_id: "uuid-1".into(),
        st_dev: 7,
        rotulo: Some("dados".into()),
        fs_type: "ext4".into(),
        capacidade_bytes: Some(1000),
        livre_bytes: Some(400),
        pontos_de_montagem: vec![b"/mnt/dados".to_vec(), b"/srv/\xff".to_vec()],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: Some(DispositivoDescoberto {
            identificador: "wwn-1".into(),
            modelo: Some("M".into()),
            tipo: TipoDispositivo::Ssd,
            removivel: false,
        }),
    }
}

fn pasta(c: &Catalogo, vol: i64, pai: Option<i64>, caminho: &[u8], run: i64) -> i64 {
    inventario::upsert_pasta(
        c.armazem().conexao(),
        &NovaPasta {
            volume_id: vol,
            pai,
            path_raw: caminho,
            name_raw: caminho.rsplit(|b| *b == b'/').next().unwrap(),
            dev: 7,
            inode: caminho.len() as u64,
            attrs: "{}",
            mtime_ns: Some(1),
            run_id: run,
        },
    )
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn arq(
    c: &Catalogo,
    vol: i64,
    pasta: i64,
    caminho: &[u8],
    ino: u64,
    nlink: u64,
    logico: u64,
    alocado: Option<u64>,
    run: i64,
) {
    inventario::upsert_arquivo(
        c.armazem().conexao(),
        &NovoArquivo {
            volume_id: vol,
            pasta_id: pasta,
            path_raw: caminho,
            name_raw: caminho.rsplit(|b| *b == b'/').next().unwrap(),
            tipo: TipoEntrada::Arquivo,
            dev: 7,
            inode: ino,
            nlink,
            bytes_logicos: logico,
            bytes_alocados: alocado,
            mtime_ns: Some(5),
            ctime_ns: Some(6),
            birthtime_ns: None,
            attrs: "{}",
            alvo_symlink: None,
            mime_id: None,
            run_id: run,
        },
    )
    .unwrap();
}

fn n(c: &Catalogo, sql: &str) -> i64 {
    c.armazem()
        .conexao()
        .query_row(sql, [], |l| l.get(0))
        .unwrap()
}

#[test]
fn exibir_escapa_bytes_invalidos_e_controles() {
    assert_eq!(exibir("ação.txt".as_bytes()), "ação.txt");
    assert_eq!(exibir(b"a\xffb"), "a\\xffb");
    assert_eq!(exibir(b"x\ny"), "x\\x0ay");
    assert_eq!(exibir(b"fim\xe2\x82"), "fim\\xe2\\x82");
    assert_eq!(exibir(b""), "");
}

#[test]
fn registrar_volume_e_idempotente_e_atualiza() {
    let c = Catalogo::em_memoria().unwrap();
    let con = c.armazem().conexao();
    let a = registrar_volume(con, &volume_desc(), 100).unwrap();
    let mut v = volume_desc();
    v.livre_bytes = Some(300);
    let b = registrar_volume(con, &v, 200).unwrap();
    assert_eq!(a, b);
    assert_eq!(n(&c, "SELECT COUNT(*) FROM volumes"), 1);
    assert_eq!(n(&c, "SELECT COUNT(*) FROM dispositivos"), 1);
    assert_eq!(n(&c, "SELECT COUNT(*) FROM mount_points"), 2);
    assert_eq!(n(&c, "SELECT free_bytes FROM volumes"), 300);
    assert_eq!(n(&c, "SELECT mounted FROM volumes"), 1);
    let tipo: String = con
        .query_row("SELECT tipo FROM dispositivos", [], |l| l.get(0))
        .unwrap();
    assert_eq!(tipo, "ssd");
    let mp: Vec<u8> = con
        .query_row(
            "SELECT path_raw FROM mount_points WHERE length(path_raw) = 6",
            [],
            |l| l.get(0),
        )
        .unwrap();
    assert_eq!(mp, b"/srv/\xff");
}

#[test]
fn upserts_nao_duplicam_e_symlink_guarda_alvo_bruto() {
    let c = Catalogo::em_memoria().unwrap();
    let con = c.armazem().conexao();
    let vol = registrar_volume(con, &volume_desc(), 1).unwrap();
    let run = inventario::criar_execucao(con, "full", "[]", None, 1).unwrap();
    let raiz = pasta(&c, vol, None, b"/r", run);
    let raiz2 = pasta(&c, vol, None, b"/r", run);
    assert_eq!(raiz, raiz2);
    for _ in 0..3 {
        arq(&c, vol, raiz, b"/r/a", 10, 1, 5, Some(4096), run);
    }
    inventario::upsert_arquivo(
        con,
        &NovoArquivo {
            volume_id: vol,
            pasta_id: raiz,
            path_raw: b"/r/lnk",
            name_raw: b"lnk",
            tipo: TipoEntrada::Symlink,
            dev: 7,
            inode: 11,
            nlink: 1,
            bytes_logicos: 3,
            bytes_alocados: Some(0),
            mtime_ns: None,
            ctime_ns: None,
            birthtime_ns: None,
            attrs: "{}",
            alvo_symlink: Some(b"..\xff"),
            mime_id: None,
            run_id: run,
        },
    )
    .unwrap();
    assert_eq!(n(&c, "SELECT COUNT(*) FROM files"), 2);
    let (kind, alvo): (String, Vec<u8>) = con
        .query_row(
            "SELECT kind, link_target_raw FROM files WHERE name_raw = x'6c6e6b'",
            [],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind, "symlink");
    assert_eq!(alvo, b"..\xff");
    // O CHECK de kind recusa valor desconhecido.
    assert!(con.execute("UPDATE files SET kind = 'xyz'", []).is_err());
}

#[test]
fn fila_persistente_reserva_conclui_e_libera() {
    let mut c = Catalogo::em_memoria().unwrap();
    let vol = registrar_volume(c.armazem().conexao(), &volume_desc(), 1).unwrap();
    let run = inventario::criar_execucao(c.armazem().conexao(), "full", "[]", None, 1).unwrap();
    let r = pasta(&c, vol, None, b"/r", run);
    let a = pasta(&c, vol, Some(r), b"/r/a", run);
    let b = pasta(&c, vol, Some(r), b"/r/b", run);
    let con = c.armazem().conexao();
    assert!(inventario::enfileirar_diretorio(con, run, r, b"/r", 1).unwrap());
    assert!(!inventario::enfileirar_diretorio(con, run, r, b"/r", 1).unwrap());
    inventario::enfileirar_diretorio(con, run, a, b"/r/a", 1).unwrap();
    inventario::enfileirar_diretorio(con, run, b, b"/r/b", 1).unwrap();
    let s = inventario::situacao_da_fila(con, run).unwrap();
    assert_eq!((s.livres, s.reservadas), (3, 0));

    let res = c
        .armazem_mut()
        .transacao_imediata(|tx| -> dag_store::Resultado<_> {
            inventario::reservar_diretorios(tx, run, 2)
        })
        .unwrap();
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].path_raw, b"/r/b", "os mais recentes primeiro");
    assert_eq!(res[0].dev, 7);
    let con = c.armazem().conexao();
    inventario::finalizar_diretorio(con, res[0].fila_id, &FimDiretorio::Concluido).unwrap();
    inventario::finalizar_diretorio(con, res[1].fila_id, &FimDiretorio::Falhou("x".into()))
        .unwrap();
    let s = inventario::situacao_da_fila(con, run).unwrap();
    assert_eq!((s.livres, s.concluidas, s.falhas), (1, 1, 1));
    // Reenfileirar diretório concluído não o reabre.
    assert!(!inventario::enfileirar_diretorio(con, run, b, b"/r/b", 1).unwrap());
    // Queda: o que estava reservada volta à fila.
    inventario::reservar_diretorios(con, run, 5).unwrap();
    assert_eq!(
        inventario::situacao_da_fila(con, run).unwrap().reservadas,
        1
    );
    assert_eq!(inventario::liberar_reservas(con, run).unwrap(), 1);
    assert_eq!(inventario::situacao_da_fila(con, run).unwrap().livres, 1);
}

#[test]
fn estados_da_execucao_e_interrompidas() {
    let c = Catalogo::em_memoria().unwrap();
    let con = c.armazem().conexao();
    let run = inventario::criar_execucao(con, "full", "[]", Some("{}"), 10).unwrap();
    let e = inventario::obter_execucao(con, run).unwrap().unwrap();
    assert_eq!(e.estado, EstadoScan::EmAndamento);
    assert_eq!(inventario::marcar_interrompidas(con, 20).unwrap(), 1);
    let e = inventario::obter_execucao(con, run).unwrap().unwrap();
    assert_eq!(e.estado, EstadoScan::Interrompida);
    assert_eq!(e.encerrada_em, Some(20));
    assert!(e.estado.retomavel());
    inventario::definir_estado(con, run, EstadoScan::EmAndamento, None, 30).unwrap();
    assert_eq!(
        inventario::obter_execucao(con, run)
            .unwrap()
            .unwrap()
            .encerrada_em,
        None
    );
    assert!(inventario::obter_execucao(con, 999).unwrap().is_none());
}

#[test]
fn problemas_sao_idempotentes_e_paginados() {
    let c = Catalogo::em_memoria().unwrap();
    let con = c.armazem().conexao();
    let run = inventario::criar_execucao(con, "full", "[]", None, 1).unwrap();
    for i in 0..5u8 {
        for _ in 0..2 {
            inventario::registrar_problema(
                con,
                &NovoProblema {
                    run_id: run,
                    pasta_id: None,
                    path_raw: Some(&[b'/', b'p', i]),
                    fase: "inventario",
                    codigo: "permissao_negada",
                    motivo: Some("negado"),
                    severidade: if i % 2 == 0 {
                        Severidade::Aviso
                    } else {
                        Severidade::Info
                    },
                    observado_em: 1,
                },
            )
            .unwrap();
        }
    }
    let todos = listar_problemas(con, run, 0, 100, None).unwrap();
    assert_eq!(todos.len(), 5);
    let pag1 = listar_problemas(con, run, 0, 2, None).unwrap();
    let pag2 = listar_problemas(con, run, pag1[1].id, 2, None).unwrap();
    assert_eq!(pag2[0].id, todos[2].id);
    assert_eq!(
        listar_problemas(con, run, 0, 100, Some(true))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        listar_problemas(con, run, 0, 100, Some(false))
            .unwrap()
            .len(),
        3
    );
    let c2 = inventario::contadores_da_execucao(con, run).unwrap();
    assert_eq!((c2.ignorados, c2.erros), (2, 3));
}

/// Árvore: /r { a/ { f1 (hl) f2 }, b/ { f3 (hl, mesmo inode de f1) f4 }, solto }
#[test]
fn agregacao_bottom_up_hardlinks_e_semantica_de_null() {
    let mut c = Catalogo::em_memoria().unwrap();
    let vol = registrar_volume(c.armazem().conexao(), &volume_desc(), 1).unwrap();
    let run = inventario::criar_execucao(c.armazem().conexao(), "full", "[]", None, 1).unwrap();
    let r = pasta(&c, vol, None, b"/r", run);
    let a = pasta(&c, vol, Some(r), b"/r/a", run);
    let b = pasta(&c, vol, Some(r), b"/r/b", run);
    let vazia = pasta(&c, vol, Some(r), b"/r/vazia", run);
    arq(&c, vol, a, b"/r/a/f1", 100, 2, 1000, Some(4096), run);
    arq(&c, vol, a, b"/r/a/f2", 101, 1, 10, Some(4096), run);
    arq(&c, vol, b, b"/r/b/f3", 100, 2, 1000, Some(4096), run);
    arq(&c, vol, b, b"/r/b/f4", 102, 1, 20, Some(4096), run);
    arq(&c, vol, r, b"/r/solto", 103, 1, 7, Some(4096), run);
    // Dois links do mesmo inode na mesma pasta: ainda um só.
    arq(&c, vol, a, b"/r/a/f1b", 100, 3, 1000, Some(4096), run);
    // Erro de acesso em b: b e r ficam parciais, a não.
    inventario::registrar_problema(
        c.armazem().conexao(),
        &NovoProblema {
            run_id: run,
            pasta_id: Some(b),
            path_raw: Some(b"/r/b/sub"),
            fase: "inventario",
            codigo: "permissao_negada",
            motivo: None,
            severidade: Severidade::Aviso,
            observado_em: 1,
        },
    )
    .unwrap();
    let resumo = agregar_execucao(c.armazem_mut().conexao_mut(), run, 50).unwrap();
    assert_eq!(resumo.pastas, 4);
    assert_eq!(resumo.grupos_hardlink, 1);
    assert_eq!(resumo.pastas_sem_agregado, 0);

    let con = c.armazem().conexao();
    let ga: Option<i64> = con
        .query_row(
            "SELECT hardlink_group FROM files WHERE name_raw = x'6631'",
            [],
            |l| l.get(0),
        )
        .unwrap();
    let gb: Option<i64> = con
        .query_row(
            "SELECT hardlink_group FROM files WHERE name_raw = x'6633'",
            [],
            |l| l.get(0),
        )
        .unwrap();
    assert!(ga.is_some());
    assert_eq!(ga, gb);
    assert_eq!(
        n(
            &c,
            "SELECT COUNT(*) FROM files WHERE hardlink_group IS NULL"
        ),
        3
    );

    let sa = estatisticas_da_pasta(con, a).unwrap().unwrap();
    assert_eq!((sa.arquivos, sa.pastas, sa.filhos_diretos), (3, 0, 3));
    assert_eq!(sa.bytes_logicos, 2010);
    assert_eq!(sa.bytes_alocados_por_entrada, Some(3 * 4096));
    assert_eq!(
        sa.bytes_fisicos_unicos,
        Some(2 * 4096),
        "f1 e f1b são o mesmo arquivo físico"
    );
    assert_eq!(sa.estado, "completo");

    let sr = estatisticas_da_pasta(con, r).unwrap().unwrap();
    assert_eq!((sr.arquivos, sr.pastas, sr.filhos_diretos), (6, 3, 4));
    assert_eq!(sr.bytes_logicos, 1000 + 10 + 1000 + 20 + 7 + 1000);
    assert_eq!(sr.bytes_alocados_por_entrada, Some(6 * 4096));
    // inodes distintos: 100 (uma vez), 101, 102, 103 → 4 unidades físicas.
    assert_eq!(sr.bytes_fisicos_unicos, Some(4 * 4096));
    assert_eq!(sr.estado, "parcial");
    assert_eq!(
        estatisticas_da_pasta(con, b).unwrap().unwrap().estado,
        "parcial"
    );
    assert_eq!(
        estatisticas_da_pasta(con, a).unwrap().unwrap().estado,
        "completo"
    );
    let sv = estatisticas_da_pasta(con, vazia).unwrap().unwrap();
    assert_eq!(
        (sv.arquivos, sv.bytes_logicos, sv.bytes_alocados_por_entrada),
        (0, 0, Some(0))
    );

    let vs = estatisticas_do_volume(con, vol).unwrap().unwrap();
    assert_eq!((vs.pastas, vs.arquivos), (4, 6));
    assert_eq!(vs.bytes_fisicos_unicos, Some(4 * 4096));
    assert_eq!(vs.problemas, 1);
    assert_eq!(vs.estado, "parcial");

    // Idempotente: rodar de novo dá o mesmo resultado.
    agregar_execucao(c.armazem_mut().conexao_mut(), run, 60).unwrap();
    assert_eq!(n(&c, "SELECT COUNT(*) FROM folder_stats"), 4);
    assert_eq!(
        marcar_grupos_hardlink(c.armazem().conexao(), run).unwrap(),
        1
    );
}

#[test]
fn alocado_indisponivel_vira_null_e_nunca_zero() {
    let mut c = Catalogo::em_memoria().unwrap();
    let vol = registrar_volume(c.armazem().conexao(), &volume_desc(), 1).unwrap();
    let run = inventario::criar_execucao(c.armazem().conexao(), "full", "[]", None, 1).unwrap();
    let r = pasta(&c, vol, None, b"/r", run);
    let a = pasta(&c, vol, Some(r), b"/r/a", run);
    arq(&c, vol, r, b"/r/ok", 1, 1, 10, Some(4096), run);
    arq(&c, vol, a, b"/r/a/sem", 2, 1, 10, None, run);
    agregar_execucao(c.armazem_mut().conexao_mut(), run, 1).unwrap();
    let con = c.armazem().conexao();
    let sa = estatisticas_da_pasta(con, a).unwrap().unwrap();
    assert_eq!(
        (
            sa.bytes_logicos,
            sa.bytes_alocados_por_entrada,
            sa.bytes_fisicos_unicos
        ),
        (10, None, None)
    );
    let sr = estatisticas_da_pasta(con, r).unwrap().unwrap();
    assert_eq!(
        (
            sr.bytes_logicos,
            sr.bytes_alocados_por_entrada,
            sr.bytes_fisicos_unicos
        ),
        (20, None, None)
    );
    let vs = estatisticas_do_volume(con, vol).unwrap().unwrap();
    assert_eq!(vs.bytes_alocados_por_entrada, None);
}

#[test]
fn filhos_por_tamanho_paginam_por_keyset_sem_repetir() {
    let mut c = Catalogo::em_memoria().unwrap();
    let vol = registrar_volume(c.armazem().conexao(), &volume_desc(), 1).unwrap();
    let run = inventario::criar_execucao(c.armazem().conexao(), "full", "[]", None, 1).unwrap();
    let r = pasta(&c, vol, None, b"/r", run);
    // 30 arquivos com tamanhos repetidos (empates) e 5 subpastas com conteúdo.
    for i in 0..30u64 {
        let nome = format!("/r/f{i:02}");
        arq(
            &c,
            vol,
            r,
            nome.as_bytes(),
            1000 + i,
            1,
            (i % 7) * 100,
            Some(4096),
            run,
        );
    }
    for i in 0..5u64 {
        let nome = format!("/r/d{i}");
        let p = pasta(&c, vol, Some(r), nome.as_bytes(), run);
        arq(
            &c,
            vol,
            p,
            format!("{nome}/x").as_bytes(),
            2000 + i,
            1,
            i * 100,
            Some(4096),
            run,
        );
    }
    agregar_execucao(c.armazem_mut().conexao_mut(), run, 1).unwrap();
    let con = c.armazem().conexao();

    let mut vistos = Vec::new();
    let mut cursor = None;
    let mut paginas = 0;
    loop {
        let p = filhos_por_tamanho(con, r, 8, cursor).unwrap();
        assert!(p.itens.len() <= 8);
        vistos.extend(p.itens);
        paginas += 1;
        match p.proximo {
            Some(c2) => cursor = Some(c2),
            None => break,
        }
    }
    assert_eq!(vistos.len(), 35);
    assert_eq!(paginas, 5);
    assert!(
        vistos
            .windows(2)
            .all(|w| w[0].bytes_logicos >= w[1].bytes_logicos)
    );
    let ids: std::collections::HashSet<_> = vistos
        .iter()
        .map(|f| (f.tipo == TipoFilho::Pasta, f.id))
        .collect();
    assert_eq!(ids.len(), 35, "sem repetição entre páginas");
    // Uma página só traz tudo e não tem próximo.
    let tudo = filhos_por_tamanho(con, r, 100, None).unwrap();
    assert_eq!(tudo.itens.len(), 35);
    assert!(tudo.proximo.is_none());
    assert_eq!(tudo.itens, vistos);
    // A pasta d4 (400 bytes) aparece com estado agregado.
    let d4 = vistos.iter().find(|f| f.nome_display == "d4").unwrap();
    assert_eq!((d4.tipo, d4.bytes_logicos), (TipoFilho::Pasta, 400));
    assert_eq!(d4.estado_agregado.as_deref(), Some("completo"));
}

#[test]
fn migracao_2_cria_o_que_o_inventario_precisa() {
    let c = Catalogo::em_memoria().unwrap();
    let con = c.armazem().conexao();
    for coluna in ["kind", "link_target_raw", "link_target_display"] {
        let n: i64 = con
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('files') WHERE name = ?1",
                [coluna],
                |l| l.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "files.{coluna}");
    }
    assert_eq!(
        n(
            &c,
            "SELECT COUNT(*) FROM pragma_table_info('folder_stats') WHERE name = 'recursive_special_count'"
        ),
        1
    );
    assert_eq!(
        n(
            &c,
            "SELECT COUNT(*) FROM pragma_table_info('scan_errors') WHERE name = 'folder_id'"
        ),
        1
    );
    assert_eq!(
        n(
            &c,
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'volume_stats'"
        ),
        1
    );
}
