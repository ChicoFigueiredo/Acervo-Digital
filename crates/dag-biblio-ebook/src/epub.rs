//! EPUB 2 e 3: `container.xml` → OPF (metadados, manifesto, spine, guia) → sumário (nav ou NCX),
//! capa, texto das primeiras seções e DRM. Tudo em memória, em até quatro passadas baratas
//! pelo zip (a central de diretório do zip permite pular o que não interessa).
//!
//! # Páginas
//!
//! Uma "página" do EPUB é um **bloco de até `caracteres_por_pagina` caracteres** (padrão 2000),
//! cortado em fronteira de parágrafo, do texto das seções do spine lidas em ordem; os blocos
//! são numerados de 1 em sequência, sem passar de `paginas_texto`. O EPUB não tem paginação
//! fixa, então `total_paginas` fica `None`.

use std::collections::{HashMap, HashSet};

use dag_biblio::{Campo, EstadoLeitura, FormatoLivro, ItemSumario, ResultadoExtracao};
use roxmltree::Node;

use crate::OpcoesExtracao;
use crate::finalizar;
use crate::limites::Medidor;
use crate::origem::Origem;
use crate::pacote::{Decisao, Falha, varrer};
use crate::util::{self, adicionar, adicionar_lista};
use crate::xml::{self, TETO_XML, atributo, filho, filhos, texto};

/// Seções do spine carregadas no máximo (para páginas, ficha e ISBN).
const SECOES_MAX: usize = 40;
/// Bytes lidos de cada seção (só o começo interessa).
const TETO_SECAO: u64 = 2 << 20;
const TETO_CAPA: u64 = 32 << 20;
const ITENS_SUMARIO_MAX: usize = 3000;

const ALGORITMOS_OFUSCACAO: [&str; 2] = [
    "http://www.idpf.org/2008/embedding",
    "http://ns.adobe.com/pdf/enc#rc",
];

#[derive(Debug, Default)]
struct Item {
    id: String,
    href: String,
    tipo: String,
    props: String,
}

#[derive(Debug, Default)]
struct Opf {
    titulos: Vec<(String, Option<String>)>,
    autores: Vec<(String, Option<String>)>,
    outros_papeis: Vec<(String, String)>,
    editora: Option<String>,
    data: Option<String>,
    idiomas: Vec<String>,
    identificadores: Vec<(String, Option<String>)>,
    assuntos: Vec<String>,
    descricao: Option<String>,
    serie: Option<String>,
    indice_serie: Option<String>,
    titulo_ordenacao: Option<String>,
    colunas_calibre: usize,
    capa_id: Option<String>,
    manifesto: Vec<Item>,
    spine: Vec<String>,
    spine_toc: Option<String>,
    guia: Vec<(String, String)>,
}

pub(crate) fn extrair(origem: &Origem, opc: &OpcoesExtracao, med: &Medidor) -> ResultadoExtracao {
    let mut r = util::vazio(FormatoLivro::Epub, EstadoLeitura::Lido);
    if let Err(f) = corpo(origem, opc, med, &mut r) {
        r.estado = f.estado();
    }
    r
}

fn nomes_ci(nomes: &[String]) -> HashMap<String, String> {
    nomes
        .iter()
        .map(|n| (n.to_ascii_lowercase(), n.clone()))
        .collect()
}

