//! Etapa "Conteúdo de contêineres" (ADR 0006): Membros, hashes, aninhados, cifrados, falhas,
//! retomada e a garantia de que nada é gravado fora do Catálogo.
#![allow(clippy::unwrap_used)]

mod comum;

use std::collections::BTreeSet;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use comum::*;
use dag_core::ExecucaoId;
use dag_scanner::{
    Cofre, ControleScan, Desfecho, EtapaConteineres, EtapaHash, ResultadoConteineres,
};
use dag_store::Catalogo;
use dag_store::conteineres as store;

fn b3(d: &[u8]) -> Vec<u8> {
    blake3::hash(d).as_bytes().to_vec()
}

fn conteudo(semente: u8, n: usize) -> Vec<u8> {
    let mut x = u32::from(semente) + 99;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        })
        .collect()
}

fn zip_com(arqs: &[(&str, Vec<u8>)], senha: Option<&str>) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (n, d) in arqs {
        let mut o =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        if let Some(s) = senha {
            o = o.with_aes_encryption(zip::AesMode::Aes256, s);
        }
        z.start_file(*n, o).unwrap();
        z.write_all(d).unwrap();
    }
    z.finish().unwrap().into_inner()
}

fn tar_gz_com(arqs: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for (n, d) in arqs {
        let mut h = tar::Header::new_gnu();
        h.set_size(d.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, n, &d[..]).unwrap();
    }
    let t = b.into_inner().unwrap();
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(&t).unwrap();
    e.finish().unwrap()
}

/// Cofre de brincadeira: "cifra" por XOR. A cifra de verdade é testada no `dag-agent`.
struct CofreFalso {
    ok: AtomicBool,
}

impl CofreFalso {
    fn novo(ok: bool) -> Arc<Self> {
        Arc::new(Self {
            ok: AtomicBool::new(ok),
        })
    }
}

impl Cofre for CofreFalso {
    fn disponivel(&self) -> bool {
        self.ok.load(Ordering::SeqCst)
    }

    fn cifrar(&self, s: &str) -> Option<Vec<u8>> {
        Some(s.bytes().map(|b| b ^ 0x5a).collect())
    }

    fn decifrar(&self, b: &[u8]) -> Option<String> {
        String::from_utf8(b.iter().map(|x| x ^ 0x5a).collect()).ok()
    }
}

struct Cenario {
    _tmp: tempfile::TempDir,
    raiz: PathBuf,
    cat: Catalogo,
}

fn cenario(montar: impl FnOnce(&Path)) -> Cenario {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    fs::create_dir_all(&raiz).unwrap();
    montar(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    hashear(&raiz, &mut cat);
    Cenario {
        _tmp: tmp,
        raiz,
        cat,
    }
}

fn hashear(raiz: &Path, cat: &mut Catalogo) {
    let run = scan(raiz).executar(cat).unwrap().execucao_id;
    EtapaHash::nova(ExecucaoId(run)).executar(cat).unwrap();
}

fn rodar(c: &mut Cenario, cofre: Arc<CofreFalso>) -> ResultadoConteineres {
    EtapaConteineres::nova()
        .com_cofre(cofre)
        .executar(&mut c.cat)
        .unwrap()
}

fn estado(c: &Cenario, nome: &str) -> String {
    c.cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT c.estado FROM conteineres c JOIN files f ON f.id = c.file_id WHERE f.name_display = ?1",
            [nome],
            |l| l.get(0),
        )
        .unwrap()
}

fn file_id(c: &Cenario, nome: &str) -> i64 {
    c.cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT id FROM files WHERE name_display = ?1",
            [nome],
            |l| l.get(0),
        )
        .unwrap()
}

fn hash_do_membro(c: &Cenario, conteiner: &str, caminho: &str) -> Option<Vec<u8>> {
    c.cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT m.hash FROM membros m JOIN files f ON f.id = m.conteiner_file_id
             WHERE f.name_display = ?1 AND m.caminho_display = ?2",
            [conteiner, caminho],
            |l| l.get(0),
        )
        .unwrap()
}

fn listar(raiz: &Path) -> BTreeSet<(PathBuf, u64)> {
    let mut v = BTreeSet::new();
    let mut pilha = vec![raiz.to_path_buf()];
    while let Some(d) = pilha.pop() {
        for e in fs::read_dir(d).unwrap() {
            let e = e.unwrap();
            let m = e.metadata().unwrap();
            if m.is_dir() {
                pilha.push(e.path());
            } else {
                v.insert((e.path(), m.len()));
            }
        }
    }
    v
}

