//! ext2/ext3/ext4: extents, mapa de blocos, dados inline, 64 bits. Somente leitura.
//!
//! Diretórios com htree são lidos linearmente; o journal não é reexecutado.

use super::{Ctx, MAX_PROFUNDIDADE};
use crate::disco::{Corrente, MAX_DIR, Memoria, Runs, Vista};
use crate::erro::{Result, corrompido, sem_suporte};
use crate::util::{byte, fatia, juntar, le16, le32, limpar_nome};
use crate::{Controle, TipoMembro};
use std::collections::HashSet;
use std::io::{Cursor, empty};

const INCOMPAT_COMPRESSAO: u32 = 0x1;
const INCOMPAT_META_BG: u32 = 0x10;
const INCOMPAT_64BIT: u32 = 0x80;
const FL_CRIPTO: u32 = 0x800;
const FL_EXTENTS: u32 = 0x80000;
const FL_INLINE: u32 = 0x1000_0000;

struct Ext {
    v: Vista,
    bs: u64,
    ipg: u64,
    inode_size: u64,
    grupos: u64,
    gdt_off: u64,
    desc_size: u64,
    b64: bool,
    n_inodes: u64,
}

struct Inode {
    modo: u16,
    tamanho: u64,
    mtime: Option<i64>,
    flags: u32,
    i_block: [u8; 60],
    bruto: Vec<u8>,
}

impl Ext {
    fn abrir(v: Vista) -> Result<Ext> {
        let sb = v.ler_ate(1024, 1024)?;
        if le16(&sb, 56) != 0xEF53 {
            return corrompido("ext: assinatura inválida");
        }
        let log = le32(&sb, 24);
        if log > 6 {
            return corrompido("ext: tamanho de bloco inválido");
        }
        let bs = 1024u64 << log;
        let incompat = le32(&sb, 96);
        if incompat & INCOMPAT_COMPRESSAO != 0 {
            return sem_suporte("ext: compressão de arquivos");
        }
        if incompat & INCOMPAT_META_BG != 0 {
            return sem_suporte("ext: meta_bg");
        }
        let b64 = incompat & INCOMPAT_64BIT != 0;
        let rev = le32(&sb, 76);
        let inode_size = if rev == 0 {
            128
        } else {
            u64::from(le16(&sb, 88))
        };
        if !(128..=bs).contains(&inode_size) || !inode_size.is_power_of_two() {
            return corrompido("ext: tamanho de inode inválido");
        }
        let mut blocos = u64::from(le32(&sb, 4));
        if b64 {
            blocos |= u64::from(le32(&sb, 336)) << 32;
        }
        let bpg = u64::from(le32(&sb, 32));
        let ipg = u64::from(le32(&sb, 40));
        let primeiro = u64::from(le32(&sb, 20));
        if bpg == 0 || ipg == 0 || blocos <= primeiro {
            return corrompido("ext: geometria inválida");
        }
        let grupos = (blocos - primeiro).div_ceil(bpg);
        let desc_size = if b64 {
            u64::from(le16(&sb, 254)).max(64)
        } else {
            32
        };
        if desc_size > 1024 || grupos > 1 << 24 {
            return corrompido("ext: descritores de grupo inválidos");
        }
        Ok(Ext {
            v,
            bs,
            ipg,
            inode_size,
            grupos,
            gdt_off: (primeiro + 1) * bs,
            desc_size,
            b64,
            n_inodes: u64::from(le32(&sb, 0)),
        })
    }

    fn inode(&self, ino: u64) -> Result<Inode> {
        if ino == 0 || ino > self.n_inodes {
            return corrompido("ext: número de inode inválido");
        }
        let g = (ino - 1) / self.ipg;
        let idx = (ino - 1) % self.ipg;
        if g >= self.grupos {
            return corrompido("ext: inode fora dos grupos");
        }
        let d = self
            .v
            .ler_vec(self.gdt_off + g * self.desc_size, self.desc_size)?;
        let mut tab = u64::from(le32(&d, 8));
        if self.b64 {
            tab |= u64::from(le32(&d, 0x28)) << 32;
        }
        let off = tab
            .checked_mul(self.bs)
            .and_then(|t| t.checked_add(idx * self.inode_size))
            .ok_or_else(|| crate::Erro::Corrompido("ext: tabela de inodes".into()))?;
        let b = self.v.ler_vec(off, self.inode_size)?;
        let modo = le16(&b, 0);
        let mut tam = u64::from(le32(&b, 4));
        if modo & 0xF000 == 0x8000 || modo & 0xF000 == 0x4000 {
            tam |= u64::from(le32(&b, 108)) << 32;
        }
        let extra = if self.inode_size > 128 {
            u64::from(le16(&b, 128))
        } else {
            0
        };
        let mut mt = i64::from(le32(&b, 16) as i32);
        if extra >= 8 && self.inode_size >= 128 + 12 {
            mt += i64::from(le32(&b, 136) & 3) << 32;
        }
        let mut i_block = [0u8; 60];
        i_block.copy_from_slice(fatia(&b, 40, 60));
        Ok(Inode {
            modo,
            tamanho: tam,
            mtime: (mt != 0).then_some(mt),
            flags: le32(&b, 32),
            i_block,
            bruto: b,
        })
    }

