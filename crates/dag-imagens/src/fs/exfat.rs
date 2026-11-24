//! exFAT. Somente leitura.

use super::fat::{Prox, TabelaFat};
use super::{Ctx, MAX_PROFUNDIDADE};
use crate::disco::{Corrente, Runs, Vista, ler_runs};
use crate::erro::{Result, corrompido};
use crate::util::{
    byte, data_para_unix, juntar, le16, le32, le64, limpar_nome, utf16le_para_string,
};
use crate::{Controle, TipoMembro};
use std::collections::HashSet;
use std::io::empty;

struct ExFat {
    v: Vista,
    tab: TabelaFat,
    bytes_cluster: u64,
    heap_off: u64,
    n_clusters: u32,
}

impl ExFat {
    fn off(&self, c: u32) -> Result<u64> {
        if c < 2 || c > self.n_clusters + 1 {
            return corrompido("exFAT: cluster inválido");
        }
        Ok(self.heap_off + (u64::from(c) - 2) * self.bytes_cluster)
    }

    /// Runs de um objeto; `sem_cadeia` indica clusters contíguos.
    fn runs(&mut self, inicio: u32, tam: u64, sem_cadeia: bool) -> Result<Runs> {
        let mut r = Runs::default();
        if tam == 0 {
            return Ok(r);
        }
        let n = tam.div_ceil(self.bytes_cluster);
        if sem_cadeia {
            let primeiro = self.off(inicio)?;
            let ultimo = u64::from(inicio) + n - 1;
            if ultimo > u64::from(self.n_clusters) + 1 {
                return corrompido("exFAT: extensão fora do heap");
            }
            r.push(Some(primeiro), n * self.bytes_cluster)?;
            return Ok(r);
        }
        let mut c = inicio;
        for i in 0..n {
            r.push(Some(self.off(c)?), self.bytes_cluster)?;
            if i + 1 == n {
                break;
            }
            match self.tab.proximo(c)? {
                Prox::Cluster(x) => c = x,
                _ => return corrompido("exFAT: cadeia interrompida"),
            }
        }
        Ok(r)
    }

    /// Cadeia completa (diretório sem tamanho conhecido).
    fn cadeia_total(&mut self, inicio: u32) -> Result<Runs> {
        let mut r = Runs::default();
        let mut c = inicio;
        for _ in 0..=self.n_clusters {
            r.push(Some(self.off(c)?), self.bytes_cluster)?;
            match self.tab.proximo(c)? {
                Prox::Fim => return Ok(r),
                Prox::Cluster(x) => c = x,
                Prox::Ruim => return corrompido("exFAT: cadeia interrompida"),
            }
        }
        corrompido("exFAT: laço na cadeia")
    }
}

pub(crate) fn percorrer(v: Vista, ctx: &mut Ctx<'_>) -> Result<()> {
    let b = v.ler_ate(0, 512)?;
    let sh = byte(&b, 108);
    let sch = byte(&b, 109);
    if !(9..=12).contains(&sh) || u32::from(sh) + u32::from(sch) > 25 {
        return corrompido("exFAT: parâmetros de boot inválidos");
    }
    let bps = 1u64 << sh;
    let bytes_cluster = bps << sch;
    let fat_off = u64::from(le32(&b, 80)) * bps;
    let fat_len = u64::from(le32(&b, 84)) * bps;
    let heap_off = u64::from(le32(&b, 88)) * bps;
    let n_clusters = le32(&b, 92);
    let raiz = le32(&b, 96);
    if n_clusters == 0 || fat_len == 0 || heap_off == 0 {
        return corrompido("exFAT: geometria inválida");
    }
    let tab = TabelaFat::nova(v.clone(), fat_off, fat_len, 0, n_clusters + 1);
    let mut fs = ExFat {
        v,
        tab,
        bytes_cluster,
        heap_off,
        n_clusters,
    };
    let mut vistos = HashSet::new();
    dir(&mut fs, ctx, raiz, None, &[], 0, &mut vistos)
}

