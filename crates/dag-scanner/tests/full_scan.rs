//! Testes do Full Scan (CA-A03, CA-A04, CA-A05, CA-A14, CA-A15 e correlatos).
#![allow(clippy::unwrap_used)]

mod comum;

use std::fs::{self, File};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::Path;
use std::sync::{Arc, Mutex};

use comum::*;
use dag_scanner::{
    ConfigScan, ControleScan, DesfechoScan, ErroScan, Exclusao, FaseProgresso, FullScan,
};
use dag_store::Catalogo;
use dag_store::inventario::consulta::{
    estatisticas_da_pasta, estatisticas_do_volume, filhos_por_tamanho, listar_problemas,
    pastas_raiz,
};
use dag_store::inventario::{self, EstadoScan, FaseScan};
use tempfile::tempdir;

fn id_volume(c: &Catalogo) -> i64 {
    n(c, "SELECT id FROM volumes ORDER BY id LIMIT 1")
}

// ------------------------------------------------------------------ CA-A03

#[test]
fn full_scan_confere_contagens_caminhos_hierarquia_tamanhos_e_datas() {
    let tmp = tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    criar_arvore(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    let r = scan(&raiz).executar(&mut cat).unwrap();
    assert_eq!(r.desfecho, DesfechoScan::Concluida);

    let fs_ = percorrer_fs(&raiz);
    let fs_pastas: Vec<_> = fs_.iter().filter(|(_, m)| m.kind == "pasta").collect();
    let fs_arqs: Vec<_> = fs_.iter().filter(|(_, m)| m.kind != "pasta").collect();
    assert!(
        fs_arqs.len() > 2500,
        "tem milhares de arquivos: {}",
        fs_arqs.len()
    );
    assert!(fs_pastas.len() > 360);

    let f = foto(&cat);
    // Mesmos conjuntos de caminhos, em tabelas separadas.
    assert_eq!(
        conjunto(f.pastas.keys().cloned()),
        conjunto(fs_pastas.iter().map(|(p, _)| (*p).clone()))
    );
    assert_eq!(
        conjunto(f.arquivos.keys().cloned()),
        conjunto(fs_arqs.iter().map(|(p, _)| (*p).clone()))
    );
    assert_eq!(r.contadores.pastas as usize, fs_pastas.len());
    assert_eq!(r.contadores.arquivos as usize, fs_arqs.len());
    assert_eq!(
        n(&cat, "SELECT COUNT(*) FROM folders") as usize,
        fs_pastas.len()
    );
    assert_eq!(
        n(&cat, "SELECT COUNT(*) FROM files") as usize,
        fs_arqs.len()
    );

    // Hierarquia: o pai de cada pasta/arquivo é o diretório do caminho; a raiz não tem pai.
    let raiz_b = raiz.as_os_str().as_bytes().to_vec();
    for (p, pai) in &f.pastas {
        if *p == raiz_b {
            assert!(pai.is_none());
        } else {
            assert_eq!(
                pai.as_ref().unwrap(),
                &pais_de(p),
                "pai de {}",
                inventario::exibir(p)
            );
        }
    }
    let con = cat.armazem().conexao();
    let mut st = con
        .prepare("SELECT a.path_raw, p.path_raw, a.name_raw FROM files a JOIN folders p ON p.id = a.folder_id")
        .unwrap();
    let mut rows = st.query([]).unwrap();
    let mut conferidos = 0;
    while let Some(l) = rows.next().unwrap() {
        let (caminho, pasta, nome): (Vec<u8>, Vec<u8>, Vec<u8>) =
            (l.get(0).unwrap(), l.get(1).unwrap(), l.get(2).unwrap());
        assert_eq!(pais_de(&caminho), pasta);
        assert_eq!(nome_bruto(&caminho), nome.as_slice());
        conferidos += 1;
    }
    assert_eq!(conferidos, fs_arqs.len());

    // Tamanhos, alocação, nlink e datas conferem com o FS.
    let con = cat.armazem().conexao();
    let mut st = con
        .prepare("SELECT path_raw, logical_bytes, allocated_bytes, nlink, mtime_ns, ctime_ns, native_dev, native_file_id FROM files")
        .unwrap();
    let mut rows = st.query([]).unwrap();
    while let Some(l) = rows.next().unwrap() {
        let p: Vec<u8> = l.get(0).unwrap();
        let m = &fs_[&p];
        assert_eq!(l.get::<_, i64>(1).unwrap(), m.tamanho);
        assert_eq!(l.get::<_, Option<i64>>(2).unwrap(), Some(m.alocado));
        assert_eq!(l.get::<_, i64>(3).unwrap(), m.nlink);
        assert_eq!(l.get::<_, Option<i64>>(4).unwrap(), Some(m.mtime_ns));
        assert_eq!(l.get::<_, Option<i64>>(5).unwrap(), Some(m.ctime_ns));
        assert_eq!(l.get::<_, i64>(6).unwrap() as u64, m.dev);
        assert_eq!(l.get::<_, i64>(7).unwrap() as u64, m.ino);
    }
    // Datas conhecidas (nanossegundos preservados).
    let mt: i64 = con
        .query_row(
            "SELECT mtime_ns FROM files WHERE name_display = 'raiz3.dat'",
            [],
            |l| l.get(0),
        )
        .unwrap();
    assert_eq!(mt, 1_600_000_003 * 1_000_000_000 + 123_456_789);
    // mtime das pastas.
    let mut st = con
        .prepare("SELECT path_raw, mtime_ns FROM folders")
        .unwrap();
    let mut rows = st.query([]).unwrap();
    while let Some(l) = rows.next().unwrap() {
        let p: Vec<u8> = l.get(0).unwrap();
        assert_eq!(l.get::<_, Option<i64>>(1).unwrap(), Some(fs_[&p].mtime_ns));
    }

    // Pastas com nomes de pseudo-filesystem são varridas (decisão pelo flag, não pelo nome).
    for nome in ["proc", "sys", "dev"] {
        let p = raiz.join(nome).join("arquivo.txt");
        assert!(
            f.arquivos.contains_key(p.as_os_str().as_bytes()),
            "{nome} não foi varrida"
        );
    }

    // Agregados conferem com a soma independente do FS.
    let total_logico: i64 = fs_arqs.iter().map(|(_, m)| m.tamanho).sum();
    let total_alocado: i64 = fs_arqs.iter().map(|(_, m)| m.alocado).sum();
    let s = estatisticas_da_pasta(
        con,
        n(
            &cat,
            "SELECT id FROM folders WHERE parent_folder_id IS NULL",
        ),
    )
    .unwrap()
    .unwrap();
    assert_eq!(s.bytes_logicos, total_logico);
    assert_eq!(s.bytes_alocados_por_entrada, Some(total_alocado));
    assert_eq!(s.arquivos as usize, fs_arqs.len());
    assert_eq!(s.pastas as usize, fs_pastas.len() - 1);
    assert_eq!(s.especiais, 0);
    assert_eq!(s.estado, "completo");
    assert_eq!(
        s.bytes_fisicos_unicos,
        Some(total_alocado),
        "sem hard links, físico = por entrada"
    );
    // Uma subárvore: t2 (soma pelos caminhos).
    let t2 = raiz.join("t2");
    let t2_b = t2.as_os_str().as_bytes();
    let esperado: i64 = fs_arqs
        .iter()
        .filter(|(p, _)| p.starts_with(t2_b) && p[t2_b.len()] == b'/')
        .map(|(_, m)| m.tamanho)
        .sum();
    let t2_id = n(&cat, "SELECT id FROM folders WHERE name_display = 't2'");
    assert_eq!(
        estatisticas_da_pasta(con, t2_id)
            .unwrap()
            .unwrap()
            .bytes_logicos,
        esperado
    );

    // Volume e execução.
    let v = estatisticas_do_volume(con, id_volume(&cat))
        .unwrap()
        .unwrap();
    assert_eq!(v.arquivos as usize, fs_arqs.len());
    assert_eq!(v.pastas as usize, fs_pastas.len());
    assert_eq!(v.bytes_logicos, total_logico);
    assert_eq!(v.estado, "completo");
    let e = inventario::obter_execucao(con, r.execucao_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (e.estado, e.fase),
        (EstadoScan::Concluida, FaseScan::Concluida)
    );
    assert!(e.encerrada_em.is_some());
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_queue WHERE lease_state <> 'concluida'"
        ),
        0
    );
    cat.verificar_integridade().unwrap();
}

