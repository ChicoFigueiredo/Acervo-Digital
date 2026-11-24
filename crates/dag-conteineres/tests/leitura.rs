//! Leitura de compactados gerados em memória pelos escritores das próprias crates.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Cursor, Write};
use std::sync::Arc;

use dag_conteineres::{
    Controle, ErroConteiner, EstadoMembro, FabricaLeitor, Fonte, FonteBox, LeitorConteiner,
    MembroInfo, MembroLido, Opcoes, Registro, Resumo, TipoMembro, Visita, ler_conteiner,
};

fn pseudo(semente: u8, n: usize) -> Vec<u8> {
    let mut x = u32::from(semente) + 12345;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        })
        .collect()
}

fn arquivos() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("a.txt", b"ola, mundo".to_vec()),
        ("dir/b.bin", pseudo(1, 300_000)),
        ("dir/vazio", Vec::new()),
        ("dir/sub/c.txt", b"texto repetido ".repeat(2000)),
    ]
}

fn b3(d: &[u8]) -> [u8; 32] {
    *blake3::hash(d).as_bytes()
}

fn ler(
    bytes: Vec<u8>,
    senha: Option<&str>,
    opcoes: Option<Opcoes>,
) -> Result<(Option<&'static str>, Vec<MembroLido>), ErroConteiner> {
    let mut op = opcoes.unwrap_or_default();
    op.senha = senha.map(str::to_owned);
    let mut v = Vec::new();
    let r = ler_conteiner(
        &Registro::padrao(),
        Box::new(Cursor::new(bytes)),
        &op,
        &|| false,
        &mut |_| {},
        &mut |m| {
            v.push(m);
            Ok(())
        },
    )?;
    Ok((r.formato, v))
}

fn confere(membros: &[MembroLido]) {
    for (nome, dados) in arquivos() {
        let m = membros
            .iter()
            .find(|m| m.info.caminho == nome.as_bytes())
            .unwrap_or_else(|| panic!("falta {nome}"));
        assert_eq!(m.estado, EstadoMembro::Lido, "{nome}");
        assert_eq!(m.hash, Some(b3(&dados)), "hash de {nome}");
        assert_eq!(
            m.info.tamanho,
            Some(dados.len() as u64),
            "tamanho de {nome}"
        );
    }
}

fn zip_bytes(senha: Option<&str>, armazenado: bool) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (n, d) in arquivos() {
        let mut o = SimpleFileOptions::default().compression_method(if armazenado {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        });
        if let Some(s) = senha {
            o = o.with_aes_encryption(zip::AesMode::Aes256, s);
        }
        z.start_file(n, o).unwrap();
        z.write_all(&d).unwrap();
    }
    z.finish().unwrap().into_inner()
}

fn sete_z_bytes(senha: Option<&str>) -> Vec<u8> {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter, EncoderMethod, Password, encoder_options};
    let mut w = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    if let Some(s) = senha {
        w.set_content_methods(vec![
            encoder_options::AesEncoderOptions::new(Password::new(s)).into(),
            sevenz_rust2::EncoderConfiguration::new(EncoderMethod::LZMA2),
        ]);
    }
    for (n, d) in arquivos() {
        w.push_archive_entry(ArchiveEntry::new_file(n), Some(Cursor::new(d)))
            .unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn tar_bytes() -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for (n, d) in arquivos() {
        let mut h = tar::Header::new_gnu();
        h.set_size(d.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(1_700_000_000);
        h.set_cksum();
        b.append_data(&mut h, n, &d[..]).unwrap();
    }
    b.into_inner().unwrap()
}

fn gz(d: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(d).unwrap();
    e.finish().unwrap()
}

fn bz2(d: &[u8]) -> Vec<u8> {
    let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::best());
    e.write_all(d).unwrap();
    e.finish().unwrap()
}

fn xz(d: &[u8]) -> Vec<u8> {
    let mut e =
        lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1)).unwrap();
    e.write_all(d).unwrap();
    e.finish().unwrap()
}

fn zst(d: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(d, ruzstd::encoding::CompressionLevel::Fastest)
}

fn rar_bytes(versao: rars::ArchiveVersion, senha: Option<&str>) -> Vec<u8> {
    let mut b = rars::Builder::new(versao);
    if let Some(s) = senha {
        b = b.password(Some(s.as_bytes().to_vec()));
    }
    for (n, d) in arquivos() {
        b.add_bytes(n.as_bytes().to_vec(), d, None, None).unwrap();
    }
    let mut out = Vec::new();
    b.write_to(&mut out, &rars::WriterResources::default(), None)
        .unwrap();
    out
}

#[test]
fn zip_deflate_e_armazenado() {
    for arm in [false, true] {
        let (f, m) = ler(zip_bytes(None, arm), None, None).unwrap();
        assert_eq!(f, Some("zip"));
        assert_eq!(m.len(), 4);
        confere(&m);
    }
}

