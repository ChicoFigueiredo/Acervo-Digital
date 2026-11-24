//! Consultas da GUI sobre um Catálogo de fixture em disco: paginação, totais do computador sem
//! contar duas vezes um Volume com dois pontos de montagem, e nível de análise limitado.
#![allow(clippy::unwrap_used)]

use dag_core::{DispositivoDescoberto, TipoDispositivo, VolumeDescoberto};
use dag_store::Catalogo;
use dag_store::consulta_gui::{
    AlvoAnalise, ChaveNo, abrir_leitura, analisar, arquivos_da_pasta, listar_volumes, raizes,
    subpastas, totais_do_catalogo, trilha_da_pasta, ultimas_execucoes,
};
use dag_store::inventario::agregacao::agregar_execucao;
use dag_store::inventario::{self, NovaPasta, NovoArquivo, TipoEntrada, registrar_volume};
use tempfile::TempDir;

fn volume(uuid: &str, dev: u64, montagens: &[&[u8]]) -> VolumeDescoberto {
    VolumeDescoberto {
        platform_id: uuid.into(),
        st_dev: dev,
        rotulo: Some(format!("vol-{uuid}")),
        fs_type: "ext4".into(),
        capacidade_bytes: Some(10_000),
        livre_bytes: Some(4_000),
        pontos_de_montagem: montagens.iter().map(|m| m.to_vec()).collect(),
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: Some(DispositivoDescoberto {
            identificador: format!("wwn-{uuid}"),
            modelo: Some("Modelo".into()),
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
            inode: caminho.len() as u64 + vol as u64 * 1000,
            attrs: "{}",
            mtime_ns: Some(1),
            run_id: run,
        },
    )
    .unwrap()
}

fn arq(c: &Catalogo, vol: i64, pasta: i64, caminho: &[u8], ino: u64, bytes: u64, run: i64) {
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
            nlink: 1,
            bytes_logicos: bytes,
            bytes_alocados: Some(bytes),
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

struct Fixture {
    _tmp: TempDir,
    caminho: std::path::PathBuf,
    raiz1: i64,
    sub_a: i64,
    vol1: i64,
    vol2: i64,
}

/// Volume 1 (montado em dois lugares, bind mount): /r com sub0..sub6 (100, 200, ... 700 bytes)
/// e 5 arquivos soltos. Volume 2: /s com um arquivo de 50 bytes.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let caminho = tmp.path().join("catalogo.sqlite");
    let mut c = Catalogo::abrir(&caminho).unwrap();
    let con = c.armazem().conexao();
    let vol1 = registrar_volume(con, &volume("u1", 7, &[b"/mnt/dados", b"/srv/dados"]), 1).unwrap();
    let vol2 = registrar_volume(con, &volume("u2", 8, &[b"/mnt/outro"]), 1).unwrap();
    let run = inventario::criar_execucao(con, "full", r#"["/r","/s"]"#, None, 1).unwrap();
    let r = pasta(&c, vol1, None, b"/r", run);
    let mut sub_a = 0;
    for i in 0..7u64 {
        let p = pasta(&c, vol1, Some(r), format!("/r/sub{i}").as_bytes(), run);
        if i == 0 {
            sub_a = p;
        }
        arq(
            &c,
            vol1,
            p,
            format!("/r/sub{i}/f").as_bytes(),
            100 + i,
            (i + 1) * 100,
            run,
        );
    }
    for i in 0..5u64 {
        arq(
            &c,
            vol1,
            r,
            format!("/r/solto{i}").as_bytes(),
            200 + i,
            (i + 1) * 10,
            run,
        );
    }
    let s = pasta(&c, vol2, None, b"/s", run);
    arq(&c, vol2, s, b"/s/x", 300, 50, run);
    agregar_execucao(c.armazem_mut().conexao_mut(), run, 50).unwrap();
    Fixture {
        _tmp: tmp,
        caminho,
        raiz1: r,
        sub_a,
        vol1,
        vol2,
    }
}

const TOTAL_V1: u64 = 2800 + 150;
const TOTAL_V2: u64 = 50;

#[test]
fn conexao_de_leitura_nao_escreve() {
    let f = fixture();
    let conn = abrir_leitura(&f.caminho).unwrap();
    assert!(conn.execute("DELETE FROM files", []).is_err());
}

#[test]
fn subpastas_paginam_do_maior_para_o_menor_sem_repetir() {
    let f = fixture();
    let conn = abrir_leitura(&f.caminho).unwrap();
    let mut vistos = Vec::new();
    let mut cursor = None;
    let mut paginas = 0;
    loop {
        let p = subpastas(&conn, f.raiz1, cursor, 3).unwrap();
        assert!(p.itens.len() <= 3);
        vistos.extend(p.itens.iter().map(|i| (i.nome.clone(), i.bytes_logicos)));
        paginas += 1;
        match p.proximo {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(paginas, 3, "7 pastas em páginas de 3");
    assert_eq!(vistos.len(), 7);
    assert_eq!(vistos[0], ("sub6".to_string(), 700));
    assert_eq!(vistos[6], ("sub0".to_string(), 100));
    assert!(vistos.windows(2).all(|w| w[0].1 >= w[1].1));
    let mut nomes: Vec<_> = vistos.iter().map(|v| v.0.clone()).collect();
    nomes.sort();
    nomes.dedup();
    assert_eq!(nomes.len(), 7);
}

#[test]
fn limite_de_pagina_e_limitado_a_500() {
    let f = fixture();
    let conn = abrir_leitura(&f.caminho).unwrap();
    let p = subpastas(&conn, f.raiz1, None, 1_000_000).unwrap();
    assert_eq!(p.itens.len(), 7);
    assert!(p.proximo.is_none());
}

#[test]
fn arquivos_paginam_por_keyset() {
    let f = fixture();
    let conn = abrir_leitura(&f.caminho).unwrap();
    let p1 = arquivos_da_pasta(&conn, f.raiz1, None, 2).unwrap();
    assert_eq!(p1.itens.len(), 2);
    assert_eq!(p1.itens[0].bytes_logicos, 50);
    let p2 = arquivos_da_pasta(&conn, f.raiz1, p1.proximo, 2).unwrap();
    assert_eq!(
        p2.itens.iter().map(|a| a.bytes_logicos).collect::<Vec<_>>(),
        [30, 20]
    );
    let p3 = arquivos_da_pasta(&conn, f.raiz1, p2.proximo, 2).unwrap();
    assert_eq!(p3.itens.len(), 1);
    assert!(p3.proximo.is_none());
    assert!(!p3.itens[0].tem_hash && !p3.itens[0].duplicado && p3.itens[0].marcacao.is_none());
}

#[test]
fn raizes_listam_uma_por_volume_catalogado() {
    let f = fixture();
    let conn = abrir_leitura(&f.caminho).unwrap();
    let r = raizes(&conn).unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!((r[0].nome.as_str(), r[0].bytes_logicos), ("r", TOTAL_V1));
    assert!(r[0].tem_subpastas && !r[1].tem_subpastas);
}

#[test]
fn computador_nao_conta_duas_vezes_volume_com_dois_pontos_de_montagem() {
    let f = fixture();
    let conn = abrir_leitura(&f.caminho).unwrap();
    let vols = listar_volumes(&conn).unwrap();
    assert_eq!(vols.len(), 2, "um Volume por platform_id");
    let v1 = vols.iter().find(|v| v.id == f.vol1).unwrap();
    assert_eq!(v1.montagens, 2);
    assert_eq!(
        v1.montagem.as_deref(),
        Some("/mnt/dados"),
        "o ponto principal é o mais curto"
    );
    assert_eq!(v1.uso_percentual(), Some(60.0));
    assert_eq!(v1.bytes_catalogados, Some(TOTAL_V1));

    let t = totais_do_catalogo(&conn).unwrap();
    assert_eq!(t.volumes, 2);
    assert_eq!(t.bytes_logicos, TOTAL_V1 + TOTAL_V2);

    let a = analisar(&conn, AlvoAnalise::Computador, 50).unwrap();
    assert_eq!(a.bytes_nivel, TOTAL_V1 + TOTAL_V2);
    assert_eq!(a.bytes_computador, TOTAL_V1 + TOTAL_V2);
    assert_eq!(a.filhos.len(), 2);
    assert_eq!(
        a.filhos.iter().map(|x| x.bytes).sum::<u64>(),
        TOTAL_V1 + TOTAL_V2
    );
    assert_eq!(a.outros_bytes, 0);
    assert_eq!(a.filhos[0].chave, ChaveNo::Volume(f.vol1));
    assert_eq!(a.filhos[1].chave, ChaveNo::Volume(f.vol2));
    assert!(a.bytes_volume.is_none());
}

#[test]
fn analise_do_volume_e_da_pasta_trazem_bases_e_trilha() {
    let f = fixture();
    let conn = abrir_leitura(&f.caminho).unwrap();
    let av = analisar(&conn, AlvoAnalise::Volume(f.vol2), 10).unwrap();
    assert_eq!(av.bytes_nivel, TOTAL_V2);
    assert_eq!(av.bytes_volume, Some(TOTAL_V2));
    assert_eq!(av.bytes_computador, TOTAL_V1 + TOTAL_V2);
    assert_eq!(av.filhos.len(), 1);

    let ap = analisar(&conn, AlvoAnalise::Pasta(f.sub_a), 10).unwrap();
    assert_eq!(ap.bytes_nivel, 100);
    assert_eq!(ap.bytes_volume, Some(TOTAL_V1));
    assert_eq!(ap.profundidade, 1);
    let nomes: Vec<_> = ap.trilha.iter().map(|e| e.nome.as_str()).collect();
    assert_eq!(nomes, ["Computador", "vol-u1", "r", "sub0"]);
    assert_eq!(ap.filhos.len(), 1);
    assert!(matches!(ap.filhos[0].chave, ChaveNo::Arquivo(_)));
}

#[test]
fn nivel_limitado_agrupa_a_cauda_em_outros() {
    let f = fixture();
    let conn = abrir_leitura(&f.caminho).unwrap();
    let a = analisar(&conn, AlvoAnalise::Pasta(f.raiz1), 4).unwrap();
    assert_eq!(a.filhos.len(), 4);
    assert!(a.truncado);
    let mostrado: u64 = a.filhos.iter().map(|x| x.bytes).sum();
    assert_eq!(mostrado, 700 + 600 + 500 + 400);
    assert_eq!(a.outros_bytes, TOTAL_V1 - mostrado);
    assert!(a.filhos.windows(2).all(|w| w[0].bytes >= w[1].bytes));

    let tudo = analisar(&conn, AlvoAnalise::Pasta(f.raiz1), 100).unwrap();
    assert!(!tudo.truncado);
    assert_eq!(tudo.filhos.len(), 12);
    assert_eq!(tudo.outros_bytes, 0);
}

#[test]
fn trilha_vai_da_raiz_ate_a_pasta() {
    let f = fixture();
    let conn = abrir_leitura(&f.caminho).unwrap();
    let t = trilha_da_pasta(&conn, f.sub_a).unwrap();
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].id, f.raiz1);
    assert_eq!(t[1].id, f.sub_a);
}

#[test]
fn ultimas_execucoes_mostram_contadores() {
    let f = fixture();
    let conn = abrir_leitura(&f.caminho).unwrap();
    let e = ultimas_execucoes(&conn, 5).unwrap();
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].modo, "full");
    assert_eq!(e[0].raizes, ["/r", "/s"]);
}