#[test]
fn varredura_repetida_no_mesmo_catalogo_nao_duplica() {
    let tmp = tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    criar_arvore(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    let r1 = scan(&raiz).executar(&mut cat).unwrap();
    let f1 = foto(&cat);
    escrever(&raiz.join("t0/novo.txt"), 123);
    let r2 = scan(&raiz).executar(&mut cat).unwrap();
    assert_ne!(r1.execucao_id, r2.execucao_id);
    let f2 = foto(&cat);
    assert_eq!(f2.pastas, f1.pastas);
    assert_eq!(f2.arquivos.len(), f1.arquivos.len() + 1);
    assert_eq!(n(&cat, "SELECT COUNT(*) FROM volumes"), 1);
    assert_eq!(
        n(
            &cat,
            &format!(
                "SELECT COUNT(*) FROM files WHERE seen_run_id = {}",
                r2.execucao_id
            )
        ),
        f2.arquivos.len() as i64
    );
}

// ------------------------------------------------------------------ CA-A04

#[test]
fn symlinks_em_laco_e_para_ancestral_terminam_sao_registrados_e_nao_seguidos() {
    let tmp = tempdir().unwrap();
    let raiz = tmp.path().join("raiz");
    let fora = tmp.path().join("fora");
    escrever(&raiz.join("a/arquivo.txt"), 10);
    fs::create_dir_all(raiz.join("a/b")).unwrap();
    escrever(&fora.join("segredo.txt"), 99);
    symlink(".", raiz.join("a/loop")).unwrap();
    symlink("../..", raiz.join("a/b/sobe")).unwrap();
    symlink(&raiz, raiz.join("a/absoluto")).unwrap();
    symlink("nao_existe", raiz.join("a/quebrado")).unwrap();
    symlink("arquivo.txt", raiz.join("a/para_arquivo")).unwrap();
    symlink(&fora, raiz.join("a/para_fora")).unwrap();

    let mut cat = Catalogo::em_memoria().unwrap();
    let r = scan(&raiz).executar(&mut cat).unwrap();
    assert_eq!(r.desfecho, DesfechoScan::Concluida);

    let f = foto(&cat);
    let esperadas = conjunto([
        raiz.as_os_str().as_bytes().to_vec(),
        raiz.join("a").as_os_str().as_bytes().to_vec(),
        raiz.join("a/b").as_os_str().as_bytes().to_vec(),
    ]);
    assert_eq!(
        conjunto(f.pastas.keys().cloned()),
        esperadas,
        "nenhuma pasta nasceu de symlink"
    );
    assert_eq!(
        n(&cat, "SELECT COUNT(*) FROM files WHERE kind = 'symlink'"),
        6
    );
    assert_eq!(
        n(&cat, "SELECT COUNT(*) FROM files WHERE kind = 'arquivo'"),
        1
    );
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM files WHERE path_raw LIKE '%segredo%'"
        ),
        0
    );
    // Alvos brutos preservados, sem resolver.
    let con = cat.armazem().conexao();
    let alvo = |nome: &str| -> Vec<u8> {
        con.query_row(
            "SELECT link_target_raw FROM files WHERE name_raw = ?1",
            [nome.as_bytes()],
            |l| l.get(0),
        )
        .unwrap()
    };
    assert_eq!(alvo("loop"), b".");
    assert_eq!(alvo("sobe"), b"../..");
    assert_eq!(alvo("quebrado"), b"nao_existe");
    assert_eq!(alvo("absoluto"), raiz.as_os_str().as_bytes());
    // Contagem: arquivos regulares e especiais separados; sem problemas.
    let s = estatisticas_da_pasta(
        con,
        n(
            &cat,
            "SELECT id FROM folders WHERE parent_folder_id IS NULL",
        ),
    )
    .unwrap()
    .unwrap();
    assert_eq!((s.arquivos, s.especiais, s.pastas), (1, 6, 2));
    assert_eq!(s.estado, "completo");
    assert_eq!(r.contadores.erros, 0);
}

