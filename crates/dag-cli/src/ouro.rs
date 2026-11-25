//! Conjunto ouro do piloto (M5): `dag biblioteca ouro exportar|avaliar`.
//!
//! `exportar` monta uma amostra estratificada dos itens em revisão e grava UM JSON NOVO por item
//! (nunca sobrescreve; ver `escrita.rs`) no formato de `dag_biblio_ia::benchmark::carregar_casos`
//! (`id`, `entrada`, `gabarito`), com o gabarito vazio para o Chico preencher, mais campos extras
//! que o benchmark ignora: a sugestão gravada do DAG (`sugestao_dag`), os vínculos
//! (`vinculo_dag`), a detecção de duplicata exata e o gabarito de vínculos (`gabarito_extra`).
//! `avaliar` compara o gabarito preenchido com a sugestão gravada e confere as metas da spec.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use dag_biblio::{Campo, EstadoLeitura, FormatoLivro, ResultadoExtracao, ValorSugerido};
use dag_biblio_ia::benchmark::{Avaliacao, CasoBenchmark, GabaritoValor};
use dag_biblio_ia::validador::chave_comparacao as chave_comparacao_de;
use dag_core::ItemRevisaoInfo;
use serde::Serialize;
use serde_json::{Value, json};

/// Estratos aceitos em `--estratos`.
pub const ESTRATOS: &[&str] = &[
    "matematica",
    "xadrez",
    "ficcao",
    "quadrinhos",
    "pdf_textual",
    "pdf_imagem",
    "epub",
    "outros_formatos",
    "duplicata_exata",
    "titulo_coincidente",
];

/// Limite de itens lidos da revisão para formar a amostra.
pub const MAX_POOL: usize = 5_000;

const INSTRUCOES: &str = "Preencha `gabarito` com os campos que voce conferiu: \"titulo\": \"...\", \"autores\": [\"A\", \"B\"], \"tags\": [...], \"estante\": \"...\", \"isbn\": \"...\". `null` = o campo deve ficar DESCONHECIDO (a IA nao pode inventar); campo ausente = nao conferido. Em `gabarito_extra`: `duplicata_exata` (true/false) e, com ids de arquivo de outros casos, `mesma_edicao_que`, `mesma_obra_que`, `obra_diferente_de`. Casos com `gabarito` vazio sao ignorados na avaliacao.";

/// Valida `--estratos` (vazio = todos).
///
/// # Errors
/// Nome de estrato desconhecido.
pub fn validar_estratos(pedidos: &[String]) -> Result<Vec<&'static str>, String> {
    if pedidos.is_empty() {
        return Ok(ESTRATOS.to_vec());
    }
    let mut v = Vec::new();
    for p in pedidos {
        let nome = p.trim().to_lowercase().replace('-', "_");
        match ESTRATOS.iter().find(|e| **e == nome) {
            Some(e) => {
                if !v.contains(e) {
                    v.push(*e);
                }
            }
            None => {
                return Err(format!(
                    "estrato desconhecido `{p}`; use: {}",
                    ESTRATOS.join(", ")
                ));
            }
        }
    }
    Ok(v)
}

fn sem_acentos(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            o => o,
        })
        .collect()
}

fn formato_do_item(i: &ItemRevisaoInfo) -> String {
    i.formato
        .clone()
        .or_else(|| {
            i.caminho
                .as_deref()
                .and_then(|c| c.rsplit('.').next().map(str::to_string))
        })
        .unwrap_or_default()
        .to_lowercase()
}

fn eh_exata(rel: Option<&str>) -> bool {
    rel.is_some_and(|r| r.eq_ignore_ascii_case("exata"))
}

/// Estratos a que o item pertence (heurísticas sobre o que o DAG já sabe do arquivo).
#[must_use]
pub fn estratos_do_item(i: &ItemRevisaoInfo) -> BTreeSet<&'static str> {
    let mut texto = i.caminho.clone().unwrap_or_default();
    for c in &i.campos {
        if matches!(c.campo.as_str(), "titulo" | "tags" | "estante" | "serie") {
            for v in [&c.sugerido, &c.extraido].into_iter().flatten() {
                texto.push(' ');
                texto.push_str(v);
            }
            for v in &c.sugerido_lista {
                texto.push(' ');
                texto.push_str(v);
            }
        }
    }
    let t = sem_acentos(&texto.to_lowercase());
    let tem = |ps: &[&str]| ps.iter().any(|p| t.contains(p));
    let fmt = formato_do_item(i);
    let mut s = BTreeSet::new();
    if tem(&[
        "matematic",
        "mathemat",
        "algebra",
        "calculo",
        "calculus",
        "geometr",
        "topolog",
        "teoria dos numeros",
        "number theory",
        "analise real",
        "real analysis",
        "equacoes diferenciais",
        "differential equations",
    ]) {
        s.insert("matematica");
    }
    if tem(&["xadrez", "chess", "schach", "echecs", "ajedrez"]) {
        s.insert("xadrez");
    }
    if tem(&[
        "ficcao", "fiction", "romance", "novel", "fantasia", "fantasy", "contos",
    ]) {
        s.insert("ficcao");
    }
    if tem(&["quadrinho", "comic", "manga", "graphic novel"])
        || matches!(fmt.as_str(), "cbz" | "cbr" | "cb7")
    {
        s.insert("quadrinhos");
    }
    match fmt.as_str() {
        "pdf" => {
            let ocr = i.avisos.iter().any(|a| a.to_lowercase().contains("ocr"))
                || i.campos.iter().any(|c| {
                    c.origem
                        .as_deref()
                        .is_some_and(|o| o.eq_ignore_ascii_case("ocr"))
                });
            s.insert(if ocr { "pdf_imagem" } else { "pdf_textual" });
        }
        "epub" => {
            s.insert("epub");
        }
        _ => {
            s.insert("outros_formatos");
        }
    }
    for p in &i.homologados_parecidos {
        if eh_exata(p.relacao.as_deref()) {
            s.insert("duplicata_exata");
        } else if p.motivo.to_lowercase().contains("titulo")
            || p.relacao
                .as_deref()
                .is_some_and(|r| matches!(r, "mesmaobra" | "mesmaedicao" | "incerta"))
        {
            s.insert("titulo_coincidente");
        }
    }
    s
}

