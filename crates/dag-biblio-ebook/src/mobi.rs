//! MOBI / AZW / AZW3 (PalmDB + PalmDOC + MOBI header + EXTH). Parser próprio e defensivo:
//! nenhum acesso por índice sem checagem de limite, nenhuma alocação guiada por campo hostil
//! sem teto.
//!
//! Suporta: metadados (cabeçalho e EXTH), capa, DRM (tipo de criptografia e campo DRM do
//! cabeçalho), texto PalmDOC (compressão 1 e 2). HUFF/CDIC (17480) e KFX ficam sem texto.
//! O sumário (índice NCX do MOBI) não é lido. Em AZW3/KF8 o texto vem em partes de
//! "skeleton"/fragmentos: sai como texto corrido, de melhor esforço.

use dag_biblio::{Campo, EstadoLeitura, FormatoLivro, ResultadoExtracao};

use crate::OpcoesExtracao;
use crate::finalizar;
use crate::util::{self, adicionar, adicionar_lista};

const NULO: u32 = 0xFFFF_FFFF;
const TETO_REGISTRO_TEXTO: usize = 64 << 10;
const TETO_IMAGEM: usize = 32 << 20;
const TETO_EXTH_ITENS: u32 = 4096;

fn u16b(d: &[u8], o: usize) -> Option<u16> {
    let b = d.get(o..o.checked_add(2)?)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

fn u32b(d: &[u8], o: usize) -> Option<u32> {
    let b = d.get(o..o.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// O arquivo parece PalmDB de livro (`BOOKMOBI` ou `TEXtREAd`)?
pub(crate) fn assinatura(prefixo: &[u8]) -> bool {
    matches!(prefixo.get(60..68), Some(b"BOOKMOBI" | b"TEXtREAd"))
}

/// Container KFX ("CONT"): só detectado.
pub(crate) fn e_kfx(prefixo: &[u8]) -> bool {
    prefixo.len() >= 8 && prefixo.starts_with(b"CONT") && prefixo[4] == 0 && prefixo[5] <= 4
}

struct PalmDb<'a> {
    dados: &'a [u8],
    inicios: Vec<usize>,
    nome: String,
}

impl<'a> PalmDb<'a> {
    fn ler(dados: &'a [u8]) -> Result<Self, String> {
        if dados.len() < 78 + 8 {
            return Err("arquivo menor que um cabeçalho PalmDB".to_owned());
        }
        let n = usize::from(u16b(dados, 76).ok_or("cabeçalho PalmDB truncado")?);
        if n < 1 {
            return Err("PalmDB sem registros".to_owned());
        }
        let mut inicios = Vec::with_capacity(n);
        for i in 0..n {
            let o = u32b(dados, 78 + i * 8).ok_or("lista de registros truncada")?;
            inicios.push(o as usize);
        }
        let nome_bruto = dados.get(..32).unwrap_or(&[]);
        let fim = nome_bruto.iter().position(|b| *b == 0).unwrap_or(32);
        let nome = util::decodificar_cp1252(nome_bruto.get(..fim).unwrap_or(&[]));
        Ok(Self {
            dados,
            inicios,
            nome,
        })
    }

    fn registro(&self, i: usize) -> Option<&'a [u8]> {
        let ini = *self.inicios.get(i)?;
        let fim = self
            .inicios
            .get(i + 1)
            .copied()
            .unwrap_or(self.dados.len())
            .min(self.dados.len());
        if ini > fim {
            return None;
        }
        self.dados.get(ini..fim)
    }
}

#[derive(Debug, Default)]
struct Exth {
    autores: Vec<String>,
    editora: Option<String>,
    descricao: Option<String>,
    isbn: Option<String>,
    assuntos: Vec<String>,
    data: Option<String>,
    titulo: Option<String>,
    idioma: Option<String>,
    capa: Option<u32>,
    miniatura: Option<u32>,
    kf8: Option<u32>,
    asin: Option<String>,
    direitos: Option<String>,
}

fn ler_exth(bloco: &[u8], utf8: bool) -> Option<Exth> {
    if bloco.get(..4)? != b"EXTH" {
        return None;
    }
    let n = u32b(bloco, 8)?.min(TETO_EXTH_ITENS);
    let mut pos = 12usize;
    let mut e = Exth::default();
    let dec = |b: &[u8]| -> String {
        let s = if utf8 {
            String::from_utf8_lossy(b).into_owned()
        } else {
            util::decodificar_cp1252(b)
        };
        s.trim().trim_matches('\0').to_owned()
    };
    for _ in 0..n {
        let tipo = u32b(bloco, pos)?;
        let tam = u32b(bloco, pos + 4)? as usize;
        if tam < 8 {
            break;
        }
        let Some(conteudo) = bloco.get(pos + 8..pos.checked_add(tam)?) else {
            break;
        };
        pos += tam;
        match tipo {
            100 => {
                let a = dec(conteudo);
                if !a.is_empty() {
                    e.autores.push(inverter_nome(&a));
                }
            }
            101 => e.editora = Some(dec(conteudo)).filter(|s| !s.is_empty() && s != "Unknown"),
            103 => e.descricao = Some(dec(conteudo)).filter(|s| !s.is_empty()),
            104 => e.isbn = Some(dec(conteudo)).filter(|s| !s.is_empty()),
            105 => {
                e.assuntos.extend(
                    dec(conteudo)
                        .split(';')
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty()),
                );
            }
            106 => e.data = Some(dec(conteudo)).filter(|s| !s.is_empty()),
            109 => e.direitos = Some(dec(conteudo)).filter(|s| !s.is_empty()),
            113 => e.asin = Some(dec(conteudo)).filter(|s| !s.is_empty()),
            121 => e.kf8 = u32b(conteudo, 0).filter(|v| *v != NULO),
            201 => e.capa = u32b(conteudo, 0).filter(|v| *v != NULO),
            202 => e.miniatura = u32b(conteudo, 0).filter(|v| *v != NULO),
            503 => e.titulo = Some(dec(conteudo)).filter(|s| !s.is_empty()),
            524 => e.idioma = Some(dec(conteudo)).filter(|s| !s.is_empty()),
            _ => {}
        }
    }
    Some(e)
}