#[test]
fn simbolico_cujo_alvo_e_tamanho_nao_alteram_o_alvo_real() {
    // O tamanho de um symlink é o do próprio link (comprimento do alvo), não do arquivo apontado.
    let tmp = tempdir().unwrap();
    let raiz = tmp.path().join("raiz");
    escrever(&raiz.join("grande.bin"), 100_000);
    symlink("grande.bin", raiz.join("link")).unwrap();
    let mut cat = Catalogo::em_memoria().unwrap();
    scan(&raiz).executar(&mut cat).unwrap();
    let (kind, bytes): (String, i64) = cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT kind, logical_bytes FROM files WHERE name_display = 'link'",
            [],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind, "symlink");
    assert_eq!(bytes, "grande.bin".len() as i64);
}

#[test]
fn hard_links_formam_um_grupo_e_uma_unidade_fisica() {
    let tmp = tempdir().unwrap();
    let raiz = tmp.path().join("raiz");
    escrever(&raiz.join("a/f1"), 4000);
    fs::create_dir_all(raiz.join("b")).unwrap();
    fs::hard_link(raiz.join("a/f1"), raiz.join("b/f2")).unwrap();
    fs::hard_link(raiz.join("a/f1"), raiz.join("b/f3")).unwrap();
    escrever(&raiz.join("c/normal"), 4000);
    let alocado = (fs::symlink_metadata(raiz.join("a/f1")).unwrap().blocks() * 512) as i64;
    assert!(alocado > 0);

    let mut cat = Catalogo::em_memoria().unwrap();
    let r = scan(&raiz).executar(&mut cat).unwrap();
    assert_eq!(r.agregacao.unwrap().grupos_hardlink, 1);
    let con = cat.armazem().conexao();
    let grupos: Vec<(String, Option<i64>, i64)> = {
        let mut st = con
            .prepare("SELECT name_display, hardlink_group, nlink FROM files ORDER BY name_display")
            .unwrap();
        st.query_map([], |l| Ok((l.get::<_, String>(0)?, l.get(1)?, l.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let g = grupos.iter().find(|x| x.0 == "f1").unwrap().1;
    assert!(g.is_some());
    for nome in ["f1", "f2", "f3"] {
        let x = grupos.iter().find(|x| x.0 == nome).unwrap();
        assert_eq!((x.1, x.2), (g, 3), "{nome}");
    }
    assert_eq!(grupos.iter().find(|x| x.0 == "normal").unwrap().1, None);

    let raiz_id = n(
        &cat,
        "SELECT id FROM folders WHERE parent_folder_id IS NULL",
    );
    let s = estatisticas_da_pasta(con, raiz_id).unwrap().unwrap();
    assert_eq!(s.arquivos, 4);
    assert_eq!(s.bytes_logicos, 4 * 4000, "soma lógica conta cada caminho");
    assert_eq!(s.bytes_alocados_por_entrada, Some(4 * alocado));
    assert_eq!(
        s.bytes_fisicos_unicos,
        Some(2 * alocado),
        "três caminhos, uma unidade física"
    );
    let b_id = n(&cat, "SELECT id FROM folders WHERE name_display = 'b'");
    let sb = estatisticas_da_pasta(con, b_id).unwrap().unwrap();
    assert_eq!(sb.bytes_alocados_por_entrada, Some(2 * alocado));
    assert_eq!(
        sb.bytes_fisicos_unicos,
        Some(alocado),
        "dois links em b contam uma vez"
    );
    let v = estatisticas_do_volume(con, id_volume(&cat))
        .unwrap()
        .unwrap();
    assert_eq!(v.bytes_fisicos_unicos, Some(2 * alocado));
}

// ------------------------------------------------------------------ CA-A05

#[test]
fn arquivo_esparso_tem_alocado_menor_que_logico_e_vazio_tem_zero() {
    use std::io::{Seek, SeekFrom, Write};
    let tmp = tempdir().unwrap();
    let raiz = tmp.path().join("raiz");
    fs::create_dir_all(&raiz).unwrap();
    let mut f = File::create(raiz.join("esparso.img")).unwrap();
    f.set_len(8 * 1024 * 1024).unwrap();
    f.seek(SeekFrom::Start(0)).unwrap();
    f.write_all(&[7u8; 4096]).unwrap();
    f.sync_all().unwrap();
    drop(f);
    escrever(&raiz.join("pequeno.txt"), 100);
    escrever(&raiz.join("vazio.txt"), 0);
    escrever(&raiz.join("denso.bin"), 200_000);

    let mut cat = Catalogo::em_memoria().unwrap();
    scan(&raiz).executar(&mut cat).unwrap();
    let con = cat.armazem().conexao();
    let dados = |nome: &str| -> (i64, Option<i64>) {
        con.query_row(
            "SELECT logical_bytes, allocated_bytes FROM files WHERE name_raw = ?1",
            [nome.as_bytes()],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )
        .unwrap()
    };
    let nativo = |nome: &str| fs::symlink_metadata(raiz.join(nome)).unwrap().blocks() as i64 * 512;
    let (logico, alocado) = dados("esparso.img");
    assert_eq!(logico, 8 * 1024 * 1024);
    assert_eq!(alocado, Some(nativo("esparso.img")));
    assert!(
        alocado.unwrap() < logico,
        "esparso: alocado {alocado:?} < lógico {logico}"
    );
    assert!(alocado.unwrap() >= 4096);
    assert_eq!(dados("vazio.txt"), (0, Some(0)));
    let (l, a) = dados("pequeno.txt");
    assert_eq!(l, 100);
    assert_eq!(a, Some(nativo("pequeno.txt")));
    assert!(
        a.unwrap() >= 100,
        "pequeno ocupa ao menos um bloco ou fica inline"
    );
    let (l, a) = dados("denso.bin");
    assert_eq!(l, 200_000);
    assert!(a.unwrap() >= 200_000);
    // Três métricas diferentes na pasta: lógico, por entrada e físico único.
    let s = estatisticas_da_pasta(con, n(&cat, "SELECT id FROM folders LIMIT 1"))
        .unwrap()
        .unwrap();
    assert!(s.bytes_alocados_por_entrada.unwrap() < s.bytes_logicos);
    assert_eq!(s.bytes_fisicos_unicos, s.bytes_alocados_por_entrada);
}

#[test]
fn birthtime_reflete_o_que_o_sistema_informa_ou_fica_nulo() {
    let tmp = tempdir().unwrap();
    let raiz = tmp.path().join("raiz");
    escrever(&raiz.join("a.txt"), 5);
    let esperado = fs::symlink_metadata(raiz.join("a.txt"))
        .unwrap()
        .created()
        .ok()
        .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as i64);
    let mut cat = Catalogo::em_memoria().unwrap();
    scan(&raiz).executar(&mut cat).unwrap();
    let b: Option<i64> = cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT birthtime_ns FROM files WHERE name_display = 'a.txt'",
            [],
            |l| l.get(0),
        )
        .unwrap();
    assert_eq!(b, esperado);
}

// ------------------------------------------------------------------ nomes, exclusões, raízes

#[test]
fn nome_nao_utf8_faz_ida_e_volta_nos_bytes() {
    use std::ffi::OsStr;
    let tmp = tempdir().unwrap();
    let raiz = tmp.path().join("raiz");
    let dir_ruim = raiz.join(OsStr::from_bytes(b"d\xfe\xfdir"));
    fs::create_dir_all(&dir_ruim).unwrap();
    let arq_ruim = dir_ruim.join(OsStr::from_bytes(b"n\xffo\x01.txt"));
    fs::write(&arq_ruim, b"abc").unwrap();
    fs::write(raiz.join(OsStr::from_bytes(b"ok-\xc3\xa7.txt")), b"x").unwrap();

    let mut cat = Catalogo::em_memoria().unwrap();
    scan(&raiz).executar(&mut cat).unwrap();
    let con = cat.armazem().conexao();
    let (raw, nome_raw, exibir): (Vec<u8>, Vec<u8>, String) = con
        .query_row(
            "SELECT path_raw, name_raw, name_display FROM files WHERE logical_bytes = 3",
            [],
            |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)),
        )
        .unwrap();
    assert_eq!(raw, arq_ruim.as_os_str().as_bytes());
    assert_eq!(nome_raw, b"n\xffo\x01.txt");
    assert_eq!(exibir, "n\\xffo\\x01.txt");
    let (pasta_raw, pasta_exibir): (Vec<u8>, String) = con
        .query_row(
            "SELECT path_raw, name_display FROM folders WHERE path_raw = ?1",
            [dir_ruim.as_os_str().as_bytes()],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )
        .unwrap();
    assert_eq!(pasta_raw, dir_ruim.as_os_str().as_bytes());
    assert_eq!(pasta_exibir, "d\\xfe\\xfdir");
    // UTF-8 válido passa direto.
    let ok: String = con
        .query_row(
            "SELECT name_display FROM files WHERE logical_bytes = 1",
            [],
            |l| l.get(0),
        )
        .unwrap();
    assert_eq!(ok, "ok-ç.txt");
    // Dá para achar a pasta pelo caminho bruto e listar os filhos.
    let id = dag_store::inventario::consulta::buscar_pasta(
        con,
        id_volume(&cat),
        dir_ruim.as_os_str().as_bytes(),
    )
    .unwrap()
    .unwrap();
    let filhos = filhos_por_tamanho(con, id, 10, None).unwrap();
    assert_eq!(filhos.itens.len(), 1);
    assert_eq!(filhos.itens[0].nome_raw, b"n\xffo\x01.txt");
}

