//! Aprendizado sem rede do vocabulário pessoal (B-10, WOW-03, WOW-06).
//!
//! O classificador é um kNN sobre TF-IDF (título, autores, comentários, editora, série), com
//! votos ponderados pela similaridade ao quadrado. Sugere SÓ tags e estantes que já existem no
//! vocabulário do usuário, devolve a confiança e os livros vizinhos usados ("exemplos"), aprende
//! coocorrência de tags (HQ, Gibi e História em Quadrinhos andam juntas: sugere o conjunto sem
//! unificar nada) e dá peso maior às decisões humanas. Tag nova nunca é aplicada: vira proposta.
//!
//! Estante = valor único da coluna `#pasta` do Calibre (não é tag). Tags que começam com `!`
//! (p.ex. `!Catalogar`) são de controle: nunca saem como assunto.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::normalizacao::{CompatAutor, comparar_autores_norm, normalizar_autor, normalizar_texto};
use crate::taxonomia;

// ---------------------------------------------------------------------------
// Chaves e aliases
// ---------------------------------------------------------------------------

fn singularizar(t: &str) -> String {
    let n = t.chars().count();
    if n > 4 && t.ends_with("oes") {
        return format!("{}ao", &t[..t.len() - 3]);
    }
    if n > 4 && t.ends_with("ais") {
        return format!("{}al", &t[..t.len() - 3]);
    }
    if n > 4 && t.ends_with("eis") {
        return format!("{}el", &t[..t.len() - 3]);
    }
    if n > 4 && t.ends_with("ns") {
        return format!("{}m", &t[..t.len() - 2]);
    }
    if n > 3 && t.ends_with('s') && !t.ends_with("ss") && !t.ends_with("us") {
        return t[..t.len() - 1].to_owned();
    }
    t.to_owned()
}

/// Tokens normalizados com plural simples removido (base de aliases e da taxonomia).
pub(crate) fn tokens_chave(s: &str) -> Vec<String> {
    normalizar_texto(s)
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(singularizar)
        .collect()
}

/// Chave de comparação de uma tag: caixa, acento, plural simples; cada segmento da hierarquia
/// (separador `.`) é normalizado em separado.
#[must_use]
pub fn chave_tag(tag: &str) -> String {
    tag.split('.')
        .map(|seg| tokens_chave(seg).join(" "))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(".")
}

/// Tag de controle (fila de trabalho como `!Catalogar`): nunca é assunto.
#[must_use]
pub fn e_tag_controle(tag: &str) -> bool {
    tag.trim_start().starts_with('!')
}

/// Ancestrais de uma tag hierárquica ("Matemática.Cálculo.Integral" -> "Matemática",
/// "Matemática.Cálculo"), na grafia recebida.
#[must_use]
pub fn ancestrais_hierarquia(tag: &str) -> Vec<String> {
    let partes: Vec<&str> = tag.split('.').collect();
    (1..partes.len()).map(|i| partes[..i].join(".")).collect()
}

// ---------------------------------------------------------------------------
// Vocabulário
// ---------------------------------------------------------------------------

/// Vocabulário homologado: grafia exata do usuário, com busca por alias.
#[derive(Debug, Clone, Default)]
pub struct Vocabulario {
    tags: Vec<String>,
    por_chave: HashMap<String, usize>,
    /// Último segmento da hierarquia -> tags (p.ex. "calculo" -> "Matemática.Cálculo").
    por_folha: HashMap<String, Vec<usize>>,
    controle: Vec<String>,
    estantes: Vec<String>,
    estante_por_chave: HashMap<String, usize>,
}

impl Vocabulario {
    /// Constrói a partir de tags e estantes; a primeira grafia de cada chave vence (passe as
    /// mais frequentes primeiro, ver [`Vocabulario::de_livros`]).
    pub fn novo<T, E>(tags: T, estantes: E) -> Self
    where
        T: IntoIterator,
        T::Item: AsRef<str>,
        E: IntoIterator,
        E::Item: AsRef<str>,
    {
        let mut v = Self::default();
        for t in tags {
            let t = t.as_ref().trim();
            if t.is_empty() {
                continue;
            }
            if e_tag_controle(t) {
                if !v.controle.iter().any(|c| c == t) {
                    v.controle.push(t.to_owned());
                }
                continue;
            }
            let k = chave_tag(t);
            if k.is_empty() || v.por_chave.contains_key(&k) {
                continue;
            }
            let idx = v.tags.len();
            v.tags.push(t.to_owned());
            if let Some(folha) = k.rsplit('.').next() {
                v.por_folha.entry(folha.to_owned()).or_default().push(idx);
            }
            v.por_chave.insert(k, idx);
        }
        for e in estantes {
            let e = e.as_ref().trim();
            let k = chave_tag(e);
            if k.is_empty() || v.estante_por_chave.contains_key(&k) {
                continue;
            }
            v.estante_por_chave.insert(k, v.estantes.len());
            v.estantes.push(e.to_owned());
        }
        v
    }

