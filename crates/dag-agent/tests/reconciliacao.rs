//! Fast Scan, monitor e volumes offline pelo agente, com arquivos reais.
#![allow(clippy::unwrap_used)]

use std::fs::{self, File};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use dag_agent::{Agente, ErroAgente};
use dag_core::{
    Alvo, Comando, DadosRelatorio, EstadoMarcacao, ModoVarredura, Resposta, TipoRelatorio,
    VolumeDescoberto,
};
use tempfile::TempDir;

fn volume(raiz: &Path) -> VolumeDescoberto {
    let dev = fs::metadata(raiz).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("fast-{dev}"),
        st_dev: dev,
        rotulo: Some("FAST".into()),
        fs_type: "teste".into(),
        capacidade_bytes: Some(1_000_000),
        livre_bytes: Some(400_000),
        pontos_de_montagem: vec![raiz.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        dispositivo: None,
    }
}

fn escrever(p: &Path, c: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, c).unwrap();
}

struct T {
    _tmp: TempDir,
    raiz: PathBuf,
    catalogo: PathBuf,
}

/// raiz/{a/dup1, b/dup2 (iguais), c/unico}
fn arvore() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("acervo");
    escrever(&raiz.join("a/dup1.bin"), &[b'a'; 10_000]);
    escrever(&raiz.join("b/dup2.bin"), &[b'a'; 10_000]);
    escrever(&raiz.join("c/unico.bin"), &[b'u'; 7_000]);
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

fn full(t: &T) -> Comando {
    Comando::IniciarScan {
        modo: ModoVarredura::FullScan,
        raizes: vec![t.raiz.to_str().unwrap().into()],
        sem_hash: false,
    }
}

fn fast(raizes: Vec<String>) -> Comando {
    Comando::IniciarScan {
        modo: ModoVarredura::FastScan,
        raizes,
        sem_hash: false,
    }
}

fn contar(a: &Agente, sql: &str) -> i64 {
    a.catalogo()
        .armazem()
        .conexao()
        .query_row(sql, [], |l| l.get(0))
        .unwrap()
}

#[test]
fn ca_a09_a25_fast_scan_sem_mudancas_nao_le_conteudo_e_mantem_hashes() {
    let t = arvore();
    let mut a = agente(&t);
    a.executar(full(&t));
    let hashes = contar(&a, "SELECT COUNT(*) FROM hashes");
    assert_eq!(hashes, 3);
    let Resposta::ScanConcluido {
        hash,
        reconciliacao,
        desfecho,
        ..
    } = a.executar(fast(vec![]))
    else {
        panic!()
    };
    assert_eq!(desfecho, "concluida");
    let r = reconciliacao.unwrap();
    assert_eq!((r.arquivos_novos, r.arquivos_alterados), (0, 0));
    assert_eq!(r.arquivos_inalterados, 3);
    assert!(r.remocao_aplicada);
    let h = hash.unwrap();
    assert_eq!((h.hasheados, h.bytes_lidos), (0, 0));
    assert_eq!(contar(&a, "SELECT COUNT(*) FROM hashes"), hashes);
}