/// Amostra estratificada determinística: reveza entre os estratos pedidos, pegando em cada rodada
/// o primeiro item ainda não escolhido do estrato, até `n` ou esgotar.
#[must_use]
pub fn amostrar<'a>(
    itens: &'a [ItemRevisaoInfo],
    estratos: &[&'static str],
    n: usize,
) -> Vec<(&'a ItemRevisaoInfo, Vec<&'static str>)> {
    let classes: Vec<BTreeSet<&'static str>> = itens.iter().map(estratos_do_item).collect();
    let filas: Vec<Vec<usize>> = estratos
        .iter()
        .map(|e| {
            classes
                .iter()
                .enumerate()
                .filter(|(_, c)| c.contains(e))
                .map(|(i, _)| i)
                .collect()
        })
        .collect();
    let mut pos = vec![0_usize; estratos.len()];
    let mut escolhidos: Vec<usize> = Vec::new();
    let mut vistos = BTreeSet::new();
    'fora: loop {
        let mut avancou = false;
        for (k, fila) in filas.iter().enumerate() {
            while pos[k] < fila.len() && vistos.contains(&fila[pos[k]]) {
                pos[k] += 1;
            }
            if pos[k] < fila.len() {
                let idx = fila[pos[k]];
                pos[k] += 1;
                vistos.insert(idx);
                escolhidos.push(idx);
                avancou = true;
                if escolhidos.len() >= n {
                    break 'fora;
                }
            }
        }
        if !avancou {
            break;
        }
    }
    escolhidos
        .into_iter()
        .map(|i| {
            let est = estratos
                .iter()
                .filter(|e| classes[i].contains(*e))
                .copied()
                .collect();
            (&itens[i], est)
        })
        .collect()
}

fn campo_de(nome: &str) -> Option<Campo> {
    serde_json::from_value(Value::String(nome.to_string())).ok()
}

fn extracao_minima(i: &ItemRevisaoInfo) -> ResultadoExtracao {
    let mut metadados = Vec::new();
    for c in &i.campos {
        let (Some(campo), Some(ext)) = (campo_de(&c.campo), c.extraido.as_deref()) else {
            continue;
        };
        let lista: Vec<String> = if matches!(campo, Campo::Autores | Campo::Tags) {
            ext.split(['&', '|'])
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        } else {
            Vec::new()
        };
        metadados.push(ValorSugerido {
            campo,
            valor: lista.is_empty().then(|| ext.to_string()),
            lista,
            confianca: 1.0,
            evidencias: Vec::new(),
        });
    }
    ResultadoExtracao {
        formato: FormatoLivro::da_extensao(&formato_do_item(i)).unwrap_or(FormatoLivro::Outro),
        estado: EstadoLeitura::Lido,
        metadados,
        isbns: Vec::new(),
        sumario: Vec::new(),
        paginas: Vec::new(),
        total_paginas: None,
        ficha_catalografica: None,
        capa: None,
        capa_mime: None,
        paginas_ocr: Vec::new(),
        avisos: vec![
            "caso montado só com os campos extraídos; use --com-texto para incluir as páginas"
                .into(),
        ],
    }
}

fn nome_arquivo(i: &ItemRevisaoInfo) -> String {
    i.caminho
        .as_deref()
        .and_then(|c| c.rsplit('/').next())
        .map_or_else(|| format!("arquivo-{}", i.file_id), str::to_string)
}

/// O JSON de um caso (compatível com `carregar_casos`).
#[must_use]
pub fn caso_json(
    i: &ItemRevisaoInfo,
    estratos: &[&str],
    extracao: Option<ResultadoExtracao>,
) -> Value {
    let id = format!("ouro-{:06}", i.file_id);
    let mut sugestao = serde_json::Map::new();
    for c in &i.campos {
        sugestao.insert(
            c.campo.clone(),
            json!({
                "valor": c.sugerido,
                "lista": c.sugerido_lista,
                "confianca": c.confianca,
                "faixa": c.faixa,
                "estado": c.estado,
                "origem": c.origem,
                "modelo": c.modelo,
            }),
        );
    }
    let extr = extracao.unwrap_or_else(|| extracao_minima(i));
    json!({
        "id": id,
        "file_id": i.file_id,
        "estrato": estratos,
        "caminho": i.caminho,
        "instrucoes": INSTRUCOES,
        "entrada": {
            "id": id,
            "nome_arquivo": nome_arquivo(i),
            "extracao": serde_json::to_value(&extr).unwrap_or(Value::Null),
            "vocabulario": {},
        },
        "gabarito": {},
        "gabarito_extra": {
            "duplicata_exata": null,
            "mesma_edicao_que": [],
            "mesma_obra_que": [],
            "obra_diferente_de": [],
        },
        "sugestao_dag": Value::Object(sugestao),
        "vinculo_dag": {"obra_id": i.obra_id, "edicao_id": i.edicao_id},
        "duplicata_exata_dag": i.homologados_parecidos.iter().any(|p| eh_exata(p.relacao.as_deref())),
        "homologados_parecidos": i.homologados_parecidos.iter().map(|p| json!({
            "titulo": p.livro.titulo,
            "autores": p.livro.autores,
            "relacao": p.relacao,
            "motivo": p.motivo,
        })).collect::<Vec<_>>(),
    })
}