fn corpo(
    origem: &Origem,
    opc: &OpcoesExtracao,
    med: &Medidor,
    r: &mut ResultadoExtracao,
) -> Result<(), Falha> {
    // Passada 1: nomes, mimetype, container.xml, encryption.xml.
    let v1 = varrer(
        origem,
        med,
        &mut |n, _| match n.to_ascii_lowercase().as_str() {
            "mimetype" => Decisao::Ler(512),
            "meta-inf/container.xml" | "meta-inf/encryption.xml" => Decisao::Ler(TETO_XML),
            _ => Decisao::Ignorar,
        },
    )?;
    r.avisos.extend(v1.avisos);
    let nomes: Vec<String> = v1.entradas.iter().map(|e| e.nome.clone()).collect();
    let ci = nomes_ci(&nomes);
    let dado = |alvo: &str| -> Option<&Vec<u8>> {
        v1.entradas
            .iter()
            .find(|e| e.nome.eq_ignore_ascii_case(alvo))
            .and_then(|e| e.dados.as_ref())
    };
    if let Some(m) = dado("mimetype") {
        if String::from_utf8_lossy(m).trim() != "application/epub+zip" {
            r.avisos
                .push("mimetype diferente de application/epub+zip".to_owned());
        }
    }

    // DRM.
    let cripto = dado("META-INF/encryption.xml")
        .map(|b| analisar_criptografia(b))
        .unwrap_or_default();
    let tem = |n: &str| ci.contains_key(n);
    let mut motivos: Vec<String> = Vec::new();
    if tem("meta-inf/license.lcpl") {
        motivos.push("Readium LCP (license.lcpl)".to_owned());
    }
    if tem("meta-inf/sinf.xml") {
        motivos.push("Apple FairPlay (sinf.xml)".to_owned());
    }
    if !cripto.cifrados.is_empty() || !cripto.algoritmos.is_empty() {
        let adept = tem("meta-inf/rights.xml");
        motivos.push(format!(
            "{} (algoritmos: {})",
            if adept {
                "Adobe ADEPT"
            } else {
                "criptografia de conteúdo"
            },
            cripto.algoritmos.join(", ")
        ));
    }
    let drm = !motivos.is_empty();
    if drm {
        r.estado = EstadoLeitura::Drm;
        r.avisos.push(format!(
            "DRM detectado, não contornado: {}",
            motivos.join("; ")
        ));
    } else if cripto.ofuscacao {
        r.avisos
            .push("fontes ofuscadas (IDPF/Adobe): não é DRM".to_owned());
    }

    // container.xml → OPF.
    let opf_caminho = dado("META-INF/container.xml")
        .and_then(|b| caminho_do_opf(b))
        .or_else(|| {
            nomes
                .iter()
                .find(|n| util::extensao(n) == "opf")
                .inspect(|_| {
                    r.avisos
                        .push("container.xml ausente ou inválido: OPF achado pelo nome".to_owned());
                })
                .cloned()
        })
        .ok_or_else(|| Falha::corrompido("EPUB sem container.xml/OPF"))?;
    let opf_caminho = ci
        .get(&opf_caminho.to_ascii_lowercase())
        .cloned()
        .ok_or_else(|| Falha::corrompido(format!("OPF {opf_caminho:?} não está no arquivo")))?;
    if cripto.cifrados.contains(&opf_caminho) {
        return Ok(());
    }

    // Passada 2: o OPF.
    let alvo = opf_caminho.clone();
    let v2 = varrer(origem, med, &mut |n, _| {
        if n == alvo {
            Decisao::Ler(TETO_XML)
        } else {
            Decisao::Ignorar
        }
    })?;
    let opf_bytes = v2
        .entradas
        .into_iter()
        .find(|e| e.nome == opf_caminho)
        .and_then(|e| e.dados)
        .ok_or_else(|| Falha::corrompido("OPF ilegível"))?;
    if opf_bytes.len() as u64 >= TETO_XML {
        return Err(Falha::Limite(format!("OPF maior que {TETO_XML} bytes")));
    }
    let opf = xml::analisar(&opf_bytes, ler_opf)
        .map_err(|e| Falha::corrompido(format!("OPF inválido: {e}")))?;
    let dir_opf = util::diretorio(&opf_caminho).to_owned();
    montar_metadados(&opf, r);

    // Arquivos desejados na passada 3.
    let mut desejados: HashMap<String, u64> = HashMap::new();
    let achar = |href: &str| -> Option<String> {
        let p = util::resolver_href(&dir_opf, href)?;
        ci.get(&p.to_ascii_lowercase()).cloned()
    };
    let item_por_id: HashMap<&str, &Item> =
        opf.manifesto.iter().map(|i| (i.id.as_str(), i)).collect();
    let nav = opf
        .manifesto
        .iter()
        .find(|i| i.props.split_whitespace().any(|p| p == "nav"))
        .and_then(|i| achar(&i.href));
    let item_ncx = opf
        .spine_toc
        .as_deref()
        .and_then(|id| item_por_id.get(id).copied())
        .or_else(|| {
            opf.manifesto
                .iter()
                .find(|i| i.tipo == "application/x-dtbncx+xml")
        });
    let ncx = item_ncx.and_then(|i| achar(&i.href));
    for p in nav.iter().chain(ncx.iter()) {
        desejados.insert(p.clone(), TETO_XML);
    }
    let mut secoes: Vec<String> = Vec::new();
    if !drm {
        for idref in &opf.spine {
            if secoes.len() >= SECOES_MAX {
                break;
            }
            if let Some(p) = item_por_id.get(idref.as_str()).and_then(|i| achar(&i.href)) {
                if !secoes.contains(&p) && !cripto.cifrados.contains(&p) {
                    secoes.push(p);
                }
            }
        }
        for s in &secoes {
            desejados.insert(s.clone(), TETO_SECAO);
        }
        // Páginas de copyright/título apontadas pelo guia ou por landmarks, mesmo fora do começo.
        for (tipo, href) in &opf.guia {
            if matches!(tipo.as_str(), "copyright-page" | "title-page" | "cover") {
                if let Some(p) = achar(href) {
                    if !cripto.cifrados.contains(&p) {
                        desejados.entry(p).or_insert(TETO_SECAO);
                    }
                }
            }
        }
    }
    let capa = if opc.extrair_capa {
        localizar_capa(&opf, &item_por_id)
    } else {
        None
    };
    let mut capa_imagem: Option<(String, String)> = None; // (caminho, mime declarado)
    let mut capa_pagina: Option<String> = None;
    match capa {
        Some(Capa::Imagem(href, mime)) => {
            if let Some(p) = achar(&href) {
                if !cripto.cifrados.contains(&p) {
                    desejados.insert(p.clone(), TETO_CAPA);
                    capa_imagem = Some((p, mime));
                }
            }
        }
        Some(Capa::Pagina(href)) => {
            if let Some(p) = achar(&href) {
                desejados.entry(p.clone()).or_insert(TETO_SECAO);
                capa_pagina = Some(p);
            }
        }
        None => {}
    }

    // Passada 3.
    let v3 = varrer(origem, med, &mut |n, _| match desejados.get(n) {
        Some(t) => Decisao::Ler(*t),
        None => Decisao::Ignorar,
    })?;
    let mut dados: HashMap<String, Vec<u8>> = v3
        .entradas
        .into_iter()
        .filter_map(|e| e.dados.map(|d| (e.nome, d)))
        .collect();

    // Sumário.
    if !drm || nav.is_some() || ncx.is_some() {
        let mut sumario: Vec<ItemSumario> = Vec::new();
        if let Some(b) = nav.as_ref().and_then(|p| dados.get(p)) {
            match xml::analisar(b, sumario_nav) {
                Ok(s) => sumario = s,
                Err(e) => r.avisos.push(format!("nav ilegível: {e}")),
            }
        }
        if sumario.is_empty() {
            if let Some(b) = ncx.as_ref().and_then(|p| dados.get(p)) {
                match xml::analisar(b, sumario_ncx) {
                    Ok(s) => sumario = s,
                    Err(e) => r.avisos.push(format!("NCX ilegível: {e}")),
                }
            }
        }
        r.sumario = sumario;
    }

    // Capa.
    if let Some((p, mime)) = capa_imagem {
        if let Some(b) = dados.remove(&p) {
            guardar_capa(r, b, &mime);
        }
    } else if let Some(pagina) = capa_pagina {
        let img = dados
            .get(&pagina)
            .map(|b| util::decodificar_documento(b))
            .and_then(|h| {
                util::atributo_de_tag(&h, "img", "src")
                    .or_else(|| util::atributo_de_tag(&h, "image", "href"))
            });
        if let Some(src) = img {
            let dir = util::diretorio(&pagina).to_owned();
            if let Some(p) = util::resolver_href(&dir, &src)
                .and_then(|p| ci.get(&p.to_ascii_lowercase()).cloned())
            {
                let v4 = varrer(origem, med, &mut |n, _| {
                    if n == p {
                        Decisao::Ler(TETO_CAPA)
                    } else {
                        Decisao::Ignorar
                    }
                })?;
                if let Some(b) = v4
                    .entradas
                    .into_iter()
                    .find(|e| e.nome == p)
                    .and_then(|e| e.dados)
                {
                    guardar_capa(r, b, "");
                }
            }
        }
    }

    // Texto.
    if !drm {
        let mut textos: Vec<String> = Vec::new();
        let mut blocos: Vec<String> = Vec::new();
        let maximo = opc.paginas_texto as usize;
        for s in &secoes {
            let Some(b) = dados.get(s) else { continue };
            let html = util::decodificar_documento(b);
            let t = util::texto_de_html(&html, 400_000);
            if t.is_empty() {
                continue;
            }
            if blocos.len() < maximo {
                blocos.extend(util::paginar(&t, opc.caracteres_por_pagina));
            }
            textos.push(t);
        }
        // Páginas do guia fora do começo do spine entram só como texto para ISBN.
        for (tipo, href) in &opf.guia {
            if tipo == "copyright-page" {
                if let Some(p) = achar(href) {
                    if !secoes.contains(&p) {
                        if let Some(b) = dados.get(&p) {
                            textos.push(util::texto_de_html(
                                &util::decodificar_documento(b),
                                200_000,
                            ));
                        }
                    }
                }
            }
        }
        r.paginas = util::montar_paginas(blocos, maximo);
        // O texto das páginas já está em `r.paginas`; o resto vai como extra.
        finalizar::analisar_texto(r, &textos);
    }
    Ok(())
}

