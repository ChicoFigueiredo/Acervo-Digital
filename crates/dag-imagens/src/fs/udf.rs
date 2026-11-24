//! UDF básico (1.02 a 2.01): partições simples e esparsas, File Entry e Extended
//! File Entry, descritores curtos, longos e embutidos. Somente leitura.
//!
//! Sem suporte: partição virtual (VAT, mídia multissessão), partição de metadados
//! (UDF 2.50+) e stream directories. Checksums e CRC dos descritores não são
//! verificados (a leitura é tolerante).

use super::{Ctx, MAX_PROFUNDIDADE};
use crate::disco::{Corrente, MAX_DIR, Memoria, Runs, Vista};
use crate::erro::{Result, corrompido, sem_suporte};
use crate::util::{
    byte, data_para_unix, fatia, juntar, le16, le32, le64, limpar_nome, utf16be_para_string,
};
use crate::{Controle, TipoMembro};
use std::collections::HashSet;
use std::io::{Cursor, empty};

struct Udf {
    v: Vista,
    bs: u64,
    /// Setor inicial (em blocos) de cada mapa de partição.
    mapas: Vec<u64>,
    fsd: (u32, u16),
}

#[derive(Clone, Copy)]
struct Ad {
    len: u32,
    partref: u16,
    lbn: u32,
}

struct Fe {
    tipo: u8,
    info_len: u64,
    mtime: Option<i64>,
    ads: Vec<Ad>,
    embutido: Option<Vec<u8>>,
}

fn timestamp(b: &[u8]) -> Option<i64> {
    let tz = le16(b, 0);
    let ano = i64::from(le16(b, 2));
    if ano < 1970 {
        return None;
    }
    let mut min = i64::from(tz & 0x0FFF);
    if min & 0x800 != 0 {
        min -= 0x1000;
    }
    if min == -2047 {
        min = 0;
    }
    let t = data_para_unix(
        ano,
        i64::from(byte(b, 4)),
        i64::from(byte(b, 5)),
        i64::from(byte(b, 6)),
        i64::from(byte(b, 7)),
        i64::from(byte(b, 8)),
    )?;
    Some(t - min * 60)
}

impl Udf {
    fn abrir(v: Vista) -> Result<Udf> {
        let setores = v.len() / 2048;
        let mut avdp = None;
        for s in [256, setores.saturating_sub(1), setores.saturating_sub(257)] {
            if let Ok(b) = v.ler_vec(s * 2048, 2048)
                && le16(&b, 0) == 2
            {
                avdp = Some(b);
                break;
            }
        }
        let Some(avdp) = avdp else {
            return corrompido("UDF: AVDP não encontrado");
        };
        let mut particoes: Vec<(u16, u64)> = Vec::new();
        let mut lvd: Option<Vec<u8>> = None;
        for extent in [16usize, 24] {
            let (len, loc) = (
                u64::from(le32(&avdp, extent)),
                u64::from(le32(&avdp, extent + 4)),
            );
            for i in 0..(len / 2048).min(256) {
                let Ok(b) = v.ler_vec((loc + i) * 2048, 2048) else {
                    break;
                };
                match le16(&b, 0) {
                    5 => particoes.push((le16(&b, 22), u64::from(le32(&b, 188)))),
                    6 if lvd.is_none() => lvd = Some(b),
                    8 => break,
                    0 => break,
                    _ => {}
                }
            }
            if lvd.is_some() && !particoes.is_empty() {
                break;
            }
        }
        let Some(lvd) = lvd else {
            return corrompido("UDF: sem Logical Volume Descriptor");
        };
        let bs = u64::from(le32(&lvd, 212));
        if !matches!(bs, 512 | 1024 | 2048 | 4096) {
            return corrompido("UDF: tamanho de bloco lógico inválido");
        }
        let n_mapas = le32(&lvd, 268).min(64) as usize;
        let mut mapas = Vec::new();
        let mut p = 440usize;
        for _ in 0..n_mapas {
            let (tipo, len) = (byte(&lvd, p), usize::from(byte(&lvd, p + 1)));
            if len < 6 || p + len > lvd.len() {
                return corrompido("UDF: mapa de partição inválido");
            }
            let numero = match tipo {
                1 => le16(&lvd, p + 4),
                2 => {
                    let id = fatia(&lvd, p + 5, 23);
                    if id.starts_with(b"*UDF Metadata") {
                        return sem_suporte("UDF 2.50 com partição de metadados");
                    }
                    if id.starts_with(b"*UDF Virtual") {
                        return sem_suporte("UDF com partição virtual (VAT)");
                    }
                    le16(&lvd, p + 38)
                }
                _ => return sem_suporte("UDF: tipo de mapa de partição desconhecido"),
            };
            let Some(&(_, ini)) = particoes.iter().find(|(n, _)| *n == numero) else {
                return corrompido("UDF: partição referenciada não existe");
            };
            // `ini` está em setores físicos; converte para blocos lógicos.
            mapas.push(ini * 2048 / bs);
            p += len;
        }
        if mapas.is_empty() {
            return corrompido("UDF: volume sem partições");
        }
        let fsd = (le32(&lvd, 252), le16(&lvd, 256));
        Ok(Udf { v, bs, mapas, fsd })
    }