    /// Vocabulário a partir dos livros da Homologada: grafia mais frequente de cada tag.
    #[must_use]
    pub fn de_livros(livros: &[LivroTreino]) -> Self {
        let mut freq: HashMap<&str, usize> = HashMap::new();
        let mut est: HashMap<&str, usize> = HashMap::new();
        for l in livros {
            for t in &l.tags {
                *freq.entry(t.as_str()).or_default() += 1;
            }
            if let Some(e) = &l.estante {
                *est.entry(e.as_str()).or_default() += 1;
            }
        }
        let ordenar = |m: HashMap<&str, usize>| {
            let mut v: Vec<(&str, usize)> = m.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            v.into_iter().map(|(s, _)| s.to_owned()).collect::<Vec<_>>()
        };
        Self::novo(ordenar(freq), ordenar(est))
    }

    /// Tags de assunto (sem as de controle), na grafia homologada.
    #[must_use]
    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    /// Tags de controle (`!Catalogar`...).
    #[must_use]
    pub fn tags_controle(&self) -> &[String] {
        &self.controle
    }

    /// Estantes (valores da coluna `#pasta`).
    #[must_use]
    pub fn estantes(&self) -> &[String] {
        &self.estantes
    }

    /// Resolve um texto para a tag existente (grafia homologada): exato por alias, ou pelo
    /// último segmento da hierarquia quando for inequívoco.
    #[must_use]
    pub fn resolver_tag(&self, texto: &str) -> Option<&str> {
        self.indice_tag(texto).map(|i| self.tags[i].as_str())
    }

    pub(crate) fn indice_tag(&self, texto: &str) -> Option<usize> {
        let k = chave_tag(texto);
        if let Some(i) = self.por_chave.get(&k) {
            return Some(*i);
        }
        match self.por_folha.get(&k).map(Vec::as_slice) {
            Some([unico]) => Some(*unico),
            _ => None,
        }
    }

    /// Resolve para uma estante existente.
    #[must_use]
    pub fn resolver_estante(&self, texto: &str) -> Option<&str> {
        self.indice_estante(texto)
            .map(|i| self.estantes[i].as_str())
    }

    pub(crate) fn indice_estante(&self, texto: &str) -> Option<usize> {
        self.estante_por_chave.get(&chave_tag(texto)).copied()
    }

    /// Ancestrais existentes de uma tag hierárquica do vocabulário.
    #[must_use]
    pub fn ancestrais_existentes(&self, tag: &str) -> Vec<&str> {
        ancestrais_hierarquia(tag)
            .iter()
            .filter_map(|a| self.resolver_tag(a))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Entrada e saída
// ---------------------------------------------------------------------------

/// Livro como o classificador o vê (da Homologada, de uma decisão ou a consultar).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LivroTreino {
    pub id: i64,
    pub titulo: String,
    #[serde(default)]
    pub autores: Vec<String>,
    #[serde(default)]
    pub comentarios: Option<String>,
    #[serde(default)]
    pub editora: Option<String>,
    #[serde(default)]
    pub serie: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Estante (`#pasta`) candidata/atual.
    #[serde(default)]
    pub estante: Option<String>,
}

/// Decisão humana aprovada ou corrigida: entra com peso maior.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DecisaoHumana {
    /// Livro (características) sobre o qual se decidiu; `tags` e `estante` são os valores
    /// FINAIS aprovados.
    pub livro: LivroTreino,
    /// Tags que o usuário removeu/rejeitou para este livro (voto negativo para vizinhos).
    #[serde(default)]
    pub tags_rejeitadas: Vec<String>,
}

/// Configuração do classificador.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigClassificador {
    pub k: usize,
    pub similaridade_minima: f32,
    pub confianca_minima: f32,
    pub max_tags: usize,
    /// Peso de uma decisão humana frente a um livro da Homologada (1,0).
    pub peso_humano: f32,
    /// Confiança máxima de uma tag vinda só da taxonomia (evidência auxiliar).
    pub teto_taxonomia: f32,
    /// P(B|A) mínima para sugerir B junto de A (coocorrência / sinônimos).
    pub coocorrencia_minima: f32,
    /// Mínimo de livros com A para confiar na coocorrência.
    pub coocorrencia_suporte: u32,
    /// Opção: tag de controle aplicada a livro novo na A Classificar (ex.: "!Catalogar").
    #[serde(default)]
    pub tag_controle_livro_novo: Option<String>,
}

impl Default for ConfigClassificador {
    fn default() -> Self {
        Self {
            k: 7,
            similaridade_minima: 0.08,
            confianca_minima: 0.4,
            max_tags: 8,
            peso_humano: 3.0,
            teto_taxonomia: 0.45,
            coocorrencia_minima: 0.8,
            coocorrencia_suporte: 3,
            tag_controle_livro_novo: None,
        }
    }
}