fn guardar_capa(r: &mut ResultadoExtracao, bytes: Vec<u8>, mime_declarado: &str) {
    match util::mime_imagem(&bytes) {
        Some(m) => {
            r.capa_mime = Some(m.to_owned());
            r.capa = Some(bytes);
        }
        None if mime_declarado.starts_with("image/") && !bytes.is_empty() => {
            r.avisos.push(
                "capa com assinatura de imagem desconhecida; mantido o tipo do manifesto"
                    .to_owned(),
            );
            r.capa_mime = Some(mime_declarado.to_owned());
            r.capa = Some(bytes);
        }
        None => r
            .avisos
            .push("arquivo de capa não é uma imagem reconhecida".to_owned()),
    }
}

// ---------------------------------------------------------------------------------------------
// DRM
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Criptografia {
    /// Algoritmos de criptografia de **conteúdo** (não ofuscação de fonte).
    algoritmos: Vec<String>,
    /// Recursos cifrados por esses algoritmos.
    cifrados: HashSet<String>,
    ofuscacao: bool,
}

fn analisar_criptografia(bytes: &[u8]) -> Criptografia {
    let mut c = Criptografia::default();
    let ok = xml::analisar(bytes, |doc| {
        for ed in doc
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "EncryptedData")
        {
            let alg = ed
                .descendants()
                .find(|n| n.is_element() && n.tag_name().name() == "EncryptionMethod")
                .and_then(|m| atributo(m, "Algorithm"))
                .unwrap_or("")
                .to_owned();
            let uri = ed
                .descendants()
                .find(|n| n.is_element() && n.tag_name().name() == "CipherReference")
                .and_then(|m| atributo(m, "URI"))
                .map(|u| util::resolver_href("", u).unwrap_or_else(|| u.to_owned()));
            if ALGORITMOS_OFUSCACAO.contains(&alg.to_ascii_lowercase().as_str()) {
                c.ofuscacao = true;
                continue;
            }
            if !c.algoritmos.contains(&alg) {
                c.algoritmos.push(alg);
            }
            if let Some(u) = uri {
                c.cifrados.insert(u);
            }
        }
    });
    if ok.is_err() {
        // Não deu para entender o encryption.xml: melhor assumir que há algo cifrado.
        c.algoritmos.push("encryption.xml ilegível".to_owned());
    }
    c
}