#[test]
fn membros_com_hash_blake3_aninhado_e_duplicado_de_arquivo_solto() {
    let a = conteudo(1, 70_000);
    let b = b"texto".repeat(500);
    let interno = zip_com(&[("fundo.txt", b"no fundo".to_vec())], None);
    let mut c = cenario(|r| {
        fs::write(
            r.join("pacote.zip"),
            zip_com(&[("a.bin", a.clone()), ("d/b.txt", b.clone())], None),
        )
        .unwrap();
        fs::write(
            r.join("dados.tar.gz"),
            tar_gz_com(&[("x/interno.zip", interno.clone()), ("y.txt", b"y".to_vec())]),
        )
        .unwrap();
        // O mesmo conteúdo de um Membro, solto, e um .dat que não é compactado.
        fs::write(r.join("a_solto.bin"), &a).unwrap();
        fs::write(r.join("nao_sou.zip"), b"isto nao e um zip").unwrap();
    });
    let antes = listar(&c.raiz);
    let r = rodar(&mut c, CofreFalso::novo(true));
    assert_eq!(r.desfecho, Desfecho::Concluida);
    assert_eq!(r.lidos, 2);
    assert_eq!(r.nao_conteineres, 1);
    assert_eq!(estado(&c, "pacote.zip"), "lido");
    assert_eq!(estado(&c, "nao_sou.zip"), "nao_conteiner");
    assert_eq!(hash_do_membro(&c, "pacote.zip", "a.bin"), Some(b3(&a)));
    assert_eq!(hash_do_membro(&c, "pacote.zip", "d/b.txt"), Some(b3(&b)));
    // Aninhado: o zip dentro do tar.gz tem hash e filhos.
    assert_eq!(
        hash_do_membro(&c, "dados.tar.gz", "x/interno.zip"),
        Some(b3(&interno))
    );
    assert_eq!(
        hash_do_membro(&c, "dados.tar.gz", "fundo.txt"),
        Some(b3(b"no fundo"))
    );
    let pai: Option<i64> = c
        .cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT pai_ordem FROM membros WHERE caminho_display = 'fundo.txt'",
            [],
            |l| l.get(0),
        )
        .unwrap();
    assert!(pai.is_some());
    // O membro tem o mesmo (tamanho, hash) do arquivo solto: é a base do grupo de duplicados.
    let dupl = n(
        &c.cat,
        "SELECT COUNT(*) FROM membros m JOIN hashes h ON h.hash = m.hash AND h.size_bytes = m.tamanho
         JOIN files f ON f.id = h.file_id WHERE f.name_display = 'a_solto.bin'",
    );
    assert_eq!(dupl, 1);
    // Nada em `files`/`folders` por causa dos Membros: só os 4 arquivos reais.
    assert_eq!(n(&c.cat, "SELECT COUNT(*) FROM files"), 4);
    // Segurança: nenhum arquivo criado, apagado ou alterado fora do Catálogo.
    assert_eq!(listar(&c.raiz), antes);
    // Segunda passada: nada a ler.
    let r2 = rodar(&mut c, CofreFalso::novo(true));
    assert_eq!((r2.lidos, r2.membros, r2.copiados), (0, 0, 0));
}

#[test]
fn copia_identica_reaproveita_os_membros_sem_reler() {
    let z = zip_com(&[("a.txt", b"aaa".to_vec())], None);
    let mut c = cenario(|r| {
        fs::write(r.join("um.zip"), &z).unwrap();
        fs::write(r.join("dois.zip"), &z).unwrap();
    });
    let r = rodar(&mut c, CofreFalso::novo(true));
    // Uma foi lida; a outra copiou a lista de Membros (ou o contrário, conforme a ordem).
    assert_eq!(r.lidos + r.copiados, 2);
    assert_eq!(r.copiados, 1);
    assert_eq!(hash_do_membro(&c, "um.zip", "a.txt"), Some(b3(b"aaa")));
    assert_eq!(hash_do_membro(&c, "dois.zip", "a.txt"), Some(b3(b"aaa")));
    assert_eq!(estado(&c, "um.zip"), "lido");
    assert_eq!(estado(&c, "dois.zip"), "lido");
}

