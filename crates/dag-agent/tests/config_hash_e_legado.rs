//! Paralelismo do hash configurável (precedência, validação, persistência) e retomada do estado
//! que o binário antigo deixou (execução `concluida_com_erros` na fase `hash`, sem fila).
#![allow(clippy::unwrap_used)]

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use dag_agent::{Agente, PoliticaAbertura};
use dag_core::{Comando, ExecucaoId, ModoVarredura, OpcoesHash, Resposta, VolumeDescoberto};
use dag_store::Catalogo;

fn volume(raiz: &Path) -> VolumeDescoberto {
    let dev = fs::metadata(raiz).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("cfg-{dev}"),
        st_dev: dev,
        rotulo: Some("CFG".into()),
        fs_type: "teste".into(),
        capacidade_bytes: Some(1_000_000),
        livre_bytes: Some(400_000),
        pontos_de_montagem: vec![raiz.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: None,
    }
}

struct T {
    _tmp: tempfile::TempDir,
    raiz: PathBuf,
    catalogo: PathBuf,
}

fn arvore() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("acervo");
    for (n, b) in [("a/um.bin", 1u8), ("a/dois.bin", 2), ("b/tres.bin", 3)] {
        let p = raiz.join(n);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, vec![b; 4_000 + usize::from(b)]).unwrap();
    }
    let catalogo = tmp.path().join("dados/catalogo.sqlite");
    T {
        _tmp: tmp,
        raiz,
        catalogo,
    }
}

fn agente(t: &T) -> Agente {
    Agente::abrir(&t.catalogo)
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![volume(&t.raiz)])
}

fn cfg(a: &mut Agente, t: Option<u32>, d: Option<u32>) -> Resposta {
    a.executar(Comando::ConfigHash {
        threads_por_disco: t,
        max_discos: d,
    })
}

fn par(r: Resposta) -> (u32, u32) {
    match r {
        Resposta::ConfigHash {
            threads_por_disco,
            max_discos,
        } => (threads_por_disco, max_discos),
        r => panic!("{r:?}"),
    }
}

fn scan(t: &T, th: Option<u32>, md: Option<u32>) -> Comando {
    Comando::IniciarScan {
        modo: ModoVarredura::FullScan,
        raizes: vec![t.raiz.to_str().unwrap().into()],
        sem_hash: false,
        elevar: false,
        releitura_total: false,
        threads_por_disco: th,
        max_discos: md,
    }
}

fn n(t: &T, sql: &str) -> i64 {
    let cat = Catalogo::abrir(&t.catalogo).unwrap();
    cat.armazem()
        .conexao()
        .query_row(sql, [], |l| l.get(0))
        .unwrap()
}

#[test]
fn padrao_e_um_por_disco_e_persiste_entre_aberturas() {
    let t = arvore();
    {
        let mut a = agente(&t);
        assert_eq!(par(cfg(&mut a, None, None)), (1, 16));
        assert_eq!(par(cfg(&mut a, Some(3), None)), (3, 16));
        assert_eq!(par(cfg(&mut a, None, Some(5))), (3, 5));
    }
    // Round-trip: outro processo (nova abertura) lê o que foi gravado.
    let mut a = agente(&t);
    assert_eq!(par(cfg(&mut a, None, None)), (3, 5));
}

#[test]
fn valores_fora_da_faixa_dao_erro_claro_e_nao_gravam() {
    let t = arvore();
    let mut a = agente(&t);
    for (th, md) in [
        (Some(0), None),
        (Some(9), None),
        (None, Some(0)),
        (None, Some(17)),
    ] {
        match cfg(&mut a, th, md) {
            Resposta::Erro { mensagem, .. } => {
                assert!(mensagem.contains("entre 1 e"), "{mensagem}");
            }
            r => panic!("{r:?}"),
        }
    }
    assert_eq!(par(cfg(&mut a, None, None)), (1, 16));
    // Pedido de scan com valor inválido falha antes de varrer.
    match a.executar(scan(&t, Some(99), None)) {
        Resposta::Erro { mensagem, .. } => assert!(mensagem.contains("entre 1 e 8"), "{mensagem}"),
        r => panic!("{r:?}"),
    }
    assert_eq!(n(&t, "SELECT COUNT(*) FROM scan_runs"), 0);
    drop(a);
    assert!(
        Agente::abrir(&t.catalogo)
            .unwrap()
            .com_opcoes_hash(OpcoesHash {
                threads_por_disco: Some(0),
                max_discos: None
            })
            .is_err()
    );
}

#[test]
fn leitura_da_config_nao_exige_ser_escritor() {
    let t = arvore();
    {
        let mut a = agente(&t);
        cfg(&mut a, Some(2), Some(4));
    }
    let mut w = agente(&t);
    let mut leitor = Agente::abrir_leitura(&t.catalogo).unwrap();
    assert_eq!(par(cfg(&mut leitor, None, None)), (2, 4));
    // Gravar sem ser escritor é recusado.
    assert!(matches!(
        cfg(&mut leitor, Some(1), None),
        Resposta::Erro { .. }
    ));
    drop(w.executar(Comando::ListarVolumes));
}