#[test]
fn exclusoes_sao_registradas_com_justificativa_e_nao_entram_no_catalogo() {
    let tmp = tempdir().unwrap();
    let raiz = tmp.path().join("raiz");
    escrever(&raiz.join("proj/node_modules/pacote/x.js"), 10);
    escrever(&raiz.join("proj/src/main.rs"), 20);
    escrever(&raiz.join("cache/y.tmp"), 30);
    escrever(&raiz.join("manter.txt"), 5);
    let cfg = ConfigScan {
        exclusoes: vec![
            Exclusao::nome(b"node_modules".to_vec(), "dependências regeneráveis"),
            Exclusao::caminho(
                raiz.join("cache").as_os_str().as_bytes().to_vec(),
                "cache descartável",
            ),
        ],
        ..ConfigScan::default()
    };
    let mut cat = Catalogo::em_memoria().unwrap();
    let r = scan(&raiz).com_config(cfg).executar(&mut cat).unwrap();
    assert_eq!(r.desfecho, DesfechoScan::Concluida);
    let f = foto(&cat);
    assert!(
        f.arquivos
            .keys()
            .all(|p| !p.windows(12).any(|w| w == b"node_modules")
                && !p.windows(5).any(|w| w == b"cache"))
    );
    assert_eq!(f.arquivos.len(), 2);
    let con = cat.armazem().conexao();
    let ign = listar_problemas(con, r.execucao_id, 0, 10, Some(true)).unwrap();
    assert_eq!(ign.len(), 2);
    assert!(
        ign.iter()
            .all(|p| p.codigo == "excluido" && p.severidade == "info")
    );
    let motivos: Vec<_> = ign.iter().map(|p| p.motivo.clone().unwrap()).collect();
    assert!(motivos.contains(&"dependências regeneráveis".to_string()));
    assert!(motivos.contains(&"cache descartável".to_string()));
    assert_eq!(r.contadores.ignorados, 2);
    // Exclusão deliberada não torna o agregado parcial.
    let s = estatisticas_da_pasta(
        con,
        n(
            &cat,
            "SELECT id FROM folders WHERE parent_folder_id IS NULL",
        ),
    )
    .unwrap()
    .unwrap();
    assert_eq!(s.estado, "completo");
}

