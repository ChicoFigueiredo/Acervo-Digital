//! Tabelas de partição MBR (com estendidas) e GPT.

use crate::Particao;
use crate::disco::Vista;
use crate::erro::Result;
use crate::fs::{Sonda, sondar};
use crate::util::{le16, le32, le64, utf16le_para_string};

const MAX_PARTICOES: usize = 256;

/// O primeiro setor parece um MBR com tabela de partições?
pub(crate) fn mbr_plausivel(s: &[u8]) -> bool {
    if s.len() < 512 || s[510] != 0x55 || s[511] != 0xAA {
        return false;
    }
    let mut n = 0;
    for i in 0..4 {
        let e = &s[446 + i * 16..446 + (i + 1) * 16];
        if e[0] != 0 && e[0] != 0x80 {
            return false;
        }
        let tipo = e[4];
        let len = le32(e, 12);
        if tipo != 0 && len > 0 {
            n += 1;
        }
    }
    n > 0
}

fn nova(
    indice: usize,
    inicio: u64,
    tamanho: u64,
    tipo: String,
    nome: Option<String>,
    disco: &Vista,
) -> Particao {
    let mut p = Particao {
        indice,
        inicio,
        tamanho,
        tipo,
        nome,
        sistema: None,
        nao_lida: None,
    };
    match disco.sub(inicio, tamanho) {
        Ok(v) => match sondar(&v) {
            Sonda::Fs(f) => p.sistema = Some(f),
            Sonda::Outro(m) => p.nao_lida = Some(m),
        },
        Err(e) => p.nao_lida = Some(e.to_string()),
    }
    p
}

/// ISO: o disco inteiro é uma "partição" única.
pub(crate) fn iso_inteira(v: &Vista) -> Vec<Particao> {
    let mut p = nova(0, 0, v.len(), "iso".into(), None, v);
    if p.sistema.is_none() {
        p.nao_lida = Some("ISO sem volume reconhecido".into());
    }
    vec![p]
}

/// Lê a tabela de partições do disco lógico.
pub(crate) fn ler(v: &Vista, avisos: &mut Vec<String>) -> Result<Vec<Particao>> {
    // Sistema de arquivos direto no início (sem tabela).
    if let Sonda::Fs(_) = sondar(v) {
        return Ok(vec![nova(0, 0, v.len(), "sem-tabela".into(), None, v)]);
    }
    for ss in [512u64, 4096] {
        if let Ok(h) = v.ler_ate(ss, 92)
            && h.starts_with(b"EFI PART")
        {
            match gpt(v, ss, &h, avisos) {
                Ok(p) if !p.is_empty() => return Ok(p),
                Ok(_) => {}
                Err(e) => avisos.push(format!("GPT inválida: {e}")),
            }
        }
    }
    let s0 = v.ler_ate(0, 512)?;
    if mbr_plausivel(&s0) {
        return mbr(v, &s0, avisos);
    }
    Ok(vec![nova(0, 0, v.len(), "sem-tabela".into(), None, v)])
}

fn gpt(v: &Vista, ss: u64, h: &[u8], avisos: &mut Vec<String>) -> Result<Vec<Particao>> {
    let lba_ent = le64(h, 72);
    let n = le32(h, 80) as usize;
    let tam = le32(h, 84) as usize;
    if !(128..=4096).contains(&tam) || n == 0 || n > 4096 {
        return crate::erro::corrompido("cabeçalho GPT com valores inválidos");
    }
    let Some(off) = lba_ent.checked_mul(ss) else {
        return crate::erro::corrompido("GPT: LBA das entradas inválido");
    };
    let total = (n * tam) as u64;
    let ents = v.ler_ate(off, total as usize)?;
    let mut out = Vec::new();
    for i in 0..n {
        let e = &ents[i * tam..(i + 1) * tam];
        if e[..16].iter().all(|&b| b == 0) {
            continue;
        }
        if out.len() >= MAX_PARTICOES {
            avisos.push("GPT: partições demais, o restante foi ignorado".into());
            break;
        }
        let (ini, fim) = (le64(e, 32), le64(e, 40));
        if fim < ini {
            avisos.push(format!("GPT: entrada {i} com LBA final antes do inicial"));
            continue;
        }
        let (Some(o), Some(t)) = (
            ini.checked_mul(ss),
            (fim - ini).checked_add(1).and_then(|x| x.checked_mul(ss)),
        ) else {
            continue;
        };
        let (o, t) = clamp(o, t, v.len(), avisos);
        if t == 0 {
            continue;
        }
        let nome = utf16le_para_string(&e[56..tam.min(128)])
            .trim_end_matches('\0')
            .to_string();
        out.push(nova(
            out.len(),
            o,
            t,
            format!("gpt:{}", guid(&e[..16])),
            (!nome.is_empty()).then_some(nome),
            v,
        ));
    }
    Ok(out)
}

