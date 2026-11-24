//! NTFS: MFT, atributos residentes e não residentes, índices `$I30`, listas de
//! atributos. Somente leitura; arquivos comprimidos ou cifrados (EFS) são listados
//! mas o conteúdo fica sem suporte.

use super::{Ctx, MAX_PROFUNDIDADE};
use crate::disco::{Corrente, MAX_METADADO, Memoria, Run, Runs, Vista};
use crate::erro::{Erro, Result, corrompido, sem_suporte};
use crate::util::{byte, fatia, juntar, le16, le32, le64, limpar_nome, utf16le_para_string};
use crate::{Controle, TipoMembro};
use std::collections::HashSet;
use std::io::{Cursor, empty};

const T_STD: u32 = 0x10;
const T_LISTA: u32 = 0x20;
const T_DADOS: u32 = 0x80;
const T_RAIZ_IDX: u32 = 0x90;
const T_ALOC_IDX: u32 = 0xA0;
const T_BITMAP: u32 = 0xB0;
const T_REPARSE: u32 = 0xC0;
const PRIMEIRO_REGISTRO_USUARIO: u64 = 24;

#[derive(Clone)]
struct Attr {
    tipo: u32,
    nome: Vec<u16>,
    flags: u16,
    residente: Option<Vec<u8>>,
    vcn_ini: u64,
    runlist: Vec<u8>,
    tam_dados: u64,
    tam_init: u64,
}

/// Mapa lógico->físico em bytes (para $MFT e $INDEX_ALLOCATION).
struct Mapa {
    ini: Vec<u64>,
    runs: Vec<Run>,
    total: u64,
}

impl Mapa {
    fn novo(runs: Vec<Run>) -> Self {
        let mut ini = Vec::with_capacity(runs.len());
        let mut a = 0u64;
        for r in &runs {
            ini.push(a);
            a = a.saturating_add(r.len);
        }
        Mapa {
            ini,
            runs,
            total: a,
        }
    }
    fn ler(&self, v: &Vista, pos: u64, buf: &mut [u8]) -> Result<()> {
        let mut feito = 0usize;
        while feito < buf.len() {
            let p = pos + feito as u64;
            let i = self.ini.partition_point(|&x| x <= p).saturating_sub(1);
            let (Some(r), Some(&base)) = (self.runs.get(i), self.ini.get(i)) else {
                return corrompido("NTFS: leitura fora do mapa de runs");
            };
            let dentro = p - base;
            if dentro >= r.len {
                return corrompido("NTFS: leitura além dos dados do atributo");
            }
            let n = ((r.len - dentro) as usize).min(buf.len() - feito);
            match r.desloc {
                Some(d) => v.ler(d + dentro, &mut buf[feito..feito + n])?,
                None => buf[feito..feito + n].fill(0),
            }
            feito += n;
        }
        Ok(())
    }
}

struct Ntfs {
    v: Vista,
    cluster: u64,
    tam_registro: u64,
    mft: Mapa,
}

fn aplicar_fixups(b: &mut [u8]) -> Result<()> {
    let off = usize::from(le16(b, 4));
    let n = usize::from(le16(b, 6));
    if n < 2 || off + n * 2 > b.len() {
        return corrompido("NTFS: matriz de fixup inválida");
    }
    let usn = le16(b, off);
    for i in 1..n {
        let fim = i * 512;
        if fim > b.len() {
            break;
        }
        if le16(b, fim - 2) != usn {
            return corrompido("NTFS: fixup não confere (setor rasgado)");
        }
        let (a, c) = (b[off + i * 2], b[off + i * 2 + 1]);
        b[fim - 2] = a;
        b[fim - 1] = c;
    }
    Ok(())
}

