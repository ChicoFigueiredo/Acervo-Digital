#![allow(clippy::unwrap_used)]

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use dag_agent::{Agente, ErroAgente, GuardaEscritor, caminho_do_lock, executar_comando};
use dag_core::{
    Comando, EstadoExecucao, ExecucaoId, Finalidade, ModoVarredura, Resposta,
    TipoEntradaExploracao, VolumeDescoberto,
};
use dag_store::Catalogo;
use tempfile::TempDir;

/// Volume falso cobrindo `p` (o `st_dev` real, para o scanner associar as entradas).
fn volume_de(p: &Path) -> VolumeDescoberto {
    let dev = fs::metadata(p).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("teste-dev-{dev}"),
        st_dev: dev,
        rotulo: Some("teste".into()),
        fs_type: "teste".into(),
        capacidade_bytes: Some(1000),
        livre_bytes: Some(400),
        pontos_de_montagem: vec![p.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: None,
    }
}

fn escrever(p: &Path, bytes: usize) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    File::create(p)
        .unwrap()
        .write_all(&vec![b'x'; bytes])
        .unwrap();
}

struct Cenario {
    _tmp: TempDir,
    raiz: PathBuf,
    catalogo: PathBuf,
}

/// raiz/{big.pdf 9000, sub/{x.bin 3000, y.txt 2000}, a.txt 100, vazia/}
fn cenario() -> Cenario {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("arvore");
    escrever(&raiz.join("big.pdf"), 9000);
    escrever(&raiz.join("sub/x.bin"), 3000);
    escrever(&raiz.join("sub/y.txt"), 2000);
    escrever(&raiz.join("a.txt"), 100);
    fs::create_dir_all(raiz.join("vazia")).unwrap();
    let catalogo = tmp.path().join("dados/catalogo.sqlite");
    Cenario {
        _tmp: tmp,
        raiz,
        catalogo,
    }
}

fn agente(c: &Cenario) -> Agente {
    Agente::abrir(&c.catalogo)
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![volume_de(&c.raiz)])
}

fn scan_cmd(raiz: &Path) -> Comando {
    Comando::IniciarScan {
        modo: ModoVarredura::FullScan,
        raizes: vec![raiz.to_str().unwrap().to_string()],
        sem_hash: false,
        elevar: false,
    }
}

fn explorar_cmd(pasta: Option<&Path>, limite: Option<u32>, cursor: Option<String>) -> Comando {
    Comando::Explorar {
        pasta: pasta.map(|p| p.to_str().unwrap().to_string()),
        limite,
        cursor,
    }
}

fn codigo(r: &Resposta) -> &str {
    match r {
        Resposta::Erro { codigo, .. } => codigo,
        outra => panic!("esperava erro, veio {outra:?}"),
    }
}

#[test]
fn trava_de_escritor_e_exclusiva() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = tmp.path().join("c.sqlite");
    let g1 = GuardaEscritor::adquirir(&cat).unwrap();
    assert_eq!(g1.caminho(), caminho_do_lock(&cat));
    let e = GuardaEscritor::adquirir(&cat).unwrap_err();
    assert!(matches!(e, ErroAgente::EscritorOcupado(_)));
    assert!(
        e.to_string()
            .contains("outro processo do DAG já é o escritor deste catálogo")
    );
    // Solta ao descartar.
    drop(g1);
    let _g2 = GuardaEscritor::adquirir(&cat).unwrap();
}

#[test]
fn segundo_escritor_recebe_resposta_de_erro_clara() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = tmp.path().join("c.sqlite");
    let primeiro = Agente::abrir(&cat).unwrap();
    assert!(primeiro.eh_escritor());
    assert!(matches!(
        Agente::abrir(&cat),
        Err(ErroAgente::EscritorOcupado(_))
    ));
    let r = executar_comando(&cat, Comando::ListarVolumes);
    let Resposta::Erro { codigo, mensagem } = r else {
        panic!("esperava erro")
    };
    assert_eq!(codigo, "escritor_ocupado");
    assert!(mensagem.contains("outro processo do DAG já é o escritor deste catálogo"));
    // Leitura continua possível com um escritor ativo.
    let r = executar_comando(&cat, Comando::Status { execucao: None });
    assert!(matches!(r, Resposta::Status { .. }), "{r:?}");
}