    fn off(&self, partref: u16, lbn: u32) -> Result<u64> {
        let Some(ini) = self.mapas.get(usize::from(partref)) else {
            return corrompido("UDF: referência de partição inválida");
        };
        Ok((ini + u64::from(lbn)) * self.bs)
    }

    fn ler_ads(&self, bytes: &[u8], longo: bool, partref: u16, ads: &mut Vec<Ad>) -> Result<()> {
        let passo = if longo { 16 } else { 8 };
        for c in bytes.chunks_exact(passo) {
            let len = le32(c, 0);
            if len & 0x3FFF_FFFF == 0 && len >> 30 != 3 {
                break;
            }
            ads.push(Ad {
                len,
                lbn: le32(c, 4),
                partref: if longo { le16(c, 8) } else { partref },
            });
            if ads.len() > 4_000_000 {
                return corrompido("UDF: descritores de alocação demais");
            }
        }
        Ok(())
    }

    fn ler_fe(&self, partref: u16, lbn: u32) -> Result<Fe> {
        let b = self.v.ler_vec(self.off(partref, lbn)?, self.bs)?;
        let id = le16(&b, 0);
        let (info, ea_ini, mt, ad_len_off) = match id {
            261 => (56, 176, 84, 172),
            266 => (56, 216, 92, 212),
            _ => return corrompido("UDF: ICB não é File Entry"),
        };
        let l_ea = le32(&b, ad_len_off - 4) as usize;
        let l_ad = le32(&b, ad_len_off) as usize;
        let ini = ea_ini + l_ea;
        let tipo = byte(&b, 27);
        let flags = le16(&b, 34);
        let mut fe = Fe {
            tipo,
            info_len: le64(&b, info),
            mtime: timestamp(fatia(&b, mt, 12)),
            ads: Vec::new(),
            embutido: None,
        };
        // O File Entry cabe num bloco, mas os descritores podem ocupar o restante dele.
        let Some(area) = ini.checked_add(l_ad).and_then(|f| b.get(ini..f)) else {
            return corrompido("UDF: descritores de alocação fora do bloco");
        };
        match flags & 7 {
            0 | 1 => {
                let longo = flags & 7 == 1;
                self.ler_ads(area, longo, partref, &mut fe.ads)?;
                // continuações
                let mut seguidas = 0;
                while let Some(ult) = fe.ads.last().copied() {
                    if ult.len >> 30 != 3 {
                        break;
                    }
                    fe.ads.pop();
                    seguidas += 1;
                    if seguidas > 64 {
                        return corrompido("UDF: continuações demais");
                    }
                    let tam = u64::from(ult.len & 0x3FFF_FFFF).min(1 << 20);
                    let a = self
                        .v
                        .ler_vec(self.off(ult.partref, ult.lbn)?, tam.max(24))?;
                    if le16(&a, 0) != 258 {
                        return corrompido("UDF: AED inválido");
                    }
                    let n = le32(&a, 20) as usize;
                    let corpo = fatia(&a, 24, n);
                    self.ler_ads(corpo, longo, ult.partref, &mut fe.ads)?;
                }
            }
            3 => fe.embutido = Some(area.to_vec()),
            _ => return sem_suporte("UDF: tipo de alocação não suportado"),
        }
        Ok(fe)
    }

    fn runs(&self, fe: &Fe) -> Result<Runs> {
        let mut runs = Runs::default();
        let mut total = 0u64;
        for ad in &fe.ads {
            if total >= fe.info_len {
                break;
            }
            let l = u64::from(ad.len & 0x3FFF_FFFF);
            match ad.len >> 30 {
                0 => runs.push(Some(self.off(ad.partref, ad.lbn)?), l)?,
                _ => runs.push(None, l)?,
            }
            total += l;
        }
        Ok(runs)
    }

    fn conteudo_mem(&self, fe: &Fe) -> Result<Vec<u8>> {
        if let Some(e) = &fe.embutido {
            let n = (fe.info_len as usize).min(e.len());
            return Ok(e[..n].to_vec());
        }
        if fe.info_len > MAX_DIR {
            return corrompido("UDF: estrutura grande demais para memória");
        }
        let mut c = Corrente::novo(self.v.clone(), self.runs(fe)?.v, fe.info_len);
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut c, &mut out)?;
        Ok(out)
    }
}

pub(crate) fn percorrer(v: Vista, ctx: &mut Ctx<'_>) -> Result<()> {
    let udf = Udf::abrir(v)?;
    let (fsd_lbn, fsd_ref) = udf.fsd;
    let fsd = udf.v.ler_vec(udf.off(fsd_ref, fsd_lbn)?, udf.bs)?;
    if le16(&fsd, 0) != 256 {
        return corrompido("UDF: File Set Descriptor inválido");
    }
    let (raiz_lbn, raiz_ref) = (le32(&fsd, 404), le16(&fsd, 408));
    let mut vistos = HashSet::new();
    dir(&udf, ctx, raiz_ref, raiz_lbn, &[], 0, &mut vistos)
}

