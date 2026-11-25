//! Quadrinhos CBZ (zip), CBR (rar) e CB7 (7z): `ComicInfo.xml`, `CoMet`, contagem e ordem
//! natural das páginas (imagens), capa (primeira imagem) e, sem metadado, número da edição
//! deduzido do nome do arquivo (evidência de baixa confiança).
//!
//! Regra de ouro: **nunca misturar números diferentes**. `Number` do ComicInfo (ou o número
//! do nome do arquivo) vai para `IndiceSerie`; `Volume` vai para `Volume`; `Title` é o título
//! do exemplar e `Series` é a série. Nada é derivado de um para outro.

use dag_biblio::{
    Campo, EstadoLeitura, Evidencia, FormatoLivro, OrigemEvidencia, ResultadoExtracao,
    ValorSugerido,
};

use crate::OpcoesExtracao;
use crate::limites::Medidor;
use crate::origem::Origem;
use crate::pacote::{Decisao, Falha, varrer};
use crate::util::{self, adicionar, adicionar_lista};
use crate::xml::{self, TETO_XML, atributo, filho, filhos, texto, texto_de};

const TETO_CAPA: u64 = 48 << 20;

pub(crate) fn extrair(
    origem: &Origem,
    formato: FormatoLivro,
    nome: Option<&str>,
    opc: &OpcoesExtracao,
    med: &Medidor,
) -> ResultadoExtracao {
    let mut r = util::vazio(formato, EstadoLeitura::Lido);
    if let Err(f) = corpo(origem, nome, opc, med, &mut r) {
        r.estado = f.estado();
    }
    r
}

/// Ignora lixo de empacotadores (`__MACOSX`, `Thumbs.db`, arquivos ocultos).
fn lixo(nome: &str) -> bool {
    let base = util::nome_base(nome);
    nome.starts_with("__MACOSX/") || base.starts_with('.') || base.eq_ignore_ascii_case("thumbs.db")
}

fn corpo(
    origem: &Origem,
    nome: Option<&str>,
    opc: &OpcoesExtracao,
    med: &Medidor,
    r: &mut ResultadoExtracao,
) -> Result<(), Falha> {
    // Uma só passada (rar e 7z custam caro): guarda ComicInfo, CoMet e a imagem que, até aqui,
    // é a primeira na ordem natural.
    let mut melhor: Option<String> = None;
    let mut imagem: Option<Vec<u8>> = None;
    let v = {
        let melhor = &mut melhor;
        varrer(origem, med, &mut |n, _| {
            if lixo(n) {
                return Decisao::Ignorar;
            }
            let base = util::nome_base(n).to_ascii_lowercase();
            if base == "comicinfo.xml" || base == "comet.xml" {
                return Decisao::Ler(TETO_XML);
            }
            if util::e_imagem(n) {
                let primeira = melhor
                    .as_deref()
                    .is_none_or(|m| util::comparar_natural(n, m).is_lt());
                if primeira && opc.extrair_capa {
                    *melhor = Some(n.to_owned());
                    return Decisao::Ler(TETO_CAPA);
                }
            }
            Decisao::Ignorar
        })?
    };
    r.avisos.extend(v.avisos.iter().cloned());
    let total_entradas = v.entradas.len();
    let mut paginas: Vec<String> = v
        .entradas
        .iter()
        .filter(|e| !lixo(&e.nome) && util::e_imagem(&e.nome))
        .map(|e| e.nome.clone())
        .collect();
    paginas.sort_by(|a, b| util::comparar_natural(a, b));
    let util_total = v.entradas.iter().filter(|e| !lixo(&e.nome)).count();
    if paginas.is_empty() || paginas.len() * 2 < util_total {
        return Err(Falha::nao_suportado(format!(
            "não parece quadrinho: {} imagem(ns) em {util_total} arquivo(s)",
            paginas.len()
        )));
    }
    let _ = total_entradas;
    for e in &v.entradas {
        if Some(&e.nome) == melhor.as_ref() {
            imagem = e.dados.clone();
        }
    }
    r.total_paginas = Some(u32::try_from(paginas.len()).unwrap_or(u32::MAX));

    let comicinfo = v
        .entradas
        .iter()
        .find(|e| util::nome_base(&e.nome).eq_ignore_ascii_case("comicinfo.xml"))
        .and_then(|e| e.dados.as_ref());
    let comet = v
        .entradas
        .iter()
        .find(|e| util::nome_base(&e.nome).eq_ignore_ascii_case("comet.xml"))
        .and_then(|e| e.dados.as_ref());
    let mut capa_indice = 0usize;
    let mut tem_numero = false;
    if let Some(b) = comicinfo {
        match xml::analisar(b, |d| ler_comicinfo(d, r)) {
            Ok((tn, capa)) => {
                tem_numero = tn;
                capa_indice = capa.unwrap_or(0);
            }
            Err(e) => r.avisos.push(format!("ComicInfo.xml inválido: {e}")),
        }
    }
    if let Some(b) = comet {
        if let Err(e) = xml::analisar(b, |d| ler_comet(d, r)) {
            r.avisos.push(format!("CoMet inválido: {e}"));
        }
    }
    // PageCount declarado vs. real.
    if let Some(pc) = r.avisos.iter().find_map(|a| {
        a.strip_prefix("PageCount do ComicInfo: ")
            .map(str::to_owned)
    }) {
        if pc.trim().parse::<usize>().ok() != Some(paginas.len()) {
            r.avisos.push(format!(
                "PageCount do ComicInfo ({}) difere das imagens ({})",
                pc.trim(),
                paginas.len()
            ));
        }
        r.avisos
            .retain(|a| !a.starts_with("PageCount do ComicInfo: "));
    }

    // Nome do arquivo: só como evidência fraca, nunca sobrescreve metadado interno.
    if let Some(n) = nome {
        aplicar_nome_arquivo(n, tem_numero, r);
    }

    // Capa.
    if opc.extrair_capa {
        let alvo = paginas.get(capa_indice).cloned();
        let dados = if capa_indice == 0 {
            imagem
        } else if let Some(alvo) = alvo {
            // A capa declarada não é a primeira imagem: segunda passada só para ela.
            let v2 = varrer(origem, med, &mut |n, _| {
                if n == alvo {
                    Decisao::Ler(TETO_CAPA)
                } else {
                    Decisao::Ignorar
                }
            })?;
            v2.entradas.into_iter().find_map(|e| e.dados)
        } else {
            None
        };
        if let Some(b) = dados {
            match util::mime_imagem(&b) {
                Some(m) => {
                    r.capa_mime = Some(m.to_owned());
                    r.capa = Some(b);
                }
                None => r
                    .avisos
                    .push("primeira imagem não é um formato reconhecido".to_owned()),
            }
        }
    }
    Ok(())
}

