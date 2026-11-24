//! Gerador ISO 9660 (com Joliet e Rock Ridge opcionais) para testes.

use super::No;
use std::collections::HashMap;

const S: usize = 2048;
const DATA7: [u8; 7] = [124, 3, 15, 12, 34, 56, 0];

fn both32(v: u32) -> [u8; 8] {
    let mut r = [0u8; 8];
    r[..4].copy_from_slice(&v.to_le_bytes());
    r[4..].copy_from_slice(&v.to_be_bytes());
    r
}

fn registro(id: &[u8], ext: u32, tam: u32, flags: u8, su: &[u8]) -> Vec<u8> {
    let pad = usize::from(id.len() % 2 == 0);
    let mut len = 33 + id.len() + pad + su.len();
    if len % 2 == 1 {
        len += 1;
    }
    let mut r = vec![0u8; len];
    r[0] = len as u8;
    r[2..10].copy_from_slice(&both32(ext));
    r[10..18].copy_from_slice(&both32(tam));
    r[18..25].copy_from_slice(&DATA7);
    r[25] = flags;
    r[28..30].copy_from_slice(&1u16.to_le_bytes());
    r[30..32].copy_from_slice(&1u16.to_be_bytes());
    r[32] = id.len() as u8;
    r[33..33 + id.len()].copy_from_slice(id);
    let ini = 33 + id.len() + pad;
    r[ini..ini + su.len()].copy_from_slice(su);
    r
}

fn nome_iso(n: &str, dir: bool) -> Vec<u8> {
    let mut s: String = n
        .to_uppercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.len() > 28 {
        s.truncate(28);
    }
    if !dir {
        if !s.contains('.') {
            s.push('.');
        }
        s.push_str(";1");
    }
    s.into_bytes()
}

struct G {
    img: Vec<u8>,
    prox: usize,
    arquivos: HashMap<String, (u32, u32)>,
    rr: bool,
    joliet: bool,
}

