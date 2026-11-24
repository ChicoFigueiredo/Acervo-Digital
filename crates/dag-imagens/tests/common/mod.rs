#![allow(dead_code)]
//! Utilitários de teste: árvores de exemplo, construtores de imagens e coleta.

pub mod exfat;
pub mod fat;
pub mod iso;
pub mod part;

use dag_imagens::{Controle, Imagem, Resumo, TipoMembro, abrir};
use std::collections::BTreeMap;
use std::io::{Cursor, Read};

/// Nó de uma árvore de arquivos de teste.
#[derive(Clone)]
pub enum No {
    Arq(Vec<u8>),
    Dir(Vec<(String, No)>),
    Link(String),
}

/// Dados determinísticos pseudoaleatórios.
pub fn dados(semente: u64, n: usize) -> Vec<u8> {
    let mut x = semente.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// Árvore de exemplo, com nomes válidos em todos os sistemas de arquivos.
pub fn exemplo() -> Vec<(String, No)> {
    vec![
        ("leia-me.txt".into(), No::Arq(b"ola mundo\n".to_vec())),
        ("CONFIG.SYS".into(), No::Arq(dados(1, 700))),
        ("vazio".into(), No::Arq(vec![])),
        (
            "docs".into(),
            No::Dir(vec![
                ("Relatório final.pdf".into(), No::Arq(dados(2, 5000))),
                (
                    "sub".into(),
                    No::Dir(vec![
                        ("dados.bin".into(), No::Arq(dados(3, 100_000))),
                        ("um byte".into(), No::Arq(vec![42])),
                    ]),
                ),
            ]),
        ),
        (
            "a".into(),
            No::Dir(vec![(
                "b".into(),
                No::Dir(vec![(
                    "c".into(),
                    No::Dir(vec![("profundo.txt".into(), No::Arq(dados(4, 513)))]),
                )]),
            )]),
        ),
        ("vazia".into(), No::Dir(vec![])),
    ]
}

/// Achata a árvore: caminho -> Some(dados) (arquivo) ou None (pasta).
pub fn achatar(arv: &[(String, No)]) -> BTreeMap<String, Option<Vec<u8>>> {
    fn rec(pref: &str, arv: &[(String, No)], out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for (n, no) in arv {
            let p = if pref.is_empty() {
                n.clone()
            } else {
                format!("{pref}/{n}")
            };
            match no {
                No::Arq(d) => {
                    out.insert(p, Some(d.clone()));
                }
                No::Link(t) => {
                    out.insert(p, Some(t.clone().into_bytes()));
                }
                No::Dir(f) => {
                    out.insert(p.clone(), None);
                    rec(&p, f, out);
                }
            }
        }
    }
    let mut m = BTreeMap::new();
    rec("", arv, &mut m);
    m
}

#[derive(Debug, Clone)]
pub struct Item {
    pub particao: usize,
    pub caminho: String,
    pub tamanho: u64,
    pub tipo: TipoMembro,
    pub hash: Option<String>,
    pub id: u64,
    pub repetido: bool,
    pub mtime: Option<i64>,
}

/// Percorre a imagem e calcula o BLAKE3 do conteúdo de cada arquivo.
pub fn coletar_img(img: &mut Imagem) -> (Vec<Item>, Resumo) {
    let mut itens = Vec::new();
    let r = img
        .percorrer(&mut |m, leitor| -> Result<Controle, std::io::Error> {
            let hash = if m.tipo == TipoMembro::Pasta {
                None
            } else {
                let mut h = blake3::Hasher::new();
                let mut buf = vec![0u8; 8192];
                let mut total = 0u64;
                loop {
                    let n = leitor.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    h.update(&buf[..n]);
                    total += n as u64;
                }
                assert_eq!(total, m.tamanho, "tamanho lido de {:?}", m.caminho);
                Some(h.finalize().to_hex().to_string())
            };
            itens.push(Item {
                particao: m.particao,
                caminho: String::from_utf8_lossy(&m.caminho).into_owned(),
                tamanho: m.tamanho,
                tipo: m.tipo,
                hash,
                id: m.id,
                repetido: m.repetido,
                mtime: m.mtime,
            });
            Ok(Controle::Continuar)
        })
        .expect("percorrer");
    (itens, r)
}

pub fn coletar(bytes: Vec<u8>) -> (Vec<Item>, Resumo) {
    let mut img = abrir(Cursor::new(bytes)).expect("abrir");
    coletar_img(&mut img)
}

pub fn hash(d: &[u8]) -> String {
    blake3::hash(d).to_hex().to_string()
}

/// Confere que os itens de uma partição batem com a árvore esperada.
pub fn conferir(itens: &[Item], particao: usize, arv: &[(String, No)]) {
    let esperado = achatar(arv);
    let obtido: BTreeMap<&str, &Item> = itens
        .iter()
        .filter(|i| i.particao == particao)
        .map(|i| (i.caminho.as_str(), i))
        .collect();
    let a: Vec<&str> = esperado.keys().map(String::as_str).collect();
    let b: Vec<&str> = obtido.keys().copied().collect();
    assert_eq!(a, b, "lista de caminhos");
    for (p, d) in &esperado {
        let it = obtido[p.as_str()];
        match d {
            None => assert_eq!(it.tipo, TipoMembro::Pasta, "{p}"),
            Some(d) => {
                assert_eq!(it.tamanho, d.len() as u64, "tamanho de {p}");
                assert_eq!(it.hash.as_deref(), Some(hash(d).as_str()), "hash de {p}");
            }
        }
    }
}

pub fn eh_pasta(i: &Item) -> bool {
    i.tipo == TipoMembro::Pasta
}