#[test]
fn diretorio_0700_e_banco_0600() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = tmp.path().join("novo/dir/catalogo.sqlite");
    let _a = Agente::abrir(&cat).unwrap();
    let modo = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(modo(cat.parent().unwrap()), 0o700);
    assert_eq!(modo(&cat), 0o600);
    assert_eq!(modo(&caminho_do_lock(&cat)), 0o600);
}

#[test]
fn leitura_de_catalogo_inexistente_nao_cria_nada() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = tmp.path().join("nao/existe.sqlite");
    let r = executar_comando(&cat, Comando::Status { execucao: None });
    assert_eq!(codigo(&r), "sem_catalogo");
    assert!(!cat.exists() && !cat.parent().unwrap().exists());
}

#[test]
fn full_scan_e_explorar_ordenam_por_tamanho() {
    let c = cenario();
    let mut a = agente(&c);
    let r = a.executar(scan_cmd(&c.raiz));
    let Resposta::ScanConcluido {
        desfecho,
        arquivos,
        pastas,
        bytes_logicos,
        erros,
        ..
    } = r
    else {
        panic!("{r:?}")
    };
    assert_eq!(desfecho, "concluida");
    assert_eq!((arquivos, pastas, bytes_logicos, erros), (4, 3, 14_100, 0));

    let r = a.executar(explorar_cmd(Some(&c.raiz), None, None));
    let Resposta::Explorar {
        pasta,
        entradas,
        proximo_cursor,
    } = r
    else {
        panic!("{r:?}")
    };
    assert_eq!(pasta.as_deref(), c.raiz.to_str());
    assert!(proximo_cursor.is_none());
    let nomes: Vec<&str> = entradas.iter().map(|e| e.nome.as_str()).collect();
    assert_eq!(nomes, ["big.pdf", "sub", "a.txt", "vazia"]);
    let tam: Vec<u64> = entradas.iter().map(|e| e.bytes_logicos).collect();
    assert_eq!(tam, [9000, 5000, 100, 0]);
    // Arquivo: MIME e categoria por extensão; alocado conhecido.
    let pdf = &entradas[0];
    assert_eq!(pdf.tipo, TipoEntradaExploracao::Arquivo);
    assert_eq!(pdf.mime.as_deref(), Some("application/pdf"));
    assert_eq!(pdf.categoria.as_deref(), Some("documento"));
    assert!(pdf.bytes_alocados.unwrap() >= 9000);
    assert!(pdf.finalidade.is_some());
    // Pasta: contagens recursivas e finalidade.
    let sub = &entradas[1];
    assert_eq!(sub.tipo, TipoEntradaExploracao::Pasta);
    assert_eq!((sub.arquivos, sub.pastas), (Some(2), Some(0)));
    assert!(sub.finalidade.is_some());
    // Extensão desconhecida: sem tipo inventado.
    let r = a.executar(explorar_cmd(Some(&c.raiz.join("sub")), None, None));
    let Resposta::Explorar { entradas, .. } = r else {
        panic!()
    };
    assert_eq!(entradas[0].nome, "x.bin");
    assert_eq!(entradas[1].nome, "y.txt");
    assert_eq!(entradas[1].mime.as_deref(), Some("text/plain"));
}

#[test]
fn explorar_pagina_por_cursor() {
    let c = cenario();
    let mut a = agente(&c);
    a.executar(scan_cmd(&c.raiz));
    let Resposta::Explorar {
        entradas: p1,
        proximo_cursor,
        ..
    } = a.executar(explorar_cmd(Some(&c.raiz), Some(3), None))
    else {
        panic!()
    };
    assert_eq!(p1.len(), 3);
    let cursor = proximo_cursor.expect("há mais uma entrada");
    let Resposta::Explorar {
        entradas: p2,
        proximo_cursor,
        ..
    } = a.executar(explorar_cmd(Some(&c.raiz), Some(3), Some(cursor)))
    else {
        panic!()
    };
    assert_eq!(p2.len(), 1);
    assert_eq!(p2[0].nome, "vazia");
    assert!(proximo_cursor.is_none());
    let r = a.executar(explorar_cmd(Some(&c.raiz), None, Some("lixo".into())));
    assert_eq!(codigo(&r), "invalido");
}

