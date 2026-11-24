//! Imagens de Disco pela etapa "Conteúdo de contêineres" inteira (ADR 0006): FAT, ext4 e VHDX
//! num acervo varrido, hashes dos Membros, duplicados contra arquivo solto, Falha de Leitura e
//! retomada no meio de uma imagem sem recomeçar do zero.
#![allow(clippy::unwrap_used)]

mod comum;
#[path = "../../dag-imagens/tests/common/mod.rs"]
#[allow(clippy::unwrap_used, clippy::manual_is_multiple_of)]
mod imagens;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use comum::*;
use dag_conteineres::{
    ErroConteiner, FabricaLeitor, Fonte, FonteBox, LeitorConteiner, MembroInfo, Registro, Resumo,
    Visita,
};
use dag_core::ExecucaoId;
use dag_scanner::{ConfigConteineres, ControleScan, Desfecho, EtapaConteineres, EtapaHash};
use dag_store::Catalogo;
use dag_store::conteineres as store;
use imagens::{No, dados, fat, part, virt};

fn b3(d: &[u8]) -> Vec<u8> {
    blake3::hash(d).as_bytes().to_vec()
}

fn tem_mke2fs() -> bool {
    Command::new("mke2fs").arg("-V").output().is_ok()
        || Path::new("/usr/sbin/mke2fs").exists()
        || Path::new("/sbin/mke2fs").exists()
}

/// Imagem ext4 com `mke2fs -d` (sem root, sem montar).
fn ext4(arquivos: &[(&str, Vec<u8>)], hard_link: Option<(&str, &str)>) -> Option<Vec<u8>> {
    if !tem_mke2fs() {
        eprintln!("PULADO: mke2fs não encontrado");
        return None;
    }
    let tmp = tempfile::tempdir().unwrap();
    let fonte = tmp.path().join("fonte");
    for (n, d) in arquivos {
        let p = fonte.join(n);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, d).unwrap();
    }
    if let Some((a, b)) = hard_link {
        fs::hard_link(fonte.join(a), fonte.join(b)).unwrap();
    }
    std::os::unix::fs::symlink("docs/leia.txt", fonte.join("atalho")).unwrap();
    let img = tmp.path().join("fs.img");
    let bin = ["/usr/sbin/mke2fs", "/sbin/mke2fs"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .unwrap_or("mke2fs");
    let out = Command::new(bin)
        .args(["-t", "ext4", "-q", "-d"])
        .arg(&fonte)
        .arg(&img)
        .arg("8M")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(fs::read(img).unwrap())
}

fn arvore_fat(dup: &[u8]) -> Vec<(String, No)> {
    vec![
        ("DUP.BIN".into(), No::Arq(dup.to_vec())),
        (
            "docs".into(),
            No::Dir(vec![("leia.txt".into(), No::Arq(b"ola da fat".to_vec()))]),
        ),
    ]
}

struct Cenario {
    _tmp: tempfile::TempDir,
    cat: Catalogo,
}