#[test]
fn cifrado_vira_pedido_de_senha_e_depois_de_fornecida_e_lido() {
    let z = zip_com(
        &[("segredo.txt", b"conteudo secreto".to_vec())],
        Some("abracadabra"),
    );
    let mut c = cenario(|r| {
        fs::write(r.join("trancado.zip"), &z).unwrap();
    });
    let cofre = CofreFalso::novo(true);
    let r = rodar(&mut c, cofre.clone());
    assert_eq!((r.lidos, r.aguardando_senha), (0, 1));
    assert_eq!(estado(&c, "trancado.zip"), "aguardando_senha");
    assert_eq!(n(&c.cat, "SELECT COUNT(*) FROM membros"), 0);
    let abertos = store::listar_pedidos(c.cat.armazem().conexao(), true).unwrap();
    assert_eq!(abertos.len(), 1);
    assert_eq!(abertos[0].tipo, "pedido_senha");
    assert!(
        abertos[0]
            .objeto
            .as_deref()
            .unwrap()
            .ends_with("trancado.zip")
    );
    // O trabalho não parou: rodar de novo não repete o pedido nem lê o que espera.
    let r = rodar(&mut c, cofre.clone());
    assert_eq!(r.aguardando_senha, 0);
    assert_eq!(
        store::listar_pedidos(c.cat.armazem().conexao(), true)
            .unwrap()
            .len(),
        1
    );

    // O usuário fornece a senha: fica ligada ao hash, o Contêiner volta à fila e o pedido fecha.
    let f = file_id(&c, "trancado.zip");
    let h = store::hash_do_arquivo(c.cat.armazem().conexao(), f)
        .unwrap()
        .unwrap();
    let blob = cofre.cifrar("abracadabra").unwrap();
    store::guardar_senha(c.cat.armazem().conexao(), &h, &blob, 1).unwrap();
    assert_eq!(
        store::reenfileirar(c.cat.armazem().conexao(), f, 2).unwrap(),
        1
    );
    assert!(
        store::listar_pedidos(c.cat.armazem().conexao(), true)
            .unwrap()
            .is_empty()
    );
    let r = rodar(&mut c, cofre);
    assert_eq!(r.lidos, 1);
    assert_eq!(
        hash_do_membro(&c, "trancado.zip", "segredo.txt"),
        Some(b3(b"conteudo secreto"))
    );
    assert_eq!(store::contar_abertos(c.cat.armazem().conexao()).unwrap(), 0);
    // A senha não aparece em claro em nenhuma tabela do Catálogo.
    let achou: i64 = n(
        &c.cat,
        "SELECT (SELECT COUNT(*) FROM senhas_conteiner WHERE instr(cifrada, CAST('abracadabra' AS BLOB)) > 0)
              + (SELECT COUNT(*) FROM pedidos_servico WHERE motivo LIKE '%abracadabra%')",
    );
    assert_eq!(achou, 0);
}

#[test]
fn senha_do_cofre_abre_sozinha_e_fica_ligada_ao_hash() {
    let z = zip_com(&[("s.txt", b"s".to_vec())], Some("do-cofre"));
    let mut c = cenario(|r| {
        fs::write(r.join("c.zip"), &z).unwrap();
    });
    let cofre = CofreFalso::novo(true);
    let conn = c.cat.armazem().conexao();
    store::cofre_adicionar(conn, "errada", &cofre.cifrar("outra").unwrap(), 1).unwrap();
    store::cofre_adicionar(conn, "certa", &cofre.cifrar("do-cofre").unwrap(), 2).unwrap();
    let r = rodar(&mut c, cofre);
    assert_eq!((r.lidos, r.aguardando_senha), (1, 0));
    assert_eq!(n(&c.cat, "SELECT COUNT(*) FROM senhas_conteiner"), 1);
    assert_eq!(n(&c.cat, "SELECT com_senha FROM conteineres"), 1);
    assert_eq!(store::contar_abertos(c.cat.armazem().conexao()).unwrap(), 0);
}

#[test]
fn chaveiro_trancado_espera_e_destrancado_continua() {
    let z = zip_com(&[("s.txt", b"s".to_vec())], Some("do-cofre"));
    let mut c = cenario(|r| {
        fs::write(r.join("c.zip"), &z).unwrap();
    });
    let cofre = CofreFalso::novo(false);
    let r = rodar(&mut c, cofre.clone());
    assert_eq!(r.aguardando_senha, 1);
    assert_eq!(estado(&c, "c.zip"), "aguardando_chaveiro");
    store::cofre_adicionar(
        c.cat.armazem().conexao(),
        "x",
        &cofre.cifrar("do-cofre").unwrap(),
        1,
    )
    .unwrap();
    cofre.ok.store(true, Ordering::SeqCst);
    let r = rodar(&mut c, cofre);
    assert_eq!(r.lidos, 1);
    assert_eq!(store::contar_abertos(c.cat.armazem().conexao()).unwrap(), 0);
}

