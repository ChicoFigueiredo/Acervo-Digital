//! Conteúdo de contêineres de ponta a ponta no agente: Pedido de Senha, Cofre de Senhas com o
//! keyring falso, duplicados "dentro de" e a garantia de que a senha nunca é devolvida nem gravada
//! em claro (ADR 0006).
#![allow(clippy::unwrap_used)]

use std::fs;
use std::io::{Cursor, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;

use bancada_segredos::testes::ChaveiroEmMemoria;
use dag_agent::Agente;
use dag_core::{Comando, ModoVarredura, Resposta, Senha, VolumeDescoberto};
use tempfile::TempDir;

const SENHA: &str = "senha-super-secreta-42";

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
        dispositivo: Some(dag_core::DispositivoDescoberto {
            identificador: "disco".into(),
            modelo: None,
            tipo: dag_core::TipoDispositivo::Ssd,
            removivel: false,
        }),
    }
}

fn zip_cifrado(nome: &str, dados: &[u8]) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .with_aes_encryption(zip::AesMode::Aes256, SENHA);
    z.start_file(nome, o).unwrap();
    z.write_all(dados).unwrap();
    z.finish().unwrap().into_inner()
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

struct Cenario {
    _tmp: TempDir,
    raiz: std::path::PathBuf,
    catalogo: std::path::PathBuf,
}

fn cenario() -> Cenario {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("acervo");
    fs::create_dir_all(&raiz).unwrap();
    let conteudo = vec![b'x'; 4000];
    fs::write(raiz.join("solto.bin"), &conteudo).unwrap();
    fs::write(
        raiz.join("pacote.zip"),
        zip_cifrado("dentro/copia.bin", &conteudo),
    )
    .unwrap();
    let catalogo = tmp.path().join("dados/catalogo.sqlite");
    Cenario {
        _tmp: tmp,
        raiz,
        catalogo,
    }
}

fn agente(c: &Cenario, kr: &Arc<ChaveiroEmMemoria>) -> Agente {
    Agente::abrir(&c.catalogo)
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![volume(&c.raiz)])
        .com_chaveiro(kr.clone())
}

#[test]
fn cifrado_vira_pedido_de_senha_e_fornecer_senha_cataloga_os_membros() {
    let c = cenario();
    let kr = Arc::new(ChaveiroEmMemoria::novo());
    let mut a = agente(&c, &kr);
    let r = a.executar(scan(&c.raiz));
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");

    // Pedido de Senha na Fila de Serviço, sem popup nem leitura de senha.
    let Resposta::Pedidos { pedidos } = a.executar(Comando::ListarPedidos {
        incluir_encerrados: false,
    }) else {
        panic!()
    };
    assert_eq!(pedidos.len(), 1);
    assert_eq!(pedidos[0].tipo, "pedido_senha");
    assert!(
        pedidos[0]
            .objeto
            .as_deref()
            .unwrap()
            .ends_with("pacote.zip")
    );
    let id = pedidos[0].id;

    // Fornece a senha: o Contêiner é relido e o Membro entra no grupo de duplicados.
    let r = a.executar(Comando::ResolverPedido {
        id,
        senha: Senha(SENHA.into()),
    });
    assert!(matches!(r, Resposta::Ok { .. }), "{r:?}");
    assert!(!format!("{r:?}").contains(SENHA));

    let Resposta::Pedidos { pedidos } = a.executar(Comando::ListarPedidos {
        incluir_encerrados: false,
    }) else {
        panic!()
    };
    assert!(pedidos.is_empty(), "{pedidos:?}");

    let Resposta::Duplicados { grupos, .. } = a.executar(Comando::ListarDuplicados {
        volume: None,
        tamanho_minimo: None,
        ordem: None,
        limite: None,
        cursor: None,
    }) else {
        panic!()
    };
    assert_eq!(grupos.len(), 1, "{grupos:?}");
    assert_eq!(grupos[0].copias_em_conteineres, 1);
    assert_eq!(grupos[0].espaco_recuperavel_bytes, 0);

    // O Explorador abre o Contêiner como pasta virtual.
    let pacote = c.raiz.join("pacote.zip");
    let Resposta::Membros { entradas, .. } = a.executar(Comando::ExplorarConteiner {
        conteiner: pacote.to_str().unwrap().into(),
        pai_ordem: None,
        pasta: String::new(),
        limite: None,
        cursor: None,
    }) else {
        panic!()
    };
    assert_eq!(entradas.len(), 1);
    assert_eq!(entradas[0].nome, "dentro");
    assert!(entradas[0].navegavel);

    // A senha não aparece em nenhuma resposta do Cofre nem no Catálogo em claro.
    let r = a.executar(Comando::CofreListar);
    assert!(!format!("{r:?}").contains(SENHA));
    drop(a);
    let bytes = fs::read(&c.catalogo).unwrap();
    assert!(!bytes.windows(SENHA.len()).any(|w| w == SENHA.as_bytes()));
}

#[test]
fn sem_keyring_a_senha_nao_e_aceita() {
    let c = cenario();
    let kr = Arc::new(ChaveiroEmMemoria::indisponivel());
    let mut a = agente(&c, &kr);
    a.executar(scan(&c.raiz));
    let Resposta::Pedidos { pedidos } = a.executar(Comando::ListarPedidos {
        incluir_encerrados: false,
    }) else {
        panic!()
    };
    // Sem Cofre utilizável o Contêiner espera o chaveiro (também vira item da fila).
    assert_eq!(pedidos.len(), 1);
    let r = a.executar(Comando::ResolverPedido {
        id: pedidos[0].id,
        senha: Senha(SENHA.into()),
    });
    assert!(matches!(r, Resposta::Erro { .. }), "{r:?}");
}