fn cenario(montar: impl FnOnce(&Path)) -> Cenario {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    fs::create_dir_all(&raiz).unwrap();
    montar(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = scan(&raiz).executar(&mut cat).unwrap().execucao_id;
    EtapaHash::nova(ExecucaoId(run)).executar(&mut cat).unwrap();
    Cenario { _tmp: tmp, cat }
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

fn membros_de(c: &Cenario, nome: &str) -> Vec<(String, String, Option<Vec<u8>>)> {
    let conn = c.cat.armazem().conexao();
    let mut st = conn
        .prepare(
            "SELECT m.caminho_display, m.tipo, m.hash FROM membros m JOIN files f ON f.id = m.conteiner_file_id
             WHERE f.name_display = ?1 ORDER BY m.ordem",
        )
        .unwrap();
    st.query_map([nome], |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn hash_de(c: &Cenario, nome: &str, caminho: &str) -> Option<Vec<u8>> {
    membros_de(c, nome)
        .into_iter()
        .find(|m| m.0 == caminho)
        .unwrap_or_else(|| panic!("{nome}:{caminho} não existe"))
        .2
}

fn pedidos_abertos(c: &Cenario, tipo: &str) -> Vec<String> {
    let conn = c.cat.armazem().conexao();
    let mut st = conn
        .prepare("SELECT motivo FROM pedidos_servico WHERE estado = 'aberto' AND tipo = ?1")
        .unwrap();
    st.query_map([tipo], |l| l.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn fat_ext4_e_vhdx_catalogam_membros_com_hash_e_duplicam_com_arquivo_solto() {
    let dup = dados(7, 30_000);
    let fat_img = fat::fat_img(12, &arvore_fat(&dup), false);
    let Some(ext) = ext4(
        &[
            ("dup2.bin", dup.clone()),
            ("docs/leia.txt", b"ola da ext4".to_vec()),
            ("orig.bin", dados(9, 5000)),
        ],
        Some(("orig.bin", "docs/hard.bin")),
    ) else {
        return;
    };
    // VHDX com MBR e uma partição FAT.
    let disco = part::mbr(&[(0x0c, fat::fat_img(16, &arvore_fat(&dup), false))]);
    let vhdx = virt::vhdx(&virt::Esparso::denso(disco), 1 << 20, false, false);
    let mut c = cenario(|r| {
        fs::write(r.join("floppy.img"), &fat_img).unwrap();
        fs::write(r.join("wsl.img"), &ext).unwrap();
        fs::write(r.join("win.vhdx"), &vhdx).unwrap();
        fs::write(r.join("solto.bin"), &dup).unwrap();
        // Arquivo qualquer com extensão de imagem: nunca é imagem.
        fs::write(r.join("lixo.img"), dados(1, 4096)).unwrap();
        fs::write(r.join("zeros.raw"), vec![0u8; 4096]).unwrap();
    });
    let r = EtapaConteineres::nova().executar(&mut c.cat).unwrap();
    assert_eq!(r.desfecho, Desfecho::Concluida);
    assert_eq!(r.lidos, 3);
    assert_eq!(r.nao_conteineres, 2);
    assert_eq!(estado(&c, "lixo.img"), "nao_conteiner");
    assert_eq!(estado(&c, "zeros.raw"), "nao_conteiner");
    for n in ["floppy.img", "wsl.img", "win.vhdx"] {
        assert_eq!(estado(&c, n), "lido", "{n}");
    }
    // Prefixo da partição no caminho interno e hash BLAKE3 de cada arquivo.
    assert_eq!(hash_de(&c, "floppy.img", "p1/DUP.BIN"), Some(b3(&dup)));
    assert_eq!(
        hash_de(&c, "floppy.img", "p1/docs/leia.txt"),
        Some(b3(b"ola da fat"))
    );
    assert_eq!(hash_de(&c, "win.vhdx", "p1/DUP.BIN"), Some(b3(&dup)));
    assert_eq!(hash_de(&c, "wsl.img", "p1/dup2.bin"), Some(b3(&dup)));
    // Pasta e link: sem hash (conteúdo próprio nenhum).
    let wsl = membros_de(&c, "wsl.img");
    let docs = wsl.iter().find(|m| m.0 == "p1/docs").unwrap();
    assert_eq!((docs.1.as_str(), docs.2.is_none()), ("pasta", true));
    let atalho = wsl.iter().find(|m| m.0 == "p1/atalho").unwrap();
    assert_eq!((atalho.1.as_str(), atalho.2.is_none()), ("link", true));
    // Hard link dentro da imagem: um só Membro com o conteúdo (o repetido é pulado).
    let orig = ["p1/orig.bin", "p1/docs/hard.bin"]
        .iter()
        .filter(|p| wsl.iter().any(|m| m.0 == **p))
        .count();
    assert_eq!(orig, 1, "{wsl:?}");
    // Duplicados: o arquivo solto casa com um Membro em cada imagem.
    let casados = c
        .cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT COUNT(*) FROM membros m JOIN hashes h ON h.hash = m.hash AND h.size_bytes = m.tamanho
             JOIN files f ON f.id = h.file_id WHERE f.name_display = 'solto.bin'",
            [],
            |l| l.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(casados, 3);
    // Nada em files/folders por causa dos Membros: só os 6 arquivos reais.
    assert_eq!(n(&c.cat, "SELECT COUNT(*) FROM files"), 6);
    // Sem avisos: nenhuma Falha de Leitura.
    assert!(pedidos_abertos(&c, "falha_leitura").is_empty());
}

#[test]
fn imagem_corrompida_vira_falha_de_leitura_e_particao_ilegivel_um_aviso_agregado() {
    let dup = dados(3, 2000);
    // Cabeçalho VHDX válido na assinatura, resto lixo: não abre.
    let mut quebrada = b"vhdxfile".to_vec();
    quebrada.extend(vec![0u8; 4000]);
    // Disco com uma partição FAT boa e duas ilegíveis: lido, com um aviso agregado.
    let disco = part::mbr(&[
        (0x0c, fat::fat_img(16, &arvore_fat(&dup), false)),
        (0x83, vec![0xAB; 512 * 64]),
        (0x83, vec![0xCD; 512 * 64]),
    ]);
    let mut c = cenario(|r| {
        fs::write(r.join("quebrada.vhdx"), &quebrada).unwrap();
        fs::write(r.join("parcial.img"), &disco).unwrap();
    });
    let r = EtapaConteineres::nova().executar(&mut c.cat).unwrap();
    assert_eq!(r.falhas, 1);
    assert_eq!(r.desfecho, Desfecho::ConcluidaComErros);
    assert_eq!(estado(&c, "quebrada.vhdx"), "falha");
    assert!(membros_de(&c, "quebrada.vhdx").is_empty());
    assert_eq!(estado(&c, "parcial.img"), "lido");
    assert_eq!(hash_de(&c, "parcial.img", "p1/DUP.BIN"), Some(b3(&dup)));
    let falhas = pedidos_abertos(&c, "falha_leitura");
    assert_eq!(falhas.len(), 2, "{falhas:?}");
    let agregado = falhas
        .iter()
        .find(|m| m.starts_with("lido com"))
        .expect("pedido agregado da imagem parcial");
    assert!(agregado.contains("2 problema(s)"), "{agregado}");
    assert!(agregado.contains("partição 2"), "{agregado}");
}

/// Envolve o leitor de imagens e dorme a cada Membro: faz a leitura durar o bastante para
/// cancelar no meio, sem depender de arquivos gigantes.
struct FabricaLenta {
    inner: Arc<dyn FabricaLeitor>,
    espera: Duration,
}

struct LeitorLento {
    inner: Box<dyn LeitorConteiner>,
    espera: Duration,
}

impl FabricaLeitor for FabricaLenta {
    fn nome(&self) -> &'static str {
        self.inner.nome()
    }

    fn reconhece(&self, f: &mut dyn Fonte) -> std::io::Result<bool> {
        self.inner.reconhece(f)
    }

    fn abrir(
        &self,
        f: FonteBox,
        s: Option<&str>,
    ) -> Result<Box<dyn LeitorConteiner>, ErroConteiner> {
        Ok(Box::new(LeitorLento {
            inner: self.inner.abrir(f, s)?,
            espera: self.espera,
        }))
    }
}

impl LeitorConteiner for LeitorLento {
    fn formato(&self) -> &'static str {
        self.inner.formato()
    }

    fn retomavel(&self) -> bool {
        self.inner.retomavel()
    }

    fn avisos(&self) -> Vec<String> {
        self.inner.avisos()
    }

    fn percorrer(&mut self, visita: &mut Visita<'_>) -> Result<Resumo, ErroConteiner> {
        let espera = self.espera;
        self.inner.percorrer(&mut |info: MembroInfo, r| {
            std::thread::sleep(espera);
            visita(info, r)
        })
    }
}

fn registro_lento() -> Registro {
    let mut reg = Registro::padrao();
    reg.registrar(Arc::new(FabricaLenta {
        inner: Arc::new(dag_conteineres::FabricaImagens),
        espera: Duration::from_millis(15),
    }));
    reg
}

fn conjunto(c: &Cenario, nome: &str) -> BTreeSet<(String, Option<Vec<u8>>)> {
    membros_de(c, nome)
        .into_iter()
        .map(|m| (m.0, m.2))
        .collect()
}

#[test]
fn retomar_no_meio_da_imagem_nao_recomeca_do_zero() {
    // 40 arquivos de 20 KB: 800 KB de conteúdo, em uma imagem FAT16.
    let arv: Vec<(String, No)> = (0..40u64)
        .map(|i| (format!("F{i:03}.BIN"), No::Arq(dados(100 + i, 20_000))))
        .collect();
    let img = fat::fat_img(16, &arv, false);
    let total_bytes: u64 = 40 * 20_000;
    let mut c = cenario(|r| fs::write(r.join("grande.img"), &img).unwrap());
    // Referência: leitura de uma vez só em outro Catálogo.
    let mut ref_c = cenario(|r| fs::write(r.join("grande.img"), &img).unwrap());
    EtapaConteineres::nova().executar(&mut ref_c.cat).unwrap();
    let esperado = conjunto(&ref_c, "grande.img");
    assert_eq!(esperado.len(), 40);

    let cfg = ConfigConteineres {
        intervalo_de_progresso: Duration::from_millis(5),
        intervalo_checkpoint: Duration::ZERO,
        max_discos: 1,
        ..Default::default()
    };
    // Primeira passada: cancela quando uns 10 Membros já foram gravados.
    let controle = ControleScan::novo();
    let cc = controle.clone();
    let r1 = EtapaConteineres::nova()
        .com_registro(registro_lento())
        .com_controle(controle)
        .com_config(cfg.clone())
        .com_progresso(move |p| {
            if p.membros >= 10 {
                cc.cancelar();
            }
        })
        .executar(&mut c.cat)
        .unwrap();
    assert_eq!(r1.desfecho, Desfecho::Cancelada);
    assert_eq!(estado(&c, "grande.img"), "pendente");
    // O cursor e os Membros ficaram, gravados juntos.
    let (indice, ordem, bytes): (i64, i64, i64) = c
        .cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT retomada_indice, retomada_ordem, retomada_bytes FROM conteineres",
            [],
            |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)),
        )
        .unwrap();
    let gravados = n(&c.cat, "SELECT COUNT(*) FROM membros");
    assert!((10..40).contains(&indice), "{indice}");
    assert_eq!(gravados, indice);
    assert_eq!(ordem, indice);
    assert_eq!(bytes as u64, 20_000 * indice as u64);

    // Segunda passada: continua de onde parou e só lê o resto.
    let r2 = EtapaConteineres::nova()
        .com_registro(registro_lento())
        .com_config(cfg)
        .executar(&mut c.cat)
        .unwrap();
    assert_eq!(r2.desfecho, Desfecho::Concluida);
    assert_eq!(r2.lidos, 1);
    assert_eq!(r2.membros as i64, 40 - indice);
    assert_eq!(r2.bytes_lidos, total_bytes - bytes as u64);
    assert!(r2.bytes_lidos < total_bytes);
    assert_eq!(estado(&c, "grande.img"), "lido");
    assert_eq!(conjunto(&c, "grande.img"), esperado);
    // Cursor limpo ao concluir.
    assert_eq!(
        n(
            &c.cat,
            "SELECT COUNT(*) FROM conteineres WHERE retomada_indice IS NOT NULL"
        ),
        0
    );
}

#[test]
fn queda_no_meio_mantem_membros_e_cursor_e_hash_mudado_descarta() {
    let arv: Vec<(String, No)> = (0..20u64)
        .map(|i| (format!("F{i:03}.BIN"), No::Arq(dados(500 + i, 5000))))
        .collect();
    let img = fat::fat_img(16, &arv, false);
    let mut c = cenario(|r| fs::write(r.join("a.img"), &img).unwrap());
    let cfg = ConfigConteineres {
        intervalo_de_progresso: Duration::from_millis(5),
        intervalo_checkpoint: Duration::ZERO,
        max_discos: 1,
        ..Default::default()
    };
    let controle = ControleScan::novo();
    let cc = controle.clone();
    EtapaConteineres::nova()
        .com_registro(registro_lento())
        .com_controle(controle)
        .com_config(cfg)
        .com_progresso(move |p| {
            if p.membros >= 5 {
                cc.cancelar();
            }
        })
        .executar(&mut c.cat)
        .unwrap();
    // Simula a queda do processo: o item ficou `lendo`.
    c.cat
        .armazem()
        .conexao()
        .execute("UPDATE conteineres SET estado = 'lendo'", [])
        .unwrap();
    let antes = n(&c.cat, "SELECT COUNT(*) FROM membros");
    assert!(antes >= 5);
    // Reabertura: devolve à fila mantendo Membros e cursor.
    store::devolver_lendo(c.cat.armazem().conexao(), 1).unwrap();
    assert_eq!(n(&c.cat, "SELECT COUNT(*) FROM membros"), antes);
    // Hash do Contêiner mudou desde o cursor: o cursor não vale e a leitura é do zero.
    c.cat
        .armazem()
        .conexao()
        .execute("UPDATE conteineres SET retomada_hash = zeroblob(32)", [])
        .unwrap();
    let r = EtapaConteineres::nova().executar(&mut c.cat).unwrap();
    assert_eq!(r.membros, 20);
    assert_eq!(n(&c.cat, "SELECT COUNT(*) FROM membros"), 20);
}
