//! Lixeira em dois níveis (ADR 0005) com arquivos reais em diretórios temporários: mover e
//! restaurar, `RENAME_NOREPLACE`, Volume diferente, arquivo que mudou, Marcação, Única Cópia,
//! pastas, e a reconciliação do que o usuário faz à mão. Nada aqui toca caminhos do usuário.
#![allow(clippy::unwrap_used)]

use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dag_agent::Agente;
use dag_agent::lixeira::Dispositivos;
use dag_core::{
    Alvo, Comando, DispositivoDescoberto, ModoVarredura, Resposta, ResultadoLixeira,
    TipoDispositivo, VolumeDescoberto,
};
use tempfile::TempDir;

fn volume(raiz: &Path, rotulo: &str, disco: &str) -> VolumeDescoberto {
    let dev = fs::metadata(raiz).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("{rotulo}-{dev}"),
        st_dev: dev,
        rotulo: Some(rotulo.into()),
        fs_type: "teste".into(),
        capacidade_bytes: Some(1_000_000),
        livre_bytes: Some(400_000),
        pontos_de_montagem: vec![raiz.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: Some(DispositivoDescoberto {
            identificador: disco.into(),
            modelo: None,
            tipo: TipoDispositivo::Ssd,
            removivel: false,
        }),
    }
}

fn escrever(p: &Path, conteudo: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, conteudo).unwrap();
}

struct Arvore {
    _tmp: TempDir,
    raiz: PathBuf,
    catalogo: PathBuf,
}

/// raiz/{a/dup1.bin, b/dup2.bin (iguais), c/unico.bin, pasta/{x.bin, y.bin}, h/{l1,l2} hard links}
fn arvore() -> Arvore {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("acervo");
    escrever(&raiz.join("a/dup1.bin"), &[b'a'; 10_000]);
    escrever(&raiz.join("b/dup2.bin"), &[b'a'; 10_000]);
    escrever(&raiz.join("c/unico.bin"), &[b'u'; 7_000]);
    escrever(&raiz.join("pasta/x.bin"), &[b'x'; 3_000]);
    escrever(&raiz.join("pasta/y.bin"), &[b'y'; 4_000]);
    escrever(&raiz.join("h/l1.bin"), &[b'h'; 2_000]);
    fs::hard_link(raiz.join("h/l1.bin"), raiz.join("h/l2.bin")).unwrap();
    let catalogo = tmp.path().join("dados/catalogo.sqlite");
    Arvore {
        _tmp: tmp,
        raiz,
        catalogo,
    }
}

fn agente(t: &Arvore) -> Agente {
    Agente::abrir(&t.catalogo)
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![volume(&t.raiz, "SSD", "disco-ssd")])
}

fn scan(t: &Arvore, modo: ModoVarredura) -> Comando {
    Comando::IniciarScan {
        modo,
        raizes: vec![t.raiz.to_str().unwrap().to_string()],
        sem_hash: false,
        elevar: false,
        releitura_total: false,
        threads_por_disco: None,
        max_discos: None,
    }
}

fn p(t: &Arvore, rel: &str) -> String {
    t.raiz.join(rel).to_str().unwrap().to_string()
}

fn caminho(t: &Arvore, rel: &str) -> Alvo {
    Alvo::Caminho(p(t, rel))
}

fn marcar(a: &mut Agente, alvo: Alvo) {
    let r = a.executar(Comando::MarcarDelecao { alvo });
    assert!(matches!(r, Resposta::Marcacao { .. }), "{r:?}");
}

fn mover(
    a: &mut Agente,
    alvos: Vec<Alvo>,
    unica: bool,
    tudo: bool,
    simular: bool,
) -> ResultadoLixeira {
    let r = a.executar(Comando::MoverParaLixeira {
        alvos,
        confirmacao_unica_copia: unica,
        marcar_tudo: tudo,
        simular,
    });
    let Resposta::Lixeira { resultado } = r else {
        panic!("{r:?}")
    };
    resultado
}