fn ev(origem: OrigemEvidencia, detalhe: &str, trecho: Option<String>, conf: f32) -> Evidencia {
    Evidencia {
        origem,
        pagina: None,
        trecho,
        detalhe: Some(detalhe.to_owned()),
        confianca: conf,
    }
}

/// Devolve (tem Number, índice da página de capa declarada).
fn ler_comicinfo(
    doc: &roxmltree::Document<'_>,
    r: &mut ResultadoExtracao,
) -> (bool, Option<usize>) {
    let n = doc.root_element();
    let t = |nome: &str| texto_de(n, nome);
    adicionar(
        r,
        Campo::Serie,
        t("Series").as_deref(),
        0.9,
        "ComicInfo:Series",
    );
    let numero = t("Number");
    adicionar(
        r,
        Campo::IndiceSerie,
        numero.as_deref(),
        0.9,
        "ComicInfo:Number",
    );
    adicionar(
        r,
        Campo::Volume,
        t("Volume").as_deref(),
        0.85,
        "ComicInfo:Volume",
    );
    adicionar(
        r,
        Campo::Titulo,
        t("Title").as_deref(),
        0.85,
        "ComicInfo:Title",
    );
    let mut autores: Vec<String> = Vec::new();
    for campo in ["Writer", "Penciller"] {
        if let Some(v) = t(campo) {
            let l: Vec<String> = v
                .split([',', ';'])
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            if campo == "Writer" || autores.is_empty() {
                autores.extend(l);
            }
            if campo == "Writer" && !autores.is_empty() {
                break;
            }
        }
    }
    adicionar_lista(r, Campo::Autores, autores, 0.8, "ComicInfo:Writer");
    adicionar(
        r,
        Campo::Editora,
        t("Publisher").as_deref(),
        0.85,
        "ComicInfo:Publisher",
    );
    if let Some(ano) = t("Year").filter(|a| a.parse::<u32>().is_ok_and(|v| v > 0)) {
        let data = match (
            t("Month")
                .and_then(|m| m.parse::<u32>().ok())
                .filter(|m| (1..=12).contains(m)),
            t("Day")
                .and_then(|d| d.parse::<u32>().ok())
                .filter(|d| (1..=31).contains(d)),
        ) {
            (Some(m), Some(d)) => format!("{ano}-{m:02}-{d:02}"),
            (Some(m), None) => format!("{ano}-{m:02}"),
            _ => ano,
        };
        adicionar(r, Campo::Data, Some(&data), 0.85, "ComicInfo:Year");
    }
    adicionar(
        r,
        Campo::Idioma,
        t("LanguageISO").as_deref(),
        0.9,
        "ComicInfo:LanguageISO",
    );
    adicionar(
        r,
        Campo::Resumo,
        t("Summary").as_deref(),
        0.7,
        "ComicInfo:Summary",
    );
    let mut tags: Vec<String> = Vec::new();
    for campo in ["Genre", "Tags"] {
        if let Some(v) = t(campo) {
            tags.extend(v.split(',').map(|s| s.trim().to_owned()));
        }
    }
    adicionar_lista(r, Campo::Tags, tags, 0.6, "ComicInfo:Genre/Tags");
    if let Some(g) = t("GTIN").or_else(|| t("ISBN")) {
        if !crate::finalizar::isbn_de_identificador(r, &g, "ComicInfo:GTIN") {
            r.avisos.push(format!(
                "GTIN/ISBN do ComicInfo não é ISBN válido: {}",
                util::truncar(&g, 40)
            ));
        }
    }
    if let Some(pc) = t("PageCount") {
        r.avisos.push(format!("PageCount do ComicInfo: {pc}"));
    }
    // Página de capa declarada (`<Pages><Page Image="3" Type="FrontCover"/>`).
    let capa = filho(n, "Pages").and_then(|p| {
        filhos(p, "Page")
            .find(|pg| atributo(*pg, "Type") == Some("FrontCover"))
            .and_then(|pg| atributo(pg, "Image"))
            .and_then(|i| i.trim().parse::<usize>().ok())
    });
    (numero.is_some(), capa)
}