/// O que `exportar` fez.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct RelatorioExportacao {
    pub pool: usize,
    pub pedidos: usize,
    pub gravados: Vec<String>,
    /// Arquivos que já existiam e foram deixados como estavam.
    pub ja_existiam: Vec<String>,
    pub por_estrato: BTreeMap<String, usize>,
    pub estratos_sem_itens: Vec<String>,
    pub avisos: Vec<String>,
}

/// Grava os casos em `saida` (que já deve existir). `extrair` devolve a extração completa (com
/// páginas) de um item, ou `None` para usar só os campos extraídos.
///
/// # Errors
/// A pasta não existe ou falha de escrita (um arquivo que já existe só vira aviso).
pub fn exportar(
    itens: &[ItemRevisaoInfo],
    estratos: &[&'static str],
    n: usize,
    saida: &Path,
    extrair: &mut dyn FnMut(&ItemRevisaoInfo) -> Result<Option<ResultadoExtracao>, String>,
) -> Result<RelatorioExportacao, String> {
    if !saida.is_dir() {
        return Err(format!(
            "{} não é uma pasta existente; crie-a antes (mkdir) e rode de novo",
            saida.display()
        ));
    }
    let mut rel = RelatorioExportacao {
        pool: itens.len(),
        pedidos: n,
        ..Default::default()
    };
    let escolhidos = amostrar(itens, estratos, n);
    for e in estratos {
        let n_e = escolhidos.iter().filter(|(_, es)| es.contains(e)).count();
        rel.por_estrato.insert((*e).to_string(), n_e);
        if n_e == 0 {
            rel.estratos_sem_itens.push((*e).to_string());
        }
    }
    if escolhidos.len() < n {
        rel.avisos.push(format!(
            "só {} item(ns) elegíveis entre os {} em revisão (pedidos: {n})",
            escolhidos.len(),
            itens.len()
        ));
    }
    for (item, est) in escolhidos {
        let extracao = match extrair(item) {
            Ok(x) => x,
            Err(e) => {
                rel.avisos.push(format!(
                    "arquivo {}: extração falhou ({e}); caso só com os campos extraídos",
                    item.file_id
                ));
                None
            }
        };
        let caso = caso_json(item, &est, extracao);
        let texto = serde_json::to_string_pretty(&caso).map_err(|e| e.to_string())?;
        let destino = saida.join(format!("ouro-{:06}.json", item.file_id));
        match crate::escrita::criar_novo(&destino, texto.as_bytes()) {
            Ok(()) => rel.gravados.push(destino.display().to_string()),
            Err(e) if e.contains("já existe") => {
                rel.ja_existiam.push(destino.display().to_string())
            }
            Err(e) => return Err(e),
        }
    }
    Ok(rel)
}

/// Texto do relatório da exportação.
#[must_use]
pub fn texto_exportacao(r: &RelatorioExportacao, saida: &Path) -> String {
    let mut s = format!(
        "Conjunto ouro: {} caso(s) novo(s) gravados em {} (de {} itens em revisão; pedidos: {}).\n",
        r.gravados.len(),
        saida.display(),
        r.pool,
        r.pedidos
    );
    if !r.ja_existiam.is_empty() {
        s.push_str(&format!(
            "{} arquivo(s) já existiam e foram preservados (nada foi sobrescrito).\n",
            r.ja_existiam.len()
        ));
    }
    s.push_str("Por estrato (um item pode estar em vários):\n");
    for (e, n) in &r.por_estrato {
        s.push_str(&format!("  {e:<20} {n}\n"));
    }
    if !r.estratos_sem_itens.is_empty() {
        s.push_str(&format!(
            "Sem itens para: {} (processe e enfileire mais livros desses tipos).\n",
            r.estratos_sem_itens.join(", ")
        ));
    }
    for a in &r.avisos {
        s.push_str(&format!("aviso: {a}\n"));
    }
    s.push_str(
        "Próximo passo: abra cada JSON, preencha `gabarito` (veja `instrucoes` dentro dele) e rode\n  dag biblioteca ouro avaliar --casos <pasta>\nO mesmo diretório serve ao benchmark: dag modelos benchmark --casos <pasta> --modelo NOME\n",
    );
    s
}

// ---------------------------------------------------------------------------------------------
// Avaliação
// ---------------------------------------------------------------------------------------------

/// Estatística de um campo.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct LinhaCampo {
    pub campo: String,
    /// Casos em que o campo foi conferido.
    pub n: u32,
    pub corretos: u32,
    pub errados: u32,
    pub inventados: u32,
    pub omitidos: u32,
    /// O DAG deixou desconhecido (omitido ou desconhecido corretamente).
    pub abstencoes: u32,
    pub precisao: Option<f64>,
    pub cobertura: Option<f64>,
    /// Afirmações do DAG em alta confiança (>= 0,90) e a precisão delas.
    pub alta_afirmados: u32,
    #[serde(skip)]
    alta_certos: u32,
    pub alta_precisao: Option<f64>,
}

/// Tags: precisão e revocação micro (somando todos os casos).
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct MicroTags {
    pub casos: u32,
    pub acertos: u32,
    pub sobras: u32,
    pub faltas: u32,
    pub precisao: Option<f64>,
    pub revocacao: Option<f64>,
}

/// Uma meta da especificação.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Meta {
    pub nome: String,
    pub meta: f64,
    pub obtido: Option<f64>,
    pub amostra: u32,
    /// `None` = sem dados para julgar.
    pub atingida: Option<bool>,
}

/// Resultado da avaliação do conjunto ouro.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct RelatorioOuro {
    pub casos: usize,
    pub com_gabarito: usize,
    pub sem_gabarito: usize,
    pub campos: Vec<LinhaCampo>,
    pub tags: MicroTags,
    pub vinculos_testados: u32,
    pub vinculos_corretos: u32,
    pub duplicatas_testadas: u32,
    pub duplicatas_corretas: u32,
    pub duplicatas_falsas_positivas: u32,
    pub metas: Vec<Meta>,
}