/// Decodifica uma lista de runs `(vcn, len, lcn)` a partir de `vcn_ini`.
fn decodificar_runs(rl: &[u8], vcn_ini: u64, max_lcn: u64) -> Result<Vec<(u64, u64, Option<u64>)>> {
    let mut out = Vec::new();
    let mut p = 0usize;
    let mut vcn = vcn_ini;
    let mut lcn: i64 = 0;
    while p < rl.len() {
        let h = rl[p];
        if h == 0 {
            break;
        }
        let (nl, no) = (usize::from(h & 0xF), usize::from(h >> 4));
        p += 1;
        if nl == 0 || nl > 8 || no > 8 || p + nl + no > rl.len() {
            return corrompido("NTFS: lista de runs inválida");
        }
        let mut len = 0u64;
        for i in 0..nl {
            len |= u64::from(rl[p + i]) << (8 * i);
        }
        p += nl;
        let mut mapeado = None;
        if no > 0 {
            let mut d = 0i64;
            for i in 0..no {
                d |= i64::from(rl[p + i]) << (8 * i);
            }
            if rl[p + no - 1] & 0x80 != 0 && no < 8 {
                d |= -1i64 << (8 * no);
            }
            p += no;
            lcn = lcn
                .checked_add(d)
                .ok_or_else(|| Erro::Corrompido("NTFS: LCN estoura".into()))?;
            if lcn < 0 || lcn as u64 >= max_lcn || len > max_lcn {
                return corrompido("NTFS: run fora do volume");
            }
            mapeado = Some(lcn as u64);
        }
        if len == 0 {
            return corrompido("NTFS: run de tamanho zero");
        }
        out.push((vcn, len, mapeado));
        vcn = vcn.saturating_add(len);
        if out.len() > 4_000_000 {
            return corrompido("NTFS: runs demais");
        }
    }
    Ok(out)
}

fn parse_attrs(b: &[u8]) -> Result<Vec<Attr>> {
    let mut out = Vec::new();
    let mut p = usize::from(le16(b, 20));
    let usado = (le32(b, 24) as usize).min(b.len());
    for _ in 0..512 {
        if p + 16 > usado {
            break;
        }
        let tipo = le32(b, p);
        if tipo == 0xFFFF_FFFF {
            break;
        }
        let len = le32(b, p + 4) as usize;
        if len < 16 || p + len > usado || !len.is_multiple_of(8) {
            return corrompido("NTFS: atributo com tamanho inválido");
        }
        let a = &b[p..p + len];
        let nl = usize::from(byte(a, 9));
        let no = usize::from(le16(a, 10));
        let nome_b = fatia(a, no, nl * 2);
        let nome: Vec<u16> = nome_b
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let flags = le16(a, 12);
        if byte(a, 8) == 0 {
            let (vl, vo) = (le32(a, 16) as usize, usize::from(le16(a, 20)));
            let Some(v) = vo.checked_add(vl).and_then(|f| a.get(vo..f)) else {
                return corrompido("NTFS: valor residente fora do atributo");
            };
            out.push(Attr {
                tipo,
                nome,
                flags,
                residente: Some(v.to_vec()),
                vcn_ini: 0,
                runlist: Vec::new(),
                tam_dados: vl as u64,
                tam_init: vl as u64,
            });
        } else {
            let ro = usize::from(le16(a, 32));
            out.push(Attr {
                tipo,
                nome,
                flags,
                residente: None,
                vcn_ini: le64(a, 16),
                runlist: a.get(ro..).unwrap_or_default().to_vec(),
                tam_dados: le64(a, 48),
                tam_init: le64(a, 56),
            });
        }
        p += len;
    }
    Ok(out)
}

impl Ntfs {
    fn ler_registro(&self, n: u64) -> Result<Vec<u8>> {
        let off = n
            .checked_mul(self.tam_registro)
            .ok_or_else(|| Erro::Corrompido("NTFS: registro fora do MFT".into()))?;
        if off + self.tam_registro > self.mft.total {
            return corrompido("NTFS: registro fora do MFT");
        }
        let mut b = vec![0u8; self.tam_registro as usize];
        self.mft.ler(&self.v, off, &mut b)?;
        if &b[..4] != b"FILE" {
            return corrompido("NTFS: registro sem assinatura FILE");
        }
        aplicar_fixups(&mut b)?;
        Ok(b)
    }