fn restaurar(a: &mut Agente, alvos: Vec<Alvo>) -> ResultadoLixeira {
    let r = a.executar(Comando::RestaurarDaLixeira { alvos });
    let Resposta::Lixeira { resultado } = r else {
        panic!("{r:?}")
    };
    resultado
}

fn lixeira(a: &mut Agente) -> (Vec<dag_core::ItemLixeira>, u64, u64) {
    let r = a.executar(Comando::ListarLixeira {
        limite: None,
        cursor: None,
    });
    let Resposta::ListaLixeira {
        itens,
        total,
        descartados,
        ..
    } = r
    else {
        panic!("{r:?}")
    };
    (itens, total, descartados)
}

fn n_grupos(a: &mut Agente) -> usize {
    let r = a.executar(Comando::ListarDuplicados {
        volume: None,
        tamanho_minimo: None,
        ordem: None,
        limite: None,
        cursor: None,
    });
    let Resposta::Duplicados { grupos, .. } = r else {
        panic!("{r:?}")
    };
    grupos.len()
}

fn pronto() -> (Arvore, Agente) {
    let t = arvore();
    let mut a = agente(&t);
    let r = a.executar(scan(&t, ModoVarredura::FullScan));
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    (t, a)
}

fn codigos(r: &ResultadoLixeira) -> Vec<&str> {
    r.recusados.iter().map(|x| x.codigo.as_str()).collect()
}

#[test]
fn mover_e_restaurar_ida_e_volta() {
    let (t, mut a) = pronto();
    assert_eq!(n_grupos(&mut a), 1);
    marcar(&mut a, caminho(&t, "b/dup2.bin"));
    let ino = fs::metadata(t.raiz.join("b/dup2.bin")).unwrap().ino();
    let r = mover(&mut a, vec![caminho(&t, "b/dup2.bin")], false, false, false);
    assert!(r.recusados.is_empty(), "{r:?}");
    assert_eq!((r.total_arquivos, r.total_bytes), (1, 10_000));
    let destino = t.raiz.join("DAG-Lixeira/b/dup2.bin");
    assert!(!t.raiz.join("b/dup2.bin").exists());
    assert_eq!(fs::read(&destino).unwrap(), vec![b'a'; 10_000]);
    assert_eq!(
        fs::metadata(&destino).unwrap().ino(),
        ino,
        "rename: mesmo inode"
    );
    // Catálogo: Na Lixeira, com origem; a cópia lá não forma grupo.
    let (itens, total, _) = lixeira(&mut a);
    assert_eq!(total, 1);
    assert!(itens[0].caminho.ends_with("DAG-Lixeira/b/dup2.bin"));
    assert!(itens[0].origem.ends_with("/b/dup2.bin") && !itens[0].origem.contains("DAG-Lixeira"));
    assert_eq!(n_grupos(&mut a), 0, "cópia Na Lixeira não conta como cópia");
    // Restaurar devolve ao lugar e o grupo volta.
    let r = restaurar(&mut a, vec![caminho(&t, "DAG-Lixeira/b/dup2.bin")]);
    assert!(r.recusados.is_empty(), "{r:?}");
    assert_eq!(
        fs::read(t.raiz.join("b/dup2.bin")).unwrap(),
        vec![b'a'; 10_000]
    );
    assert!(!destino.exists());
    assert_eq!(fs::metadata(t.raiz.join("b/dup2.bin")).unwrap().ino(), ino);
    assert_eq!(lixeira(&mut a).1, 0);
    assert_eq!(n_grupos(&mut a), 1);
    // Um Fast Scan depois não acha divergência: o Catálogo e o disco concordam.
    let r = a.executar(scan(&t, ModoVarredura::FastScan));
    let Resposta::ScanConcluido { reconciliacao, .. } = r else {
        panic!("{r:?}")
    };
    let rec = reconciliacao.unwrap();
    assert_eq!(rec.arquivos_ausentes, 0, "{rec:?}");
}