#[test]
fn ca_a24_a25_fast_scan_torna_marcacao_pendente_e_so_le_o_alterado() {
    let t = arvore();
    let mut a = agente(&t);
    a.executar(full(&t));
    let alvo = t.raiz.join("b/dup2.bin");
    let Resposta::Marcacao { resultado } = a.executar(Comando::MarcarDelecao {
        alvo: Alvo::Caminho(alvo.to_str().unwrap().into()),
    }) else {
        panic!()
    };
    let id = resultado.marcacoes[0].id;
    fs::write(&alvo, [b'Z'; 10_000]).unwrap();
    File::options()
        .write(true)
        .open(&alvo)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    let r = a.executar(fast(vec![t.raiz.to_str().unwrap().into()]));
    let Resposta::ScanConcluido {
        hash,
        duplicados,
        reconciliacao,
        ..
    } = r
    else {
        panic!("{r:?}")
    };
    assert_eq!(reconciliacao.unwrap().arquivos_alterados, 1);
    let h = hash.unwrap();
    assert_eq!((h.hasheados, h.bytes_lidos), (1, 10_000));
    let d = duplicados.unwrap();
    assert_eq!((d.grupos, d.marcacoes_pendentes), (0, 1));
    let Resposta::Relatorio {
        dados: DadosRelatorio::Marcacoes { marcacoes },
        ..
    } = a.executar(Comando::Relatorio {
        tipo: TipoRelatorio::Marcacoes,
        nivel: None,
        limite: None,
        cursor: None,
        saida: None,
    })
    else {
        panic!()
    };
    assert_eq!(marcacoes[0].estado, EstadoMarcacao::Pendente);
    let hist: Vec<String> = a
        .catalogo()
        .armazem()
        .conexao()
        .prepare("SELECT evento FROM marcacoes_historico ORDER BY id")
        .unwrap()
        .query_map([], |l| l.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(hist, ["criada", "pendente"]);
    let Resposta::Marcacao { resultado } = a.executar(Comando::ConfirmarMarcacao { marcacao: id })
    else {
        panic!()
    };
    assert_eq!(resultado.marcacoes[0].estado, EstadoMarcacao::Ativa);
}

#[test]
fn fast_scan_rename_preserva_id_e_remocao_marca_ausente() {
    let t = arvore();
    let mut a = agente(&t);
    a.executar(full(&t));
    let id_antes = contar(&a, "SELECT id FROM files WHERE name_display = 'unico.bin'");
    fs::rename(t.raiz.join("c/unico.bin"), t.raiz.join("c/renomeado.bin")).unwrap();
    fs::remove_file(t.raiz.join("a/dup1.bin")).unwrap();
    let Resposta::ScanConcluido {
        hash,
        reconciliacao,
        ..
    } = a.executar(fast(vec![]))
    else {
        panic!()
    };
    let r = reconciliacao.unwrap();
    assert_eq!((r.arquivos_renomeados, r.arquivos_ausentes), (1, 1));
    assert_eq!(hash.unwrap().bytes_lidos, 0);
    let id_depois = contar(
        &a,
        "SELECT id FROM files WHERE name_display = 'renomeado.bin'",
    );
    assert_eq!(id_antes, id_depois);
    assert_eq!(
        contar(
            &a,
            "SELECT COUNT(*) FROM files WHERE name_display = 'dup1.bin' AND state = 'ausente'"
        ),
        1
    );
    let Resposta::Eventos { eventos } = a.executar(Comando::Eventos { limite: None }) else {
        panic!()
    };
    assert!(eventos.iter().any(|e| e.tipo == "renomeado"));
    assert!(eventos.iter().any(|e| e.tipo == "removido"));
}

#[test]
fn fast_scan_sem_raizes_catalogadas_e_erro() {
    let t = arvore();
    let mut a = agente(&t);
    let Resposta::Erro { codigo, .. } = a.executar(fast(vec![])) else {
        panic!()
    };
    assert_eq!(codigo, "invalido");
}

#[test]
fn volume_offline_fica_indisponivel_e_volta_ao_reconectar() {
    let t = arvore();
    let mut a = agente(&t);
    a.executar(full(&t));
    drop(a);
    // Volume some da lista: listar marca mounted = 0 sem reescrever linhas.
    let mut off = Agente::abrir(&t.catalogo)
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![]);
    let antes = contar(&off, "SELECT COUNT(*) FROM files WHERE state = 'ativo'");
    off.executar(Comando::ListarVolumes);
    assert_eq!(
        contar(&off, "SELECT COUNT(*) FROM volumes WHERE mounted = 0"),
        1
    );
    assert_eq!(
        contar(
            &off,
            "SELECT COUNT(*) FROM v_arquivos WHERE estado_efetivo = 'indisponivel'"
        ),
        3
    );
    assert_eq!(
        contar(&off, "SELECT COUNT(*) FROM files WHERE state = 'ativo'"),
        antes
    );
    // Fast Scan sem raízes disponíveis: erro, nada marcado ausente.
    let Resposta::Erro { .. } = off.executar(fast(vec![])) else {
        panic!()
    };
    assert_eq!(
        contar(&off, "SELECT COUNT(*) FROM files WHERE state = 'ausente'"),
        0
    );
    let Resposta::Eventos { eventos } = off.executar(Comando::Eventos { limite: None }) else {
        panic!()
    };
    assert!(eventos.iter().any(|e| e.tipo == "desmontado"));
    drop(off);
    // Reconexão restaura a disponibilidade.
    let mut on = agente(&t);
    on.executar(Comando::ListarVolumes);
    assert_eq!(
        contar(&on, "SELECT COUNT(*) FROM volumes WHERE mounted = 0"),
        0
    );
    assert_eq!(
        contar(
            &on,
            "SELECT COUNT(*) FROM v_arquivos WHERE estado_efetivo = 'ativo'"
        ),
        3
    );
}