/// Vizinho que sustentou uma sugestão ("por que?").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exemplo {
    pub livro_id: i64,
    pub similaridade: f32,
    /// Veio de uma decisão humana (não só da Homologada).
    pub humano: bool,
}

/// De onde veio uma sugestão de tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tipo", rename_all = "snake_case")]
pub enum OrigemSugestao {
    Vizinhos,
    /// Costuma vir junto de outra tag sugerida.
    Coocorrencia {
        com: String,
    },
    /// Só palavras-chave da taxonomia inicial (evidência auxiliar).
    Taxonomia {
        assunto: String,
    },
}

/// Tag sugerida (sempre do vocabulário existente).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SugestaoTag {
    pub tag: String,
    pub confianca: f32,
    pub origem: OrigemSugestao,
    pub exemplos: Vec<Exemplo>,
}

/// Estante sugerida (valor existente de `#pasta`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SugestaoEstante {
    pub estante: String,
    pub confianca: f32,
    pub exemplos: Vec<Exemplo>,
    pub alternativas: Vec<(String, f32)>,
}

/// Tag que NÃO existe no vocabulário: só proposta, nunca aplicada.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropostaTag {
    pub valor: String,
    pub motivo: String,
}

/// Resultado de uma consulta.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Sugestoes {
    pub tags: Vec<SugestaoTag>,
    pub estante: Option<SugestaoEstante>,
    pub propostas: Vec<PropostaTag>,
    /// Tags de controle por regra configurada (nunca assunto).
    pub tags_controle: Vec<String>,
}

/// Par de tags que andam sempre juntas: possível sinônimo (só sugestão ao usuário).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParSinonimo {
    pub a: String,
    pub b: String,
    pub livros_juntas: u32,
    /// Jaccard dos conjuntos de livros.
    pub jaccard: f32,
}

// ---------------------------------------------------------------------------
// Classificador
// ---------------------------------------------------------------------------

const STOPWORDS: &[&str] = &[
    "de", "da", "do", "das", "dos", "e", "o", "a", "os", "as", "um", "uma", "em", "no", "na",
    "nos", "nas", "por", "para", "com", "que", "se", "ao", "the", "of", "and", "to", "in", "for",
    "on", "an", "is", "with", "by", "el", "la", "los", "las", "del", "y", "le", "les", "des", "du",
    "et", "um", "ou", "como", "mais", "sobre", "pelo", "pela", "este", "esta", "seu", "sua",
    "livro", "edicao", "volume", "vol", "ed",
];

#[derive(Debug, Clone)]
struct Doc {
    id: i64,
    peso: f32,
    humano: bool,
    tags: Vec<usize>,
    rejeitadas: Vec<usize>,
    estante: Option<usize>,
}

/// Classificador kNN TF-IDF explicável. O vocabulário é imutável; os índices de tags apontam
/// para ele.
#[derive(Debug, Clone)]
pub struct Classificador {
    vocab: Vocabulario,
    config: ConfigClassificador,
    docs: Vec<Doc>,
    idf: HashMap<String, f32>,
    idf_desconhecido: f32,
    postings: HashMap<String, Vec<(u32, f32)>>,
    freq_tag: Vec<u32>,
    coocorrencia: HashMap<(usize, usize), u32>,
}

fn caracteristicas(l: &LivroTreino) -> HashMap<String, f32> {
    let mut m: HashMap<String, f32> = HashMap::new();
    let mut add = |tok: String, n: f32| *m.entry(tok).or_default() += n;
    let util = |t: &String| t.chars().count() > 1 && !STOPWORDS.contains(&t.as_str());
    for t in tokens_chave(&l.titulo).into_iter().filter(util) {
        add(t, 3.0);
    }
    for a in &l.autores {
        let n = normalizar_autor(a);
        if !n.sobrenome.is_empty() {
            add(format!("a:{}", n.chave()), 2.0);
            add(format!("a:{}", n.sobrenome_final()), 1.0);
        }
    }
    if let Some(c) = &l.comentarios {
        for t in tokens_chave(c).into_iter().filter(util).take(400) {
            add(t, 1.0);
        }
    }
    if let Some(e) = &l.editora {
        let k = normalizar_texto(e);
        if !k.is_empty() {
            add(format!("e:{k}"), 1.0);
        }
    }
    if let Some(s) = &l.serie {
        let k = normalizar_texto(s);
        if !k.is_empty() {
            add(format!("s:{k}"), 2.0);
        }
    }
    m
}