#[test]
fn sete_z_simples() {
    let (f, m) = ler(sete_z_bytes(None), None, None).unwrap();
    assert_eq!(f, Some("7z"));
    confere(&m);
}

#[test]
fn rar_versoes() {
    for v in [
        rars::ArchiveVersion::Rar50,
        rars::ArchiveVersion::Rar70,
        rars::ArchiveVersion::Rar29,
    ] {
        let (f, m) = ler(rar_bytes(v, None), None, None).unwrap();
        assert_eq!(f, Some("rar"), "{v:?}");
        confere(&m);
    }
}

#[test]
fn tar_e_comprimidos() {
    let t = tar_bytes();
    let casos: Vec<(&str, Vec<u8>)> = vec![
        ("tar", t.clone()),
        ("tar.gz", gz(&t)),
        ("tar.bz2", bz2(&t)),
        ("tar.xz", xz(&t)),
        ("tar.zst", zst(&t)),
    ];
    for (nome, bytes) in casos {
        let (f, m) = ler(bytes, None, None).unwrap();
        assert_eq!(f, Some(nome));
        confere(&m);
    }
}

#[test]
fn gz_que_nao_e_tar_nao_e_conteiner() {
    let (f, m) = ler(gz(b"um log qualquer\n".repeat(100).as_slice()), None, None).unwrap();
    assert_eq!(f, None);
    assert!(m.is_empty());
    let (f, _) = ler(pseudo(9, 10_000), None, None).unwrap();
    assert_eq!(f, None);
}

#[test]
fn zip_dentro_de_tar_gz_com_hash_do_aninhado() {
    let z = zip_bytes(None, false);
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_size(z.len() as u64);
    h.set_cksum();
    b.append_data(&mut h, "pacote/interno.zip", &z[..]).unwrap();
    let mut h = tar::Header::new_gnu();
    h.set_size(3);
    h.set_cksum();
    b.append_data(&mut h, "pacote/solto.txt", &b"abc"[..])
        .unwrap();
    let externo = gz(&b.into_inner().unwrap());
    let (f, m) = ler(externo, None, None).unwrap();
    assert_eq!(f, Some("tar.gz"));
    let pai = m
        .iter()
        .find(|m| m.info.caminho == b"pacote/interno.zip")
        .unwrap();
    assert_eq!(pai.hash, Some(b3(&z)));
    assert_eq!(pai.formato_interno, Some("zip"));
    assert_eq!(pai.pai, None);
    let filhos: Vec<_> = m.iter().filter(|m| m.pai == Some(pai.ordem)).collect();
    assert_eq!(filhos.len(), 4);
    let a = filhos.iter().find(|m| m.info.caminho == b"a.txt").unwrap();
    assert_eq!(a.hash, Some(b3(b"ola, mundo")));
    // O pai vem antes dos filhos.
    let pos = |o: u64| m.iter().position(|x| x.ordem == o).unwrap();
    assert!(pos(pai.ordem) < pos(a.ordem));
}

#[test]
fn profundidade_e_limite_de_memoria() {
    let z = zip_bytes(None, false);
    // zip dentro de zip dentro de zip: com profundidade 1, o terceiro nível não se abre.
    let mut nivel2 = {
        use zip::write::SimpleFileOptions;
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file("n2.zip", SimpleFileOptions::default())
            .unwrap();
        w.write_all(&z).unwrap();
        w.finish().unwrap().into_inner()
    };
    let mut nivel3 = {
        use zip::write::SimpleFileOptions;
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file("n3.zip", SimpleFileOptions::default())
            .unwrap();
        w.write_all(&nivel2).unwrap();
        w.finish().unwrap().into_inner()
    };
    std::mem::swap(&mut nivel2, &mut nivel3);
    let op = Opcoes {
        profundidade_max: 1,
        ..Opcoes::default()
    };
    let (_, m) = ler(nivel2.clone(), None, Some(op)).unwrap();
    // raiz (n3.zip) + n2.zip aberto por dentro (1 nível) mostrando o zip mais interno sem abrir.
    assert!(
        m.iter()
            .any(|x| x.info.caminho == b"n3.zip" && x.formato_interno == Some("zip"))
    );
    assert!(!m.iter().any(|x| x.info.caminho == b"a.txt"));
    let (_, m) = ler(nivel2.clone(), None, None).unwrap();
    assert!(m.iter().any(|x| x.info.caminho == b"a.txt"));

    // Limite de memória: aninhado grande é hasheado em fluxo e fica "não lido".
    let op = Opcoes {
        limite_aninhado: 1000,
        ..Opcoes::default()
    };
    let (_, m) = ler(nivel2, None, Some(op)).unwrap();
    let grande = m.iter().find(|x| x.info.caminho == b"n3.zip").unwrap();
    assert_eq!(grande.estado, EstadoMembro::AninhadoGrande);
    assert!(grande.hash.is_some());
    assert_eq!(m.len(), 1);
}