#[test]
fn ca_a10_a14_monitor_unico_escritor_e_reflete_mudancas() {
    let t = arvore();
    let mut a = agente(&t);
    a.executar(full(&t));
    let controle = a.controle();
    let raiz = t.raiz.to_str().unwrap().to_string();
    let h = std::thread::spawn(move || {
        let r = a.executar(Comando::Monitorar {
            raizes: vec![raiz],
            reconciliar_a_cada_s: Some(0),
            sem_hash: true,
        });
        (a, r)
    });
    // Enquanto monitora, o segundo agente não consegue virar escritor (CA-A10).
    let inicio = Instant::now();
    let mut ocupado = false;
    while inicio.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(100));
        if let Err(e) = Agente::abrir(&t.catalogo) {
            assert!(matches!(e, ErroAgente::EscritorOcupado(_)), "{e}");
            ocupado = true;
            break;
        }
    }
    assert!(ocupado);
    // Um leitor enxerga a saúde do monitor.
    let leitor = Agente::abrir_leitura(&t.catalogo).unwrap();
    let mut leitor = leitor.silencioso();
    let inicio = Instant::now();
    loop {
        let Resposta::SaudeMonitor { saude } = leitor.executar(Comando::SaudeMonitor) else {
            panic!()
        };
        if saude.em_execucao && saude.watches > 0 {
            break;
        }
        assert!(inicio.elapsed() < Duration::from_secs(10), "{saude:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
    // Um arquivo novo aparece no catálogo sem varredura manual (CA-A11/CA-A14).
    escrever(&t.raiz.join("c/novo.bin"), &[b'n'; 500]);
    let inicio = Instant::now();
    loop {
        let n: i64 = leitor
            .catalogo()
            .armazem()
            .conexao()
            .query_row(
                "SELECT COUNT(*) FROM files WHERE name_display = 'novo.bin'",
                [],
                |l| l.get(0),
            )
            .unwrap();
        if n == 1 {
            break;
        }
        assert!(inicio.elapsed() < Duration::from_secs(15), "sem evento");
        std::thread::sleep(Duration::from_millis(100));
    }
    controle.cancelar();
    let (a, r) = h.join().unwrap();
    let Resposta::MonitorEncerrado { resumo } = r else {
        panic!("{r:?}")
    };
    assert!(resumo.watches > 0);
    drop(a);
    // Encerrado: a trava foi solta e o status mostra o monitor parado.
    let mut b = Agente::abrir(&t.catalogo).unwrap().silencioso();
    let Resposta::SaudeMonitor { saude } = b.executar(Comando::SaudeMonitor) else {
        panic!()
    };
    assert!(!saude.em_execucao);
}