#[test]
fn explorar_raizes_e_pasta_desconhecida() {
    let c = cenario();
    let mut a = agente(&c);
    a.executar(scan_cmd(&c.raiz));
    let Resposta::Explorar {
        pasta, entradas, ..
    } = a.executar(explorar_cmd(None, None, None))
    else {
        panic!()
    };
    assert!(pasta.is_none());
    assert_eq!(entradas.len(), 1);
    assert_eq!(entradas[0].bytes_logicos, 14_100);
    let r = a.executar(explorar_cmd(Some(Path::new("/nao/catalogada")), None, None));
    assert_eq!(codigo(&r), "nao_encontrado");
}

#[test]
fn finalidade_e_gravada_em_classifications() {
    let c = cenario();
    let mut a = agente(&c);
    a.executar(scan_cmd(&c.raiz));
    let conn = a.catalogo().armazem().conexao();
    let n = |sql: &str| conn.query_row(sql, [], |l| l.get::<_, i64>(0)).unwrap();
    assert_eq!(n("SELECT COUNT(*) FROM folders WHERE class_id IS NULL"), 0);
    assert_eq!(n("SELECT COUNT(*) FROM files WHERE class_id IS NULL"), 0);
    assert!(n("SELECT COUNT(*) FROM classifications WHERE scope = 'finalidade'") >= 1);
    let Resposta::Explorar { entradas, .. } = a.executar(explorar_cmd(Some(&c.raiz), None, None))
    else {
        panic!()
    };
    // Tempdir fica sob /tmp: regra de caminho conhecida, nunca "indefinida".
    assert!(entradas.iter().all(|e| e.finalidade.is_some()));
    let _: Finalidade = entradas[0].finalidade.unwrap();
}

#[test]
fn status_mostra_execucoes_e_totais() {
    let c = cenario();
    let mut a = agente(&c);
    a.executar(scan_cmd(&c.raiz));
    let Resposta::Status {
        execucoes,
        catalogo,
        ..
    } = a.executar(Comando::Status { execucao: None })
    else {
        panic!()
    };
    assert_eq!(execucoes.len(), 1);
    let e = &execucoes[0];
    assert_eq!(e.estado, EstadoExecucao::Concluida);
    assert_eq!((e.arquivos, e.pastas, e.bytes_logicos), (4, 3, 14_100));
    assert_eq!(e.itens_processados, 7);
    assert_eq!(e.raizes, [c.raiz.to_str().unwrap()]);
    assert_eq!(
        (catalogo.volumes, catalogo.arquivos, catalogo.bytes_logicos),
        (1, 4, 14_100)
    );
    let r = a.executar(Comando::Status {
        execucao: Some(ExecucaoId(999)),
    });
    assert_eq!(codigo(&r), "nao_encontrado");
}

/// Deixa o catálogo como um processo morto no meio do hash o deixaria: execução `concluida` na
/// fase `hash`, 2 dos 4 arquivos sem hash e com a fila `pendente`, atividade da fila em `quando`.
fn simular_hash_no_meio(c: &Cenario, quando: i64) {
    let cat = Catalogo::abrir(&c.catalogo).unwrap();
    let con = cat.armazem().conexao();
    con.execute("UPDATE scan_runs SET phase = 'hash'", [])
        .unwrap();
    con.execute(
        "DELETE FROM hashes WHERE file_id IN (SELECT id FROM files WHERE kind = 'arquivo' ORDER BY id LIMIT 2)",
        [],
    )
    .unwrap();
    con.execute(
        "UPDATE fila_hash SET state = 'pendente'
         WHERE file_id IN (SELECT id FROM files WHERE kind = 'arquivo' ORDER BY id LIMIT 2)",
        [],
    )
    .unwrap();
    con.execute("UPDATE fila_hash SET updated_at = ?1", [quando])
        .unwrap();
}