impl Classificador {
    /// Treina com os livros da Homologada. O vocabulário vem de `vocab` (tags não presentes
    /// nele são ignoradas no treino).
    #[must_use]
    pub fn treinar(
        vocab: Vocabulario,
        livros: &[LivroTreino],
        config: ConfigClassificador,
    ) -> Self {
        let n = livros.len().max(1) as f32;
        let mut df: HashMap<String, u32> = HashMap::new();
        let feats: Vec<HashMap<String, f32>> = livros.iter().map(caracteristicas).collect();
        for f in &feats {
            for t in f.keys() {
                *df.entry(t.clone()).or_default() += 1;
            }
        }
        let idf: HashMap<String, f32> = df
            .iter()
            .map(|(t, d)| (t.clone(), ((n + 1.0) / (*d as f32 + 1.0)).ln() + 1.0))
            .collect();
        let freq_tag = vec![0; vocab.tags.len()];
        let mut c = Self {
            vocab,
            config,
            docs: Vec::new(),
            idf,
            idf_desconhecido: (n + 1.0).ln() + 1.0,
            postings: HashMap::new(),
            freq_tag,
            coocorrencia: HashMap::new(),
        };
        for (l, f) in livros.iter().zip(feats) {
            c.inserir(l, &f, 1.0, false, &[]);
        }
        c
    }