#[test]
fn cifrados_pedem_senha_e_abrem_com_a_certa() {
    let casos: Vec<(&str, Vec<u8>)> = vec![
        ("zip", zip_bytes(Some("segredo"), false)),
        ("7z", sete_z_bytes(Some("segredo"))),
        (
            "rar",
            rar_bytes(rars::ArchiveVersion::Rar50, Some("segredo")),
        ),
    ];
    for (nome, bytes) in casos {
        let e = ler(bytes.clone(), None, None).unwrap_err();
        assert!(matches!(e, ErroConteiner::SenhaNecessaria), "{nome}: {e:?}");
        let e = ler(bytes.clone(), Some("errada"), None).unwrap_err();
        assert!(matches!(e, ErroConteiner::SenhaIncorreta), "{nome}: {e:?}");
        let (_, m) = ler(bytes, Some("segredo"), None).unwrap();
        confere(&m);
    }
}

#[test]
fn corrompidos_e_truncados_dao_erro_e_nao_hash_falso() {
    let z = zip_bytes(None, false);
    let e = ler(z[..z.len() / 2].to_vec(), None, None).unwrap_err();
    assert!(matches!(e, ErroConteiner::Corrompido(_)), "{e:?}");
    let g = gz(&tar_bytes());
    let e = ler(g[..g.len() / 2].to_vec(), None, None).unwrap_err();
    assert!(
        matches!(e, ErroConteiner::Corrompido(_) | ErroConteiner::Io(_)),
        "{e:?}"
    );
    let s = sete_z_bytes(None);
    let e = ler(s[..s.len() - 40].to_vec(), None, None).unwrap_err();
    assert!(
        matches!(e, ErroConteiner::Corrompido(_) | ErroConteiner::Io(_)),
        "{e:?}"
    );
    let r = rar_bytes(rars::ArchiveVersion::Rar50, None);
    let e = ler(r[..r.len() / 2].to_vec(), None, None).unwrap_err();
    assert!(
        matches!(e, ErroConteiner::Corrompido(_) | ErroConteiner::Io(_)),
        "{e:?}"
    );
}

#[test]
fn cancelamento_interrompe() {
    let r = ler_conteiner(
        &Registro::padrao(),
        Box::new(Cursor::new(zip_bytes(None, false))),
        &Opcoes::default(),
        &|| true,
        &mut |_| {},
        &mut |_| Ok(()),
    );
    assert!(matches!(r, Err(ErroConteiner::Cancelado)));
}

/// Um formato de mentira, para provar que leitores externos (imagens de disco) plugam no
/// registro sem mexer na crate.
struct FabricaFalsa;

struct LeitorFalso;

impl LeitorConteiner for LeitorFalso {
    fn formato(&self) -> &'static str {
        "falso"
    }

    fn percorrer(&mut self, visita: &mut Visita<'_>) -> Result<Resumo, ErroConteiner> {
        let info = MembroInfo {
            caminho: b"p1/arquivo".to_vec(),
            tamanho: Some(3),
            mtime_ns: None,
            tipo: TipoMembro::Arquivo,
            cifrado: false,
        };
        assert_eq!(
            visita(info, &mut Cursor::new(b"xyz".to_vec()))?,
            Controle::Continuar
        );
        Ok(Resumo {
            membros: 1,
            bytes: 3,
        })
    }
}

impl FabricaLeitor for FabricaFalsa {
    fn nome(&self) -> &'static str {
        "falso"
    }

    fn reconhece(&self, fonte: &mut dyn Fonte) -> std::io::Result<bool> {
        let mut m = [0u8; 5];
        fonte.read_exact(&mut m)?;
        Ok(&m == b"FALSO")
    }

    fn abrir(
        &self,
        _fonte: FonteBox,
        _senha: Option<&str>,
    ) -> Result<Box<dyn LeitorConteiner>, ErroConteiner> {
        Ok(Box::new(LeitorFalso))
    }
}

#[test]
fn leitor_externo_plugado_no_registro() {
    let mut reg = Registro::padrao();
    reg.registrar(Arc::new(FabricaFalsa));
    let mut v = Vec::new();
    let r = ler_conteiner(
        &reg,
        Box::new(Cursor::new(b"FALSO...".to_vec())),
        &Opcoes::default(),
        &|| false,
        &mut |_| {},
        &mut |m| {
            v.push(m);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(r.formato, Some("falso"));
    assert_eq!(v[0].hash, Some(b3(b"xyz")));
}