// ---------------------------------------------------------------------------------------------
// container.xml e OPF
// ---------------------------------------------------------------------------------------------

fn caminho_do_opf(bytes: &[u8]) -> Option<String> {
    xml::analisar(bytes, |doc| {
        let rf: Vec<Node> = doc
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "rootfile")
            .collect();
        rf.iter()
            .find(|n| atributo(**n, "media-type") == Some("application/oebps-package+xml"))
            .or_else(|| rf.first())
            .and_then(|n| atributo(*n, "full-path"))
            .map(|p| util::normalizar_nome(p.as_bytes()))
    })
    .ok()
    .flatten()
}

fn ler_opf(doc: &roxmltree::Document<'_>) -> Opf {
    let mut o = Opf::default();
    let raiz = doc.root_element();
    // `meta refines`: id -> [(propriedade, valor)]
    let mut refina: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let metadata = filho(raiz, "metadata");
    if let Some(m) = metadata {
        for n in filhos(m, "meta") {
            if let (Some(alvo), Some(p)) = (atributo(n, "refines"), atributo(n, "property")) {
                refina
                    .entry(alvo.trim_start_matches('#').to_owned())
                    .or_default()
                    .push((p.to_owned(), texto(n)));
            }
        }
        let prop = |id: Option<&str>, nome: &str| -> Option<String> {
            refina
                .get(id?)?
                .iter()
                .find(|(p, _)| p == nome)
                .map(|(_, v)| v.clone())
        };
        for n in m.children().filter(Node::is_element) {
            let nome = n.tag_name().name();
            let valor = texto(n);
            let id = n.attribute("id");
            match nome {
                "title" if !valor.is_empty() => {
                    let tipo = prop(id, "title-type");
                    o.titulos.push((valor, tipo));
                }
                "creator" | "contributor" if !valor.is_empty() => {
                    let papel = atributo(n, "role")
                        .map(str::to_owned)
                        .or_else(|| prop(id, "role"))
                        .map(|p| p.to_ascii_lowercase());
                    let ordem = atributo(n, "file-as")
                        .map(str::to_owned)
                        .or_else(|| prop(id, "file-as"));
                    if nome == "creator" && papel.as_deref().is_none_or(|p| p == "aut") {
                        o.autores.push((valor, ordem));
                    } else if nome == "creator" {
                        o.outros_papeis.push((valor, papel.unwrap_or_default()));
                    }
                }
                "publisher" if o.editora.is_none() && !valor.is_empty() => o.editora = Some(valor),
                "date" if !valor.is_empty() => {
                    let evento = atributo(n, "event").unwrap_or("").to_ascii_lowercase();
                    if evento != "modification" && (o.data.is_none() || evento == "publication") {
                        o.data = Some(valor);
                    }
                }
                "language" if !valor.is_empty() => o.idiomas.push(valor),
                "identifier" if !valor.is_empty() => {
                    let esq = atributo(n, "scheme")
                        .map(str::to_owned)
                        .or_else(|| prop(id, "identifier-type"));
                    o.identificadores.push((valor, esq));
                }
                "subject" if !valor.is_empty() => o.assuntos.push(valor),
                "description" if o.descricao.is_none() && !valor.is_empty() => {
                    // Calibre grava HTML escapado: só o texto interessa.
                    let t = n.text().unwrap_or("").to_owned();
                    let limpo = util::texto_de_html(&t, 100_000);
                    o.descricao = Some(if limpo.is_empty() { valor } else { limpo });
                }
                "meta" => {
                    if let Some(p) = n.attribute("property") {
                        if p == "belongs-to-collection" && o.serie.is_none() && !valor.is_empty() {
                            let tipo = prop(id, "collection-type");
                            if tipo.as_deref().is_none_or(|t| t == "series") {
                                o.serie = Some(valor);
                                o.indice_serie = prop(id, "group-position");
                            }
                        }
                    }
                    let nm = n.attribute("name").unwrap_or("");
                    let conteudo = n.attribute("content").unwrap_or("").trim();
                    match nm {
                        "cover" if !conteudo.is_empty() => o.capa_id = Some(conteudo.to_owned()),
                        "calibre:series" if !conteudo.is_empty() => {
                            o.serie = Some(conteudo.to_owned());
                        }
                        "calibre:series_index" if !conteudo.is_empty() => {
                            o.indice_serie = Some(conteudo.to_owned());
                        }
                        "calibre:title_sort" if !conteudo.is_empty() => {
                            o.titulo_ordenacao = Some(conteudo.to_owned());
                        }
                        "calibre:user_metadata" | "calibre:user_metadata:" => {
                            o.colunas_calibre += 1;
                        }
                        _ if nm.starts_with("calibre:user_metadata:") => o.colunas_calibre += 1,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(mf) = filho(raiz, "manifest") {
        for i in filhos(mf, "item") {
            o.manifesto.push(Item {
                id: i.attribute("id").unwrap_or("").to_owned(),
                href: i.attribute("href").unwrap_or("").to_owned(),
                tipo: i.attribute("media-type").unwrap_or("").to_owned(),
                props: i.attribute("properties").unwrap_or("").to_owned(),
            });
        }
    }
    if let Some(sp) = filho(raiz, "spine") {
        o.spine_toc = sp.attribute("toc").map(str::to_owned);
        for i in filhos(sp, "itemref") {
            if let Some(id) = i.attribute("idref") {
                o.spine.push(id.to_owned());
            }
        }
    }
    if let Some(g) = filho(raiz, "guide") {
        for rf in filhos(g, "reference") {
            if let (Some(t), Some(h)) = (rf.attribute("type"), rf.attribute("href")) {
                o.guia.push((t.to_ascii_lowercase(), h.to_owned()));
            }
        }
    }
    o
}

fn montar_metadados(o: &Opf, r: &mut ResultadoExtracao) {
    // Título principal: title-type=main, senão o primeiro sem tipo, senão o primeiro.
    let principal = o
        .titulos
        .iter()
        .find(|(_, t)| t.as_deref() == Some("main"))
        .or_else(|| o.titulos.iter().find(|(_, t)| t.is_none()))
        .or_else(|| o.titulos.first());
    if let Some((t, _)) = principal {
        let mut v = util::valor(Campo::Titulo, t, 0.85, "opf:dc:title");
        if let Some(ord) = &o.titulo_ordenacao {
            if let Some(e) = v.evidencias.first_mut() {
                e.detalhe = Some(format!("opf:dc:title; calibre:title_sort={ord}"));
            }
        }
        r.metadados.push(v);
    }
    if let Some((s, _)) = o
        .titulos
        .iter()
        .find(|(_, t)| t.as_deref() == Some("subtitle"))
    {
        adicionar(r, Campo::Subtitulo, Some(s), 0.8, "opf:dc:title (subtitle)");
    }
    let autores: Vec<String> = o.autores.iter().map(|(n, _)| n.clone()).collect();
    if !autores.is_empty() {
        let mut v = util::lista(Campo::Autores, autores, 0.85, "opf:dc:creator");
        let ordens: Vec<String> = o.autores.iter().filter_map(|(_, s)| s.clone()).collect();
        if !ordens.is_empty() {
            if let Some(e) = v.evidencias.first_mut() {
                e.detalhe = Some(format!("opf:dc:creator; file-as={}", ordens.join(" & ")));
            }
        }
        r.metadados.push(v);
    }
    if !o.outros_papeis.is_empty() {
        let l: Vec<String> = o
            .outros_papeis
            .iter()
            .map(|(n, p)| format!("{n} ({p})"))
            .collect();
        r.avisos.push(format!(
            "criadores com outro papel (fora de Autores): {}",
            l.join("; ")
        ));
    }
    adicionar(
        r,
        Campo::Editora,
        o.editora.as_deref(),
        0.8,
        "opf:dc:publisher",
    );
    adicionar(r, Campo::Data, o.data.as_deref(), 0.8, "opf:dc:date");
    adicionar(
        r,
        Campo::Idioma,
        o.idiomas.first().map(String::as_str),
        0.9,
        "opf:dc:language",
    );
    adicionar(
        r,
        Campo::Resumo,
        o.descricao.as_deref(),
        0.7,
        "opf:dc:description",
    );
    adicionar_lista(r, Campo::Tags, o.assuntos.clone(), 0.6, "opf:dc:subject");
    adicionar(
        r,
        Campo::Serie,
        o.serie.as_deref(),
        0.8,
        "opf:calibre:series/belongs-to-collection",
    );
    adicionar(
        r,
        Campo::IndiceSerie,
        o.indice_serie.as_deref(),
        0.8,
        "opf:calibre:series_index/group-position",
    );
    for (id, esq) in &o.identificadores {
        let detalhe = format!(
            "opf:dc:identifier{}",
            esq.as_deref().map_or(String::new(), |e| format!(" ({e})"))
        );
        let bruto = id.trim();
        let baixo = bruto.to_ascii_lowercase();
        if baixo.starts_with("urn:uuid:") || baixo.starts_with("uuid:") {
            continue;
        }
        if finalizar::isbn_de_identificador(r, bruto, &detalhe) {
            continue;
        }
        let doi = baixo
            .strip_prefix("doi:")
            .or_else(|| baixo.strip_prefix("https://doi.org/"))
            .or_else(|| baixo.strip_prefix("http://dx.doi.org/"))
            .or_else(|| baixo.strip_prefix("urn:doi:"))
            .map(str::to_owned)
            .or_else(|| (baixo.starts_with("10.") && baixo.contains('/')).then(|| baixo.clone()));
        if let Some(d) = doi {
            adicionar(r, Campo::Doi, Some(d.trim()), 0.85, &detalhe);
        }
    }
    if o.colunas_calibre > 0 {
        r.avisos.push(format!(
            "calibre:user_metadata presente ({} entrada(s)); não interpretado",
            o.colunas_calibre
        ));
    }
}

// ---------------------------------------------------------------------------------------------
// Capa
// ---------------------------------------------------------------------------------------------

enum Capa {
    /// href e MIME declarado.
    Imagem(String, String),
    /// Página XHTML de capa (a imagem está dentro).
    Pagina(String),
}

fn localizar_capa(o: &Opf, por_id: &HashMap<&str, &Item>) -> Option<Capa> {
    let de_item = |i: &Item| -> Capa {
        if i.tipo.starts_with("image/") || util::e_imagem(&i.href) {
            Capa::Imagem(i.href.clone(), i.tipo.clone())
        } else {
            Capa::Pagina(i.href.clone())
        }
    };
    if let Some(i) = o
        .manifesto
        .iter()
        .find(|i| i.props.split_whitespace().any(|p| p == "cover-image"))
    {
        return Some(de_item(i));
    }
    if let Some(i) = o.capa_id.as_deref().and_then(|id| por_id.get(id)) {
        return Some(de_item(i));
    }
    if let Some((_, h)) = o.guia.iter().find(|(t, _)| t == "cover") {
        return Some(if util::e_imagem(h) {
            Capa::Imagem(h.clone(), String::new())
        } else {
            Capa::Pagina(h.clone())
        });
    }
    o.manifesto
        .iter()
        .find(|i| {
            (i.tipo.starts_with("image/") || util::e_imagem(&i.href))
                && (i.id.to_ascii_lowercase().contains("cover")
                    || util::nome_base(&i.href)
                        .to_ascii_lowercase()
                        .starts_with("cover"))
        })
        .map(de_item)
}

// ---------------------------------------------------------------------------------------------
// Sumário
// ---------------------------------------------------------------------------------------------

fn sumario_nav(doc: &roxmltree::Document<'_>) -> Vec<ItemSumario> {
    let navs: Vec<Node> = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "nav")
        .collect();
    let toc = navs
        .iter()
        .find(|n| atributo(**n, "type").is_some_and(|t| t.split_whitespace().any(|x| x == "toc")))
        .or_else(|| navs.first());
    let mut itens = Vec::new();
    if let Some(n) = toc {
        if let Some(ol) = filho(*n, "ol") {
            percorrer_ol(ol, 0, &mut itens);
        }
    }
    itens
}

fn percorrer_ol(ol: Node<'_, '_>, nivel: u8, itens: &mut Vec<ItemSumario>) {
    if nivel > 24 {
        return;
    }
    for li in filhos(ol, "li") {
        if itens.len() >= ITENS_SUMARIO_MAX {
            return;
        }
        let rotulo = li
            .children()
            .filter(|c| c.is_element() && matches!(c.tag_name().name(), "a" | "span"))
            .map(texto)
            .find(|t| !t.is_empty());
        if let Some(t) = rotulo {
            itens.push(ItemSumario {
                titulo: t,
                nivel,
                pagina: None,
            });
        }
        if let Some(sub) = filho(li, "ol") {
            percorrer_ol(sub, nivel + 1, itens);
        }
    }
}

fn sumario_ncx(doc: &roxmltree::Document<'_>) -> Vec<ItemSumario> {
    let mut itens = Vec::new();
    if let Some(mapa) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "navMap")
    {
        percorrer_pontos(mapa, 0, &mut itens);
    }
    itens
}

fn percorrer_pontos(pai: Node<'_, '_>, nivel: u8, itens: &mut Vec<ItemSumario>) {
    if nivel > 24 {
        return;
    }
    for p in filhos(pai, "navPoint") {
        if itens.len() >= ITENS_SUMARIO_MAX {
            return;
        }
        let t = filho(p, "navLabel").map(texto).filter(|t| !t.is_empty());
        if let Some(t) = t {
            itens.push(ItemSumario {
                titulo: t,
                nivel,
                pagina: None,
            });
        }
        percorrer_pontos(p, nivel + 1, itens);
    }
}