    fn inserir(
        &mut self,
        l: &LivroTreino,
        feats: &HashMap<String, f32>,
        peso: f32,
        humano: bool,
        rejeitadas: &[String],
    ) {
        let tags: Vec<usize> = {
            let mut v: Vec<usize> = l
                .tags
                .iter()
                .filter_map(|t| self.vocab.indice_tag(t))
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let rej: Vec<usize> = rejeitadas
            .iter()
            .filter_map(|t| self.vocab.indice_tag(t))
            .collect();
        let estante = l
            .estante
            .as_deref()
            .and_then(|e| self.vocab.indice_estante(e));
        let idx = self.docs.len() as u32;
        // Vetor TF-IDF normalizado.
        let mut pesos: Vec<(String, f32)> = feats
            .iter()
            .map(|(t, c)| {
                let idf = self.idf.get(t).copied().unwrap_or(self.idf_desconhecido);
                (t.clone(), (1.0 + c.ln()) * idf)
            })
            .collect();
        let norma = pesos.iter().map(|(_, w)| w * w).sum::<f32>().sqrt();
        if norma > 0.0 {
            for (_, w) in &mut pesos {
                *w /= norma;
            }
            for (t, w) in pesos {
                self.postings.entry(t).or_default().push((idx, w));
            }
        }
        for (i, a) in tags.iter().enumerate() {
            self.freq_tag[*a] += 1;
            for b in &tags[i + 1..] {
                *self.coocorrencia.entry((*a, *b)).or_default() += 1;
            }
        }
        self.docs.push(Doc {
            id: l.id,
            peso,
            humano,
            tags,
            rejeitadas: rej,
            estante,
        });
    }

    /// Registra uma decisão humana: passa a influenciar as próximas sugestões, com peso maior.
    pub fn registrar_decisao(&mut self, d: &DecisaoHumana) {
        let f = caracteristicas(&d.livro);
        let peso = self.config.peso_humano;
        self.inserir(&d.livro, &f, peso, true, &d.tags_rejeitadas);
    }

    #[must_use]
    pub fn vocabulario(&self) -> &Vocabulario {
        &self.vocab
    }

    #[must_use]
    pub fn total_exemplos(&self) -> usize {
        self.docs.len()
    }

    fn vizinhos(&self, l: &LivroTreino) -> Vec<(usize, f32)> {
        let f = caracteristicas(l);
        let mut pesos: Vec<(String, f32)> = f
            .iter()
            .map(|(t, c)| {
                let idf = self.idf.get(t).copied().unwrap_or(self.idf_desconhecido);
                (t.clone(), (1.0 + c.ln()) * idf)
            })
            .collect();
        let norma = pesos.iter().map(|(_, w)| w * w).sum::<f32>().sqrt();
        if norma == 0.0 {
            return Vec::new();
        }
        for (_, w) in &mut pesos {
            *w /= norma;
        }
        let mut acc: HashMap<u32, f32> = HashMap::new();
        for (t, wq) in &pesos {
            if let Some(p) = self.postings.get(t) {
                for (d, wd) in p {
                    // Livros "gêmeos" do próprio id consultado não contam.
                    if self.docs[*d as usize].id == l.id && !self.docs[*d as usize].humano {
                        continue;
                    }
                    *acc.entry(*d).or_default() += wq * wd;
                }
            }
        }
        let mut v: Vec<(usize, f32)> = acc
            .into_iter()
            .filter(|(_, s)| *s >= self.config.similaridade_minima)
            .map(|(d, s)| (d as usize, s))
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(self.config.k);
        v
    }

    /// Sugere tags e estante para um livro. Só valores do vocabulário; o resto vai em `propostas`.
    #[must_use]
    pub fn sugerir(&self, livro: &LivroTreino) -> Sugestoes {
        let viz = self.vizinhos(livro);
        let mut s = Sugestoes::default();
        let melhor = viz.first().map_or(0.0, |v| v.1);
        let fator = 0.6 + 0.4 * (melhor / 0.5).min(1.0);
        let pesos: Vec<f32> = viz
            .iter()
            .map(|(d, sim)| sim * sim * self.docs[*d].peso)
            .collect();
        let total: f32 = pesos.iter().sum();
        let exemplo = |i: usize| Exemplo {
            livro_id: self.docs[viz[i].0].id,
            similaridade: viz[i].1,
            humano: self.docs[viz[i].0].humano,
        };

        // --- tags por vizinhos ---
        let mut votos: HashMap<usize, (f32, Vec<usize>)> = HashMap::new();
        let mut negativos: HashMap<usize, f32> = HashMap::new();
        for (i, (d, _)) in viz.iter().enumerate() {
            for t in &self.docs[*d].tags {
                let e = votos.entry(*t).or_default();
                e.0 += pesos[i];
                e.1.push(i);
            }
            for t in &self.docs[*d].rejeitadas {
                *negativos.entry(*t).or_default() += pesos[i];
            }
        }
        let mut sugeridas: HashMap<usize, SugestaoTag> = HashMap::new();
        if total > 0.0 {
            for (t, (pos, quem)) in &votos {
                let liquido = (pos - negativos.get(t).copied().unwrap_or(0.0)).max(0.0);
                let conf = (liquido / total * fator).min(0.97);
                if conf >= self.config.confianca_minima {
                    sugeridas.insert(
                        *t,
                        SugestaoTag {
                            tag: self.vocab.tags[*t].clone(),
                            confianca: conf,
                            origem: OrigemSugestao::Vizinhos,
                            exemplos: quem.iter().map(|i| exemplo(*i)).collect(),
                        },
                    );
                }
            }
        }

        // --- coocorrência: o conjunto que o usuário costuma usar junto ---
        let base: Vec<(usize, f32)> = sugeridas.iter().map(|(k, v)| (*k, v.confianca)).collect();
        for (a, conf_a) in base {
            let na = self.freq_tag[a];
            if na < self.config.coocorrencia_suporte {
                continue;
            }
            for b in 0..self.vocab.tags.len() {
                if b == a || sugeridas.get(&b).is_some_and(|x| x.confianca >= conf_a) {
                    continue;
                }
                let par = if a < b { (a, b) } else { (b, a) };
                let juntos = self.coocorrencia.get(&par).copied().unwrap_or(0);
                if juntos < self.config.coocorrencia_suporte {
                    continue;
                }
                let p = juntos as f32 / na as f32;
                if p >= self.config.coocorrencia_minima {
                    let conf = (conf_a * p * 0.95).min(0.97);
                    if conf >= self.config.confianca_minima
                        && sugeridas.get(&b).is_none_or(|x| x.confianca < conf)
                    {
                        sugeridas.insert(
                            b,
                            SugestaoTag {
                                tag: self.vocab.tags[b].clone(),
                                confianca: conf,
                                origem: OrigemSugestao::Coocorrencia {
                                    com: self.vocab.tags[a].clone(),
                                },
                                exemplos: sugeridas[&a].exemplos.clone(),
                            },
                        );
                    }
                }
            }
        }

        // --- taxonomia: evidência auxiliar, sempre mapeada ao vocabulário ---
        let texto = format!(
            "{} {}",
            livro.serie.as_deref().unwrap_or(""),
            livro.comentarios.as_deref().unwrap_or("")
        );
        let detectados = taxonomia::detectar(&livro.titulo, &texto);
        let mapeado = taxonomia::mapear(&detectados, &self.vocab);
        for m in &mapeado.tags {
            let Some(i) = self.vocab.indice_tag(&m.tag) else {
                continue;
            };
            let conf = (m.pontuacao * self.config.teto_taxonomia).min(self.config.teto_taxonomia);
            match sugeridas.get_mut(&i) {
                Some(existente) => {
                    // Concorda com os vizinhos: reforça um pouco.
                    existente.confianca = (existente.confianca + 0.08 * m.pontuacao).min(0.97);
                }
                None if conf >= self.config.confianca_minima.min(0.3) => {
                    sugeridas.insert(
                        i,
                        SugestaoTag {
                            tag: self.vocab.tags[i].clone(),
                            confianca: conf,
                            origem: OrigemSugestao::Taxonomia {
                                assunto: m.assunto.clone(),
                            },
                            exemplos: Vec::new(),
                        },
                    );
                }
                None => {}
            }
        }
        s.propostas = mapeado.propostas;

        let mut tags: Vec<SugestaoTag> = sugeridas.into_values().collect();
        tags.sort_by(|a, b| b.confianca.total_cmp(&a.confianca).then(a.tag.cmp(&b.tag)));
        tags.truncate(self.config.max_tags);
        s.tags = tags;

        // --- estante (valor único de #pasta) ---
        let mut ev: HashMap<usize, (f32, Vec<usize>)> = HashMap::new();
        let mut total_est = 0.0f32;
        for (i, (d, _)) in viz.iter().enumerate() {
            if let Some(e) = self.docs[*d].estante {
                let x = ev.entry(e).or_default();
                x.0 += pesos[i];
                x.1.push(i);
                total_est += pesos[i];
            }
        }
        if total_est > 0.0 {
            let mut ranking: Vec<(usize, f32, Vec<usize>)> = ev
                .into_iter()
                .map(|(e, (w, q))| (e, (w / total_est * fator).min(0.97), q))
                .collect();
            ranking.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            let (e, conf, quem) = &ranking[0];
            if *conf >= self.config.confianca_minima {
                s.estante = Some(SugestaoEstante {
                    estante: self.vocab.estantes[*e].clone(),
                    confianca: *conf,
                    exemplos: quem.iter().map(|i| exemplo(*i)).collect(),
                    alternativas: ranking[1..]
                        .iter()
                        .take(3)
                        .map(|(e, c, _)| (self.vocab.estantes[*e].clone(), *c))
                        .collect(),
                });
            }
        }

        if let Some(c) = &self.config.tag_controle_livro_novo
            && let Some(existente) = self.vocab.controle.iter().find(|t| *t == c)
        {
            s.tags_controle.push(existente.clone());
        }
        s
    }

    /// Separa valores propostos por outras fontes (IA, metadado interno) em tags existentes
    /// (grafia homologada, via alias) e propostas de tag nova. Tags de controle são ignoradas.
    #[must_use]
    pub fn filtrar_candidatas(&self, valores: &[String]) -> (Vec<String>, Vec<PropostaTag>) {
        let mut ok = Vec::new();
        let mut propostas = Vec::new();
        let mut vistos = HashSet::new();
        for v in valores {
            let v = v.trim();
            if v.is_empty() || e_tag_controle(v) || !vistos.insert(chave_tag(v)) {
                continue;
            }
            match self.vocab.resolver_tag(v) {
                Some(t) => ok.push(t.to_owned()),
                None => propostas.push(PropostaTag {
                    valor: v.to_owned(),
                    motivo: "tag inexistente no vocabulário homologado: precisa de aprovação"
                        .to_owned(),
                }),
            }
        }
        (ok, propostas)
    }

    /// Relatório de possíveis sinônimos (tags que quase sempre aparecem juntas). Só sugestão
    /// ao usuário: nada é unificado nem renomeado.
    #[must_use]
    pub fn possiveis_sinonimos(&self, jaccard_minimo: f32, suporte: u32) -> Vec<ParSinonimo> {
        let mut v = Vec::new();
        for (&(a, b), &n) in &self.coocorrencia {
            if n < suporte {
                continue;
            }
            let uniao = self.freq_tag[a] + self.freq_tag[b] - n;
            let j = n as f32 / uniao.max(1) as f32;
            if j >= jaccard_minimo {
                v.push(ParSinonimo {
                    a: self.vocab.tags[a].clone(),
                    b: self.vocab.tags[b].clone(),
                    livros_juntas: n,
                    jaccard: j,
                });
            }
        }
        v.sort_by(|x, y| {
            y.jaccard
                .total_cmp(&x.jaccard)
                .then(y.livros_juntas.cmp(&x.livros_juntas))
                .then(x.a.cmp(&y.a))
                .then(x.b.cmp(&y.b))
        });
        v
    }
}

/// Atalho: o nome de autor `a` casa com algum de `existentes` por [`CompatAutor::Igual`]?
#[must_use]
pub fn autor_conhecido(a: &str, existentes: &[String]) -> bool {
    let n = normalizar_autor(a);
    existentes
        .iter()
        .any(|e| comparar_autores_norm(&n, &normalizar_autor(e)) == CompatAutor::Igual)
}

#[cfg(test)]
mod testes {
    use super::*;