/// Amazon grava "Sobrenome, Nome": inverte como o Calibre faz (uma única vírgula).
fn inverter_nome(a: &str) -> String {
    let partes: Vec<&str> = a.split(',').collect();
    if let [sobrenome, nome] = partes.as_slice() {
        let (s, n) = (sobrenome.trim(), nome.trim());
        if !s.is_empty() && !n.is_empty() {
            return format!("{n} {s}");
        }
    }
    a.to_owned()
}

fn idioma_do_locale(locale: u32) -> Option<&'static str> {
    Some(match locale & 0xFF {
        0x01 => "ar",
        0x04 => "zh",
        0x05 => "cs",
        0x06 => "da",
        0x07 => "de",
        0x08 => "el",
        0x09 => "en",
        0x0A => "es",
        0x0B => "fi",
        0x0C => "fr",
        0x0D => "he",
        0x0E => "hu",
        0x10 => "it",
        0x11 => "ja",
        0x12 => "ko",
        0x13 => "nl",
        0x14 => "no",
        0x15 => "pl",
        0x16 => "pt",
        0x19 => "ru",
        0x1D => "sv",
        0x1F => "tr",
        _ => return None,
    })
}

/// Bytes finais de cada registro de texto que não fazem parte do conteúdo (flags de extra).
pub(crate) fn tamanho_extras(reg: &[u8], flags: u16) -> usize {
    fn entrada(reg: &[u8], mut fim: usize) -> Option<usize> {
        let mut r = 0usize;
        let mut bit = 0u32;
        loop {
            let v = *reg.get(fim.checked_sub(1)?)?;
            r |= usize::from(v & 0x7F) << bit;
            bit += 7;
            fim -= 1;
            if v & 0x80 != 0 || bit >= 28 || fim == 0 {
                return Some(r);
            }
        }
    }
    let mut num = 0usize;
    let mut f = flags >> 1;
    while f != 0 {
        if f & 1 != 0 {
            match reg.len().checked_sub(num).and_then(|fim| entrada(reg, fim)) {
                Some(t) => num = num.saturating_add(t),
                None => return 0,
            }
        }
        f >>= 1;
    }
    if flags & 1 != 0 {
        let Some(off) = reg.len().checked_sub(num + 1) else {
            return 0;
        };
        num += usize::from(reg.get(off).copied().unwrap_or(0) & 3) + 1;
    }
    num.min(reg.len())
}

/// Descompressão PalmDOC (LZ77 de 2 bytes). Nunca passa de `limite` bytes de saída.
pub(crate) fn descomprimir_palmdoc(entrada: &[u8], limite: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(entrada.len().saturating_mul(2).min(limite));
    let mut i = 0usize;
    while i < entrada.len() && out.len() < limite {
        let c = entrada[i];
        i += 1;
        match c {
            0 | 0x09..=0x7F => out.push(c),
            1..=8 => {
                let fim = (i + usize::from(c)).min(entrada.len());
                out.extend_from_slice(&entrada[i..fim]);
                i = fim;
            }
            0x80..=0xBF => {
                let Some(&c2) = entrada.get(i) else { break };
                i += 1;
                let v = ((u16::from(c) << 8) | u16::from(c2)) & 0x3FFF;
                let dist = usize::from(v >> 3);
                let len = usize::from(v & 7) + 3;
                if dist == 0 || dist > out.len() {
                    break;
                }
                let ini = out.len() - dist;
                for k in 0..len {
                    let b = out[ini + k];
                    out.push(b);
                }
            }
            0xC0..=0xFF => {
                out.push(b' ');
                out.push(c ^ 0x80);
            }
        }
    }
    out.truncate(limite);
    out
}

