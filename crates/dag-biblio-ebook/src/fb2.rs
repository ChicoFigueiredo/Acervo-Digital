//! FictionBook 2 (`.fb2`, `.fb2.zip`): `description` (title-info, publish-info), capa em
//! `binary` base64, sumário pelas seções e texto dos primeiros parágrafos.

use std::sync::Arc;

use dag_biblio::{Campo, EstadoLeitura, FormatoLivro, ItemSumario, ResultadoExtracao};
use roxmltree::Node;

use crate::OpcoesExtracao;
use crate::finalizar;
use crate::limites::Medidor;
use crate::origem::Origem;
use crate::pacote::{self, Decisao, Falha};
use crate::util::{self, adicionar, adicionar_lista};
use crate::xml::{self, atributo, filho, filhos, texto, texto_de};

const ITENS_SUMARIO_MAX: usize = 3000;

/// Reconhece FB2 puro pelo começo do arquivo.
pub(crate) fn assinatura(prefixo: &[u8]) -> bool {
    let p = prefixo.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(prefixo);
    let t = String::from_utf8_lossy(p);
    let t = t.trim_start();
    (t.starts_with("<?xml") || t.starts_with("<FictionBook")) && t.contains("<FictionBook")
}

pub(crate) fn extrair(
    origem: &Origem,
    opc: &OpcoesExtracao,
    med: &Medidor,
    e_zip: bool,
) -> ResultadoExtracao {
    let mut r = util::vazio(FormatoLivro::Fb2, EstadoLeitura::Lido);
    if let Err(f) = corpo(origem, opc, med, e_zip, &mut r) {
        r.estado = f.estado();
    }
    r
}

fn corpo(
    origem: &Origem,
    opc: &OpcoesExtracao,
    med: &Medidor,
    e_zip: bool,
    r: &mut ResultadoExtracao,
) -> Result<(), Falha> {
    let teto = med.limites().bytes_membro_max;
    let bytes: Arc<[u8]> = if e_zip {
        let v = pacote::varrer(origem, med, &mut |n, _| {
            if util::extensao(n) == "fb2" {
                Decisao::Ler(teto)
            } else {
                Decisao::Ignorar
            }
        })?;
        r.avisos.extend(v.avisos);
        let fb2 = v
            .entradas
            .into_iter()
            .find_map(|e| e.dados)
            .ok_or_else(|| Falha::corrompido("zip sem arquivo .fb2"))?;
        if fb2.len() as u64 >= teto {
            return Err(Falha::Limite(format!("FB2 maior que {teto} bytes")));
        }
        Arc::from(fb2)
    } else {
        origem.tudo(teto)?
    };
    let max_texto = (opc.paginas_texto as usize)
        .saturating_mul(opc.caracteres_por_pagina)
        .saturating_mul(2);
    let saida = xml::analisar(&bytes, |doc| ler(doc, opc, max_texto))
        .map_err(|e| Falha::corrompido(format!("FB2 inválido: {e}")))?;
    saida.aplicar(r, opc);
    Ok(())
}

#[derive(Debug, Default)]
struct Lido {
    titulo: Option<String>,
    autores: Vec<String>,
    generos: Vec<String>,
    resumo: Option<String>,
    data: Option<String>,
    idioma: Option<String>,
    serie: Option<String>,
    indice: Option<String>,
    editora: Option<String>,
    ano: Option<String>,
    isbn: Option<String>,
    capa_id: Option<String>,
    capa: Option<(Vec<u8>, String)>,
    sumario: Vec<ItemSumario>,
    paragrafos: Vec<String>,
}

fn nome_pessoa(a: Node<'_, '_>) -> String {
    let partes: Vec<String> = ["first-name", "middle-name", "last-name"]
        .iter()
        .filter_map(|n| texto_de(a, n))
        .collect();
    if partes.is_empty() {
        texto_de(a, "nickname").unwrap_or_default()
    } else {
        partes.join(" ")
    }
}

fn ler(doc: &roxmltree::Document<'_>, opc: &OpcoesExtracao, max_texto: usize) -> Lido {
    let mut l = Lido::default();
    let raiz = doc.root_element();
    if let Some(d) = filho(raiz, "description") {
        if let Some(ti) = filho(d, "title-info") {
            l.titulo = texto_de(ti, "book-title");
            l.autores = filhos(ti, "author")
                .map(nome_pessoa)
                .filter(|s| !s.is_empty())
                .collect();
            l.generos = filhos(ti, "genre")
                .map(texto)
                .filter(|s| !s.is_empty())
                .collect();
            if let Some(a) = filho(ti, "annotation") {
                let par: Vec<String> = a
                    .descendants()
                    .filter(|n| n.is_element() && n.tag_name().name() == "p")
                    .map(texto)
                    .filter(|s| !s.is_empty())
                    .collect();
                let t = if par.is_empty() {
                    texto(a)
                } else {
                    par.join("\n")
                };
                l.resumo = Some(t).filter(|s| !s.is_empty());
            }
            l.data = texto_de(ti, "date");
            l.idioma = texto_de(ti, "lang");
            if let Some(s) = filho(ti, "sequence") {
                l.serie = atributo(s, "name")
                    .map(str::to_owned)
                    .filter(|s| !s.is_empty());
                l.indice = atributo(s, "number")
                    .map(str::to_owned)
                    .filter(|s| !s.is_empty());
            }
            l.capa_id = filho(ti, "coverpage")
                .and_then(|c| filho(c, "image"))
                .and_then(|i| atributo(i, "href"))
                .map(|h| h.trim_start_matches('#').to_owned());
        }
        if let Some(pi) = filho(d, "publish-info") {
            l.editora = texto_de(pi, "publisher");
            l.ano = texto_de(pi, "year");
            l.isbn = texto_de(pi, "isbn");
            if l.serie.is_none() {
                if let Some(s) = filho(pi, "sequence") {
                    l.serie = atributo(s, "name")
                        .map(str::to_owned)
                        .filter(|s| !s.is_empty());
                    l.indice = atributo(s, "number")
                        .map(str::to_owned)
                        .filter(|s| !s.is_empty());
                }
            }
        }
    }
    if opc.extrair_capa {
        if let Some(id) = &l.capa_id {
            if let Some(b) = filhos(raiz, "binary").find(|b| b.attribute("id") == Some(id.as_str()))
            {
                let mime = b.attribute("content-type").unwrap_or("").to_owned();
                let cru = b.text().unwrap_or("");
                if cru.len() <= 48 << 20 {
                    l.capa = Some((base64(cru), mime));
                }
            }
        }
    }
    // Corpo principal (o primeiro `body` sem name="notes").
    let corpo = filhos(raiz, "body").find(|b| b.attribute("name") != Some("notes"));
    if let Some(c) = corpo {
        let mut total = 0usize;
        percorrer(c, 0, &mut l, &mut total, max_texto);
    }
    l
}