#[test]
fn simular_nao_toca_no_disco() {
    let (t, mut a) = pronto();
    marcar(&mut a, caminho(&t, "b/dup2.bin"));
    let r = mover(&mut a, vec![caminho(&t, "b/dup2.bin")], false, false, true);
    assert!(r.simulado);
    assert_eq!((r.total_arquivos, r.total_bytes), (1, 10_000));
    assert_eq!(r.destinos.len(), 1);
    assert!(r.destinos[0].ends_with("/DAG-Lixeira"));
    assert!(r.movidos[0].para.ends_with("DAG-Lixeira/b/dup2.bin"));
    assert!(t.raiz.join("b/dup2.bin").exists());
    assert!(!t.raiz.join("DAG-Lixeira").exists());
    assert_eq!(lixeira(&mut a).1, 0);
}

#[test]
fn arquivo_sem_marcacao_e_recusado() {
    let (t, mut a) = pronto();
    let r = mover(&mut a, vec![caminho(&t, "b/dup2.bin")], false, false, false);
    assert_eq!(codigos(&r), ["sem_marcacao"]);
    assert_eq!(r.total_arquivos, 0);
    assert!(t.raiz.join("b/dup2.bin").exists());
    assert!(!t.raiz.join("DAG-Lixeira").exists());
}

#[test]
fn unica_copia_exige_confirmacao_explicita() {
    let (t, mut a) = pronto();
    marcar(&mut a, caminho(&t, "c/unico.bin"));
    let r = mover(
        &mut a,
        vec![caminho(&t, "c/unico.bin")],
        false,
        false,
        false,
    );
    assert_eq!(codigos(&r), ["unica_copia"]);
    assert!(t.raiz.join("c/unico.bin").exists());
    let r = mover(&mut a, vec![caminho(&t, "c/unico.bin")], true, false, false);
    assert!(r.recusados.is_empty(), "{r:?}");
    assert_eq!(r.unicas_copias, 1);
    assert!(t.raiz.join("DAG-Lixeira/c/unico.bin").exists());
    assert!(!t.raiz.join("c/unico.bin").exists());
}

#[test]
fn copia_na_lixeira_nao_evita_alerta_de_unica_copia() {
    let (t, mut a) = pronto();
    marcar(&mut a, caminho(&t, "b/dup2.bin"));
    let r = mover(&mut a, vec![caminho(&t, "b/dup2.bin")], false, false, false);
    assert!(r.recusados.is_empty(), "{r:?}");
    // dup1 passa a ser a Única Cópia: marcar para deleção dispara o alerta.
    let r = a.executar(Comando::MarcarDelecao {
        alvo: caminho(&t, "a/dup1.bin"),
    });
    let Resposta::Marcacao { resultado } = r else {
        panic!("{r:?}")
    };
    assert_eq!(resultado.unicas_copias_total, 1);
    // E mover exige a confirmação extra.
    let r = mover(&mut a, vec![caminho(&t, "a/dup1.bin")], false, false, false);
    assert_eq!(codigos(&r), ["unica_copia"]);
}

#[test]
fn duas_copias_no_mesmo_lote_contam_como_unica_copia() {
    let (t, mut a) = pronto();
    marcar(&mut a, caminho(&t, "a/dup1.bin"));
    marcar(&mut a, caminho(&t, "b/dup2.bin"));
    let r = mover(
        &mut a,
        vec![caminho(&t, "a/dup1.bin"), caminho(&t, "b/dup2.bin")],
        false,
        false,
        false,
    );
    // As duas vão juntas: sobraria só a Lixeira; é Única Cópia e exige a confirmação.
    assert!(codigos(&r).contains(&"unica_copia"), "{r:?}");
    assert!(t.raiz.join("a/dup1.bin").exists() && t.raiz.join("b/dup2.bin").exists());
}