#[test]
fn status_mostra_a_etapa_de_hash_e_retomar_a_conclui() {
    let c = cenario();
    {
        let mut a = agente(&c);
        a.executar(scan_cmd(&c.raiz));
    }
    // Atividade recente: hash em andamento, 2 de 4 feitos, um disco.
    let agora = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    simular_hash_no_meio(&c, agora);
    let Resposta::Status { execucoes, .. } =
        executar_comando(&c.catalogo, Comando::Status { execucao: None })
    else {
        panic!()
    };
    let e = &execucoes[0];
    assert_eq!(e.etapa, Some(dag_core::EtapaVarredura::Hash));
    assert_eq!(e.estado, EstadoExecucao::EmAndamento);
    assert_eq!(e.estado_detalhe, "hash_em_andamento");
    let h = e.hash.as_ref().unwrap();
    assert!(h.em_andamento);
    assert_eq!((h.arquivos_hasheados, h.arquivos_pendentes), (2, 2));
    assert_eq!(h.discos.len(), 1);
    assert_eq!(h.discos[0].arquivos_pendentes, 2);
    assert_eq!(h.bytes_hasheados + h.bytes_pendentes, 14_100);
    assert!(h.bytes_por_seg.is_some());
    // Sem atividade recente: parada (retomável).
    simular_hash_no_meio(&c, 1_000);
    let Resposta::Status { execucoes, .. } =
        executar_comando(&c.catalogo, Comando::Status { execucao: None })
    else {
        panic!()
    };
    assert_eq!(execucoes[0].estado, EstadoExecucao::Pausada);
    assert_eq!(execucoes[0].estado_detalhe, "hash_parado");
    assert!(!execucoes[0].hash.as_ref().unwrap().em_andamento);
    // JSON do status carrega o bloco `hash`.
    let json = serde_json::to_value(&execucoes[0]).unwrap();
    assert_eq!(json["hash"]["arquivos_pendentes"], 2);
    assert_eq!(json["etapa"], "hash");
    // Retomar uma execução que parou no hash roda só o hash que falta.
    let id = execucoes[0].id;
    let mut a = agente(&c);
    let r = a.executar(Comando::Retomar { execucao: Some(id) });
    let Resposta::ScanConcluido { desfecho, hash, .. } = r else {
        panic!("{r:?}")
    };
    assert_eq!(desfecho, "concluida");
    let hash = hash.unwrap();
    assert_eq!((hash.hasheados, hash.reaproveitados), (2, 2));
    let Resposta::Status { execucoes, .. } = a.executar(Comando::Status { execucao: None }) else {
        panic!()
    };
    assert_eq!(execucoes[0].estado, EstadoExecucao::Concluida);
    assert!(execucoes[0].hash.is_none());
}

#[test]
fn listar_volumes_registra_no_catalogo() {
    let c = cenario();
    let mut a = agente(&c);
    let Resposta::Volumes { volumes, .. } = a.executar(Comando::ListarVolumes) else {
        panic!()
    };
    assert_eq!(volumes.len(), 1);
    let v = &volumes[0];
    assert!(v.conectado);
    assert_eq!(v.ponto_montagem.as_deref(), c.raiz.to_str());
    assert_eq!((v.capacidade_bytes, v.livre_bytes), (Some(1000), Some(400)));
    assert_eq!(v.fs_type.as_deref(), Some("teste"));
}