#[test]
fn raizes_invalidas_viram_problema_e_raizes_aninhadas_sao_deduplicadas() {
    let tmp = tempdir().unwrap();
    let raiz = tmp.path().join("raiz");
    escrever(&raiz.join("a/x.txt"), 1);
    escrever(&tmp.path().join("arquivo_solto.txt"), 1);
    let vols = vec![volume_de(&raiz)];

    let mut cat = Catalogo::em_memoria().unwrap();
    let r = FullScan::novo(
        vec![
            raiz.clone(),
            raiz.join("a"),
            tmp.path().join("nao_existe"),
            tmp.path().join("arquivo_solto.txt"),
        ],
        vols.clone(),
    )
    .executar(&mut cat)
    .unwrap();
    assert_eq!(r.desfecho, DesfechoScan::ConcluidaComErros);
    assert_eq!(
        pastas_raiz(cat.armazem().conexao(), id_volume(&cat))
            .unwrap()
            .len(),
        1
    );
    let probs = listar_problemas(cat.armazem().conexao(), r.execucao_id, 0, 10, None).unwrap();
    assert_eq!(
        probs
            .iter()
            .filter(|p| p.severidade == "impeditivo")
            .count(),
        2
    );
    assert!(
        probs
            .iter()
            .any(|p| p.codigo == "raiz_redundante" && p.severidade == "info")
    );
    assert!(probs.iter().any(|p| p.codigo == "raiz_nao_e_diretorio"));

    // Nenhuma raiz válida: erro e execução marcada como falha.
    let mut cat2 = Catalogo::em_memoria().unwrap();
    let e = FullScan::novo(vec![tmp.path().join("nada")], vols)
        .executar(&mut cat2)
        .unwrap_err();
    assert!(matches!(e, ErroScan::SemRaizes));
    assert_eq!(
        n(
            &cat2,
            "SELECT COUNT(*) FROM scan_runs WHERE status = 'falhou'"
        ),
        1
    );

    // Raiz em volume que a descoberta não conhece.
    let mut cat3 = Catalogo::em_memoria().unwrap();
    let e = FullScan::novo(vec![raiz.clone()], vec![])
        .executar(&mut cat3)
        .unwrap_err();
    assert!(matches!(e, ErroScan::SemRaizes));
    let p = listar_problemas(
        cat3.armazem().conexao(),
        n(&cat3, "SELECT id FROM scan_runs"),
        0,
        5,
        None,
    )
    .unwrap();
    assert_eq!(p[0].codigo, "raiz_sem_volume");
}