pub(crate) fn extrair(
    dados: &[u8],
    formato_dica: Option<FormatoLivro>,
    opc: &OpcoesExtracao,
) -> ResultadoExtracao {
    let base = match formato_dica {
        Some(f @ (FormatoLivro::Mobi | FormatoLivro::Azw | FormatoLivro::Azw3)) => f,
        _ => FormatoLivro::Mobi,
    };
    let mut r = util::vazio(base, EstadoLeitura::Lido);
    if let Err(m) = corpo(dados, formato_dica, opc, &mut r) {
        r.estado = EstadoLeitura::Corrompido { motivo: m };
    }
    r
}

fn corpo(
    dados: &[u8],
    dica: Option<FormatoLivro>,
    opc: &OpcoesExtracao,
    r: &mut ResultadoExtracao,
) -> Result<(), String> {
    let db = PalmDb::ler(dados)?;
    let r0 = db.registro(0).ok_or("registro 0 fora do arquivo")?;
    if r0.len() < 16 {
        return Err("registro 0 menor que o cabeçalho PalmDOC".to_owned());
    }
    let compressao = u16b(r0, 0).unwrap_or(0);
    let n_texto = usize::from(u16b(r0, 8).unwrap_or(0));
    let cripto = u16b(r0, 12).unwrap_or(0);
    let tem_mobi = r0.get(16..20) == Some(b"MOBI");
    let mut exth = None;
    let mut titulo_completo: Option<String> = None;
    let mut codepage = 1252u32;
    let mut flags_extra = 0u16;
    let mut primeira_imagem = NULO;
    let mut versao = 0u32;
    let mut locale = 0u32;
    let mut drm_campo = false;
    if tem_mobi {
        let comp = u32b(r0, 20).unwrap_or(0) as usize;
        codepage = u32b(r0, 28).unwrap_or(1252);
        versao = u32b(r0, 36).unwrap_or(0).max(u32b(r0, 0x68).unwrap_or(0));
        primeira_imagem = u32b(r0, 0x6C).unwrap_or(NULO);
        locale = u32b(r0, 0x5C).unwrap_or(0);
        let utf8 = codepage == 65001;
        if (0xE4..=500).contains(&comp) {
            flags_extra = u16b(r0, 0xF2).unwrap_or(0);
        }
        let drm_off = u32b(r0, 0xA8).unwrap_or(NULO);
        let drm_n = u32b(r0, 0xAC).unwrap_or(0);
        drm_campo = drm_off != NULO && drm_n > 0;
        if u32b(r0, 0x80).unwrap_or(0) & 0x40 != 0 {
            exth = r0.get(16 + comp..).and_then(|b| ler_exth(b, utf8));
        }
        let (to, tl) = (
            u32b(r0, 0x54).unwrap_or(0) as usize,
            u32b(r0, 0x58).unwrap_or(0) as usize,
        );
        if let Some(b) = to.checked_add(tl).and_then(|fim| r0.get(to..fim)) {
            let t = if utf8 {
                String::from_utf8_lossy(b).into_owned()
            } else {
                util::decodificar_cp1252(b)
            };
            titulo_completo = Some(t.trim().to_owned()).filter(|s| !s.is_empty());
        }
    }
    let utf8 = codepage == 65001;

    // Formato: a dica manda entre MOBI/AZW/AZW3; sem dica, KF8 vira AZW3.
    let kf8 = versao >= 8 || exth.as_ref().is_some_and(|e| e.kf8.is_some());
    r.formato = match dica {
        Some(f @ (FormatoLivro::Mobi | FormatoLivro::Azw | FormatoLivro::Azw3)) => f,
        _ if kf8 => FormatoLivro::Azw3,
        _ => FormatoLivro::Mobi,
    };

    // Metadados.
    let e = exth.unwrap_or_default();
    let titulo = e
        .titulo
        .clone()
        .or(titulo_completo)
        .or_else(|| Some(db.nome.clone()).filter(|s| !s.is_empty()));
    adicionar(
        r,
        Campo::Titulo,
        titulo.as_deref(),
        0.8,
        "mobi:titulo (EXTH 503/cabeçalho)",
    );
    adicionar_lista(r, Campo::Autores, e.autores.clone(), 0.8, "mobi:EXTH 100");
    adicionar(
        r,
        Campo::Editora,
        e.editora.as_deref(),
        0.75,
        "mobi:EXTH 101",
    );
    adicionar(
        r,
        Campo::Resumo,
        e.descricao
            .as_deref()
            .map(|d| util::texto_de_html(d, 100_000))
            .as_deref(),
        0.7,
        "mobi:EXTH 103",
    );
    adicionar_lista(r, Campo::Tags, e.assuntos.clone(), 0.6, "mobi:EXTH 105");
    adicionar(r, Campo::Data, e.data.as_deref(), 0.75, "mobi:EXTH 106");
    match e.idioma.as_deref() {
        Some(i) => adicionar(r, Campo::Idioma, Some(i), 0.85, "mobi:EXTH 524"),
        None => {
            if let Some(i) = idioma_do_locale(locale) {
                adicionar(r, Campo::Idioma, Some(i), 0.5, "mobi:locale do cabeçalho");
            }
        }
    }
    if let Some(i) = e.isbn.as_deref() {
        if !finalizar::isbn_de_identificador(r, i, "mobi:EXTH 104") {
            r.avisos.push(format!(
                "ISBN do EXTH 104 inválido: {}",
                util::truncar(i, 40)
            ));
        }
    }
    if let Some(a) = &e.asin {
        r.avisos
            .push(format!("ASIN (EXTH 113): {}", util::truncar(a, 40)));
    }
    if let Some(d) = &e.direitos {
        r.avisos
            .push(format!("direitos (EXTH 109): {}", util::truncar(d, 200)));
    }

    // DRM.
    let drm = cripto != 0 || drm_campo;
    if drm {
        r.estado = EstadoLeitura::Drm;
        r.avisos.push(format!(
            "DRM detectado, não contornado (criptografia {cripto}{})",
            if drm_campo {
                ", campo DRM do cabeçalho"
            } else {
                ""
            }
        ));
    }

    // Capa.
    if opc.extrair_capa && primeira_imagem != NULO {
        let cands = [e.capa, e.miniatura];
        for off in cands.into_iter().flatten() {
            let idx = (primeira_imagem as usize).checked_add(off as usize);
            let reg = idx.and_then(|i| db.registro(i));
            if let Some(b) = reg.filter(|b| b.len() <= TETO_IMAGEM) {
                if let Some(m) = util::mime_imagem(b) {
                    r.capa = Some(b.to_vec());
                    r.capa_mime = Some(m.to_owned());
                    break;
                }
            }
        }
        if r.capa.is_none() && (e.capa.is_some() || e.miniatura.is_some()) {
            r.avisos
                .push("registro de capa do EXTH não é uma imagem reconhecida".to_owned());
        }
    }

    // Texto.
    if drm {
        return Ok(());
    }
    match compressao {
        1 | 2 => {}
        17480 => {
            r.avisos
                .push("texto HUFF/CDIC (compressão 17480) não suportado; só metadados".to_owned());
            return Ok(());
        }
        outra => {
            r.avisos
                .push(format!("compressão {outra} desconhecida; só metadados"));
            return Ok(());
        }
    }
    let alvo = (opc.paginas_texto as usize)
        .saturating_mul(opc.caracteres_por_pagina)
        .saturating_mul(6)
        .clamp(16 << 10, 8 << 20);
    let mut bruto: Vec<u8> = Vec::new();
    for i in 1..=n_texto {
        if bruto.len() >= alvo {
            break;
        }
        let Some(reg) = db.registro(i) else {
            r.avisos
                .push(format!("registro de texto {i} fora do arquivo"));
            break;
        };
        let extras = tamanho_extras(reg, flags_extra);
        let uteis = reg.get(..reg.len() - extras).unwrap_or(reg);
        if compressao == 2 {
            bruto.extend(descomprimir_palmdoc(uteis, TETO_REGISTRO_TEXTO));
        } else {
            bruto.extend_from_slice(
                uteis
                    .get(..uteis.len().min(TETO_REGISTRO_TEXTO))
                    .unwrap_or(&[]),
            );
        }
    }
    let html = if utf8 {
        String::from_utf8_lossy(&bruto).into_owned()
    } else {
        util::decodificar_cp1252(&bruto)
    };
    let texto = util::texto_de_html(&html, alvo);
    let maximo = opc.paginas_texto as usize;
    r.paginas = util::montar_paginas(util::paginar(&texto, opc.caracteres_por_pagina), maximo);
    r.total_paginas = None;
    finalizar::analisar_texto(r, &[]);
    Ok(())
}