    /// Atributos de um registro, incluindo os de registros de extensão.
    fn atributos(&self, n: u64) -> Result<(Vec<u8>, Vec<Attr>)> {
        let reg = self.ler_registro(n)?;
        let mut attrs = parse_attrs(&reg)?;
        if let Some(lista) = attrs.iter().find(|a| a.tipo == T_LISTA).cloned() {
            let dados = self.conteudo_attr(&[lista])?;
            let mut vistos: HashSet<u64> = HashSet::new();
            let mut p = 0usize;
            while p + 26 <= dados.len() && vistos.len() < 16 {
                let rl = usize::from(le16(&dados, p + 4));
                if rl < 26 {
                    break;
                }
                let ref_rec = le64(&dados, p + 16) & 0xFFFF_FFFF_FFFF;
                if ref_rec != n && vistos.insert(ref_rec) {
                    let ext = self.ler_registro(ref_rec)?;
                    for a in parse_attrs(&ext)? {
                        if a.tipo != T_LISTA {
                            attrs.push(a);
                        }
                    }
                }
                p += rl;
            }
        }
        Ok((reg, attrs))
    }

    /// Runs (em bytes) de um conjunto de segmentos do mesmo atributo.
    fn runs_attr(&self, segs: &[Attr]) -> Result<(Runs, u64, u64)> {
        let mut todos: Vec<(u64, u64, Option<u64>)> = Vec::new();
        let (mut tam, mut init) = (0u64, 0u64);
        let max_lcn = self.v.len() / self.cluster;
        for s in segs {
            if s.vcn_ini == 0 {
                tam = s.tam_dados;
                init = s.tam_init;
            }
            todos.extend(decodificar_runs(&s.runlist, s.vcn_ini, max_lcn)?);
        }
        todos.sort_by_key(|r| r.0);
        let mut runs = Runs::default();
        let mut cur = 0u64;
        for (vcn, len, lcn) in todos {
            if vcn < cur {
                return corrompido("NTFS: runs sobrepostos");
            }
            if vcn > cur {
                runs.push(None, (vcn - cur) * self.cluster)?;
            }
            runs.push(lcn.map(|l| l * self.cluster), len * self.cluster)?;
            cur = vcn + len;
        }
        Ok((runs, tam, init))
    }

    /// Conteúdo completo de um atributo em memória (limitado).
    fn conteudo_attr(&self, segs: &[Attr]) -> Result<Vec<u8>> {
        let Some(a) = segs.first() else {
            return Ok(Vec::new());
        };
        if let Some(r) = &a.residente {
            return Ok(r.clone());
        }
        if a.flags & 0x4001 != 0 {
            return sem_suporte("NTFS: atributo comprimido ou cifrado");
        }
        let (runs, tam, init) = self.runs_attr(segs)?;
        if tam > MAX_METADADO {
            return corrompido("NTFS: atributo grande demais para memória");
        }
        let mut c = Corrente::novo(self.v.clone(), cortar_init(runs, tam, init)?, tam);
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut c, &mut out)?;
        Ok(out)
    }
}

/// Aplica o tamanho inicializado: além dele, o conteúdo lê como zeros.
fn cortar_init(runs: Runs, tam: u64, init: u64) -> Result<Vec<Run>> {
    let init = init.min(tam);
    let mut out = Runs::default();
    let mut acc = 0u64;
    for r in runs.v {
        if acc >= init {
            break;
        }
        let n = r.len.min(init - acc);
        out.push(r.desloc, n)?;
        acc += n;
    }
    if tam > init {
        out.push(None, tam - init)?;
    }
    Ok(out.v)
}