struct OutroVolume;
impl Dispositivos for OutroVolume {
    fn dev(&self, p: &Path) -> io::Result<u64> {
        let d = fs::symlink_metadata(p)?.dev();
        Ok(if p.to_string_lossy().contains("DAG-Lixeira") {
            d + 1
        } else {
            d
        })
    }
}

#[test]
fn outro_sistema_de_arquivos_e_recusado() {
    let t = arvore();
    let mut a = agente(&t).com_dispositivos(Arc::new(OutroVolume));
    a.executar(scan(&t, ModoVarredura::FullScan));
    marcar(&mut a, caminho(&t, "b/dup2.bin"));
    let r = mover(&mut a, vec![caminho(&t, "b/dup2.bin")], false, false, false);
    assert_eq!(codigos(&r), ["outro_volume"]);
    assert_eq!(
        fs::read(t.raiz.join("b/dup2.bin")).unwrap(),
        vec![b'a'; 10_000]
    );
    assert_eq!(lixeira(&mut a).1, 0);
}

#[test]
fn arquivo_que_mudou_no_disco_e_recusado() {
    let (t, mut a) = pronto();
    marcar(&mut a, caminho(&t, "b/dup2.bin"));
    // Mesmo tamanho, outro conteúdo (mtime novo): o Catálogo está defasado.
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(t.raiz.join("b/dup2.bin"), [b'z'; 10_000]).unwrap();
    let r = mover(&mut a, vec![caminho(&t, "b/dup2.bin")], false, false, false);
    assert_eq!(codigos(&r), ["mudou_no_disco"]);
    assert_eq!(
        fs::read(t.raiz.join("b/dup2.bin")).unwrap(),
        vec![b'z'; 10_000]
    );
    assert!(!t.raiz.join("DAG-Lixeira/b/dup2.bin").exists());
}

#[test]
fn rename_nunca_sobrescreve_na_ida_nem_na_volta() {
    let (t, mut a) = pronto();
    marcar(&mut a, caminho(&t, "b/dup2.bin"));
    // Já existe algo (não catalogado) no destino da Lixeira: o novo nome ganha sufixo.
    escrever(&t.raiz.join("DAG-Lixeira/b/dup2.bin"), b"do usuario");
    let r = mover(&mut a, vec![caminho(&t, "b/dup2.bin")], false, false, false);
    assert!(r.recusados.is_empty(), "{r:?}");
    assert_eq!(
        fs::read(t.raiz.join("DAG-Lixeira/b/dup2.bin")).unwrap(),
        b"do usuario"
    );
    assert_eq!(
        fs::read(t.raiz.join("DAG-Lixeira/b/dup2 (2).bin")).unwrap(),
        vec![b'a'; 10_000]
    );
    // Restaurar com a origem ocupada: volta com sufixo e o arquivo novo fica intacto.
    escrever(&t.raiz.join("b/dup2.bin"), b"outro arquivo");
    let r = restaurar(&mut a, vec![caminho(&t, "DAG-Lixeira/b/dup2 (2).bin")]);
    assert!(r.recusados.is_empty(), "{r:?}");
    assert_eq!(
        fs::read(t.raiz.join("b/dup2.bin")).unwrap(),
        b"outro arquivo"
    );
    assert_eq!(
        fs::read(t.raiz.join("b/dup2 (restaurado).bin")).unwrap(),
        vec![b'a'; 10_000]
    );
    assert!(r.movidos[0].para.ends_with("dup2 (restaurado).bin"));
}

#[test]
fn restaurar_recria_pasta_de_origem_ausente() {
    let (t, mut a) = pronto();
    marcar(&mut a, caminho(&t, "b/dup2.bin"));
    mover(&mut a, vec![caminho(&t, "b/dup2.bin")], false, false, false);
    // O usuário apagou a pasta de origem (vazia) à mão.
    fs::remove_dir(t.raiz.join("b")).unwrap();
    let r = restaurar(&mut a, vec![caminho(&t, "DAG-Lixeira/b/dup2.bin")]);
    assert!(r.recusados.is_empty(), "{r:?}");
    assert!(t.raiz.join("b/dup2.bin").exists());
}