fn ler_comet(doc: &roxmltree::Document<'_>, r: &mut ResultadoExtracao) {
    let n = doc.root_element();
    let presentes: Vec<Campo> = r.metadados.iter().map(|m| m.campo).collect();
    let tem = |c: Campo| presentes.contains(&c);
    let t = |nome: &str| texto_de(n, nome);
    let (tem_titulo, tem_serie, tem_num, tem_vol) = (
        tem(Campo::Titulo),
        tem(Campo::Serie),
        tem(Campo::IndiceSerie),
        tem(Campo::Volume),
    );
    let mut novos: Vec<(Campo, String, &str)> = Vec::new();
    if !tem_titulo {
        if let Some(v) = t("title") {
            novos.push((Campo::Titulo, v, "CoMet:title"));
        }
    }
    if !tem_serie {
        if let Some(v) = t("series") {
            novos.push((Campo::Serie, v, "CoMet:series"));
        }
    }
    if !tem_num {
        if let Some(v) = t("issue") {
            novos.push((Campo::IndiceSerie, v, "CoMet:issue"));
        }
    }
    if !tem_vol {
        if let Some(v) = t("volume") {
            novos.push((Campo::Volume, v, "CoMet:volume"));
        }
    }
    if !tem(Campo::Editora) {
        if let Some(v) = t("publisher") {
            novos.push((Campo::Editora, v, "CoMet:publisher"));
        }
    }
    if !tem(Campo::Resumo) {
        if let Some(v) = t("description") {
            novos.push((Campo::Resumo, v, "CoMet:description"));
        }
    }
    if !tem(Campo::Idioma) {
        if let Some(v) = t("language") {
            novos.push((Campo::Idioma, v, "CoMet:language"));
        }
    }
    if !tem(Campo::Data) {
        if let Some(v) = t("date") {
            novos.push((Campo::Data, v, "CoMet:date"));
        }
    }
    for (c, v, d) in novos {
        adicionar(r, c, Some(&v), 0.8, d);
    }
    if !tem(Campo::Autores) {
        let ws: Vec<String> = filhos(n, "writer").map(texto).collect();
        adicionar_lista(r, Campo::Autores, ws, 0.75, "CoMet:writer");
    }
}

// ---------------------------------------------------------------------------------------------
// Nome do arquivo
// ---------------------------------------------------------------------------------------------

/// O que o nome do arquivo sugere.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct NomeQuadrinho {
    pub serie: Option<String>,
    pub numero: Option<String>,
    pub volume: Option<String>,
}