#[test]
fn backup_do_catalogo_passa_na_integridade() {
    let c = cenario();
    let mut a = agente(&c);
    a.executar(scan_cmd(&c.raiz));
    let destino = c.catalogo.with_file_name("copia.sqlite");
    let r = a.executar(Comando::BackupCatalogo {
        destino: destino.to_str().unwrap().into(),
    });
    assert!(matches!(r, Resposta::Ok { .. }), "{r:?}");
    assert_eq!(
        fs::metadata(&destino).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let copia = Catalogo::abrir(&destino).unwrap();
    copia.verificar_integridade().unwrap();
    let n: i64 = copia
        .armazem()
        .conexao()
        .query_row("SELECT COUNT(*) FROM files", [], |l| l.get(0))
        .unwrap();
    assert_eq!(n, 4);
    // Nunca sobrescreve.
    let r = a.executar(Comando::BackupCatalogo {
        destino: destino.to_str().unwrap().into(),
    });
    assert_eq!(codigo(&r), "destino_existe");
}

#[test]
fn scan_cancelado_e_retomado_em_outro_processo() {
    let c = cenario();
    let id = {
        let mut a = agente(&c);
        a.controle().cancelar(); // como um Ctrl-C antes de começar
        let r = a.executar(scan_cmd(&c.raiz));
        let Resposta::ScanConcluido {
            execucao, desfecho, ..
        } = r
        else {
            panic!("{r:?}")
        };
        assert_eq!(desfecho, "cancelada");
        execucao
    };
    let mut b = agente(&c);
    let r = b.executar(Comando::Retomar { execucao: Some(id) });
    let Resposta::ScanConcluido {
        desfecho,
        arquivos,
        bytes_logicos,
        ..
    } = r
    else {
        panic!("{r:?}")
    };
    assert_eq!(desfecho, "concluida");
    assert_eq!((arquivos, bytes_logicos), (4, 14_100));
    let r = b.executar(Comando::Retomar { execucao: None });
    assert_eq!(codigo(&r), "invalido");
}

#[test]
fn pausar_e_cancelar_em_processo_apontam_para_o_agente_em_segundo_plano() {
    let c = cenario();
    let mut a = agente(&c);
    for comando in [
        Comando::Pausar { execucao: None },
        Comando::Cancelar { execucao: None },
    ] {
        let Resposta::Erro { codigo, mensagem } = a.executar(comando) else {
            panic!()
        };
        assert_eq!(codigo, "sem_agente");
        assert!(mensagem.contains("dag agente"), "{mensagem}");
    }
}

fn volume_oculto(plataforma: &str, classe: dag_core::ClasseVolume, mont: &str) -> VolumeDescoberto {
    VolumeDescoberto {
        platform_id: plataforma.into(),
        st_dev: 0x7_0000 + u64::from(plataforma.len() as u32),
        rotulo: None,
        fs_type: "squashfs".into(),
        capacidade_bytes: Some(10),
        livre_bytes: Some(0),
        pontos_de_montagem: vec![mont.as_bytes().to_vec()],
        pseudo: false,
        classe,
        sistema: false,
        dispositivo: None,
    }
}

#[test]
fn listar_volumes_separa_reais_de_imagem_e_virtuais_sem_registrar_o_ruido() {
    use dag_core::ClasseVolume::{Imagem, Virtual};
    let c = cenario();
    let mut a = Agente::abrir(&c.catalogo)
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![
            volume_de(&c.raiz),
            volume_oculto("loop-a", Imagem, "/snap/core/1"),
            volume_oculto("zram-b", Virtual, "/mnt/zram"),
        ]);
    let Resposta::Volumes { volumes, ocultos } = a.executar(Comando::ListarVolumes) else {
        panic!()
    };
    assert_eq!(volumes.len(), 1);
    assert_eq!(volumes[0].classe, dag_core::ClasseVolume::Fisico);
    assert_eq!(ocultos.len(), 2);
    assert!(ocultos.iter().all(|o| o.id.0 == 0 && !o.classe.eh_real()));
    // Nada de ruído no Catálogo nem nos totais.
    let Resposta::Status { catalogo, .. } = a.executar(Comando::Status { execucao: None }) else {
        panic!()
    };
    assert_eq!(catalogo.volumes, 1);
}

#[test]
fn varrer_raiz_em_volume_de_imagem_registra_sob_demanda() {
    let c = cenario();
    let mut v = volume_de(&c.raiz);
    v.classe = dag_core::ClasseVolume::Imagem;
    let mut a = Agente::abrir(&c.catalogo)
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![v]);
    let Resposta::Volumes { volumes, ocultos } = a.executar(Comando::ListarVolumes) else {
        panic!()
    };
    assert_eq!((volumes.len(), ocultos.len()), (0, 1));
    let r = a.executar(scan_cmd(&c.raiz));
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    // Já varrido de propósito: entra no Catálogo (visível em ocultos, com id) e nos totais.
    let Resposta::Volumes { ocultos, .. } = a.executar(Comando::ListarVolumes) else {
        panic!()
    };
    assert_eq!(ocultos.len(), 1);
    assert_ne!(ocultos[0].id.0, 0);
    let Resposta::Status { catalogo, .. } = a.executar(Comando::Status { execucao: None }) else {
        panic!()
    };
    assert_eq!(catalogo.volumes, 1);
}