    fn livro(id: i64, titulo: &str, autor: &str, tags: &[&str], estante: &str) -> LivroTreino {
        LivroTreino {
            id,
            titulo: titulo.into(),
            autores: vec![autor.into()],
            tags: tags.iter().map(|s| (*s).into()).collect(),
            estante: Some(estante.into()),
            ..Default::default()
        }
    }

    fn base() -> Vec<LivroTreino> {
        let mut v = Vec::new();
        for i in 0..6 {
            v.push(livro(
                100 + i,
                &format!("Cálculo Diferencial e Integral volume {i}"),
                "James Stewart",
                &["Matemática", "Cálculo"],
                "Matemática",
            ));
            v.push(livro(
                200 + i,
                &format!("Aventuras do Capitão Mirim número {i}"),
                "Turma da Mônica",
                &["Gibi", "HQ", "História em Quadrinhos", "!Catalogar"],
                "Gibi",
            ));
            v.push(livro(
                300 + i,
                &format!("Direito Civil Brasileiro tomo {i}"),
                "Carlos Gonçalves",
                &["Direito"],
                "Direito",
            ));
        }
        v
    }

    fn classificador() -> Classificador {
        let livros = base();
        Classificador::treinar(
            Vocabulario::de_livros(&livros),
            &livros,
            ConfigClassificador::default(),
        )
    }