fn so_digitos(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

fn sem_zeros(s: &str) -> String {
    let t = s.trim_start_matches('0');
    if t.is_empty() {
        "0".to_owned()
    } else {
        t.to_owned()
    }
}

fn ano_provavel(s: &str) -> bool {
    s.len() == 4 && s.parse::<u32>().is_ok_and(|a| (1900..=2100).contains(&a))
}

/// Remove grupos `(...)`, `[...]`, `{...}`; `_` e `.` viram espaço (menos entre dígitos).
fn limpar_nome(stem: &str) -> String {
    let mut out = String::new();
    let mut prof = 0i32;
    let chars: Vec<char> = stem.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '(' | '[' | '{' => {
                prof += 1;
                out.push(' ');
            }
            ')' | ']' | '}' => {
                prof = (prof - 1).max(0);
                out.push(' ');
            }
            _ if prof > 0 => {}
            '_' => out.push(' '),
            '.' => {
                let entre_digitos = i > 0
                    && chars.get(i + 1).is_some_and(char::is_ascii_digit)
                    && chars[i - 1].is_ascii_digit();
                out.push(if entre_digitos { '.' } else { ' ' });
            }
            _ => out.push(c),
        }
    }
    out
}

pub(crate) fn analisar_nome(nome_arquivo: &str) -> NomeQuadrinho {
    let base = util::nome_base(nome_arquivo);
    let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
    let limpo = limpar_nome(stem);
    let tokens: Vec<&str> = limpo.split_whitespace().collect();
    let mut res = NomeQuadrinho::default();
    let mut fim_serie = tokens.len();
    let mut i = 0usize;
    while i < tokens.len() {
        let t = tokens[i];
        let baixo = t.to_lowercase();
        let proximo = tokens.get(i + 1).copied();
        // "#045", "# 045"
        if let Some(d) = baixo.strip_prefix('#') {
            let d = if d.is_empty() {
                proximo.unwrap_or("")
            } else {
                d
            };
            if so_digitos(d.trim_end_matches(['.', ',', ':'])) && res.numero.is_none() {
                res.numero = Some(sem_zeros(d.trim_end_matches(['.', ',', ':'])));
                fim_serie = fim_serie.min(i);
                i += if baixo == "#" { 2 } else { 1 };
                continue;
            }
        }
        // "vol", "vol.", "volume", "v" + número; "vol2", "v2"
        let eh_vol = matches!(baixo.as_str(), "vol" | "volume" | "v" | "tomo");
        if eh_vol {
            if let Some(p) = proximo.filter(|p| so_digitos(p)) {
                if res.volume.is_none() {
                    res.volume = Some(sem_zeros(p));
                    fim_serie = fim_serie.min(i);
                }
                i += 2;
                continue;
            }
        }
        for pref in ["volume", "vol", "tomo"] {
            if let Some(d) = baixo.strip_prefix(pref).filter(|d| so_digitos(d)) {
                if res.volume.is_none() {
                    res.volume = Some(sem_zeros(d));
                    fim_serie = fim_serie.min(i);
                }
            }
        }
        // "nº 12", "no 12", "n 12", "ed 12", "edição 12", "issue 12", "nr"
        let marcador_num = matches!(
            baixo.as_str(),
            "n" | "no"
                | "nº"
                | "n°"
                | "nr"
                | "num"
                | "edição"
                | "edicao"
                | "ed"
                | "issue"
                | "numero"
                | "número"
        );
        if marcador_num {
            if let Some(p) = proximo.filter(|p| so_digitos(p)) {
                if res.numero.is_none() {
                    res.numero = Some(sem_zeros(p));
                    fim_serie = fim_serie.min(i);
                }
                i += 2;
                continue;
            }
        }
        for pref in ["nº", "n°", "no"] {
            if let Some(d) = baixo.strip_prefix(pref).filter(|d| so_digitos(d)) {
                if res.numero.is_none() {
                    res.numero = Some(sem_zeros(d));
                    fim_serie = fim_serie.min(i);
                }
            }
        }
        i += 1;
    }
    // Sem marcador: último token numérico (que não seja um ano).
    if res.numero.is_none() {
        if let Some((idx, t)) = tokens
            .iter()
            .enumerate()
            .rev()
            .find(|(_, t)| so_digitos(t) || t.trim_end_matches('.').parse::<f32>().is_ok())
        {
            let is_vol_token = res.volume.is_some() && idx + 1 < tokens.len();
            if so_digitos(t) && t.len() <= 5 && !ano_provavel(t) && !is_vol_token && idx > 0 {
                // O número vem depois do nome da série: "Tex 123".
                let anterior = tokens[idx - 1].to_lowercase();
                if !matches!(anterior.as_str(), "vol" | "volume" | "v" | "tomo") {
                    res.numero = Some(sem_zeros(t));
                    fim_serie = fim_serie.min(idx);
                }
            }
        }
    }
    let serie: Vec<&str> = tokens.iter().take(fim_serie).copied().collect();
    let serie = serie
        .join(" ")
        .trim_matches(|c: char| c == '-' || c == ':' || c == '–' || c.is_whitespace())
        .to_owned();
    if !serie.is_empty() && (res.numero.is_some() || res.volume.is_some()) {
        res.serie = Some(serie);
    }
    res
}