    /// Valor do xattr `system.data` (conteúdo inline além dos 60 bytes de i_block).
    fn xattr_data(&self, ino: &Inode) -> Vec<u8> {
        let b = &ino.bruto;
        if b.len() <= 128 {
            return Vec::new();
        }
        let extra = usize::from(le16(b, 128));
        let ini = 128 + extra;
        if le32(b, ini) != 0xEA02_0000 {
            return Vec::new();
        }
        let base = ini + 4;
        let mut p = base;
        while p + 16 <= b.len() {
            let nl = usize::from(byte(b, p));
            if nl == 0 && le32(b, p) == 0 {
                break;
            }
            let idx = byte(b, p + 1);
            let voff = usize::from(le16(b, p + 2));
            let vsz = le32(b, p + 8) as usize;
            let nome = fatia(b, p + 16, nl);
            if idx == 7 && nome == b"data" {
                return fatia(b, base + voff, vsz).to_vec();
            }
            p += (16 + nl).div_ceil(4) * 4;
        }
        Vec::new()
    }

    fn runs(&self, ino: &Inode) -> Result<Runs> {
        let mut runs = Runs::default();
        let needed = ino.tamanho.div_ceil(self.bs);
        if ino.flags & FL_EXTENTS != 0 {
            let mut folhas: Vec<(u64, u64, u64, bool)> = Vec::new();
            let mut visitados = 0usize;
            self.extents(&ino.i_block, 0, &mut folhas, &mut visitados)?;
            folhas.sort_by_key(|f| f.0);
            let mut cur = 0u64;
            for (l, n, p, uninit) in folhas {
                if l < cur {
                    return corrompido("ext: extents sobrepostos");
                }
                if l > cur {
                    runs.push(None, (l - cur) * self.bs)?;
                }
                if uninit {
                    runs.push(None, n * self.bs)?;
                } else {
                    let off = p
                        .checked_mul(self.bs)
                        .filter(|o| o.saturating_add(n * self.bs) <= self.v.len())
                        .ok_or_else(|| {
                            crate::Erro::Corrompido("ext: extent fora da imagem".into())
                        })?;
                    runs.push(Some(off), n * self.bs)?;
                }
                cur = l + n;
            }
            if cur < needed {
                runs.push(None, (needed - cur) * self.bs)?;
            }
        } else {
            let mut restante = needed;
            let ib: Vec<u32> = (0..15).map(|i| le32(&ino.i_block, i * 4)).collect();
            for &b in &ib[..12] {
                if restante == 0 {
                    break;
                }
                self.bloco(u64::from(b), &mut runs)?;
                restante -= 1;
            }
            for (nivel, &b) in ib[12..].iter().enumerate() {
                if restante == 0 {
                    break;
                }
                let consumido =
                    self.indireto(u64::from(b), nivel as u32 + 1, restante, &mut runs)?;
                restante = restante.saturating_sub(consumido);
            }
        }
        Ok(runs)
    }

    fn bloco(&self, b: u64, runs: &mut Runs) -> Result<()> {
        if b == 0 {
            return runs.push(None, self.bs);
        }
        let off = b
            .checked_mul(self.bs)
            .filter(|o| o.saturating_add(self.bs) <= self.v.len())
            .ok_or_else(|| crate::Erro::Corrompido("ext: bloco fora da imagem".into()))?;
        runs.push(Some(off), self.bs)
    }