fn razao(a: u32, b: u32) -> Option<f64> {
    (b > 0).then(|| f64::from(a) / f64::from(b))
}

fn valor_do_gabarito(campo: Campo, g: &GabaritoValor) -> ValorSugerido {
    match g {
        GabaritoValor::Texto(t) if !dag_biblio_ia::esquema::e_lista(campo) => ValorSugerido {
            campo,
            valor: Some(t.clone()),
            lista: vec![],
            confianca: 1.0,
            evidencias: vec![],
        },
        GabaritoValor::Texto(t) => ValorSugerido {
            campo,
            valor: None,
            lista: vec![t.clone()],
            confianca: 1.0,
            evidencias: vec![],
        },
        GabaritoValor::Lista(l) => ValorSugerido {
            campo,
            valor: None,
            lista: l.clone(),
            confianca: 1.0,
            evidencias: vec![],
        },
    }
}

fn conhecido(v: Option<&ValorSugerido>) -> bool {
    v.is_some_and(|v| {
        v.valor.as_deref().is_some_and(|t| !t.trim().is_empty()) || !v.lista.is_empty()
    })
}

/// Mesma regra do benchmark de modelos (`dag_biblio_ia::benchmark`), aplicada à sugestão gravada.
fn avaliar_campo(
    campo: Campo,
    esperado: Option<&GabaritoValor>,
    obtido: Option<&ValorSugerido>,
) -> Avaliacao {
    let sabe = conhecido(obtido);
    let Some(esp) = esperado else {
        return if sabe {
            Avaliacao::Inventado
        } else {
            Avaliacao::DesconhecidoCorreto
        };
    };
    if !sabe {
        return Avaliacao::Omitido;
    }
    match (
        chave_comparacao_de(&valor_do_gabarito(campo, esp)),
        obtido.and_then(chave_comparacao_de),
    ) {
        (Some(a), Some(b)) if a == b => Avaliacao::Correto,
        _ => Avaliacao::Errado,
    }
}

fn sugestao_gravada(extras: &Value, campo: Campo) -> Option<ValorSugerido> {
    let nome = dag_biblio_ia::esquema::nome_campo(campo);
    let o = extras.get("sugestao_dag")?.get(&nome)?;
    let lista: Vec<String> = o
        .get("lista")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut valor = o.get("valor").and_then(Value::as_str).map(str::to_string);
    let mut lista = lista;
    // Campo de lista gravado como texto único (`a & b`): quebra para comparar como lista.
    if lista.is_empty()
        && dag_biblio_ia::esquema::e_lista(campo)
        && let Some(t) = valor.take()
    {
        lista = t
            .split(['&', '|'])
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
    }
    #[allow(clippy::cast_possible_truncation)]
    let confianca = o.get("confianca").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    Some(ValorSugerido {
        campo,
        valor,
        lista,
        confianca,
        evidencias: vec![],
    })
}

fn chaves_de_tags(
    v: Option<&ValorSugerido>,
    gab: Option<&GabaritoValor>,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let chave = |t: &str| {
        chave_comparacao_de(&ValorSugerido {
            campo: Campo::Tags,
            valor: None,
            lista: vec![t.to_string()],
            confianca: 1.0,
            evidencias: vec![],
        })
    };
    let dag = v
        .map(|v| v.lista.iter().filter_map(|t| chave(t)).collect())
        .unwrap_or_default();
    let ouro = match gab {
        Some(GabaritoValor::Lista(l)) => l.iter().filter_map(|t| chave(t)).collect(),
        Some(GabaritoValor::Texto(t)) => chave(t).into_iter().collect(),
        None => BTreeSet::new(),
    };
    (dag, ouro)
}

fn meta(nome: &str, meta: f64, obtido: Option<f64>, amostra: u32) -> Meta {
    Meta {
        nome: nome.to_string(),
        meta,
        obtido,
        amostra,
        atingida: obtido.map(|o| o + 1e-9 >= meta),
    }
}