fn percorrer(n: Node<'_, '_>, nivel: u8, l: &mut Lido, total: &mut usize, max: usize) {
    if nivel > 24 {
        return;
    }
    for c in n.children().filter(Node::is_element) {
        match c.tag_name().name() {
            "title" => {
                let t = texto(c);
                if !t.is_empty() {
                    if l.sumario.len() < ITENS_SUMARIO_MAX && nivel > 0 {
                        l.sumario.push(ItemSumario {
                            titulo: t.clone(),
                            nivel: nivel - 1,
                            pagina: None,
                        });
                    }
                    if *total < max {
                        *total += t.len();
                        l.paragrafos.push(t);
                    }
                }
            }
            "section" => percorrer(c, nivel + 1, l, total, max),
            "p" | "subtitle" | "epigraph" | "cite" | "poem" | "stanza" | "v" | "text-author" => {
                if *total < max {
                    let t = texto(c);
                    if !t.is_empty() {
                        *total += t.len();
                        l.paragrafos.push(t);
                    }
                }
            }
            "body" => percorrer(c, nivel, l, total, max),
            _ => {}
        }
        // Estruturas com parágrafos aninhados (epigraph, cite, poem) já foram achatadas por texto().
    }
}

impl Lido {
    fn aplicar(self, r: &mut ResultadoExtracao, opc: &OpcoesExtracao) {
        adicionar(
            r,
            Campo::Titulo,
            self.titulo.as_deref(),
            0.85,
            "fb2:book-title",
        );
        adicionar_lista(r, Campo::Autores, self.autores, 0.85, "fb2:author");
        adicionar_lista(r, Campo::Tags, self.generos, 0.6, "fb2:genre");
        adicionar(
            r,
            Campo::Resumo,
            self.resumo.as_deref(),
            0.7,
            "fb2:annotation",
        );
        adicionar(r, Campo::Idioma, self.idioma.as_deref(), 0.9, "fb2:lang");
        adicionar(
            r,
            Campo::Editora,
            self.editora.as_deref(),
            0.8,
            "fb2:publish-info/publisher",
        );
        match self.ano.as_deref() {
            Some(a) => adicionar(r, Campo::Data, Some(a), 0.8, "fb2:publish-info/year"),
            None => adicionar(
                r,
                Campo::Data,
                self.data.as_deref(),
                0.7,
                "fb2:title-info/date",
            ),
        }
        adicionar(
            r,
            Campo::Serie,
            self.serie.as_deref(),
            0.8,
            "fb2:sequence/@name",
        );
        adicionar(
            r,
            Campo::IndiceSerie,
            self.indice.as_deref(),
            0.8,
            "fb2:sequence/@number",
        );
        if let Some(i) = self.isbn.as_deref() {
            if !finalizar::isbn_de_identificador(r, i, "fb2:publish-info/isbn") {
                r.avisos
                    .push(format!("ISBN do FB2 inválido: {}", util::truncar(i, 40)));
            }
        }
        if let Some((bytes, mime)) = self.capa {
            match util::mime_imagem(&bytes) {
                Some(m) => {
                    r.capa_mime = Some(m.to_owned());
                    r.capa = Some(bytes);
                }
                None if mime.starts_with("image/") && !bytes.is_empty() => {
                    r.capa_mime = Some(mime);
                    r.capa = Some(bytes);
                }
                None => r
                    .avisos
                    .push("capa do FB2 não é imagem reconhecida".to_owned()),
            }
        } else if self.capa_id.is_some() && opc.extrair_capa {
            r.avisos
                .push("capa do FB2 referenciada mas sem `binary`".to_owned());
        }
        r.sumario = self.sumario;
        let texto = self.paragrafos.join("\n");
        let maximo = opc.paginas_texto as usize;
        r.paginas = util::montar_paginas(util::paginar(&texto, opc.caracteres_por_pagina), maximo);
        finalizar::analisar_texto(r, &[]);
    }
}

/// Base64 padrão (ignora espaços e quebras de linha; para no primeiro byte inválido).
pub(crate) fn base64(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            c if c.is_ascii_whitespace() => continue,
            _ => break,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}