    /// Percorre um bloco indireto de `nivel` (1 simples, 2 duplo, 3 triplo); devolve
    /// quantos blocos de dados cobriu (até `restante`).
    fn indireto(&self, b: u64, nivel: u32, restante: u64, runs: &mut Runs) -> Result<u64> {
        let apb = self.bs / 4;
        let cobre = apb.saturating_pow(nivel);
        let n = restante.min(cobre);
        if b == 0 {
            runs.push(None, n * self.bs)?;
            return Ok(n);
        }
        let dados = self.v.ler_vec(
            b.checked_mul(self.bs)
                .ok_or_else(|| crate::Erro::Corrompido("ext: indireto inválido".into()))?,
            self.bs,
        )?;
        let mut feito = 0u64;
        for i in 0..apb as usize {
            if feito >= n {
                break;
            }
            let p = u64::from(le32(&dados, i * 4));
            if nivel == 1 {
                self.bloco(p, runs)?;
                feito += 1;
            } else {
                feito += self.indireto(p, nivel - 1, n - feito, runs)?;
            }
        }
        Ok(feito)
    }

    fn extents(
        &self,
        no: &[u8],
        prof: u32,
        folhas: &mut Vec<(u64, u64, u64, bool)>,
        visitados: &mut usize,
    ) -> Result<()> {
        *visitados += 1;
        if *visitados > 100_000 || prof > 5 {
            return corrompido("ext: árvore de extents grande demais");
        }
        if le16(no, 0) != 0xF30A {
            return corrompido("ext: cabeçalho de extents inválido");
        }
        let n = usize::from(le16(no, 2));
        let profundidade = le16(no, 6);
        for i in 0..n {
            let e = fatia(no, 12 + i * 12, 12);
            if e.len() < 12 {
                return corrompido("ext: nó de extents truncado");
            }
            if profundidade == 0 {
                let mut len = u64::from(le16(e, 4));
                let uninit = len > 32768;
                if uninit {
                    len -= 32768;
                }
                let p = u64::from(le32(e, 8)) | (u64::from(le16(e, 6)) << 32);
                if len > 0 {
                    folhas.push((u64::from(le32(e, 0)), len, p, uninit));
                }
                if folhas.len() > 4_000_000 {
                    return corrompido("ext: extents demais");
                }
            } else {
                let filho = u64::from(le32(e, 4)) | (u64::from(le16(e, 8)) << 32);
                let bloco = self.v.ler_vec(
                    filho
                        .checked_mul(self.bs)
                        .ok_or_else(|| crate::Erro::Corrompido("ext: nó inválido".into()))?,
                    self.bs,
                )?;
                self.extents(&bloco, prof + 1, folhas, visitados)?;
            }
        }
        Ok(())
    }

    /// Conteúdo completo em memória (diretórios, links); limitado.
    fn conteudo_mem(&self, ino: &Inode) -> Result<Vec<u8>> {
        if ino.tamanho > MAX_DIR {
            return corrompido("ext: estrutura grande demais para memória");
        }
        if ino.flags & FL_INLINE != 0 {
            let mut d = ino.i_block.to_vec();
            if ino.tamanho > 60 {
                d.extend(self.xattr_data(ino));
            }
            d.truncate(ino.tamanho as usize);
            return Ok(d);
        }
        if ino.modo & 0xF000 == 0xA000 && ino.tamanho < 60 && ino.flags & FL_EXTENTS == 0 {
            return Ok(ino.i_block[..ino.tamanho as usize].to_vec());
        }
        let runs = self.runs(ino)?;
        let mut c = Corrente::novo(self.v.clone(), runs.v, ino.tamanho);
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut c, &mut out)?;
        Ok(out)
    }
}

pub(crate) fn percorrer(v: Vista, ctx: &mut Ctx<'_>) -> Result<()> {
    let fs = Ext::abrir(v)?;
    let mut vistos = HashSet::new();
    dir(&fs, ctx, 2, &[], 0, &mut vistos)
}