impl G {
    fn alloc(&mut self, bytes: usize) -> u32 {
        let n = bytes.div_ceil(S).max(1);
        let e = self.prox;
        self.prox += n;
        if self.img.len() < self.prox * S {
            self.img.resize(self.prox * S, 0);
        }
        e as u32
    }
    fn su_nm(&self, nome: &str, raiz_ponto: bool, link: Option<&str>) -> Vec<u8> {
        let mut su = Vec::new();
        if !self.rr {
            return su;
        }
        if raiz_ponto {
            su.extend_from_slice(&[b'S', b'P', 7, 1, 0xBE, 0xEF, 0]);
        }
        if raiz_ponto {
            return su;
        }
        su.extend_from_slice(&[b'N', b'M', (5 + nome.len()) as u8, 1, 0]);
        su.extend_from_slice(nome.as_bytes());
        if let Some(t) = link {
            let mut comp = Vec::new();
            for parte in t.split('/') {
                comp.push(0u8);
                comp.push(parte.len() as u8);
                comp.extend_from_slice(parte.as_bytes());
            }
            su.extend_from_slice(&[b'S', b'L', (5 + comp.len()) as u8, 1, 0]);
            su.extend_from_slice(&comp);
        }
        su
    }
    fn id(&self, nome: &str, dir: bool, jol: bool) -> Vec<u8> {
        if jol {
            let mut u: Vec<u8> = Vec::new();
            for c in nome.encode_utf16() {
                u.extend_from_slice(&c.to_be_bytes());
            }
            u
        } else {
            nome_iso(nome, dir)
        }
    }
    /// Emite um diretório; devolve (extent, tamanho).
    fn dir(
        &mut self,
        entradas: &[(String, No)],
        pai: Option<(u32, u32)>,
        jol: bool,
        caminho: &str,
    ) -> (u32, u32) {
        // calcula tamanho simulando o empacotamento por setor
        let mut itens: Vec<(Vec<u8>, &No, &str)> = Vec::new();
        for (n, no) in entradas {
            itens.push((self.id(n, matches!(no, No::Dir(_)), jol), no, n.as_str()));
        }
        // 1) estima o tamanho com placeholders
        let mut tam_est = 0usize;
        let mut pos = 0usize;
        let mut prever = |len: usize| {
            if pos % S + len > S {
                pos = (pos / S + 1) * S;
            }
            pos += len;
            tam_est = pos;
        };
        prever(registro(&[0], 0, 0, 2, &self.su_nm("", pai.is_none() && !jol, None)).len());
        prever(registro(&[1], 0, 0, 2, &[]).len());
        for (id, no, n) in &itens {
            let link = if let No::Link(t) = no {
                Some(t.as_str())
            } else {
                None
            };
            let su = self.su_nm(n, false, link);
            prever(registro(id, 0, 0, 0, &su).len());
        }
        let ext = self.alloc(tam_est);
        let tam = tam_est.div_ceil(S) * S;
        let pai_e = pai.unwrap_or((ext, tam as u32));
        // 2) gera os registros finais
        let mut bytes: Vec<u8> = Vec::new();
        let mut colocar = |bytes: &mut Vec<u8>, r: Vec<u8>| {
            if bytes.len() % S + r.len() > S {
                let n = (bytes.len() / S + 1) * S;
                bytes.resize(n, 0);
            }
            bytes.extend_from_slice(&r);
        };
        let su_ponto = self.su_nm("", pai.is_none() && !jol, None);
        colocar(&mut bytes, registro(&[0], ext, tam as u32, 2, &su_ponto));
        colocar(&mut bytes, registro(&[1], pai_e.0, pai_e.1, 2, &[]));
        for (id, no, n) in itens {
            let sub = format!("{caminho}/{n}");
            match no {
                No::Dir(f) => {
                    let (e, t) = self.dir(f, Some((ext, tam as u32)), jol, &sub);
                    let su = self.su_nm(n, false, None);
                    colocar(&mut bytes, registro(&id, e, t, 2, &su));
                }
                No::Arq(d) => {
                    let (e, t) = match self.arquivos.get(&sub) {
                        Some(x) => *x,
                        None => {
                            let e = if d.is_empty() { 0 } else { self.alloc(d.len()) };
                            if !d.is_empty() {
                                self.img[e as usize * S..e as usize * S + d.len()]
                                    .copy_from_slice(d);
                            }
                            self.arquivos.insert(sub.clone(), (e, d.len() as u32));
                            (e, d.len() as u32)
                        }
                    };
                    let su = self.su_nm(n, false, None);
                    colocar(&mut bytes, registro(&id, e, t, 0, &su));
                }
                No::Link(t) => {
                    let su = self.su_nm(n, false, Some(t));
                    colocar(&mut bytes, registro(&id, 0, 0, 0, &su));
                }
            }
        }
        bytes.resize(tam, 0);
        if self.img.len() < (ext as usize) * S + tam {
            self.img.resize((ext as usize) * S + tam, 0);
        }
        self.img[ext as usize * S..ext as usize * S + tam].copy_from_slice(&bytes);
        (ext, tam as u32)
    }
}

fn descritor(tipo: u8, raiz: (u32, u32), total: u32, jol: bool) -> Vec<u8> {
    let mut d = vec![0u8; S];
    d[0] = tipo;
    d[1..6].copy_from_slice(b"CD001");
    d[6] = 1;
    if jol {
        d[88..91].copy_from_slice(b"%/E");
    }
    d[80..88].copy_from_slice(&both32(total));
    d[120..124].copy_from_slice(&[1, 0, 0, 1]);
    d[128..132].copy_from_slice(&[0, 8, 8, 0]);
    let r = registro(&[0], raiz.0, raiz.1, 2, &[]);
    d[156..156 + 34].copy_from_slice(&r[..34]);
    d
}

/// Gera uma ISO 9660. `joliet`/`rr` ligam as extensões. Links só com `rr`.
pub fn iso_img(arv: &[(String, No)], joliet: bool, rr: bool) -> Vec<u8> {
    let mut g = G {
        img: vec![0u8; 19 * S],
        prox: 19,
        arquivos: HashMap::new(),
        rr,
        joliet,
    };
    let raiz = g.dir(arv, None, false, "");
    let raiz_j = if joliet {
        Some(g.dir(arv, None, true, ""))
    } else {
        None
    };
    let total = g.prox as u32;
    g.img[16 * S..17 * S].copy_from_slice(&descritor(1, raiz, total, false));
    if let Some(rj) = raiz_j {
        g.img[17 * S..18 * S].copy_from_slice(&descritor(2, rj, total, true));
    }
    let mut t = vec![0u8; S];
    t[0] = 255;
    t[1..6].copy_from_slice(b"CD001");
    t[6] = 1;
    g.img[18 * S..19 * S].copy_from_slice(&t);
    let _ = g.joliet;
    g.img
}