fn aplicar_nome_arquivo(nome: &str, tem_numero_interno: bool, r: &mut ResultadoExtracao) {
    let a = analisar_nome(nome);
    let trecho = Some(util::truncar(util::nome_base(nome), 200));
    let ja = |r: &ResultadoExtracao, c: Campo| r.metadados.iter().any(|m| m.campo == c);
    let mut novos: Vec<ValorSugerido> = Vec::new();
    if let Some(n) = &a.numero {
        if tem_numero_interno {
            let interno = r
                .metadados
                .iter()
                .find(|m| m.campo == Campo::IndiceSerie)
                .and_then(|m| m.valor.clone())
                .unwrap_or_default();
            if sem_zeros(interno.trim()) != *n {
                r.avisos.push(format!(
                    "número no nome do arquivo ({n}) difere do ComicInfo ({interno}); mantido o ComicInfo"
                ));
            }
        } else {
            novos.push(ValorSugerido {
                campo: Campo::IndiceSerie,
                valor: Some(n.clone()),
                lista: Vec::new(),
                confianca: 0.35,
                evidencias: vec![ev(
                    OrigemEvidencia::NomeArquivo,
                    "número do nome do arquivo",
                    trecho.clone(),
                    0.35,
                )],
            });
        }
    }
    if let Some(v) = &a.volume {
        if !ja(r, Campo::Volume) {
            novos.push(ValorSugerido {
                campo: Campo::Volume,
                valor: Some(v.clone()),
                lista: Vec::new(),
                confianca: 0.3,
                evidencias: vec![ev(
                    OrigemEvidencia::NomeArquivo,
                    "volume do nome do arquivo",
                    trecho.clone(),
                    0.3,
                )],
            });
        }
    }
    if let Some(s) = &a.serie {
        if !ja(r, Campo::Serie) {
            novos.push(ValorSugerido {
                campo: Campo::Serie,
                valor: Some(s.clone()),
                lista: Vec::new(),
                confianca: 0.3,
                evidencias: vec![ev(
                    OrigemEvidencia::NomeArquivo,
                    "série do nome do arquivo",
                    trecho,
                    0.3,
                )],
            });
        }
    }
    r.metadados.extend(novos);
}

#[cfg(test)]
mod testes {
    use super::*;

    fn n(s: &str) -> NomeQuadrinho {
        analisar_nome(s)
    }

    #[test]
    fn numeros_do_nome() {
        let a = n("Tex 123.cbr");
        assert_eq!(a.serie.as_deref(), Some("Tex"));
        assert_eq!(a.numero.as_deref(), Some("123"));
        assert_eq!(a.volume, None);
        let b = n("Tex #045 (1990).cbz");
        assert_eq!(b.numero.as_deref(), Some("45"));
        assert_eq!(b.serie.as_deref(), Some("Tex"));
        let c = n("Zagor_Nº_12_[Mythos].cbz");
        assert_eq!(c.numero.as_deref(), Some("12"));
        assert_eq!(c.serie.as_deref(), Some("Zagor"));
    }

    #[test]
    fn volume_e_numero_nao_se_misturam() {
        let a = n("Sandman Vol. 2 #15.cbz");
        assert_eq!(a.volume.as_deref(), Some("2"));
        assert_eq!(a.numero.as_deref(), Some("15"));
        assert_eq!(a.serie.as_deref(), Some("Sandman"));
        let b = n("Akira Vol 3.cb7");
        assert_eq!(b.volume.as_deref(), Some("3"));
        assert_eq!(b.numero, None);
    }

    #[test]
    fn ano_nao_e_numero() {
        let a = n("Batman (2011).cbz");
        assert_eq!(a.numero, None);
        let b = n("Batman 1989.cbz");
        assert_eq!(b.numero, None);
        assert_eq!(n("so um nome.cbz"), NomeQuadrinho::default());
    }
}
