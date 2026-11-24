//! Imagens de Disco de ponta a ponta no agente: Membros contam como cópia nos Grupos de
//! Duplicados mas nunca como espaço recuperável, e a imagem Na Lixeira deixa de contar (os
//! Membros continuam ligados e voltam a contar ao restaurar). ADR 0005 e 0006.
#![allow(clippy::unwrap_used, clippy::manual_is_multiple_of)]

#[path = "../../dag-imagens/tests/common/mod.rs"]
#[allow(clippy::unwrap_used)]
mod imagens;

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use dag_agent::Agente;
use dag_core::{
    Alvo, Comando, DispositivoDescoberto, ModoVarredura, Resposta, ResultadoLixeira,
    TipoDispositivo, VolumeDescoberto,
};
use imagens::{No, dados, fat};

fn volume(raiz: &Path) -> VolumeDescoberto {
    let dev = fs::metadata(raiz).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("V-{dev}"),
        st_dev: dev,
        rotulo: Some("V".into()),
        fs_type: "teste".into(),
        capacidade_bytes: Some(1_000_000),
        livre_bytes: Some(400_000),
        pontos_de_montagem: vec![raiz.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: Some(DispositivoDescoberto {
            identificador: "disco".into(),
            modelo: None,
            tipo: TipoDispositivo::Ssd,
            removivel: false,
        }),
    }
}

fn scan(raiz: &Path) -> Comando {
    Comando::IniciarScan {
        modo: ModoVarredura::FullScan,
        raizes: vec![raiz.to_str().unwrap().to_string()],
        sem_hash: false,
        elevar: false,
        releitura_total: false,
        threads_por_disco: None,
        max_discos: None,
    }
}

fn caminho(raiz: &Path, rel: &str) -> Alvo {
    Alvo::Caminho(raiz.join(rel).to_str().unwrap().to_string())
}

fn grupos(a: &mut Agente) -> Vec<dag_core::GrupoDuplicados> {
    let Resposta::Duplicados { grupos, .. } = a.executar(Comando::ListarDuplicados {
        volume: None,
        tamanho_minimo: None,
        ordem: None,
        limite: None,
        cursor: None,
    }) else {
        panic!()
    };
    grupos
}

fn mover(a: &mut Agente, alvo: Alvo, unica: bool) -> ResultadoLixeira {
    let r = a.executar(Comando::MoverParaLixeira {
        alvos: vec![alvo],
        confirmacao_unica_copia: unica,
        marcar_tudo: false,
        simular: false,
    });
    let Resposta::Lixeira { resultado } = r else {
        panic!("{r:?}")
    };
    resultado
}

#[test]
fn membro_de_imagem_e_copia_sem_espaco_recuperavel_e_a_imagem_na_lixeira_nao_conta() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("acervo");
    fs::create_dir_all(raiz.join("a")).unwrap();
    let x = dados(11, 20_000);
    fs::write(raiz.join("a/solto1.bin"), &x).unwrap();
    fs::write(raiz.join("a/solto2.bin"), &x).unwrap();
    let arv = vec![("DUP.BIN".to_string(), No::Arq(x.clone()))];
    fs::write(raiz.join("disco.img"), fat::fat_img(12, &arv, false)).unwrap();
    let mut a = Agente::abrir(tmp.path().join("dados/c.sqlite"))
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![volume(&raiz)]);
    let r = a.executar(scan(&raiz));
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");

    // Dois soltos + um Membro: um grupo, e só um dos soltos é recuperável (o Membro não soma).
    let g = grupos(&mut a);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0].copias_em_conteineres, 1);
    assert_eq!(g[0].espaco_recuperavel_bytes, 20_000);

    // O Membro evita o alerta de Única Cópia mesmo sem outro solto: remove-se um solto antes.
    let Resposta::Marcacao { resultado } = a.executar(Comando::MarcarDelecao {
        alvo: caminho(&raiz, "a/solto1.bin"),
    }) else {
        panic!()
    };
    assert_eq!(resultado.unicas_copias_total, 0);

    // A imagem inteira vai para a Lixeira (é Única Cópia dela mesma: confirmação extra).
    a.executar(Comando::MarcarDelecao {
        alvo: caminho(&raiz, "disco.img"),
    });
    let r = mover(&mut a, caminho(&raiz, "disco.img"), true);
    assert!(r.recusados.is_empty(), "{r:?}");
    assert!(raiz.join("DAG-Lixeira/disco.img").exists());
    // Os Membros não contam como cópia: ainda há 2 soltos iguais, nada vem do contêiner.
    let g = grupos(&mut a);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0].copias_em_conteineres, 0);

    // Restaurar: os Membros (que nunca saíram do Catálogo) voltam a contar.
    let Resposta::Lixeira { resultado } = a.executar(Comando::RestaurarDaLixeira {
        alvos: vec![caminho(&raiz, "DAG-Lixeira/disco.img")],
    }) else {
        panic!()
    };
    assert!(resultado.recusados.is_empty(), "{resultado:?}");
    assert!(raiz.join("disco.img").exists());
    let g = grupos(&mut a);
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].copias_em_conteineres, 1);
    assert_eq!(g[0].espaco_recuperavel_bytes, 20_000);
}

#[test]
fn imagem_na_lixeira_faz_o_arquivo_solto_virar_unica_copia() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("acervo");
    fs::create_dir_all(&raiz).unwrap();
    let x = dados(12, 9_000);
    fs::write(raiz.join("solto.bin"), &x).unwrap();
    let arv = vec![("DUP.BIN".to_string(), No::Arq(x))];
    fs::write(raiz.join("disco.img"), fat::fat_img(12, &arv, false)).unwrap();
    let mut a = Agente::abrir(tmp.path().join("dados/c.sqlite"))
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![volume(&raiz)]);
    a.executar(scan(&raiz));
    assert_eq!(grupos(&mut a).len(), 1);
    // Com o Membro, o solto não é Única Cópia.
    let Resposta::Marcacao { resultado } = a.executar(Comando::MarcarDelecao {
        alvo: caminho(&raiz, "solto.bin"),
    }) else {
        panic!()
    };
    assert_eq!(resultado.unicas_copias_total, 0);
    // Imagem Na Lixeira: o solto passa a ser a Única Cópia e o movimento exige confirmação.
    a.executar(Comando::MarcarDelecao {
        alvo: caminho(&raiz, "disco.img"),
    });
    let r = mover(&mut a, caminho(&raiz, "disco.img"), true);
    assert!(r.recusados.is_empty(), "{r:?}");
    let r = mover(&mut a, caminho(&raiz, "solto.bin"), false);
    let codigos: Vec<&str> = r.recusados.iter().map(|x| x.codigo.as_str()).collect();
    assert_eq!(codigos, ["unica_copia"], "{r:?}");
    assert!(raiz.join("solto.bin").exists());
}