#[test]
fn corrompido_vira_falha_de_leitura_e_tentar_de_novo_relê() {
    let z = zip_com(&[("a.txt", conteudo(3, 5000))], None);
    let mut c = cenario(|r| {
        fs::write(r.join("quebrado.zip"), &z[..z.len() / 2]).unwrap();
        fs::write(
            r.join("bom.zip"),
            zip_com(&[("b.txt", b"b".to_vec())], None),
        )
        .unwrap();
    });
    let r = rodar(&mut c, CofreFalso::novo(true));
    assert_eq!((r.lidos, r.falhas), (1, 1));
    assert_eq!(r.desfecho, Desfecho::ConcluidaComErros);
    assert_eq!(estado(&c, "quebrado.zip"), "falha");
    assert_eq!(
        n(
            &c.cat,
            "SELECT COUNT(*) FROM membros m JOIN files f ON f.id = m.conteiner_file_id WHERE f.name_display = 'quebrado.zip'"
        ),
        0
    );
    let p = store::listar_pedidos(c.cat.armazem().conexao(), true).unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].tipo, "falha_leitura");
    assert!(!p[0].motivo.is_empty());
    // Tentar de novo (nada mudou): falha de novo, sem duplicar o pedido.
    let f = file_id(&c, "quebrado.zip");
    store::reenfileirar(c.cat.armazem().conexao(), f, 5).unwrap();
    let r = rodar(&mut c, CofreFalso::novo(true));
    assert_eq!(r.falhas, 1);
    assert_eq!(
        store::listar_pedidos(c.cat.armazem().conexao(), true)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn cancelar_no_meio_volta_para_a_fila_sem_membros_parciais_e_retoma() {
    let mut c = cenario(|r| {
        for i in 0..6u8 {
            fs::write(
                r.join(format!("z{i}.zip")),
                zip_com(
                    &[
                        ("a.bin", conteudo(i, 20_000)),
                        ("b.bin", conteudo(i + 50, 20_000)),
                    ],
                    None,
                ),
            )
            .unwrap();
        }
    });
    let controle = ControleScan::novo();
    let cc = controle.clone();
    let r = EtapaConteineres::nova()
        .com_controle(controle)
        .com_progresso(move |p| {
            if p.andamento.arquivos_feitos >= 2 {
                cc.cancelar();
            }
        })
        .com_config(dag_scanner::ConfigConteineres {
            intervalo_de_progresso: std::time::Duration::ZERO,
            max_discos: 1,
            ..Default::default()
        })
        .executar(&mut c.cat)
        .unwrap();
    assert_eq!(r.desfecho, Desfecho::Cancelada);
    let andamento = store::andamento(c.cat.armazem().conexao()).unwrap();
    assert!(andamento.lidos >= 1 && andamento.lidos < 6, "{andamento:?}");
    assert_eq!(andamento.lendo, 0);
    // Todo Contêiner que não terminou não tem Membro nenhum (tudo ou nada).
    assert_eq!(
        n(
            &c.cat,
            "SELECT COUNT(*) FROM membros m JOIN conteineres c ON c.file_id = m.conteiner_file_id WHERE c.estado <> 'lido'"
        ),
        0
    );
    // Retoma: lê só o que faltava.
    let r = rodar(&mut c, CofreFalso::novo(true));
    assert_eq!(r.desfecho, Desfecho::Concluida);
    assert_eq!(r.lidos + andamento.lidos, 6);
    assert_eq!(n(&c.cat, "SELECT COUNT(*) FROM membros"), 12);
}

#[test]
fn arquivo_que_mudou_depois_do_hash_nao_e_lido_com_o_hash_velho() {
    let mut c = cenario(|r| {
        fs::write(r.join("m.zip"), zip_com(&[("a.txt", b"a".to_vec())], None)).unwrap();
    });
    // Troca o conteúdo depois do hash (tamanho diferente).
    fs::write(
        c.raiz.join("m.zip"),
        zip_com(
            &[("a.txt", b"a".to_vec()), ("b.txt", b"bbbbb".to_vec())],
            None,
        ),
    )
    .unwrap();
    let r = rodar(&mut c, CofreFalso::novo(true));
    assert_eq!((r.lidos, r.membros), (0, 0));
    assert_eq!(estado(&c, "m.zip"), "pendente");
}