fn filetime_unix(ft: u64) -> Option<i64> {
    if ft == 0 {
        return None;
    }
    Some((ft / 10_000_000) as i64 - 11_644_473_600)
}

pub(crate) fn percorrer(v: Vista, ctx: &mut Ctx<'_>) -> Result<()> {
    let b = v.ler_ate(0, 512)?;
    let bps = u64::from(le16(&b, 11));
    let spc_raw = byte(&b, 13);
    let spc = if spc_raw <= 128 {
        u64::from(spc_raw)
    } else {
        1u64 << (256 - u32::from(spc_raw)).min(20)
    };
    if !matches!(bps, 512 | 1024 | 2048 | 4096) || spc == 0 || !spc.is_power_of_two() {
        return corrompido("NTFS: geometria de boot inválida");
    }
    let cluster = bps * spc;
    let rec_raw = byte(&b, 64) as i8;
    let tam_registro = if rec_raw > 0 {
        cluster * rec_raw as u64
    } else {
        1u64 << (-i32::from(rec_raw)).clamp(0, 30)
    };
    if !(512..=65536).contains(&tam_registro) {
        return corrompido("NTFS: tamanho de registro MFT inválido");
    }
    let mft_off = le64(&b, 48)
        .checked_mul(cluster)
        .filter(|o| o.saturating_add(tam_registro) <= v.len())
        .ok_or_else(|| Erro::Corrompido("NTFS: MFT fora do volume".into()))?;
    // Registro 0 ($MFT) descreve o próprio MFT.
    let mut r0 = v.ler_vec(mft_off, tam_registro)?;
    if &r0[..4] != b"FILE" {
        return corrompido("NTFS: $MFT sem assinatura");
    }
    aplicar_fixups(&mut r0)?;
    let provisorio = Ntfs {
        v: v.clone(),
        cluster,
        tam_registro,
        mft: Mapa::novo(vec![Run {
            desloc: Some(mft_off),
            len: tam_registro,
        }]),
    };
    let mut attrs = parse_attrs(&r0)?;
    if attrs.iter().any(|a| a.tipo == T_LISTA) {
        attrs = provisorio.atributos(0)?.1;
    }
    let segs: Vec<Attr> = attrs
        .into_iter()
        .filter(|a| a.tipo == T_DADOS && a.nome.is_empty())
        .collect();
    if segs.is_empty() || segs[0].residente.is_some() {
        return corrompido("NTFS: $MFT sem dados não residentes");
    }
    let (runs, tam, _) = provisorio.runs_attr(&segs)?;
    let mut rv = runs.v;
    // Limita ao tamanho real do MFT.
    let mut acc = 0u64;
    for r in &mut rv {
        let n = r.len.min(tam.saturating_sub(acc));
        r.len = n;
        acc += n;
    }
    rv.retain(|r| r.len > 0);
    let fs = Ntfs {
        v,
        cluster,
        tam_registro,
        mft: Mapa::novo(rv),
    };
    let mut vistos = HashSet::new();
    dir(&fs, ctx, 5, &[], 0, &mut vistos)
}

struct Entrada {
    registro: u64,
    nome: String,
}