#[test]
fn pasta_so_vai_se_todo_arquivo_tem_marcacao() {
    let (t, mut a) = pronto();
    marcar(&mut a, caminho(&t, "pasta/x.bin"));
    let alvo = Alvo::Pasta(p(&t, "pasta"));
    let r = mover(&mut a, vec![alvo.clone()], true, false, false);
    assert_eq!(codigos(&r), ["sem_marcacao"]);
    assert!(r.recusados[0].arquivos[0].ends_with("pasta/y.bin"), "{r:?}");
    assert!(t.raiz.join("pasta/x.bin").exists() && t.raiz.join("pasta/y.bin").exists());
    // "Marcar tudo e mover" é o passo explícito separado.
    let r = mover(&mut a, vec![alvo], true, true, false);
    assert!(r.recusados.is_empty(), "{r:?}");
    assert_eq!(r.marcados_agora, 1);
    assert_eq!(r.total_arquivos, 2);
    assert!(t.raiz.join("DAG-Lixeira/pasta/x.bin").exists());
    assert!(t.raiz.join("DAG-Lixeira/pasta/y.bin").exists());
    assert_eq!(lixeira(&mut a).1, 2);
}

#[test]
fn hard_link_nao_vai() {
    let (t, mut a) = pronto();
    marcar(&mut a, caminho(&t, "h/l1.bin"));
    let r = mover(&mut a, vec![caminho(&t, "h/l1.bin")], true, false, false);
    assert_eq!(codigos(&r), ["hard_link"]);
    assert!(t.raiz.join("h/l1.bin").exists() && t.raiz.join("h/l2.bin").exists());
}

#[test]
fn marcacao_por_regra_de_pasta_e_fixada_na_copia_ao_mover() {
    let (t, mut a) = pronto();
    let r = a.executar(Comando::MarcarDelecao {
        alvo: Alvo::Pasta(p(&t, "b")),
    });
    assert!(matches!(r, Resposta::Marcacao { .. }), "{r:?}");
    let r = mover(&mut a, vec![caminho(&t, "b/dup2.bin")], false, false, false);
    assert!(r.recusados.is_empty(), "{r:?}");
    // Na Lixeira continua marcada (a regra da pasta de origem já não a alcança).
    let r = a.executar(Comando::Relatorio {
        tipo: dag_core::TipoRelatorio::Marcacoes,
        nivel: None,
        limite: None,
        cursor: None,
        saida: None,
    });
    let Resposta::Relatorio { dados, .. } = r else {
        panic!("{r:?}")
    };
    let dag_core::DadosRelatorio::Marcacoes { marcacoes } = dados else {
        panic!()
    };
    assert!(
        marcacoes
            .iter()
            .any(|m| m.alvo.contains("DAG-Lixeira/b/dup2.bin")),
        "{marcacoes:?}"
    );
}