fn dir(
    fs: &mut ExFat,
    ctx: &mut Ctx<'_>,
    cluster: u32,
    contiguo: Option<u64>,
    pai: &[u8],
    prof: usize,
    vistos: &mut HashSet<u32>,
) -> Result<()> {
    if prof >= MAX_PROFUNDIDADE {
        ctx.aviso("profundidade máxima de diretórios excedida");
        return Ok(());
    }
    if !vistos.insert(cluster) {
        ctx.aviso("diretório exFAT repetido (laço)");
        return Ok(());
    }
    let runs = match contiguo {
        Some(t) => fs.runs(cluster, t, true)?,
        None => fs.cadeia_total(cluster)?,
    };
    let dados = ler_runs(&fs.v, runs)?;
    let mut i = 0usize;
    while (i + 1) * 32 <= dados.len() {
        if ctx.parar {
            break;
        }
        let e = &dados[i * 32..(i + 1) * 32];
        match e[0] {
            0x00 => break,
            0x85 => {}
            _ => {
                i += 1;
                continue;
            }
        }
        let sec = usize::from(e[1]);
        let attr = le16(e, 4);
        let mt = le32(e, 12);
        let (d, t) = ((mt >> 16) as u16, (mt & 0xFFFF) as u16);
        let mtime = data_para_unix(
            1980 + i64::from(d >> 9),
            i64::from((d >> 5) & 0xF),
            i64::from(d & 0x1F),
            i64::from(t >> 11),
            i64::from((t >> 5) & 0x3F),
            i64::from(t & 0x1F) * 2,
        );
        let fim = (i + 1 + sec).min(dados.len() / 32);
        let mut stream = None;
        let mut nome16: Vec<u8> = Vec::new();
        for k in i + 1..fim {
            let s = &dados[k * 32..(k + 1) * 32];
            match s[0] {
                0xC0 => stream = Some(s),
                0xC1 => nome16.extend_from_slice(&s[2..32]),
                _ => {}
            }
        }
        i = fim.max(i + 1);
        let Some(s) = stream else { continue };
        let tam_nome = usize::from(s[3]);
        nome16.truncate(tam_nome * 2);
        let nome = limpar_nome(utf16le_para_string(&nome16).into_bytes());
        if nome.is_empty() {
            continue;
        }
        let caminho = juntar(pai, &nome);
        let sem_cadeia = s[1] & 2 != 0;
        let primeiro = le32(s, 20);
        let tam = le64(s, 24);
        if attr & 0x10 != 0 {
            let c = ctx.emitir(
                caminho.clone(),
                TipoMembro::Pasta,
                0,
                mtime,
                0,
                &mut empty(),
            )?;
            if c == Controle::Continuar && primeiro >= 2 {
                let r = dir(
                    fs,
                    ctx,
                    primeiro,
                    sem_cadeia.then_some(tam),
                    &caminho,
                    prof + 1,
                    vistos,
                );
                ctx.tolerar(r, &caminho)?;
            }
        } else {
            let runs = if primeiro >= 2 || tam > 0 {
                fs.runs(primeiro, tam, sem_cadeia)
            } else {
                Ok(Runs::default())
            };
            let v = fs.v.clone();
            let id = u64::from(primeiro);
            match runs {
                Ok(r) => {
                    let mut l = Corrente::novo(v, r.v, tam);
                    ctx.emitir(caminho, TipoMembro::Arquivo, tam, mtime, id, &mut l)?;
                }
                Err(e) => {
                    ctx.aviso(format!("{}: {e}", String::from_utf8_lossy(&caminho)));
                    let mut l = Corrente::novo(v, Vec::new(), tam);
                    ctx.emitir(caminho, TipoMembro::Arquivo, tam, mtime, id, &mut l)?;
                }
            }
        }
    }
    Ok(())
}