    #[test]
    fn aliases_de_tags() {
        let v = Vocabulario::novo(
            ["Matemática", "Equações Diferenciais", "Matemática.Cálculo"],
            ["Gibi"],
        );
        assert_eq!(v.resolver_tag("matematica"), Some("Matemática"));
        assert_eq!(v.resolver_tag("MATEMÁTICAS"), Some("Matemática"));
        assert_eq!(
            v.resolver_tag("equacao diferencial"),
            Some("Equações Diferenciais")
        );
        // Folha única da hierarquia resolve; a grafia homologada é preservada.
        assert_eq!(v.resolver_tag("Cálculo"), Some("Matemática.Cálculo"));
        assert_eq!(v.resolver_tag("Física"), None);
        assert_eq!(v.resolver_estante("gibis"), Some("Gibi"));
        assert_eq!(chave_tag("Matemática.Cálculo"), "matematica.calculo");
        assert_eq!(ancestrais_hierarquia("A.B.C"), ["A", "A.B"]);
    }

    #[test]
    fn controle_nunca_vira_assunto() {
        let v = Vocabulario::novo(["!Catalogar", "Matemática"], Vec::<String>::new());
        assert_eq!(v.tags(), ["Matemática"]);
        assert_eq!(v.tags_controle(), ["!Catalogar"]);
        assert_eq!(v.resolver_tag("!Catalogar"), None);
        let c = classificador();
        for l in base() {
            let s = c.sugerir(&LivroTreino { id: 9000, ..l });
            assert!(s.tags.iter().all(|t| !e_tag_controle(&t.tag)));
        }
        let (ok, prop) = c.filtrar_candidatas(&[
            "!Catalogar".into(),
            "gibis".into(),
            "Culinária Vegana".into(),
        ]);
        assert_eq!(ok, ["Gibi"]);
        assert_eq!(prop.len(), 1);
    }

    #[test]
    fn regra_opcional_de_tag_de_controle_para_livro_novo() {
        let livros = base();
        let cfg = ConfigClassificador {
            tag_controle_livro_novo: Some("!Catalogar".into()),
            ..Default::default()
        };
        let c = Classificador::treinar(Vocabulario::de_livros(&livros), &livros, cfg);
        let s = c.sugerir(&livro(1, "Qualquer", "Alguém", &[], "Matemática"));
        assert_eq!(s.tags_controle, ["!Catalogar"]);
        // Desligada por padrão.
        assert!(
            classificador()
                .sugerir(&livro(1, "Qualquer", "Alguém", &[], "x"))
                .tags_controle
                .is_empty()
        );
    }

    #[test]
    fn sugere_so_do_vocabulario_com_exemplos() {
        let c = classificador();
        let q = LivroTreino {
            id: 999,
            titulo: "Cálculo Diferencial e Integral volume 9".into(),
            autores: vec!["Stewart, James".into()],
            ..Default::default()
        };
        let s = c.sugerir(&q);
        let nomes: Vec<&str> = s.tags.iter().map(|t| t.tag.as_str()).collect();
        assert!(nomes.contains(&"Matemática"), "{nomes:?}");
        assert!(nomes.contains(&"Cálculo"), "{nomes:?}");
        for t in &s.tags {
            assert!(c.vocabulario().resolver_tag(&t.tag).is_some());
            assert!(t.confianca > 0.0 && t.confianca <= 0.97);
        }
        let mat = s.tags.iter().find(|t| t.tag == "Matemática").unwrap();
        assert!(!mat.exemplos.is_empty());
        assert!(
            mat.exemplos
                .iter()
                .all(|e| (100..110).contains(&e.livro_id))
        );
        let est = s.estante.unwrap();
        assert_eq!(est.estante, "Matemática");
        assert!(!est.exemplos.is_empty());
    }

    #[test]
    fn quadrinho_recebe_o_conjunto_de_sinonimos() {
        let c = classificador();
        let s = c.sugerir(&livro(
            5,
            "Aventuras do Capitão Mirim número 77",
            "Turma da Mônica",
            &[],
            "",
        ));
        let nomes: HashSet<&str> = s.tags.iter().map(|t| t.tag.as_str()).collect();
        for esperado in ["Gibi", "HQ", "História em Quadrinhos"] {
            assert!(nomes.contains(esperado), "faltou {esperado}: {nomes:?}");
        }
        assert_eq!(s.estante.unwrap().estante, "Gibi");
    }