fn dir(
    udf: &Udf,
    ctx: &mut Ctx<'_>,
    partref: u16,
    lbn: u32,
    pai: &[u8],
    prof: usize,
    vistos: &mut HashSet<(u16, u32)>,
) -> Result<()> {
    if prof >= MAX_PROFUNDIDADE {
        ctx.aviso("profundidade máxima de diretórios excedida");
        return Ok(());
    }
    if !vistos.insert((partref, lbn)) {
        ctx.aviso("diretório UDF repetido (laço)");
        return Ok(());
    }
    let fe = udf.ler_fe(partref, lbn)?;
    if fe.tipo != 4 {
        return corrompido("UDF: ICB de diretório não é diretório");
    }
    let dados = udf.conteudo_mem(&fe)?;
    let mut pos = 0usize;
    let mut filhos: Vec<(Vec<u8>, u32, u16)> = Vec::new();
    while pos + 38 <= dados.len() {
        if le16(&dados, pos) != 257 {
            break;
        }
        let carac = byte(&dados, pos + 18);
        let l_fi = usize::from(byte(&dados, pos + 19));
        let l_iu = usize::from(le16(&dados, pos + 36));
        let tam = (38 + l_iu + l_fi).div_ceil(4) * 4;
        if pos + tam > dados.len() {
            break;
        }
        if carac & 0x0C == 0 && l_fi > 0 {
            let id = fatia(&dados, pos + 38 + l_iu, l_fi);
            let nome = match id.first() {
                Some(8) => id[1..].iter().map(|&c| c as char).collect::<String>(),
                Some(16) => utf16be_para_string(&id[1..]),
                _ => String::from_utf8_lossy(id).into_owned(),
            };
            filhos.push((
                nome.into_bytes(),
                le32(&dados, pos + 24),
                le16(&dados, pos + 28),
            ));
        }
        pos += tam;
    }
    for (nome, l, p) in filhos {
        if ctx.parar {
            break;
        }
        let nome = limpar_nome(nome);
        if nome.is_empty() {
            continue;
        }
        let caminho = juntar(pai, &nome);
        let r = membro(udf, ctx, p, l, caminho.clone(), prof, vistos);
        ctx.tolerar(r, &caminho)?;
    }
    Ok(())
}

fn membro(
    udf: &Udf,
    ctx: &mut Ctx<'_>,
    partref: u16,
    lbn: u32,
    caminho: Vec<u8>,
    prof: usize,
    vistos: &mut HashSet<(u16, u32)>,
) -> Result<()> {
    let fe = udf.ler_fe(partref, lbn)?;
    let id = (u64::from(partref) << 32) | u64::from(lbn);
    match fe.tipo {
        4 => {
            let c = ctx.emitir(
                caminho.clone(),
                TipoMembro::Pasta,
                0,
                fe.mtime,
                id,
                &mut empty(),
            )?;
            if c == Controle::Continuar {
                dir(udf, ctx, partref, lbn, &caminho, prof + 1, vistos)?;
            }
        }
        12 => {
            let d = udf.conteudo_mem(&fe)?;
            let alvo = caminho_symlink(&d);
            let tam = alvo.len() as u64;
            let mut l = Memoria(Cursor::new(alvo));
            ctx.emitir(caminho, TipoMembro::Link, tam, fe.mtime, id, &mut l)?;
        }
        0 | 5 => {
            if let Some(e) = &fe.embutido {
                let n = (fe.info_len as usize).min(e.len());
                let mut l = Memoria(Cursor::new(e[..n].to_vec()));
                ctx.emitir(caminho, TipoMembro::Arquivo, n as u64, fe.mtime, id, &mut l)?;
                return Ok(());
            }
            match udf.runs(&fe) {
                Ok(r) => {
                    let mut l = Corrente::novo(udf.v.clone(), r.v, fe.info_len);
                    ctx.emitir(
                        caminho,
                        TipoMembro::Arquivo,
                        fe.info_len,
                        fe.mtime,
                        id,
                        &mut l,
                    )?;
                }
                Err(e) => {
                    ctx.aviso(format!("{}: {e}", String::from_utf8_lossy(&caminho)));
                    let mut l = Corrente::novo(udf.v.clone(), Vec::new(), fe.info_len);
                    ctx.emitir(
                        caminho,
                        TipoMembro::Arquivo,
                        fe.info_len,
                        fe.mtime,
                        id,
                        &mut l,
                    )?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn caminho_symlink(d: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut p = 0usize;
    while p + 4 <= d.len() {
        let (tipo, len) = (d[p], usize::from(d[p + 1]));
        let id = fatia(d, p + 4, len);
        if !out.is_empty() && !out.ends_with(b"/") {
            out.push(b'/');
        }
        match tipo {
            1 | 2 => out.push(b'/'),
            3 => out.extend_from_slice(b".."),
            4 => out.push(b'.'),
            _ => match id.first() {
                Some(16) => out.extend_from_slice(utf16be_para_string(&id[1..]).as_bytes()),
                Some(_) => out.extend_from_slice(&id[1..]),
                None => {}
            },
        }
        p += 4 + len;
    }
    out
}