fn dir(
    fs: &Ext,
    ctx: &mut Ctx<'_>,
    ino_dir: u64,
    pai: &[u8],
    prof: usize,
    vistos: &mut HashSet<u64>,
) -> Result<()> {
    if prof >= MAX_PROFUNDIDADE {
        ctx.aviso("profundidade máxima de diretórios excedida");
        return Ok(());
    }
    if !vistos.insert(ino_dir) {
        ctx.aviso("diretório ext repetido (laço)");
        return Ok(());
    }
    let di = fs.inode(ino_dir)?;
    if di.modo & 0xF000 != 0x4000 {
        return corrompido("ext: inode de diretório não é diretório");
    }
    if di.flags & FL_CRIPTO != 0 {
        return sem_suporte("ext: diretório cifrado");
    }
    let mut dados = fs.conteudo_mem(&di)?;
    if di.flags & FL_INLINE != 0 && dados.len() >= 4 {
        dados.drain(..4); // inode do pai
    }
    let bs = if di.flags & FL_INLINE != 0 {
        usize::MAX / 2
    } else {
        fs.bs as usize
    };
    let mut pos = 0usize;
    let mut entradas: Vec<(u64, Vec<u8>)> = Vec::new();
    while pos + 8 <= dados.len() {
        let ino = u64::from(le32(&dados, pos));
        let rec = usize::from(le16(&dados, pos + 4));
        let nl = usize::from(byte(&dados, pos + 6));
        if rec < 8 || rec % 4 != 0 || pos + rec > dados.len() || 8 + nl > rec {
            // bloco corrompido: tenta o próximo bloco
            if bs >= dados.len() {
                break;
            }
            pos = (pos / bs + 1) * bs;
            continue;
        }
        if ino != 0 && nl > 0 {
            let nome = fatia(&dados, pos + 8, nl).to_vec();
            if nome != b"." && nome != b".." {
                entradas.push((ino, nome));
            }
        }
        pos += rec;
    }
    for (ino, nome) in entradas {
        if ctx.parar {
            break;
        }
        let nome = limpar_nome(nome);
        let caminho = juntar(pai, &nome);
        let r = membro(fs, ctx, ino, caminho.clone(), prof, vistos);
        ctx.tolerar(r, &caminho)?;
    }
    Ok(())
}

fn membro(
    fs: &Ext,
    ctx: &mut Ctx<'_>,
    ino: u64,
    caminho: Vec<u8>,
    prof: usize,
    vistos: &mut HashSet<u64>,
) -> Result<()> {
    let i = fs.inode(ino)?;
    match i.modo & 0xF000 {
        0x4000 => {
            let c = ctx.emitir(
                caminho.clone(),
                TipoMembro::Pasta,
                0,
                i.mtime,
                ino,
                &mut empty(),
            )?;
            if c == Controle::Continuar {
                dir(fs, ctx, ino, &caminho, prof + 1, vistos)?;
            }
        }
        0xA000 => {
            let alvo = fs.conteudo_mem(&i)?;
            let tam = alvo.len() as u64;
            let mut l = Memoria(Cursor::new(alvo));
            ctx.emitir(caminho, TipoMembro::Link, tam, i.mtime, ino, &mut l)?;
        }
        0x8000 => {
            if i.flags & FL_CRIPTO != 0 {
                ctx.aviso(format!(
                    "{}: conteúdo cifrado (ext4 encrypt) não lido",
                    String::from_utf8_lossy(&caminho)
                ));
                let mut l = Corrente::novo(fs.v.clone(), Vec::new(), i.tamanho);
                ctx.emitir(
                    caminho,
                    TipoMembro::Arquivo,
                    i.tamanho,
                    i.mtime,
                    ino,
                    &mut l,
                )?;
                return Ok(());
            }
            if i.flags & FL_INLINE != 0 {
                let d = fs.conteudo_mem(&i)?;
                let tam = d.len() as u64;
                let mut l = Memoria(Cursor::new(d));
                ctx.emitir(caminho, TipoMembro::Arquivo, tam, i.mtime, ino, &mut l)?;
                return Ok(());
            }
            match fs.runs(&i) {
                Ok(r) => {
                    let mut l = Corrente::novo(fs.v.clone(), r.v, i.tamanho);
                    ctx.emitir(
                        caminho,
                        TipoMembro::Arquivo,
                        i.tamanho,
                        i.mtime,
                        ino,
                        &mut l,
                    )?;
                }
                Err(e) => {
                    ctx.aviso(format!("{}: {e}", String::from_utf8_lossy(&caminho)));
                    let mut l = Corrente::novo(fs.v.clone(), Vec::new(), i.tamanho);
                    ctx.emitir(
                        caminho,
                        TipoMembro::Arquivo,
                        i.tamanho,
                        i.mtime,
                        ino,
                        &mut l,
                    )?;
                }
            }
        }
        _ => {} // dispositivos, fifos e sockets não têm conteúdo
    }
    Ok(())
}