    #[test]
    fn coocorrencia_completa_o_conjunto() {
        // Título sem nenhuma palavra em comum, mas com uma tag já certa: as outras vêm junto.
        let livros = base();
        let c = Classificador::treinar(
            Vocabulario::de_livros(&livros),
            &livros,
            ConfigClassificador::default(),
        );
        let s = c.sugerir(&livro(5, "Aventuras do Capitão Mirim", "Outro", &[], ""));
        assert!(s.tags.iter().any(|t| t.tag == "HQ"));
        let sin = c.possiveis_sinonimos(0.9, 3);
        assert!(
            sin.iter()
                .any(|p| (p.a == "Gibi" && p.b == "HQ") || (p.a == "HQ" && p.b == "Gibi"))
        );
        // Só relatório: o vocabulário continua com as três grafias.
        assert!(
            c.vocabulario()
                .tags()
                .contains(&"História em Quadrinhos".to_owned())
        );
    }

    #[test]
    fn correcao_humana_muda_proximas_sugestoes_antes_depois() {
        let mut livros = base();
        // Conjunto de livros de álgebra linear com tag genérica apenas.
        for i in 0..3 {
            livros.push(livro(
                400 + i,
                &format!("Tópicos de Álgebra Linear parte {i}"),
                "Gilbert Strang",
                &["Matemática"],
                "Matemática",
            ));
        }
        livros.push(livro(
            500,
            "Álgebra Linear Aplicada",
            "Gilbert Strang",
            &["Matemática", "Álgebra Linear"],
            "Matemática",
        ));
        let vocab = Vocabulario::de_livros(&livros);
        let mut c = Classificador::treinar(vocab, &livros, ConfigClassificador::default());
        let q = LivroTreino {
            id: 901,
            titulo: "Álgebra Linear Moderna".into(),
            autores: vec!["Gilbert Strang".into()],
            ..Default::default()
        };
        let antes = c.sugerir(&q);
        let conf_antes = antes
            .tags
            .iter()
            .find(|t| t.tag == "Álgebra Linear")
            .map_or(0.0, |t| t.confianca);
        // O usuário corrige alguns livros semelhantes: "Álgebra Linear" é a tag certa.
        for i in 0..2 {
            c.registrar_decisao(&DecisaoHumana {
                livro: livro(
                    600 + i,
                    &format!("Álgebra Linear Moderna edição {i}"),
                    "Gilbert Strang",
                    &["Matemática", "Álgebra Linear"],
                    "Matemática",
                ),
                tags_rejeitadas: vec![],
            });
        }
        let depois = c.sugerir(&q);
        let t = depois
            .tags
            .iter()
            .find(|t| t.tag == "Álgebra Linear")
            .expect("deve sugerir");
        assert!(t.confianca > conf_antes, "{} !> {conf_antes}", t.confianca);
        assert!(
            t.exemplos.iter().any(|e| e.humano),
            "exemplos humanos aparecem"
        );
    }

    #[test]
    fn rejeicao_humana_reduz_a_tag() {
        let mut c = classificador();
        let q = LivroTreino {
            id: 900,
            titulo: "Cálculo Diferencial e Integral volume 3".into(),
            autores: vec!["James Stewart".into()],
            ..Default::default()
        };
        let antes = c
            .sugerir(&q)
            .tags
            .iter()
            .find(|t| t.tag == "Cálculo")
            .unwrap()
            .confianca;
        for i in 0..4 {
            c.registrar_decisao(&DecisaoHumana {
                livro: livro(
                    700 + i,
                    "Cálculo Diferencial e Integral volume 3",
                    "James Stewart",
                    &["Matemática"],
                    "Matemática",
                ),
                tags_rejeitadas: vec!["Cálculo".into()],
            });
        }
        let depois = c
            .sugerir(&q)
            .tags
            .iter()
            .find(|t| t.tag == "Cálculo")
            .map_or(0.0, |t| t.confianca);
        assert!(depois < antes);
    }

    #[test]
    fn tag_nova_nunca_e_aplicada_so_proposta() {
        let c = classificador();
        let (ok, prop) = c.filtrar_candidatas(&["Topologia Algébrica".into(), "direito".into()]);
        assert_eq!(ok, ["Direito"]);
        assert_eq!(prop[0].valor, "Topologia Algébrica");
        // Taxonomia de assunto que não existe no vocabulário também vira proposta.
        let s = c.sugerir(&LivroTreino {
            id: 1,
            titulo: "Topologia Geral e Espaço Topológico".into(),
            ..Default::default()
        });
        assert!(
            s.tags
                .iter()
                .all(|t| c.vocabulario().resolver_tag(&t.tag).is_some())
        );
        assert!(s.propostas.iter().any(|p| p.valor == "Topologia"));
    }

    #[test]
    fn sem_vizinhos_nao_inventa() {
        let c = classificador();
        let s = c.sugerir(&LivroTreino {
            id: 1,
            titulo: "zzzz qqqq".into(),
            ..Default::default()
        });
        assert!(s.tags.is_empty());
        assert!(s.estante.is_none());
    }

    #[test]
    fn autor_conhecido_atalho() {
        assert!(autor_conhecido(
            "Machado de Assis",
            &["Assis, Machado de".into()]
        ));
        assert!(!autor_conhecido("João Silva", &["João Silva Filho".into()]));
    }
}