#[test]
fn raiz_em_volume_pseudo_e_recusada_pelo_flag() {
    let tmp = tempdir().unwrap();
    let raiz = tmp.path().join("proc"); // o nome não importa
    escrever(&raiz.join("x"), 1);
    let mut v = volume_de(&raiz);
    v.pseudo = true;
    let mut cat = Catalogo::em_memoria().unwrap();
    let e = FullScan::novo(vec![raiz], vec![v])
        .executar(&mut cat)
        .unwrap_err();
    assert!(matches!(e, ErroScan::SemRaizes));
    let p = listar_problemas(
        cat.armazem().conexao(),
        n(&cat, "SELECT id FROM scan_runs"),
        0,
        5,
        None,
    )
    .unwrap();
    assert_eq!(p[0].codigo, "raiz_pseudo_filesystem");
    assert_eq!(
        n(&cat, "SELECT COUNT(*) FROM volumes"),
        0,
        "pseudo-filesystem não vira volume do Catálogo"
    );
}

#[test]
fn diretorio_sem_permissao_e_registrado_e_o_scan_continua() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempdir().unwrap();
    // Como root o kernel ignora o modo 000: o teste real roda como usuário comum.
    if fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0) == 0 {
        eprintln!("PULADO: executando como root (CAP_DAC_OVERRIDE ignora permissões)");
        return;
    }
    let raiz = tmp.path().join("raiz");
    escrever(&raiz.join("ok/a.txt"), 1);
    escrever(&raiz.join("trancada/b.txt"), 1);
    escrever(&raiz.join("trancada/c/d.txt"), 1);
    fs::set_permissions(raiz.join("trancada"), fs::Permissions::from_mode(0o000)).unwrap();
    let mut cat = Catalogo::em_memoria().unwrap();
    let r = scan(&raiz).executar(&mut cat).unwrap();
    fs::set_permissions(raiz.join("trancada"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(r.desfecho, DesfechoScan::ConcluidaComAvisos);
    let probs =
        listar_problemas(cat.armazem().conexao(), r.execucao_id, 0, 10, Some(false)).unwrap();
    assert_eq!(probs.len(), 1);
    assert_eq!(probs[0].codigo, "permissao_negada");
    assert_eq!(
        probs[0].path_raw.as_deref(),
        Some(raiz.join("trancada").as_os_str().as_bytes())
    );
    let f = foto(&cat);
    assert!(
        f.arquivos
            .contains_key(raiz.join("ok/a.txt").as_os_str().as_bytes())
    );
    assert!(
        !f.arquivos
            .contains_key(raiz.join("trancada/b.txt").as_os_str().as_bytes())
    );
    let con = cat.armazem().conexao();
    let raiz_id = n(
        &cat,
        "SELECT id FROM folders WHERE parent_folder_id IS NULL",
    );
    assert_eq!(
        estatisticas_da_pasta(con, raiz_id).unwrap().unwrap().estado,
        "parcial"
    );
    let ok_id = n(&cat, "SELECT id FROM folders WHERE name_display = 'ok'");
    assert_eq!(
        estatisticas_da_pasta(con, ok_id).unwrap().unwrap().estado,
        "completo"
    );
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_queue WHERE lease_state = 'falhou'"
        ),
        1
    );
}

// ------------------------------------------------------------------ CA-A14: cancelar, pausar, retomar

fn arvore_media(tmp: &Path) -> std::path::PathBuf {
    let raiz = tmp.join("acervo");
    criar_arvore(&raiz);
    raiz
}

/// Cancela quando o progresso mostra pelo menos `pastas` pastas gravadas.
fn cancelar_apos(
    pastas: u64,
    controle: &ControleScan,
) -> impl Fn(&dag_scanner::Progresso) + Send + Sync + 'static {
    let c = controle.clone();
    move |p| {
        if p.pastas >= pastas && p.fase == FaseProgresso::Inventario {
            c.cancelar();
        }
    }
}