/// Entradas do índice `$I30` de um diretório (raiz + blocos alocados em uso).
fn entradas_dir(fs: &Ntfs, attrs: &[Attr]) -> Result<Vec<Entrada>> {
    let i30: Vec<u16> = "$I30".encode_utf16().collect();
    let raiz = attrs
        .iter()
        .find(|a| a.tipo == T_RAIZ_IDX && a.nome == i30)
        .and_then(|a| a.residente.as_ref());
    let Some(raiz) = raiz else {
        return corrompido("NTFS: diretório sem $INDEX_ROOT");
    };
    let tam_bloco = u64::from(le32(raiz, 8));
    let mut out = Vec::new();
    let eo = 16 + le32(raiz, 16) as usize;
    let total = (16 + le32(raiz, 20) as usize).min(raiz.len());
    ler_entradas(raiz, eo, total, &mut out);
    let aloc: Vec<Attr> = attrs
        .iter()
        .filter(|a| a.tipo == T_ALOC_IDX && a.nome == i30)
        .cloned()
        .collect();
    if !aloc.is_empty() {
        if !(512..=65536).contains(&tam_bloco) || !tam_bloco.is_power_of_two() {
            return corrompido("NTFS: tamanho de bloco de índice inválido");
        }
        let bm: Vec<Attr> = attrs
            .iter()
            .filter(|a| a.tipo == T_BITMAP && a.nome == i30)
            .cloned()
            .collect();
        let bitmap = if bm.is_empty() {
            None
        } else {
            Some(fs.conteudo_attr(&bm)?)
        };
        let (runs, tam, _) = fs.runs_attr(&aloc)?;
        let mapa = Mapa::novo(runs.v);
        let n_blocos = tam.min(mapa.total) / tam_bloco;
        let mut buf = vec![0u8; tam_bloco as usize];
        for i in 0..n_blocos {
            if let Some(bm) = &bitmap {
                let usado = bm
                    .get((i / 8) as usize)
                    .is_some_and(|x| x & (1 << (i % 8)) != 0);
                if !usado {
                    continue;
                }
            }
            mapa.ler(&fs.v, i * tam_bloco, &mut buf)?;
            if &buf[..4] != b"INDX" || aplicar_fixups(&mut buf).is_err() {
                continue;
            }
            let eo = 24 + le32(&buf, 24) as usize;
            let total = (24 + le32(&buf, 28) as usize).min(buf.len());
            ler_entradas(&buf, eo, total, &mut out);
            if out.len() > 50_000_000 {
                return corrompido("NTFS: diretório com entradas demais");
            }
        }
    }
    Ok(out)
}

fn ler_entradas(b: &[u8], mut p: usize, fim: usize, out: &mut Vec<Entrada>) {
    for _ in 0..1_000_000 {
        if p + 16 > fim {
            break;
        }
        let tam = usize::from(le16(b, p + 8));
        let ks = usize::from(le16(b, p + 10));
        let fl = le16(b, p + 12);
        if tam < 16 || p + tam > fim {
            break;
        }
        if fl & 2 != 0 {
            break; // entrada final
        }
        let k = fatia(b, p + 16, ks);
        if k.len() >= 66 {
            let ns = k[65];
            let nl = usize::from(k[64]);
            if ns != 2 {
                let nome = utf16le_para_string(fatia(k, 66, nl * 2));
                let registro = le64(b, p) & 0xFFFF_FFFF_FFFF;
                if registro >= PRIMEIRO_REGISTRO_USUARIO && !nome.is_empty() {
                    out.push(Entrada { registro, nome });
                }
            }
        }
        p += tam;
    }
}

fn dir(
    fs: &Ntfs,
    ctx: &mut Ctx<'_>,
    reg: u64,
    pai: &[u8],
    prof: usize,
    vistos: &mut HashSet<u64>,
) -> Result<()> {
    if prof >= MAX_PROFUNDIDADE {
        ctx.aviso("profundidade máxima de diretórios excedida");
        return Ok(());
    }
    if !vistos.insert(reg) {
        ctx.aviso("diretório NTFS repetido (laço)");
        return Ok(());
    }
    let (_, attrs) = fs.atributos(reg)?;
    let entradas = entradas_dir(fs, &attrs)?;
    for e in entradas {
        if ctx.parar {
            break;
        }
        let nome = limpar_nome(e.nome.into_bytes());
        let caminho = juntar(pai, &nome);
        let r = membro(fs, ctx, e.registro, caminho.clone(), prof, vistos);
        ctx.tolerar(r, &caminho)?;
    }
    Ok(())
}