/// Avalia casos já carregados (cada um com o JSON bruto, onde estão os campos extras).
#[must_use]
pub fn avaliar_casos(casos: &[(CasoBenchmark, Value)]) -> RelatorioOuro {
    let mut r = RelatorioOuro {
        casos: casos.len(),
        ..Default::default()
    };
    let mut por_campo: BTreeMap<Campo, LinhaCampo> = BTreeMap::new();
    // file_id -> (obra, edicao) de TODOS os casos exportados (preenchidos ou não).
    let mut vinculos: BTreeMap<i64, (Option<i64>, Option<i64>)> = BTreeMap::new();
    for (_, ex) in casos {
        if let (Some(f), Some(v)) = (
            ex.get("file_id").and_then(Value::as_i64),
            ex.get("vinculo_dag"),
        ) {
            vinculos.insert(
                f,
                (
                    v.get("obra_id").and_then(Value::as_i64),
                    v.get("edicao_id").and_then(Value::as_i64),
                ),
            );
        }
    }
    let mut pares: BTreeSet<(i64, i64, &'static str)> = BTreeSet::new();
    for (caso, extras) in casos {
        if caso.gabarito.is_empty() {
            r.sem_gabarito += 1;
            continue;
        }
        r.com_gabarito += 1;
        for (campo, esperado) in &caso.gabarito {
            let obtido = sugestao_gravada(extras, *campo);
            let aval = avaliar_campo(*campo, esperado.as_ref(), obtido.as_ref());
            let l = por_campo.entry(*campo).or_insert_with(|| LinhaCampo {
                campo: dag_biblio_ia::esquema::nome_campo(*campo),
                ..Default::default()
            });
            l.n += 1;
            let alta = obtido.as_ref().is_some_and(|v| v.confianca >= 0.90);
            match aval {
                Avaliacao::Correto => l.corretos += 1,
                Avaliacao::Errado => l.errados += 1,
                Avaliacao::Inventado => l.inventados += 1,
                Avaliacao::Omitido => l.omitidos += 1,
                Avaliacao::DesconhecidoCorreto => {}
            }
            if matches!(aval, Avaliacao::Omitido | Avaliacao::DesconhecidoCorreto) {
                l.abstencoes += 1;
            }
            if alta
                && matches!(
                    aval,
                    Avaliacao::Correto | Avaliacao::Errado | Avaliacao::Inventado
                )
            {
                l.alta_afirmados += 1;
                if aval == Avaliacao::Correto {
                    l.alta_certos += 1;
                }
            }
            if *campo == Campo::Tags {
                let (dag, ouro) = chaves_de_tags(obtido.as_ref(), esperado.as_ref());
                r.tags.casos += 1;
                r.tags.acertos +=
                    u32::try_from(dag.intersection(&ouro).count()).unwrap_or(u32::MAX);
                r.tags.sobras += u32::try_from(dag.difference(&ouro).count()).unwrap_or(u32::MAX);
                r.tags.faltas += u32::try_from(ouro.difference(&dag).count()).unwrap_or(u32::MAX);
            }
        }
        let fid = extras.get("file_id").and_then(Value::as_i64);
        // Duplicata exata.
        if let (Some(esp), Some(dag)) = (
            extras
                .pointer("/gabarito_extra/duplicata_exata")
                .and_then(Value::as_bool),
            extras.get("duplicata_exata_dag").and_then(Value::as_bool),
        ) {
            r.duplicatas_testadas += 1;
            if esp == dag {
                r.duplicatas_corretas += 1;
            } else if dag {
                r.duplicatas_falsas_positivas += 1;
            }
        }
        // Vínculos declarados no gabarito.
        if let Some(a) = fid {
            for (chave, tipo) in [
                ("mesma_edicao_que", "edicao"),
                ("mesma_obra_que", "obra"),
                ("obra_diferente_de", "diferente"),
            ] {
                if let Some(lista) = extras
                    .pointer(&format!("/gabarito_extra/{chave}"))
                    .and_then(Value::as_array)
                {
                    for b in lista.iter().filter_map(Value::as_i64) {
                        pares.insert((a.min(b), a.max(b), tipo));
                    }
                }
            }
        }
    }
    for (a, b, tipo) in pares {
        let (Some(va), Some(vb)) = (vinculos.get(&a), vinculos.get(&b)) else {
            continue;
        };
        r.vinculos_testados += 1;
        let igual = |x: Option<i64>, y: Option<i64>| x.is_some() && x == y;
        let ok = match tipo {
            "edicao" => igual(va.1, vb.1),
            "obra" => igual(va.0, vb.0),
            _ => !igual(va.0, vb.0),
        };
        if ok {
            r.vinculos_corretos += 1;
        }
    }
    for l in por_campo.values_mut() {
        let afirmados = l.corretos + l.errados + l.inventados;
        let esperados = l.corretos + l.errados + l.omitidos;
        l.precisao = razao(l.corretos, afirmados);
        l.cobertura = razao(l.corretos + l.errados, esperados);
        l.alta_precisao = razao(l.alta_certos, l.alta_afirmados);
    }
    r.tags.precisao = razao(r.tags.acertos, r.tags.acertos + r.tags.sobras);
    r.tags.revocacao = razao(r.tags.acertos, r.tags.acertos + r.tags.faltas);
    r.campos = por_campo.into_values().collect();
    let alta = |nome: &str| {
        r.campos
            .iter()
            .find(|l| l.campo == nome)
            .map_or((None, 0), |l| (l.alta_precisao, l.alta_afirmados))
    };
    let (t, tn) = alta("titulo");
    let (a, an) = alta("autores");
    r.metas = vec![
        meta("título, em alta confiança", 0.95, t, tn),
        meta("autor, em alta confiança", 0.95, a, an),
        meta(
            "tags, precisão micro",
            0.90,
            r.tags.precisao,
            r.tags.acertos + r.tags.sobras,
        ),
        meta(
            "vínculo obra/edição",
            0.98,
            razao(r.vinculos_corretos, r.vinculos_testados),
            r.vinculos_testados,
        ),
        meta(
            "duplicata exata",
            1.0,
            razao(r.duplicatas_corretas, r.duplicatas_testadas),
            r.duplicatas_testadas,
        ),
    ];
    r
}

/// Lê `*.json` da pasta (ordem alfabética) como casos do benchmark, guardando o JSON bruto.
///
/// # Errors
/// Pasta ilegível ou JSON que não é um caso.
pub fn carregar(dir: &Path) -> Result<Vec<(CasoBenchmark, Value)>, String> {
    let mut caminhos: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("não consegui ler a pasta {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    caminhos.sort();
    let mut v = Vec::new();
    for p in caminhos {
        let texto = std::fs::read_to_string(&p)
            .map_err(|e| format!("não consegui ler {}: {e}", p.display()))?;
        let bruto: Value = serde_json::from_str(&texto)
            .map_err(|e| format!("{}: JSON inválido: {e}", p.display()))?;
        let mut caso: CasoBenchmark = serde_json::from_value(bruto.clone())
            .map_err(|e| format!("{}: não é um caso do conjunto ouro: {e}", p.display()))?;
        if caso.id.is_empty() {
            caso.id = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        v.push((caso, bruto));
    }
    Ok(v)
}

/// `dag biblioteca ouro avaliar`: sem agente, só lê a pasta.
pub fn comando_avaliar(dir: &Path, json: bool) -> std::process::ExitCode {
    match carregar(dir) {
        Ok(casos) => {
            let r = avaliar_casos(&casos);
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&r).unwrap_or_else(|_| "{}".into())
                );
            } else {
                print!("{}", texto_avaliacao(&r));
            }
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            if json {
                println!(
                    "{}",
                    json!({"tipo":"erro","codigo":"argumento_invalido","mensagem": e})
                );
            }
            eprintln!("erro: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

fn pct(v: Option<f64>) -> String {
    v.map_or_else(
        || "n/d".to_string(),
        |x| format!("{:.1}%", x * 100.0).replace('.', ","),
    )
}

/// Texto da avaliação.
#[must_use]
pub fn texto_avaliacao(r: &RelatorioOuro) -> String {
    let mut s = format!(
        "Conjunto ouro: {} caso(s); {} com gabarito preenchido, {} sem (ignorados).\n",
        r.casos, r.com_gabarito, r.sem_gabarito
    );
    if r.com_gabarito == 0 {
        s.push_str(
            "Nada a avaliar: preencha `gabarito` nos JSON (veja `instrucoes` dentro deles).\n",
        );
        return s;
    }
    s.push_str("\nPor campo (sugestão gravada × gabarito):\n");
    let linhas: Vec<Vec<String>> = r
        .campos
        .iter()
        .map(|l| {
            vec![
                l.campo.clone(),
                l.n.to_string(),
                l.corretos.to_string(),
                l.errados.to_string(),
                l.inventados.to_string(),
                l.omitidos.to_string(),
                l.abstencoes.to_string(),
                pct(l.precisao),
                pct(l.cobertura),
                format!("{} (n={})", pct(l.alta_precisao), l.alta_afirmados),
            ]
        })
        .collect();
    s.push_str(&crate::saida::tabela(
        &[
            "CAMPO",
            "N",
            "CERTOS",
            "ERRADOS",
            "INVENT.",
            "OMIT.",
            "ABSTENÇÕES",
            "PRECISÃO",
            "COBERTURA",
            "ALTA CONF.",
        ],
        &[false, true, true, true, true, true, true, true, true, true],
        &linhas,
    ));
    s.push_str(
        "(precisão = certos / (certos + errados + inventados); cobertura = (certos + errados) / (certos + errados + omitidos))\n",
    );
    s.push_str(&format!(
        "\nTags (micro): precisão {}, revocação {} ({} acertos, {} sobras, {} faltas)\n",
        pct(r.tags.precisao),
        pct(r.tags.revocacao),
        r.tags.acertos,
        r.tags.sobras,
        r.tags.faltas
    ));
    s.push_str(&format!(
        "Duplicatas exatas: {} de {} conferidas corretas, {} falso(s) positivo(s)\n",
        r.duplicatas_corretas, r.duplicatas_testadas, r.duplicatas_falsas_positivas
    ));
    s.push_str("\nMetas da especificação:\n");
    for m in &r.metas {
        let marca = match m.atingida {
            Some(true) => "ATINGIDA ",
            Some(false) => "NÃO ATINGIDA",
            None => "SEM DADOS ",
        };
        s.push_str(&format!(
            "  [{marca}] {}: {} (meta {}; amostra {})\n",
            m.nome,
            pct(m.obtido),
            pct(Some(m.meta)),
            m.amostra
        ));
    }
    s
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod testes {
    use super::*;
    use dag_core::{CampoLadoALado, LivroRef, ParecidoHomologado};

    fn campo(
        nome: &str,
        extraido: Option<&str>,
        sug: Option<&str>,
        lista: &[&str],
        conf: f32,
    ) -> CampoLadoALado {
        CampoLadoALado {
            campo: nome.into(),
            extraido: extraido.map(Into::into),
            sugerido: sug.map(Into::into),
            sugerido_lista: lista.iter().map(|s| (*s).to_string()).collect(),
            confianca: Some(conf),
            faixa: Some("alta".into()),
            origem: Some("Ia".into()),
            modelo: Some("gemma3:12b".into()),
            evidencias: vec![],
            precisa_revisao: false,
            motivos: vec![],
            homologado: None,
            estado: "pendente".into(),
            sugestao_id: Some(1),
        }
    }

    fn item(id: i64, caminho: &str, formato: &str, titulo: &str, tags: &[&str]) -> ItemRevisaoInfo {
        ItemRevisaoInfo {
            file_id: id,
            caminho: Some(caminho.into()),
            tamanho: Some(10),
            hash: None,
            formato: Some(formato.into()),
            pendentes: 2,
            campos: vec![
                campo("titulo", Some(titulo), Some(titulo), &[], 0.97),
                campo("autores", None, None, &["Fulano de Tal"], 0.95),
                campo("tags", None, None, tags, 0.8),
            ],
            homologados_parecidos: vec![],
            obra_id: Some(id),
            edicao_id: Some(id),
            avisos: vec![],
        }
    }

    fn parecido(rel: &str, motivo: &str) -> ParecidoHomologado {
        ParecidoHomologado {
            livro: LivroRef {
                id: 1,
                calibre_id: 1,
                titulo: "X".into(),
                autores: vec![],
                isbn13: None,
                editora: None,
                idioma: None,
                estante: None,
                tags: vec![],
                caminho: "/h".into(),
            },
            motivo: motivo.into(),
            relacao: Some(rel.into()),
        }
    }

    fn acervo() -> Vec<ItemRevisaoInfo> {
        let mut v = vec![
            item(
                1,
                "/l/Algebra Linear.pdf",
                "PDF",
                "Álgebra Linear",
                &["matemática"],
            ),
            item(2, "/l/Topologia.pdf", "PDF", "Topologia Geral", &[]),
            item(
                3,
                "/l/Finais de Xadrez.epub",
                "EPUB",
                "Finais de Xadrez",
                &["xadrez"],
            ),
            item(4, "/l/Romance X.epub", "EPUB", "Um Romance", &[]),
            item(5, "/l/Batman.cbz", "CBZ", "Batman", &[]),
            item(6, "/l/Algo.mobi", "MOBI", "Algo Qualquer", &[]),
            item(7, "/l/Dup.pdf", "PDF", "Livro Repetido", &[]),
            item(8, "/l/Parecido.pdf", "PDF", "Livro Parecido", &[]),
        ];
        v[6].homologados_parecidos = vec![parecido("exata", "{\"hash\":true}")];
        v[7].homologados_parecidos = vec![parecido("mesmaobra", "{\"titulo\":0.9}")];
        v
    }

    #[test]
    fn estratos_validos_e_apelidos() {
        assert_eq!(validar_estratos(&[]).unwrap().len(), ESTRATOS.len());
        assert_eq!(
            validar_estratos(&["Xadrez".into(), "pdf-textual".into(), "xadrez".into()]).unwrap(),
            vec!["xadrez", "pdf_textual"]
        );
        let e = validar_estratos(&["poesia".into()]).unwrap_err();
        assert!(
            e.contains("estrato desconhecido") && e.contains("matematica"),
            "{e}"
        );
    }

    #[test]
    fn classifica_dominio_formato_e_duplicidade() {
        let a = acervo();
        let c = |i: usize| estratos_do_item(&a[i]);
        assert!(c(0).contains("matematica") && c(0).contains("pdf_textual"));
        assert!(c(1).contains("matematica"));
        assert!(c(2).contains("xadrez") && c(2).contains("epub"));
        assert!(c(3).contains("ficcao"));
        assert!(c(4).contains("quadrinhos") && c(4).contains("outros_formatos"));
        assert!(c(5).contains("outros_formatos"));
        assert!(c(6).contains("duplicata_exata"));
        assert!(c(7).contains("titulo_coincidente") && !c(7).contains("duplicata_exata"));
        let mut img = item(9, "/l/Scan.pdf", "PDF", "Scan", &[]);
        img.avisos = vec!["páginas sem texto: OCR aplicado".into()];
        assert!(estratos_do_item(&img).contains("pdf_imagem"));
    }

    #[test]
    fn amostra_reveza_entre_estratos_sem_repetir() {
        let a = acervo();
        let todos = validar_estratos(&[]).unwrap();
        let s = amostrar(&a, &todos, 5);
        assert_eq!(s.len(), 5);
        let ids: BTreeSet<i64> = s.iter().map(|(i, _)| i.file_id).collect();
        assert_eq!(ids.len(), 5, "sem repetição");
        // Determinística.
        let s2 = amostrar(&a, &todos, 5);
        assert_eq!(
            s.iter().map(|(i, _)| i.file_id).collect::<Vec<_>>(),
            s2.iter().map(|(i, _)| i.file_id).collect::<Vec<_>>()
        );
        // O primeiro de cada estrato pedido entra antes do segundo de qualquer um.
        let so_xadrez = amostrar(&a, &["xadrez"], 10);
        assert_eq!(so_xadrez.len(), 1);
        assert_eq!(so_xadrez[0].0.file_id, 3);
        // n maior que o acervo devolve tudo o que casa.
        assert_eq!(amostrar(&a, &todos, 100).len(), 8);
    }

    #[test]
    fn casos_exportados_sao_lidos_pelo_benchmark_e_nunca_sobrescritos() {
        let t = tempfile::tempdir().unwrap();
        let a = acervo();
        let todos = validar_estratos(&[]).unwrap();
        let rel = exportar(&a, &todos, 4, t.path(), &mut |_| Ok(None)).unwrap();
        assert_eq!(rel.gravados.len(), 4);
        assert!(rel.ja_existiam.is_empty());
        let casos = dag_biblio_ia::benchmark::carregar_casos(t.path()).unwrap();
        assert_eq!(casos.len(), 4);
        for c in &casos {
            assert!(c.id.starts_with("ouro-") && c.gabarito.is_empty());
            assert_eq!(c.entrada.id, c.id);
            assert!(!c.entrada.nome_arquivo.is_empty());
        }
        // Os extras estão no arquivo e os campos extraídos viraram metadados.
        let (caso, bruto) = carregar(t.path()).unwrap().remove(0);
        assert!(bruto.get("sugestao_dag").is_some() && bruto.get("estrato").is_some());
        assert!(!caso.entrada.extracao.metadados.is_empty());
        // Segunda exportação na mesma pasta: nada é sobrescrito.
        let antes = std::fs::read(&rel.gravados[0]).unwrap();
        let rel2 = exportar(&a, &todos, 4, t.path(), &mut |_| Ok(None)).unwrap();
        assert!(rel2.gravados.is_empty() && rel2.ja_existiam.len() == 4);
        assert_eq!(std::fs::read(&rel.gravados[0]).unwrap(), antes);
        // Pasta inexistente: erro claro.
        let e = exportar(&a, &todos, 1, &t.path().join("nao"), &mut |_| Ok(None)).unwrap_err();
        assert!(e.contains("crie-a antes"), "{e}");
        let txt = texto_exportacao(&rel, t.path());
        assert!(
            txt.contains("4 caso(s) novo(s)") && txt.contains("ouro avaliar"),
            "{txt}"
        );
    }

    #[test]
    fn extracao_com_texto_e_falha_de_extracao_viram_aviso() {
        let t = tempfile::tempdir().unwrap();
        let a = acervo();
        let rel = exportar(&a, &["xadrez", "epub"], 2, t.path(), &mut |i| {
            if i.file_id == 3 {
                Err("pdftotext ausente".into())
            } else {
                Ok(None)
            }
        })
        .unwrap();
        assert!(rel.avisos.iter().any(|a| a.contains("pdftotext ausente")));
        assert_eq!(rel.gravados.len(), 2);
    }

    // ---- avaliação ------------------------------------------------------------------------

    fn preenchido(
        id: i64,
        gab: &Value,
        extra: &Value,
        it: &ItemRevisaoInfo,
    ) -> (CasoBenchmark, Value) {
        let mut v = caso_json(it, &["epub"], None);
        v["gabarito"] = gab.clone();
        if let Some(o) = extra.as_object() {
            for (k, x) in o {
                v["gabarito_extra"][k] = x.clone();
            }
        }
        assert_eq!(v["file_id"], id);
        (serde_json::from_value(v.clone()).unwrap(), v)
    }

    #[test]
    fn avalia_precisao_cobertura_abstencoes_e_metas() {
        let a = acervo();
        let casos = vec![
            // Título, autor e tag certos.
            preenchido(
                1,
                &json!({"titulo":"Álgebra Linear","autores":["Fulano de Tal"],"tags":["matemática"]}),
                &json!({}),
                &a[0],
            ),
            // Título errado em alta confiança; autor que deve ficar desconhecido mas o DAG preencheu.
            preenchido(
                2,
                &json!({"titulo":"Outro Título","autores":null,"tags":[]}),
                &json!({}),
                &a[1],
            ),
            // Duplicata exata: DAG detectou e o gabarito confirma; só título conferido.
            preenchido(
                7,
                &json!({"titulo":"Livro Repetido"}),
                &json!({"duplicata_exata": true}),
                &a[6],
            ),
            // Falso positivo de duplicata.
            preenchido(
                8,
                &json!({"titulo":"Livro Parecido"}),
                &json!({"duplicata_exata": false}),
                &{
                    let mut x = a[7].clone();
                    x.homologados_parecidos = vec![parecido("exata", "{}")];
                    x
                },
            ),
            // Sem gabarito: ignorado.
            (
                serde_json::from_value(caso_json(&a[2], &["epub"], None)).unwrap(),
                caso_json(&a[2], &["epub"], None),
            ),
        ];
        let r = avaliar_casos(&casos);
        assert_eq!((r.casos, r.com_gabarito, r.sem_gabarito), (5, 4, 1));
        let t = r.campos.iter().find(|l| l.campo == "titulo").unwrap();
        assert_eq!((t.n, t.corretos, t.errados), (4, 3, 1));
        assert_eq!(t.alta_afirmados, 4);
        assert_eq!(t.alta_precisao, Some(0.75));
        let au = r.campos.iter().find(|l| l.campo == "autores").unwrap();
        assert_eq!((au.corretos, au.inventados), (1, 1));
        assert_eq!(au.precisao, Some(0.5));
        // Tags: caso 1 acerta (matemática); caso 2: DAG sem tags e ouro [] (nada a contar).
        assert_eq!((r.tags.acertos, r.tags.sobras, r.tags.faltas), (1, 0, 0));
        assert_eq!(r.tags.precisao, Some(1.0));
        assert_eq!(r.tags.revocacao, Some(1.0));
        // Duplicatas: 1 certa (7) e 1 falso positivo (8).
        assert_eq!(
            (
                r.duplicatas_testadas,
                r.duplicatas_corretas,
                r.duplicatas_falsas_positivas
            ),
            (2, 1, 1)
        );
        let meta_titulo = &r.metas[0];
        assert_eq!(meta_titulo.atingida, Some(false));
        assert_eq!(r.metas[3].atingida, None, "sem vínculos declarados");
        assert_eq!(r.metas[4].atingida, Some(false));
        let txt = texto_avaliacao(&r);
        assert!(txt.contains("4 com gabarito preenchido, 1 sem"), "{txt}");
        assert!(
            txt.contains("NÃO ATINGIDA") && txt.contains("SEM DADOS"),
            "{txt}"
        );
        assert!(txt.contains("75,0%"), "{txt}");
    }

    #[test]
    fn avalia_tags_micro_e_vinculos() {
        let mut a1 = item(10, "/l/a.epub", "EPUB", "A", &["xadrez", "finais", "ruido"]);
        let mut a2 = item(11, "/l/b.pdf", "PDF", "A", &["xadrez"]);
        let a3 = item(12, "/l/c.pdf", "PDF", "C", &[]);
        // a1 e a2: mesma edição segundo o DAG (edicao 5); a3 é outra obra.
        a1.obra_id = Some(1);
        a1.edicao_id = Some(5);
        a2.obra_id = Some(1);
        a2.edicao_id = Some(5);
        let casos = vec![
            preenchido(
                10,
                &json!({"tags":["Xadrez","finais","aberturas"]}),
                &json!({"mesma_edicao_que":[11], "obra_diferente_de":[12], "mesma_obra_que":[12]}),
                &a1,
            ),
            preenchido(11, &json!({"tags":["xadrez"]}), &json!({}), &a2),
            preenchido(12, &json!({"titulo":"C"}), &json!({}), &a3),
        ];
        let r = avaliar_casos(&casos);
        // Caso 10: acertos xadrez+finais (2), sobra ruido (1), falta aberturas (1); caso 11: 1 acerto.
        assert_eq!((r.tags.acertos, r.tags.sobras, r.tags.faltas), (3, 1, 1));
        assert_eq!(r.tags.precisao, Some(0.75));
        // Vínculos: edicao(10,11) ok; diferente(10,12) ok (obras 1 e 12); obra(10,12) errada.
        assert_eq!((r.vinculos_testados, r.vinculos_corretos), (3, 2));
        assert_eq!(r.metas[3].atingida, Some(false));
        assert_eq!(r.metas[2].atingida, Some(false));
    }

    #[test]
    fn sem_gabarito_nenhum_orienta() {
        let a = acervo();
        let c = caso_json(&a[0], &["epub"], None);
        let r = avaliar_casos(&[(serde_json::from_value(c.clone()).unwrap(), c)]);
        assert_eq!(r.com_gabarito, 0);
        let txt = texto_avaliacao(&r);
        assert!(txt.contains("Nada a avaliar"), "{txt}");
    }

    #[test]
    fn carregar_recusa_json_ruim() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a.json"), "{").unwrap();
        assert!(carregar(t.path()).unwrap_err().contains("JSON inválido"));
        assert!(
            carregar(&t.path().join("x"))
                .unwrap_err()
                .contains("não consegui ler")
        );
    }
}