#[test]
fn cancelar_e_retomar_da_o_mesmo_resultado_de_uma_varredura_sem_interrupcao() {
    let tmp = tempdir().unwrap();
    let raiz = arvore_media(tmp.path());

    let mut ref_cat = Catalogo::em_memoria().unwrap();
    let ref_r = scan(&raiz).executar(&mut ref_cat).unwrap();
    let ref_foto = foto(&ref_cat);

    let mut cat = Catalogo::em_memoria().unwrap();
    let ctl = ControleScan::novo();
    let s1 = scan(&raiz)
        .com_config(config_miuda())
        .com_controle(ctl.clone())
        .com_progresso(cancelar_apos(30, &ctl));
    let r1 = s1.executar(&mut cat).unwrap();
    assert_eq!(r1.desfecho, DesfechoScan::Cancelada);
    assert!(r1.agregacao.is_none());
    let con = cat.armazem().conexao();
    let e = inventario::obter_execucao(con, r1.execucao_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (e.estado, e.fase),
        (EstadoScan::Cancelada, FaseScan::Inventario)
    );
    let fila = inventario::situacao_da_fila(con, r1.execucao_id).unwrap();
    assert!(fila.livres > 0, "sobrou trabalho na fila: {fila:?}");
    assert_eq!(fila.reservadas, 0);
    assert!(r1.contadores.pastas < ref_r.contadores.pastas);
    assert_eq!(
        n(&cat, "SELECT COUNT(*) FROM folder_stats"),
        0,
        "agregação só no fim"
    );

    // Retoma (outro objeto FullScan, controle novo, como depois de reabrir o app).
    let r2 = scan(&raiz)
        .com_config(config_miuda())
        .retomar(&mut cat, r1.execucao_id)
        .unwrap();
    assert_eq!(r2.execucao_id, r1.execucao_id);
    assert_eq!(r2.desfecho, DesfechoScan::Concluida);
    assert_eq!(r2.contadores.pastas, ref_r.contadores.pastas);
    assert_eq!(r2.contadores.arquivos, ref_r.contadores.arquivos);
    assert_eq!(r2.contadores.bytes_logicos, ref_r.contadores.bytes_logicos);
    assert_eq!(foto(&cat), ref_foto, "mesmas pastas, arquivos e agregados");
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_queue WHERE lease_state <> 'concluida'"
        ),
        0
    );
    let e = inventario::obter_execucao(cat.armazem().conexao(), r1.execucao_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (e.estado, e.fase),
        (EstadoScan::Concluida, FaseScan::Concluida)
    );
    // Retomar uma execução concluída é inofensivo.
    let r3 = scan(&raiz).retomar(&mut cat, r1.execucao_id).unwrap();
    assert_eq!(r3.contadores, r2.contadores);
}

#[test]
fn cancelar_varias_vezes_e_retomar_continua_correto() {
    let tmp = tempdir().unwrap();
    let raiz = arvore_media(tmp.path());
    let mut ref_cat = Catalogo::em_memoria().unwrap();
    scan(&raiz).executar(&mut ref_cat).unwrap();
    let ref_foto = foto(&ref_cat);

    let mut cat = Catalogo::em_memoria().unwrap();
    let ctl = ControleScan::novo();
    let r = scan(&raiz)
        .com_config(config_miuda())
        .com_controle(ctl.clone())
        .com_progresso(cancelar_apos(10, &ctl))
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r.desfecho, DesfechoScan::Cancelada);
    let id = r.execucao_id;
    let mut alvo = 10;
    for _ in 0..3 {
        alvo += 40;
        let ctl = ControleScan::novo();
        let r = scan(&raiz)
            .com_config(config_miuda())
            .com_controle(ctl.clone())
            .com_progresso(cancelar_apos(alvo, &ctl))
            .retomar(&mut cat, id)
            .unwrap();
        assert_eq!(r.desfecho, DesfechoScan::Cancelada);
    }
    let r = scan(&raiz)
        .com_config(config_miuda())
        .retomar(&mut cat, id)
        .unwrap();
    assert_eq!(r.desfecho, DesfechoScan::Concluida);
    assert_eq!(foto(&cat), ref_foto);
}

#[test]
fn retomada_apos_queda_abrupta_refaz_diretorios_reservados_sem_duplicar() {
    let tmp = tempdir().unwrap();
    let raiz = arvore_media(tmp.path());
    let mut ref_cat = Catalogo::em_memoria().unwrap();
    scan(&raiz).executar(&mut ref_cat).unwrap();
    let ref_foto = foto(&ref_cat);

    let mut cat = Catalogo::em_memoria().unwrap();
    let ctl = ControleScan::novo();
    let r1 = scan(&raiz)
        .com_config(config_miuda())
        .com_controle(ctl.clone())
        .com_progresso(cancelar_apos(60, &ctl))
        .executar(&mut cat)
        .unwrap();
    // Simula a morte do processo: estado "em_andamento", algumas reservas penduradas e alguns
    // diretórios já concluídos que voltam a ser reservados (a visita foi gravada pela metade).
    let con = cat.armazem().conexao();
    con.execute(
        "UPDATE scan_runs SET status = 'em_andamento', ended_at = NULL",
        [],
    )
    .unwrap();
    con.execute(
        "UPDATE scan_queue SET lease_state = 'reservada'
         WHERE id IN (SELECT id FROM scan_queue WHERE lease_state = 'livre' LIMIT 3)",
        [],
    )
    .unwrap();
    con.execute(
        "UPDATE scan_queue SET lease_state = 'reservada'
         WHERE id IN (SELECT id FROM scan_queue WHERE lease_state = 'concluida' LIMIT 20)",
        [],
    )
    .unwrap();
    assert_eq!(
        inventario::marcar_interrompidas(con, 4_000_000_000).unwrap(),
        1
    );
    let r2 = scan(&raiz)
        .com_config(config_miuda())
        .retomar(&mut cat, r1.execucao_id)
        .unwrap();
    assert_eq!(r2.desfecho, DesfechoScan::Concluida);
    assert_eq!(foto(&cat), ref_foto, "sem linhas duplicadas nem perdidas");
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_errors WHERE severity <> 'info'"
        ),
        0
    );
}