fn clamp(o: u64, t: u64, disco: u64, avisos: &mut Vec<String>) -> (u64, u64) {
    if o >= disco {
        avisos.push(format!("partição em {o} fora do disco"));
        return (o, 0);
    }
    if o.saturating_add(t) > disco {
        avisos.push(format!("partição em {o} truncada pelo fim do disco"));
        return (o, disco - o);
    }
    (o, t)
}

fn guid(b: &[u8]) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        le32(b, 0),
        le16(b, 4),
        le16(b, 6),
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    )
}

fn mbr(v: &Vista, s0: &[u8], avisos: &mut Vec<String>) -> Result<Vec<Particao>> {
    let mut out = Vec::new();
    let mut extendida: Option<(u64, u64)> = None; // (inicio, tamanho) em setores
    for i in 0..4 {
        let e = &s0[446 + i * 16..446 + (i + 1) * 16];
        let (tipo, ini, len) = (e[4], u64::from(le32(e, 8)), u64::from(le32(e, 12)));
        if tipo == 0 || len == 0 {
            continue;
        }
        if matches!(tipo, 0x05 | 0x0F | 0x85) {
            extendida.get_or_insert((ini, len));
            continue;
        }
        if tipo == 0xEE {
            continue; // GPT protetora sem GPT válida: ignora
        }
        let (o, t) = clamp(ini * 512, len * 512, v.len(), avisos);
        if t > 0 {
            out.push(nova(out.len(), o, t, format!("mbr:0x{tipo:02x}"), None, v));
        }
    }
    if let Some((ini, len)) = extendida {
        let base = ini * 512;
        let mut atual = base;
        let mut visitados = std::collections::HashSet::new();
        for _ in 0..128 {
            if !visitados.insert(atual) || out.len() >= MAX_PARTICOES {
                avisos.push("MBR estendida em laço ou grande demais".into());
                break;
            }
            let Ok(ebr) = v.ler_ate(atual, 512) else {
                break;
            };
            if le16(&ebr, 510) != 0xAA55 {
                avisos.push("EBR sem assinatura".into());
                break;
            }
            let (tipo, rel, tam) = (
                ebr[446 + 4],
                u64::from(le32(&ebr, 446 + 8)),
                u64::from(le32(&ebr, 446 + 12)),
            );
            if tipo != 0 && tam > 0 && !matches!(tipo, 0x05 | 0x0F | 0x85) {
                let (o, t) = clamp(atual + rel * 512, tam * 512, v.len(), avisos);
                if t > 0 {
                    out.push(nova(out.len(), o, t, format!("mbr:0x{tipo:02x}"), None, v));
                }
            }
            let (t2, rel2) = (ebr[462 + 4], u64::from(le32(&ebr, 462 + 8)));
            if !matches!(t2, 0x05 | 0x0F | 0x85) || rel2 == 0 {
                break;
            }
            let prox = base + rel2 * 512;
            if prox >= base.saturating_add(len * 512) {
                break;
            }
            atual = prox;
        }
    }
    if out.is_empty() {
        // Tabela sem partições legíveis: trata o disco como uma só.
        out.push(nova(0, 0, v.len(), "sem-tabela".into(), None, v));
    }
    Ok(out)
}