#[test]
fn varredura_reconhece_o_que_o_usuario_poe_tira_e_esvazia() {
    let (t, mut a) = pronto();
    // 1. Pôs à mão: rename direto para a Lixeira.
    fs::create_dir_all(t.raiz.join("DAG-Lixeira/b")).unwrap();
    fs::rename(
        t.raiz.join("b/dup2.bin"),
        t.raiz.join("DAG-Lixeira/b/dup2.bin"),
    )
    .unwrap();
    let r = a.executar(scan(&t, ModoVarredura::FastScan));
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    let (itens, total, _) = lixeira(&mut a);
    assert_eq!(total, 1, "{itens:?}");
    assert!(itens[0].origem.ends_with("/b/dup2.bin"));
    assert_eq!(n_grupos(&mut a), 0, "Na Lixeira não conta como cópia");
    // Passou a ter Marcação para Deleção.
    let r = a.executar(Comando::Relatorio {
        tipo: dag_core::TipoRelatorio::Marcacoes,
        nivel: None,
        limite: None,
        cursor: None,
        saida: None,
    });
    let Resposta::Relatorio {
        dados: dag_core::DadosRelatorio::Marcacoes { marcacoes },
        ..
    } = r
    else {
        panic!("{r:?}")
    };
    assert_eq!(marcacoes.len(), 1, "{marcacoes:?}");
    // 2. Tirou à mão: volta a ser cópia comum, sem marcação.
    fs::rename(
        t.raiz.join("DAG-Lixeira/b/dup2.bin"),
        t.raiz.join("b/dup2.bin"),
    )
    .unwrap();
    a.executar(scan(&t, ModoVarredura::FastScan));
    assert_eq!(lixeira(&mut a).1, 0);
    assert_eq!(n_grupos(&mut a), 1);
    let r = a.executar(Comando::Relatorio {
        tipo: dag_core::TipoRelatorio::Marcacoes,
        nivel: None,
        limite: None,
        cursor: None,
        saida: None,
    });
    let Resposta::Relatorio {
        dados: dag_core::DadosRelatorio::Marcacoes { marcacoes },
        ..
    } = r
    else {
        panic!("{r:?}")
    };
    assert!(marcacoes.is_empty(), "{marcacoes:?}");
    // 3. Esvaziou: some da Lixeira e vira Descartado, com caminho, hash e data.
    marcar(&mut a, caminho(&t, "b/dup2.bin"));
    mover(&mut a, vec![caminho(&t, "b/dup2.bin")], false, false, false);
    assert_eq!(lixeira(&mut a).1, 1);
    fs::remove_file(t.raiz.join("DAG-Lixeira/b/dup2.bin")).unwrap();
    a.executar(scan(&t, ModoVarredura::FastScan));
    let r = a.executar(Comando::ListarLixeira {
        limite: None,
        cursor: None,
    });
    let Resposta::ListaLixeira {
        total,
        descartados,
        ultimos_descartados,
        ..
    } = r
    else {
        panic!("{r:?}")
    };
    assert_eq!((total, descartados), (0, 1));
    assert!(
        ultimos_descartados[0]
            .caminho
            .ends_with("DAG-Lixeira/b/dup2.bin")
    );
    assert_eq!(ultimos_descartados[0].hash.as_ref().unwrap().len(), 64);
    assert_eq!(ultimos_descartados[0].bytes, 10_000);
}

#[test]
fn varredura_de_subpasta_tambem_visita_a_lixeira_do_volume() {
    let (t, mut a) = pronto();
    marcar(&mut a, caminho(&t, "b/dup2.bin"));
    mover(&mut a, vec![caminho(&t, "b/dup2.bin")], false, false, false);
    // Usuário esvazia; a varredura só pede a subpasta `a`, mas a Lixeira é visitada.
    fs::remove_file(t.raiz.join("DAG-Lixeira/b/dup2.bin")).unwrap();
    let r = a.executar(Comando::IniciarScan {
        modo: ModoVarredura::FastScan,
        raizes: vec![p(&t, "a")],
        sem_hash: false,
        elevar: false,
        releitura_total: false,
        threads_por_disco: None,
        max_discos: None,
    });
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    let (_, total, descartados) = lixeira(&mut a);
    assert_eq!((total, descartados), (0, 1));
}

#[test]
fn leitura_nao_executa_mover() {
    let (t, a) = pronto();
    drop(a);
    // `ListarLixeira` e a simulação são de leitura; mover de verdade exige o escritor.
    assert!(dag_agent::somente_leitura(&Comando::ListarLixeira {
        limite: None,
        cursor: None
    }));
    assert!(!dag_agent::somente_leitura(&Comando::MoverParaLixeira {
        alvos: vec![],
        confirmacao_unica_copia: false,
        marcar_tudo: false,
        simular: false
    }));
    assert!(!dag_agent::somente_leitura(&Comando::RestaurarDaLixeira {
        alvos: vec![]
    }));
    let _ = (t, File::open("/dev/null"));
}