#[test]
fn pausar_persiste_o_estado_e_continuar_termina() {
    let tmp = tempdir().unwrap();
    let raiz = arvore_media(tmp.path());
    let catalogo_path = tmp.path().join("catalogo.sqlite");
    let mut cat = Catalogo::abrir(&catalogo_path).unwrap();
    let ctl = ControleScan::novo();
    let pausou = Arc::new(Mutex::new(false));
    let cb = {
        let ctl = ctl.clone();
        let pausou = Arc::clone(&pausou);
        move |p: &dag_scanner::Progresso| {
            let mut g = pausou.lock().unwrap();
            if !*g && p.pastas >= 20 && p.fase == FaseProgresso::Inventario {
                *g = true;
                ctl.pausar();
            }
        }
    };
    let observador = {
        let ctl = ctl.clone();
        let caminho = catalogo_path.clone();
        std::thread::spawn(move || {
            let leitor = Catalogo::abrir(&caminho).unwrap();
            let mut visto = None;
            for _ in 0..500 {
                let s: String = leitor
                    .armazem()
                    .conexao()
                    .query_row(
                        "SELECT status FROM scan_runs ORDER BY id DESC LIMIT 1",
                        [],
                        |l| l.get(0),
                    )
                    .unwrap_or_default();
                if s == "pausada" {
                    let c1: i64 = leitor
                        .armazem()
                        .conexao()
                        .query_row("SELECT COUNT(*) FROM folders", [], |l| l.get(0))
                        .unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    let c2: i64 = leitor
                        .armazem()
                        .conexao()
                        .query_row("SELECT COUNT(*) FROM folders", [], |l| l.get(0))
                        .unwrap();
                    visto = Some((c1, c2));
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            ctl.continuar();
            visto
        })
    };
    let r = scan(&raiz)
        .com_config(config_miuda())
        .com_controle(ctl)
        .com_progresso(cb)
        .executar(&mut cat)
        .unwrap();
    let (c1, c2) = observador.join().unwrap().expect("viu a execução pausada");
    assert_eq!(c1, c2, "nada é gravado enquanto pausado");
    assert_eq!(r.desfecho, DesfechoScan::Concluida);
    assert_eq!(
        r.contadores.pastas as usize,
        percorrer_fs(&raiz)
            .values()
            .filter(|m| m.kind == "pasta")
            .count()
    );
}

#[test]
fn progresso_e_monotonico_e_termina_em_concluido() {
    let tmp = tempdir().unwrap();
    let raiz = arvore_media(tmp.path());
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = Arc::clone(&log);
    let mut cat = Catalogo::em_memoria().unwrap();
    let r = scan(&raiz)
        .com_config(config_miuda())
        .com_progresso(move |p| l2.lock().unwrap().push(p.clone()))
        .executar(&mut cat)
        .unwrap();
    let log = log.lock().unwrap();
    assert!(log.len() > 5);
    assert!(
        log.windows(2)
            .all(|w| w[0].pastas <= w[1].pastas && w[0].arquivos <= w[1].arquivos)
    );
    let ultimo = log.last().unwrap();
    assert_eq!(ultimo.fase, FaseProgresso::Concluido);
    assert_eq!(ultimo.arquivos, r.contadores.arquivos);
    assert_eq!(ultimo.pastas, r.contadores.pastas);
    assert!(log.iter().any(|p| p.fase == FaseProgresso::Agregacao));
    assert!(log.iter().all(|p| p.execucao_id == r.execucao_id));
}

// ------------------------------------------------------------------ CA-A15

#[test]
fn varredura_nao_altera_tamanho_datas_nem_atime_de_nenhum_arquivo() {
    let tmp = tempdir().unwrap();
    let raiz = arvore_media(tmp.path());
    symlink("t0", raiz.join("lnk")).unwrap();
    fs::hard_link(raiz.join("raiz0.dat"), raiz.join("t0/hl.dat")).unwrap();
    let instantaneo = |raiz: &Path| -> std::collections::BTreeMap<Vec<u8>, (MetaFs, i64, u32)> {
        let mut m = std::collections::BTreeMap::new();
        for (p, meta) in percorrer_fs(raiz) {
            let lm = fs::symlink_metadata(Path::new(std::ffi::OsStr::from_bytes(&p))).unwrap();
            // Diretórios (readdir) e symlinks (readlink) podem ter atime atualizado pelo kernel
            // ao serem listados/lidos; arquivos regulares nunca são abertos.
            let atime = if meta.kind == "pasta" || meta.kind == "symlink" {
                0
            } else {
                lm.atime() * 1_000_000_000 + lm.atime_nsec()
            };
            m.insert(p, (meta, atime, lm.mode()));
        }
        m
    };
    let antes = instantaneo(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    scan(&raiz)
        .com_config(config_miuda())
        .executar(&mut cat)
        .unwrap();
    let depois = instantaneo(&raiz);
    assert_eq!(antes.len(), depois.len(), "nada foi criado nem removido");
    for (p, a) in &antes {
        let d = &depois[p];
        assert_eq!(a.0.tamanho, d.0.tamanho);
        assert_eq!(
            a.0.mtime_ns,
            d.0.mtime_ns,
            "mtime de {}",
            inventario::exibir(p)
        );
        assert_eq!(
            a.0.ctime_ns,
            d.0.ctime_ns,
            "ctime de {}",
            inventario::exibir(p)
        );
        assert_eq!(a.0.nlink, d.0.nlink);
        assert_eq!(a.2, d.2, "modo");
        assert_eq!(a.1, d.1, "atime (conteúdo nunca é lido)");
    }
}