fn membro(
    fs: &Ntfs,
    ctx: &mut Ctx<'_>,
    n: u64,
    caminho: Vec<u8>,
    prof: usize,
    vistos: &mut HashSet<u64>,
) -> Result<()> {
    let (reg, attrs) = fs.atributos(n)?;
    let flags = le16(&reg, 22);
    if flags & 1 == 0 {
        return corrompido("NTFS: registro não está em uso");
    }
    let mtime = attrs
        .iter()
        .find(|a| a.tipo == T_STD)
        .and_then(|a| a.residente.as_ref())
        .and_then(|v| filetime_unix(le64(v, 8)));
    if flags & 2 != 0 {
        let c = ctx.emitir(
            caminho.clone(),
            TipoMembro::Pasta,
            0,
            mtime,
            n,
            &mut empty(),
        )?;
        if c == Controle::Continuar {
            dir(fs, ctx, n, &caminho, prof + 1, vistos)?;
        }
        return Ok(());
    }
    // Links simbólicos e junções (reparse points).
    if let Some(rp) = attrs
        .iter()
        .find(|a| a.tipo == T_REPARSE)
        .and_then(|a| a.residente.as_ref())
    {
        let tag = le32(rp, 0);
        if tag == 0xA000_000C || tag == 0xA000_0003 {
            let base = if tag == 0xA000_000C { 20 } else { 16 };
            let (so, sl) = (usize::from(le16(rp, 8)), usize::from(le16(rp, 10)));
            let (po, pl) = (usize::from(le16(rp, 12)), usize::from(le16(rp, 14)));
            let (o, l) = if pl > 0 { (po, pl) } else { (so, sl) };
            let alvo = utf16le_para_string(fatia(rp, base + o, l)).into_bytes();
            let tam = alvo.len() as u64;
            let mut c = Memoria(Cursor::new(alvo));
            ctx.emitir(caminho, TipoMembro::Link, tam, mtime, n, &mut c)?;
            return Ok(());
        }
    }
    let dados: Vec<Attr> = attrs
        .iter()
        .filter(|a| a.tipo == T_DADOS && a.nome.is_empty())
        .cloned()
        .collect();
    let Some(primeiro) = dados.first() else {
        let mut c = empty();
        ctx.emitir(caminho, TipoMembro::Arquivo, 0, mtime, n, &mut c)?;
        return Ok(());
    };
    if let Some(r) = &primeiro.residente {
        let tam = r.len() as u64;
        let mut c = Memoria(Cursor::new(r.clone()));
        ctx.emitir(caminho, TipoMembro::Arquivo, tam, mtime, n, &mut c)?;
        return Ok(());
    }
    let seg0 = dados.iter().find(|a| a.vcn_ini == 0).unwrap_or(primeiro);
    let tam = seg0.tam_dados;
    if seg0.flags & 0x4001 != 0 {
        let motivo = if seg0.flags & 0x4000 != 0 {
            "cifrado (EFS)"
        } else {
            "comprimido (LZNT1)"
        };
        ctx.aviso(format!(
            "{}: conteúdo {motivo} sem suporte",
            String::from_utf8_lossy(&caminho)
        ));
        let mut c = Corrente::novo(fs.v.clone(), Vec::new(), tam);
        ctx.emitir(caminho, TipoMembro::Arquivo, tam, mtime, n, &mut c)?;
        return Ok(());
    }
    match fs
        .runs_attr(&dados)
        .and_then(|(r, t, i)| cortar_init(r, t, i))
    {
        Ok(runs) => {
            let mut c = Corrente::novo(fs.v.clone(), runs, tam);
            ctx.emitir(caminho, TipoMembro::Arquivo, tam, mtime, n, &mut c)?;
        }
        Err(e) => {
            ctx.aviso(format!("{}: {e}", String::from_utf8_lossy(&caminho)));
            let mut c = Corrente::novo(fs.v.clone(), Vec::new(), tam);
            ctx.emitir(caminho, TipoMembro::Arquivo, tam, mtime, n, &mut c)?;
        }
    }
    Ok(())
}