#[test]
fn scan_com_pedido_agente_e_persistido_roda_e_aceita_todos() {
    // A precedência em si é testada em dag-core; aqui, a fiação ponta a ponta: persistido,
    // flag do agente e pedido válidos produzem hash completo.
    let t = arvore();
    let mut a = agente(&t)
        .com_opcoes_hash(OpcoesHash {
            threads_por_disco: Some(2),
            max_discos: None,
        })
        .unwrap();
    cfg(&mut a, Some(1), Some(3));
    let r = a.executar(scan(&t, Some(4), Some(2)));
    let Resposta::ScanConcluido { desfecho, hash, .. } = r else {
        panic!("{r:?}")
    };
    assert_eq!(desfecho, "concluida");
    assert_eq!(hash.unwrap().hasheados, 3);
}

/// Estado exato do binário antigo: execução `concluida_com_erros`, fase `hash`, arquivos sem
/// hash; a fila persistente pode ter linhas pendentes ou estar vazia.
fn estado_do_binario_antigo(t: &T, esvaziar_fila: bool) -> i64 {
    let id = {
        let mut a = agente(t);
        let r = a.executar(scan(t, None, None));
        let Resposta::ScanConcluido { execucao, .. } = r else {
            panic!("{r:?}")
        };
        execucao.0
    };
    let cat = Catalogo::abrir(&t.catalogo).unwrap();
    let con = cat.armazem().conexao();
    con.execute_batch(
        "DELETE FROM hashes;
         UPDATE scan_runs SET status = 'concluida_com_erros', phase = 'hash',
                parada_deliberada = 0, retomada_automatica = 0;
         UPDATE fila_hash SET state = 'pendente', bytes_lidos = 0;",
    )
    .unwrap();
    if esvaziar_fila {
        con.execute("DELETE FROM fila_hash", []).unwrap();
    }
    id
}

fn sem_hash(t: &T) -> i64 {
    n(
        t,
        "SELECT COUNT(*) FROM files f WHERE kind = 'arquivo'
         AND NOT EXISTS (SELECT 1 FROM hashes h WHERE h.file_id = f.id)",
    )
}

#[test]
fn abertura_retoma_o_hash_deixado_pelo_binario_antigo_com_fila_pendente_ou_vazia() {
    for esvaziar in [false, true] {
        let t = arvore();
        let id = estado_do_binario_antigo(&t, esvaziar);
        assert_eq!(sem_hash(&t), 3);
        let mut a = agente(&t);
        let rel = a
            .recuperar_ao_abrir(&PoliticaAbertura {
                verificar: dag_agent::VerificarAoAbrir::Nunca,
                ..PoliticaAbertura::default()
            })
            .unwrap();
        assert_eq!(rel.interrompidas, vec![id], "esvaziar_fila = {esvaziar}");
        assert_eq!(rel.retomadas, vec![id]);
        assert_eq!(sem_hash(&t), 0);
        let fase: String = Catalogo::abrir(&t.catalogo)
            .unwrap()
            .armazem()
            .conexao()
            .query_row("SELECT phase FROM scan_runs WHERE id = ?1", [id], |l| {
                l.get(0)
            })
            .unwrap();
        assert_eq!(fase, "concluida");
    }
}

#[test]
fn retomar_na_mao_tambem_funciona_para_o_estado_antigo() {
    let t = arvore();
    let id = estado_do_binario_antigo(&t, true);
    let mut a = agente(&t);
    let r = a.executar(Comando::Retomar {
        execucao: Some(ExecucaoId(id)),
        threads_por_disco: Some(2),
        max_discos: None,
    });
    let Resposta::ScanConcluido { hash, .. } = r else {
        panic!("{r:?}")
    };
    assert_eq!(hash.unwrap().hasheados, 3);
    assert_eq!(sem_hash(&t), 0);
}

#[test]
fn hash_cancelado_de_proposito_nao_retoma_sozinho() {
    let t = arvore();
    let id = estado_do_binario_antigo(&t, false);
    Catalogo::abrir(&t.catalogo)
        .unwrap()
        .armazem()
        .conexao()
        .execute(
            "UPDATE scan_runs SET parada_deliberada = 1 WHERE id = ?1",
            [id],
        )
        .unwrap();
    let mut a = agente(&t);
    let rel = a
        .recuperar_ao_abrir(&PoliticaAbertura {
            verificar: dag_agent::VerificarAoAbrir::Nunca,
            ..PoliticaAbertura::default()
        })
        .unwrap();
    assert!(rel.interrompidas.is_empty());
    assert_eq!(sem_hash(&t), 3);
}
