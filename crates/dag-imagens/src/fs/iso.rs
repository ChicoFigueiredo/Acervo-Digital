//! ISO 9660 com extensões Joliet e Rock Ridge. Somente leitura.

use super::{Ctx, MAX_PROFUNDIDADE};
use crate::disco::{Corrente, MAX_DIR, Runs, Vista};
use crate::erro::{Result, corrompido};
use crate::util::{byte, data_para_unix, fatia, juntar, le32, limpar_nome, utf16be_para_string};
use crate::{Controle, TipoMembro};
use std::collections::HashSet;
use std::io::empty;

const SETOR: u64 = 2048;

#[derive(Default)]
struct Rr {
    nome: Option<Vec<u8>>,
    link: Option<Vec<u8>>,
    cl: Option<u32>,
    re: bool,
}

struct Raiz {
    extent: u32,
    tamanho: u32,
    joliet: bool,
    skip: usize,
}

pub(crate) fn percorrer(v: Vista, ctx: &mut Ctx<'_>) -> Result<()> {
    let mut pvd: Option<Vec<u8>> = None;
    let mut joliet: Option<Vec<u8>> = None;
    for i in 16..16 + 64u64 {
        let Ok(s) = v.ler_vec(i * SETOR, SETOR) else {
            break;
        };
        if fatia(&s, 1, 5) != b"CD001" {
            break;
        }
        match s[0] {
            1 if pvd.is_none() => pvd = Some(s),
            2 if matches!(fatia(&s, 88, 3), b"%/@" | b"%/C" | b"%/E") && joliet.is_none() => {
                joliet = Some(s);
            }
            255 => break,
            _ => {}
        }
    }
    let (desc, eh_joliet) = match (joliet, pvd) {
        (Some(j), _) => (j, true),
        (None, Some(p)) => (p, false),
        (None, None) => return corrompido("ISO sem descritor de volume primário"),
    };
    let r = fatia(&desc, 156, 34);
    let mut raiz = Raiz {
        extent: le32(r, 2),
        tamanho: le32(r, 10),
        joliet: eh_joliet,
        skip: 0,
    };
    if !eh_joliet {
        raiz.skip = detectar_skip(&v, &raiz);
    }
    let mut vistos = HashSet::new();
    dir(
        &v,
        ctx,
        &raiz,
        raiz.extent,
        raiz.tamanho,
        &[],
        0,
        &mut vistos,
    )
}

/// Lê `SP` do primeiro registro da raiz para saber quantos bytes pular nas áreas SUSP.
fn detectar_skip(v: &Vista, raiz: &Raiz) -> usize {
    let Ok(b) = v.ler_vec(u64::from(raiz.extent) * SETOR, SETOR) else {
        return 0;
    };
    let len = usize::from(byte(&b, 0));
    let nl = usize::from(byte(&b, 32));
    let ini = 33 + nl + usize::from(nl % 2 == 0);
    let su = fatia(&b, ini, len.saturating_sub(ini));
    if su.len() >= 7 && &su[..2] == b"SP" && su[4] == 0xBE && su[5] == 0xEF {
        return usize::from(su[6]);
    }
    0
}

fn tempo(b: &[u8]) -> Option<i64> {
    if b.len() < 7 || b[1] == 0 {
        return None;
    }
    let t = data_para_unix(
        1900 + i64::from(b[0]),
        i64::from(b[1]),
        i64::from(b[2]),
        i64::from(b[3]),
        i64::from(b[4]),
        i64::from(b[5]),
    )?;
    Some(t - i64::from(b[6] as i8) * 900)
}

fn parse_rr(v: &Vista, mut su: Vec<u8>, skip: usize) -> Rr {
    let mut rr = Rr::default();
    let mut nome: Vec<u8> = Vec::new();
    let mut tem_nome = false;
    let mut link: Vec<u8> = Vec::new();
    let mut tem_link = false;
    let mut ces = 0;
    if skip <= su.len() {
        su.drain(..skip);
    }
    let mut pos = 0usize;
    loop {
        if pos + 4 > su.len() {
            break;
        }
        let (sig, len) = ([su[pos], su[pos + 1]], usize::from(su[pos + 2]));
        if len < 4 || pos + len > su.len() {
            break;
        }
        let corpo = &su[pos + 4..pos + len];
        match &sig {
            b"NM" if !corpo.is_empty() => {
                let fl = corpo[0];
                if fl & 6 == 0 {
                    nome.extend_from_slice(&corpo[1..]);
                } else if fl & 2 != 0 {
                    nome.push(b'.');
                } else {
                    nome.extend_from_slice(b"..");
                }
                tem_nome = true;
            }
            b"SL" if !corpo.is_empty() => {
                tem_link = true;
                let mut c = &corpo[1..];
                while c.len() >= 2 {
                    let (fl, n) = (c[0], usize::from(c[1]));
                    let comp = fatia(c, 2, n);
                    if !link.is_empty() && !link.ends_with(b"/") {
                        link.push(b'/');
                    }
                    if fl & 8 != 0 {
                        link.push(b'/');
                    } else if fl & 4 != 0 {
                        link.extend_from_slice(b"..");
                    } else if fl & 2 != 0 {
                        link.push(b'.');
                    } else {
                        link.extend_from_slice(comp);
                    }
                    c = c.get(2 + n..).unwrap_or_default();
                }
            }
            b"CL" => rr.cl = Some(le32(corpo, 0)),
            b"RE" => rr.re = true,
            b"CE" if ces < 8 => {
                ces += 1;
                let (bloco, off, tam) = (le32(corpo, 0), le32(corpo, 8), le32(corpo, 16));
                if let Ok(c) = v.ler_vec(
                    u64::from(bloco) * SETOR + u64::from(off),
                    u64::from(tam).min(SETOR),
                ) {
                    su.drain(..pos + len);
                    su.extend_from_slice(&c);
                    pos = 0;
                    continue;
                }
            }
            b"ST" => break,
            _ => {}
        }
        pos += len;
    }
    if tem_nome {
        rr.nome = Some(nome);
    }
    if tem_link {
        rr.link = Some(link);
    }
    rr
}

struct Reg {
    nome: Vec<u8>,
    link: Option<Vec<u8>>,
    cl: Option<u32>,
    extent: u32,
    tam: u32,
    mtime: Option<i64>,
    dir: bool,
    runs: Vec<(u32, u32)>,
}

#[allow(clippy::too_many_arguments)]
fn dir(
    v: &Vista,
    ctx: &mut Ctx<'_>,
    raiz: &Raiz,
    extent: u32,
    tam: u32,
    pai: &[u8],
    prof: usize,
    vistos: &mut HashSet<u32>,
) -> Result<()> {
    if prof >= MAX_PROFUNDIDADE {
        ctx.aviso("profundidade máxima de diretórios excedida");
        return Ok(());
    }
    if !vistos.insert(extent) {
        ctx.aviso("diretório ISO repetido (laço)");
        return Ok(());
    }
    if u64::from(tam) > MAX_DIR {
        return corrompido("diretório ISO grande demais");
    }
    let b = v.ler_vec(u64::from(extent) * SETOR, u64::from(tam))?;
    let mut regs: Vec<Reg> = Vec::new();
    let mut pend: Option<Reg> = None;
    let mut pos = 0usize;
    while pos < b.len() {
        let len = usize::from(b[pos]);
        if len == 0 {
            pos = (pos / SETOR as usize + 1) * SETOR as usize;
            continue;
        }
        if len < 34 || pos + len > b.len() {
            break;
        }
        let r = &b[pos..pos + len];
        pos += len;
        let nl = usize::from(r[32]);
        let id = fatia(r, 33, nl);
        if id == [0u8] || id == [1u8] || nl == 0 {
            continue;
        }
        let flags = r[25];
        let ext = le32(r, 2);
        let sz = le32(r, 10);
        let su_ini = 33 + nl + usize::from(nl % 2 == 0);
        let su = fatia(r, su_ini, len.saturating_sub(su_ini)).to_vec();
        let rr = if raiz.joliet {
            Rr::default()
        } else {
            parse_rr(v, su, raiz.skip)
        };
        if rr.re {
            continue;
        }
        let nome = if let Some(n) = rr.nome.clone() {
            n
        } else if raiz.joliet {
            let mut s = utf16be_para_string(id);
            if let Some(i) = s.rfind(';') {
                s.truncate(i);
            }
            s.into_bytes()
        } else {
            let mut n = id.to_vec();
            if let Some(i) = n.iter().rposition(|&c| c == b';') {
                n.truncate(i);
            }
            if n.last() == Some(&b'.') && n.len() > 1 {
                n.pop();
            }
            n
        };
        if let Some(p) = pend.as_mut() {
            p.runs.push((ext, sz));
            p.tam = p.tam.saturating_add(sz);
            if flags & 0x80 == 0
                && let Some(done) = pend.take()
            {
                regs.push(done);
            }
            continue;
        }
        let reg = Reg {
            nome,
            link: rr.link,
            cl: rr.cl,
            extent: ext,
            tam: sz,
            mtime: tempo(fatia(r, 18, 7)),
            dir: flags & 2 != 0 || rr.cl.is_some(),
            runs: vec![(ext, sz)],
        };
        if flags & 0x80 != 0 {
            pend = Some(reg);
        } else {
            regs.push(reg);
        }
    }
    if let Some(p) = pend {
        regs.push(p);
    }
    for r in regs {
        if ctx.parar {
            break;
        }
        let nome = limpar_nome(r.nome.clone());
        if nome.is_empty() {
            continue;
        }
        let caminho = juntar(pai, &nome);
        if r.dir {
            let (ext, tam) = match r.cl {
                Some(cl) => {
                    let t = v
                        .ler_vec(u64::from(cl) * SETOR, 14)
                        .map(|x| le32(&x, 10))
                        .unwrap_or(0);
                    (cl, t)
                }
                None => (r.extent, r.tam),
            };
            let c = ctx.emitir(
                caminho.clone(),
                TipoMembro::Pasta,
                0,
                r.mtime,
                0,
                &mut empty(),
            )?;
            if c == Controle::Continuar {
                let res = dir(v, ctx, raiz, ext, tam, &caminho, prof + 1, vistos);
                ctx.tolerar(res, &caminho)?;
            }
        } else if let Some(alvo) = r.link {
            let mut l = std::io::Cursor::new(alvo.clone());
            ctx.emitir(
                caminho,
                TipoMembro::Link,
                alvo.len() as u64,
                r.mtime,
                0,
                &mut l,
            )?;
        } else {
            let mut runs = Runs::default();
            let mut ok = true;
            for (e, s) in &r.runs {
                if runs
                    .push(Some(u64::from(*e) * SETOR), u64::from(*s))
                    .is_err()
                {
                    ok = false;
                }
            }
            let rv = if ok { runs.v } else { Vec::new() };
            let mut l = Corrente::novo(v.clone(), rv, u64::from(r.tam));
            let id = if r.tam == 0 { 0 } else { u64::from(r.extent) };
            ctx.emitir(
                caminho,
                TipoMembro::Arquivo,
                u64::from(r.tam),
                r.mtime,
                id,
                &mut l,
            )?;
        }
    }
    Ok(())
}
