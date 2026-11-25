//! Identidade bibliográfica: Obra > Edição > Formato > Arquivo (B-11, B-14).
//!
//! Regras duras (todas com teste):
//! - mesmo título sozinho nunca basta;
//! - idiomas diferentes = Edições diferentes (da mesma Obra, se título e autores casam);
//! - tradução é Edição separada da mesma Obra;
//! - volumes ou números diferentes (quadrinhos, "Vol. 1" x "Vol. 2") nunca agrupam;
//! - PDF + EPUB da mesma Edição = um agrupamento, dois Formatos preservados, economia ZERO;
//! - hipótese fraca vai para revisão (`Incerta`), nunca vira agrupamento automático.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::isbn;
use crate::normalizacao::{
    CompatAutor, comparar_autores_norm, extrair_volume, normalizar_autor, normalizar_texto,
    normalizar_titulo, numero_edicao, similaridade_titulos_norm,
};
use crate::tipos::{FormatoLivro, TipoDuplicidade};

/// Um livro/arquivo como entrada do agrupamento (vindo do Calibre ou da extração).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RegistroBibliografico {
    /// Identificador opaco do chamador (id do arquivo no Catálogo, `books.id`...).
    pub id: String,
    pub titulo: String,
    #[serde(default)]
    pub autores: Vec<String>,
    #[serde(default)]
    pub editora: Option<String>,
    #[serde(default)]
    pub ano: Option<i32>,
    /// Código de idioma (`por`, `pt`, `eng`...), comparado em minúsculas pelas 2 primeiras letras.
    #[serde(default)]
    pub idioma: Option<String>,
    #[serde(default)]
    pub isbns: Vec<String>,
    #[serde(default)]
    pub paginas: Option<u32>,
    #[serde(default)]
    pub volume: Option<String>,
    #[serde(default)]
    pub serie: Option<String>,
    #[serde(default)]
    pub indice_serie: Option<f64>,
    /// Texto da edição ("2ª edição").
    #[serde(default)]
    pub edicao: Option<String>,
    #[serde(default)]
    pub tradutores: Vec<String>,
    #[serde(default)]
    pub formato: Option<FormatoLivro>,
    /// Hash Completo do arquivo, se conhecido.
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub tamanho: u64,
    #[serde(default)]
    pub quadrinho: bool,
}

/// Relação entre dois registros.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relacao {
    MesmaEdicao,
    MesmaObra,
    /// Hipótese fraca: revisão humana.
    Incerta,
    Distintos,
}

/// Risco de agir sobre a relação como se fosse certa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risco {
    Baixo,
    Medio,
    Alto,
}

/// Decisão sobre um par, com as evidências a favor e contra.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Veredito {
    pub relacao: Relacao,
    /// `None` quando são distintos.
    pub tipo: Option<TipoDuplicidade>,
    pub evidencias: Vec<String>,
    pub contraevidencias: Vec<String>,
    pub risco: Risco,
    pub similaridade_titulo: f64,
}

impl Veredito {
    fn novo(relacao: Relacao, sim: f64, ev: Vec<String>, contra: Vec<String>) -> Self {
        let (tipo, risco) = match relacao {
            Relacao::MesmaEdicao => (Some(TipoDuplicidade::MesmaEdicao), Risco::Baixo),
            Relacao::MesmaObra => (Some(TipoDuplicidade::MesmaObra), Risco::Medio),
            Relacao::Incerta => (Some(TipoDuplicidade::Incerta), Risco::Alto),
            Relacao::Distintos => (None, Risco::Baixo),
        };
        Self {
            relacao,
            tipo,
            evidencias: ev,
            contraevidencias: contra,
            risco,
            similaridade_titulo: sim,
        }
    }
}

fn lang(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(|x| normalizar_texto(x).chars().take(2).collect::<String>())
        .filter(|x| !x.is_empty())
}

/// ISBN-13 válidos (ISBN-10 convertidos); inválidos são ignorados.
fn isbns_validos(r: &RegistroBibliografico) -> HashSet<String> {
    r.isbns
        .iter()
        .filter_map(|i| {
            let l = isbn::limpar(i);
            if isbn::valido_13(&l) {
                Some(l)
            } else {
                isbn::para_13(&l)
            }
        })
        .collect()
}

fn volume_de(r: &RegistroBibliografico, titulo_vol: Option<&String>) -> Option<String> {
    r.volume
        .as_deref()
        .and_then(extrair_volume)
        .or_else(|| titulo_vol.cloned())
}

fn paginas_aproximadas(a: u32, b: u32) -> bool {
    let (a, b) = (i64::from(a), i64::from(b));
    (a - b).abs() <= 3.max(a.max(b) * 3 / 100)
}

#[derive(Debug, PartialEq, Eq)]
enum RelAutores {
    Iguais,
    Compativeis,
    Parciais,
    Desconhecido,
    Diferentes,
}

fn comparar_listas_autores(a: &[String], b: &[String]) -> RelAutores {
    if a.is_empty() || b.is_empty() {
        return RelAutores::Desconhecido;
    }
    let na: Vec<_> = a.iter().map(|x| normalizar_autor(x)).collect();
    let nb: Vec<_> = b.iter().map(|x| normalizar_autor(x)).collect();
    let mut usados = vec![false; nb.len()];
    let mut iguais = 0;
    let mut compat = 0;
    for x in &na {
        let mut melhor: Option<(usize, CompatAutor)> = None;
        for (j, y) in nb.iter().enumerate() {
            if usados[j] {
                continue;
            }
            let c = comparar_autores_norm(x, y);
            if c >= CompatAutor::Compativel && melhor.is_none_or(|(_, m)| c > m) {
                melhor = Some((j, c));
            }
        }
        if let Some((j, c)) = melhor {
            usados[j] = true;
            if c == CompatAutor::Igual {
                iguais += 1;
            } else {
                compat += 1;
            }
        }
    }
    let casados = iguais + compat;
    if casados == 0 {
        // Sufixo ambíguo ("João Silva" x "João Silva Filho") não é "diferente": é dúvida.
        let ambiguo = na.iter().any(|x| {
            nb.iter()
                .any(|y| comparar_autores_norm(x, y) == CompatAutor::SufixoAmbiguo)
        });
        return if ambiguo {
            RelAutores::Parciais
        } else {
            RelAutores::Diferentes
        };
    }
    if casados == na.len() && casados == nb.len() {
        if compat == 0 {
            RelAutores::Iguais
        } else {
            RelAutores::Compativeis
        }
    } else {
        RelAutores::Parciais
    }
}

/// Compara dois registros e decide a relação bibliográfica.
#[must_use]
pub fn comparar(a: &RegistroBibliografico, b: &RegistroBibliografico) -> Veredito {
    let mut ev: Vec<String> = Vec::new();
    let mut contra: Vec<String> = Vec::new();
    let ta = normalizar_titulo(&a.titulo);
    let tb = normalizar_titulo(&b.titulo);
    let sim = similaridade_titulos_norm(&ta, &tb);

    // 1) Mesmo conteúdo: mesmo arquivo, mesma Edição.
    if let (Some(ha), Some(hb)) = (&a.hash, &b.hash)
        && ha == hb
        && a.tamanho == b.tamanho
    {
        ev.push("mesmo hash e tamanho".into());
        return Veredito::novo(Relacao::MesmaEdicao, sim, ev, contra);
    }

    // 2) Volume/número diferente nunca agrupa.
    let va = volume_de(a, ta.volume.as_ref());
    let vb = volume_de(b, tb.volume.as_ref());
    if let (Some(x), Some(y)) = (&va, &vb)
        && x != y
    {
        contra.push(format!("volumes diferentes ({x} x {y})"));
        return Veredito::novo(Relacao::Distintos, sim, ev, contra);
    }
    if let (Some(x), Some(y)) = (a.indice_serie, b.indice_serie)
        && (x - y).abs() > 1e-9
        && (a.serie.is_some() || b.serie.is_some() || a.quadrinho || b.quadrinho)
    {
        contra.push(format!("números diferentes na série ({x} x {y})"));
        return Veredito::novo(Relacao::Distintos, sim, ev, contra);
    }
    let quadrinho = a.quadrinho
        || b.quadrinho
        || a.formato.is_some_and(FormatoLivro::e_quadrinho)
        || b.formato.is_some_and(FormatoLivro::e_quadrinho);
    let volume_unilateral = va.is_some() != vb.is_some();
    if volume_unilateral {
        contra.push(if quadrinho {
            "quadrinho: só um dos lados informa o número".into()
        } else {
            "só um dos lados informa o volume".into()
        });
    }
    let teto_incerta = volume_unilateral;

    let autores = comparar_listas_autores(&a.autores, &b.autores);
    let (ia, ib) = (isbns_validos(a), isbns_validos(b));
    let compartilhados: Vec<&String> = ia.intersection(&ib).collect();
    let (la, lb) = (lang(&a.idioma), lang(&b.idioma));
    let idiomas_diferem = matches!((&la, &lb), (Some(x), Some(y)) if x != y);

    // 3) ISBN validado compartilhado, corroborado por título ou autor (ISBN não prova sozinho).
    if let Some(i) = compartilhados.first() {
        ev.push(format!("ISBN {i} validado em comum"));
        if idiomas_diferem {
            contra.push("mesmo ISBN, idiomas diferentes".into());
            return Veredito::novo(Relacao::Incerta, sim, ev, contra);
        }
        let corrobora =
            sim >= 0.5 || matches!(autores, RelAutores::Iguais | RelAutores::Compativeis);
        if corrobora && !teto_incerta {
            if sim >= 0.8 {
                ev.push("título compatível".into());
            }
            if autores == RelAutores::Iguais {
                ev.push("autores iguais".into());
            }
            let rel = if sim >= 0.5 && autores != RelAutores::Diferentes {
                Relacao::MesmaEdicao
            } else {
                contra.push("ISBN sem corroboração suficiente".into());
                Relacao::Incerta
            };
            return Veredito::novo(rel, sim, ev, contra);
        }
        contra.push("ISBN sem corroboração por título/autor".into());
        return Veredito::novo(Relacao::Incerta, sim, ev, contra);
    }

    // 4) Sem ISBN em comum: título + autores + dados da edição.
    if sim < 0.6 {
        if idiomas_diferem && autores == RelAutores::Iguais && sim >= 0.4 {
            ev.push("autores iguais e título parecido em outro idioma".into());
            return Veredito::novo(Relacao::Incerta, sim, ev, contra);
        }
        contra.push("títulos diferentes".into());
        return Veredito::novo(Relacao::Distintos, sim, ev, contra);
    }
    match autores {
        RelAutores::Diferentes => {
            contra.push("autores diferentes".into());
            return Veredito::novo(Relacao::Distintos, sim, ev, contra);
        }
        RelAutores::Desconhecido => {
            contra.push("autor desconhecido: título sozinho não basta".into());
            let rel = if sim >= 0.8 {
                Relacao::Incerta
            } else {
                Relacao::Distintos
            };
            return Veredito::novo(rel, sim, ev, contra);
        }
        RelAutores::Parciais => {
            contra.push("autores só em parte coincidentes (ou sufixo ambíguo)".into());
            let rel = if sim >= 0.8 {
                Relacao::Incerta
            } else {
                Relacao::Distintos
            };
            return Veredito::novo(rel, sim, ev, contra);
        }
        RelAutores::Compativeis => {
            ev.push("título compatível".into());
            contra.push("autores compatíveis só por iniciais".into());
            return Veredito::novo(Relacao::Incerta, sim, ev, contra);
        }
        RelAutores::Iguais => ev.push("autores iguais".into()),
    }
    if sim < 0.9 {
        contra.push("título apenas parecido".into());
        return Veredito::novo(Relacao::Incerta, sim, ev, contra);
    }
    ev.push("mesmo título normalizado".into());

    // Diferenças de edição.
    let mut diferencas: Vec<String> = Vec::new();
    if idiomas_diferem {
        diferencas.push(format!(
            "idiomas diferentes ({} x {})",
            la.clone().unwrap_or_default(),
            lb.clone().unwrap_or_default()
        ));
    }
    let nn = |s: &Option<String>| s.as_deref().map(normalizar_texto).filter(|x| !x.is_empty());
    let (ea, eb) = (nn(&a.editora), nn(&b.editora));
    if let (Some(x), Some(y)) = (&ea, &eb)
        && x != y
    {
        diferencas.push("editoras diferentes".into());
    }
    if let (Some(x), Some(y)) = (a.ano, b.ano)
        && x != y
    {
        diferencas.push(format!("anos diferentes ({x} x {y})"));
    }
    let (na, nb) = (
        a.edicao.as_deref().and_then(numero_edicao),
        b.edicao.as_deref().and_then(numero_edicao),
    );
    if let (Some(x), Some(y)) = (na, nb)
        && x != y
    {
        diferencas.push(format!("edições diferentes ({x}ª x {y}ª)"));
    }
    if !a.tradutores.is_empty() && !b.tradutores.is_empty() {
        let ta: HashSet<String> = a
            .tradutores
            .iter()
            .map(|t| normalizar_autor(t).chave())
            .collect();
        let tb: HashSet<String> = b
            .tradutores
            .iter()
            .map(|t| normalizar_autor(t).chave())
            .collect();
        if ta != tb {
            diferencas.push("tradutores diferentes".into());
        }
    }
    if let (Some(x), Some(y)) = (a.paginas, b.paginas)
        && !idiomas_diferem
        && !paginas_aproximadas(x, y)
    {
        diferencas.push(format!("páginas muito diferentes ({x} x {y})"));
    }
    if !diferencas.is_empty() {
        contra.extend(diferencas);
        let rel = if teto_incerta {
            Relacao::Incerta
        } else {
            Relacao::MesmaObra
        };
        return Veredito::novo(rel, sim, ev, contra);
    }

    // ISBNs válidos e distintos dos dois lados.
    if !ia.is_empty() && !ib.is_empty() {
        contra.push("ISBNs diferentes".into());
        let formatos_diferem = matches!((a.formato, b.formato), (Some(x), Some(y)) if x != y);
        let rel = if formatos_diferem || teto_incerta {
            Relacao::Incerta
        } else {
            Relacao::MesmaObra
        };
        return Veredito::novo(rel, sim, ev, contra);
    }

    // Confirmação da Edição: editora, ano e idioma conhecidos e iguais nos dois lados.
    let mut faltam: Vec<&str> = Vec::new();
    if ea.is_none() || eb.is_none() {
        faltam.push("editora");
    }
    if a.ano.is_none() || b.ano.is_none() {
        faltam.push("ano");
    }
    if la.is_none() || lb.is_none() {
        faltam.push("idioma");
    }
    if !faltam.is_empty() || teto_incerta {
        if !faltam.is_empty() {
            contra.push(format!(
                "não dá para afirmar a edição: faltam {}",
                faltam.join(", ")
            ));
        }
        return Veredito::novo(Relacao::Incerta, sim, ev, contra);
    }
    ev.push("mesma editora, ano e idioma".into());
    match (a.paginas, b.paginas) {
        (Some(_), Some(_)) => ev.push("páginas aproximadamente iguais".into()),
        _ => ev.push("páginas não comparáveis (outro formato)".into()),
    }
    Veredito::novo(Relacao::MesmaEdicao, sim, ev, contra)
}

// ---------------------------------------------------------------------------
// Modelo Obra / Edição / Formato
// ---------------------------------------------------------------------------

/// Obra: identidade abstrata (título, autores).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Obra {
    pub titulo_normalizado: String,
    pub autores_chave: Vec<String>,
    #[serde(default)]
    pub idioma_original: Option<String>,
}

/// Um arquivo de um Formato.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArquivoDoFormato {
    pub registro: String,
    #[serde(default)]
    pub hash: Option<String>,
    pub tamanho: u64,
}

/// Formato de uma Edição ("o EPUB da edição X") com seus arquivos físicos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormatoDaEdicao {
    pub formato: FormatoLivro,
    pub arquivos: Vec<ArquivoDoFormato>,
}

impl FormatoDaEdicao {
    /// Bytes que sobrariam se só ficasse uma cópia de cada conteúdo idêntico (hash e tamanho
    /// iguais). Formatos diferentes nunca se somam.
    #[must_use]
    pub fn economia_exata(&self) -> u64 {
        let mut grupos: HashMap<(&str, u64), u64> = HashMap::new();
        for a in &self.arquivos {
            if let Some(h) = &a.hash {
                *grupos.entry((h.as_str(), a.tamanho)).or_default() += 1;
            }
        }
        grupos.iter().map(|((_, t), n)| t * (n - 1)).sum()
    }
}

/// Edição: uma manifestação (editora, ano, idioma, tradução) com seus Formatos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edicao {
    #[serde(default)]
    pub editora: Option<String>,
    #[serde(default)]
    pub ano: Option<i32>,
    #[serde(default)]
    pub idioma: Option<String>,
    #[serde(default)]
    pub isbns: Vec<String>,
    #[serde(default)]
    pub tradutores: Vec<String>,
    #[serde(default)]
    pub numero_edicao: Option<u32>,
    #[serde(default)]
    pub volume: Option<String>,
    pub formatos: Vec<FormatoDaEdicao>,
}

impl Edicao {
    /// Economia que o agrupamento entrega: só cópias exatas dentro de um mesmo Formato.
    /// PDF + EPUB da mesma Edição dá ZERO.
    #[must_use]
    pub fn economia_bytes(&self) -> u64 {
        self.formatos
            .iter()
            .map(FormatoDaEdicao::economia_exata)
            .sum()
    }
}

/// Edição agrupada e os índices dos registros de entrada.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdicaoAgrupada {
    pub registros: Vec<usize>,
    pub edicao: Edicao,
}

/// Obra agrupada com suas Edições.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObraAgrupada {
    pub registros: Vec<usize>,
    pub obra: Obra,
    pub edicoes: Vec<EdicaoAgrupada>,
}

/// Par que NÃO foi agrupado e precisa de revisão.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParParaRevisao {
    pub a: usize,
    pub b: usize,
    pub veredito: Veredito,
}

/// Resultado de [`agrupar`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Agrupamento {
    pub obras: Vec<ObraAgrupada>,
    pub revisoes: Vec<ParParaRevisao>,
}

struct Dsu {
    pai: Vec<usize>,
    membros: Vec<Vec<usize>>,
}

impl Dsu {
    fn novo(n: usize) -> Self {
        Self {
            pai: (0..n).collect(),
            membros: (0..n).map(|i| vec![i]).collect(),
        }
    }
    fn raiz(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.pai[r] != r {
            r = self.pai[r];
        }
        let mut c = x;
        while self.pai[c] != r {
            let p = self.pai[c];
            self.pai[c] = r;
            c = p;
        }
        r
    }
    fn unir(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.raiz(a), self.raiz(b));
        if ra != rb {
            let m = std::mem::take(&mut self.membros[rb]);
            self.membros[ra].extend(m);
            self.pai[rb] = ra;
        }
    }
}

fn chaves_de_bloco(r: &RegistroBibliografico) -> Vec<String> {
    let mut k = Vec::new();
    for a in &r.autores {
        let n = normalizar_autor(a);
        if !n.sobrenome.is_empty() {
            k.push(format!("a:{}", n.sobrenome_final()));
        }
    }
    for i in isbns_validos(r) {
        k.push(format!("i:{i}"));
    }
    if let Some(h) = &r.hash {
        k.push(format!("h:{h}"));
    }
    if r.autores.is_empty() {
        let t = normalizar_titulo(&r.titulo);
        if let Some(p) = t.principal.split(' ').next() {
            k.push(format!("t:{p}"));
        }
    }
    k
}

/// Agrupa registros em Obras > Edições > Formatos. Blocos por sobrenome/ISBN/hash evitam o
/// quadrado em bibliotecas grandes.
#[must_use]
pub fn agrupar(registros: &[RegistroBibliografico]) -> Agrupamento {
    let n = registros.len();
    let mut blocos: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, r) in registros.iter().enumerate() {
        for k in chaves_de_bloco(r) {
            blocos.entry(k).or_default().push(i);
        }
    }
    let mut pares: Vec<(usize, usize)> = Vec::new();
    let mut vistos: HashSet<(usize, usize)> = HashSet::new();
    let mut chaves: Vec<&String> = blocos.keys().collect();
    chaves.sort();
    for k in chaves {
        let v = &blocos[k];
        for (x, &i) in v.iter().enumerate() {
            for &j in &v[x + 1..] {
                if vistos.insert((i, j)) {
                    pares.push((i, j));
                }
            }
        }
    }
    pares.sort_unstable();
    let vereditos: Vec<(usize, usize, Veredito)> = pares
        .into_iter()
        .map(|(i, j)| (i, j, comparar(&registros[i], &registros[j])))
        .collect();

    let mut ed = Dsu::novo(n);
    let mut obra = Dsu::novo(n);
    let mut revisoes: Vec<ParParaRevisao> = Vec::new();

    let conflito = |d: &mut Dsu, i: usize, j: usize| -> bool {
        let (ri, rj) = (d.raiz(i), d.raiz(j));
        if ri == rj {
            return false;
        }
        let (mi, mj) = (d.membros[ri].clone(), d.membros[rj].clone());
        mi.iter().any(|x| {
            mj.iter()
                .any(|y| comparar(&registros[*x], &registros[*y]).relacao == Relacao::Distintos)
        })
    };

    for (i, j, v) in &vereditos {
        if v.relacao == Relacao::MesmaEdicao {
            if conflito(&mut ed, *i, *j) || conflito(&mut obra, *i, *j) {
                let mut v2 = v.clone();
                v2.relacao = Relacao::Incerta;
                v2.tipo = Some(TipoDuplicidade::Incerta);
                v2.risco = Risco::Alto;
                v2.contraevidencias
                    .push("conflito com outro membro do grupo".into());
                revisoes.push(ParParaRevisao {
                    a: *i,
                    b: *j,
                    veredito: v2,
                });
            } else {
                ed.unir(*i, *j);
                obra.unir(*i, *j);
            }
        }
    }
    for (i, j, v) in &vereditos {
        match v.relacao {
            Relacao::MesmaObra => {
                if conflito(&mut obra, *i, *j) {
                    let mut v2 = v.clone();
                    v2.relacao = Relacao::Incerta;
                    v2.tipo = Some(TipoDuplicidade::Incerta);
                    v2.risco = Risco::Alto;
                    v2.contraevidencias
                        .push("conflito com outro membro do grupo".into());
                    revisoes.push(ParParaRevisao {
                        a: *i,
                        b: *j,
                        veredito: v2,
                    });
                } else {
                    obra.unir(*i, *j);
                }
            }
            Relacao::Incerta => revisoes.push(ParParaRevisao {
                a: *i,
                b: *j,
                veredito: v.clone(),
            }),
            _ => {}
        }
    }

    // Monta a saída em ordem estável.
    let mut obras: Vec<ObraAgrupada> = Vec::new();
    let mut vistas = HashSet::new();
    for i in 0..n {
        let r = obra.raiz(i);
        if !vistas.insert(r) {
            continue;
        }
        let mut membros = obra.membros[r].clone();
        membros.sort_unstable();
        let mut edicoes: Vec<EdicaoAgrupada> = Vec::new();
        let mut ed_vistas = HashSet::new();
        for &m in &membros {
            let re = ed.raiz(m);
            if !ed_vistas.insert(re) {
                continue;
            }
            let mut regs: Vec<usize> = ed.membros[re].clone();
            regs.sort_unstable();
            edicoes.push(EdicaoAgrupada {
                edicao: montar_edicao(registros, &regs),
                registros: regs,
            });
        }
        let primeiro = &registros[membros[0]];
        obras.push(ObraAgrupada {
            obra: Obra {
                titulo_normalizado: normalizar_titulo(&primeiro.titulo).principal,
                autores_chave: primeiro
                    .autores
                    .iter()
                    .map(|a| normalizar_autor(a).chave())
                    .collect(),
                idioma_original: None,
            },
            registros: membros,
            edicoes,
        });
    }
    Agrupamento { obras, revisoes }
}

fn montar_edicao(registros: &[RegistroBibliografico], idx: &[usize]) -> Edicao {
    let primeiro = |f: &dyn Fn(&RegistroBibliografico) -> Option<String>| {
        idx.iter().find_map(|i| f(&registros[*i]))
    };
    let mut isbns: Vec<String> = idx
        .iter()
        .flat_map(|i| isbns_validos(&registros[*i]))
        .collect();
    isbns.sort();
    isbns.dedup();
    let mut formatos: Vec<FormatoDaEdicao> = Vec::new();
    for i in idx {
        let r = &registros[*i];
        let f = r.formato.unwrap_or(FormatoLivro::Outro);
        let arq = ArquivoDoFormato {
            registro: r.id.clone(),
            hash: r.hash.clone(),
            tamanho: r.tamanho,
        };
        match formatos.iter_mut().find(|x| x.formato == f) {
            Some(x) => x.arquivos.push(arq),
            None => formatos.push(FormatoDaEdicao {
                formato: f,
                arquivos: vec![arq],
            }),
        }
    }
    formatos.sort_by_key(|f| f.formato);
    Edicao {
        editora: primeiro(&|r| r.editora.clone()),
        ano: idx.iter().find_map(|i| registros[*i].ano),
        idioma: primeiro(&|r| r.idioma.clone()),
        isbns,
        tradutores: idx
            .iter()
            .find(|i| !registros[**i].tradutores.is_empty())
            .map(|i| registros[*i].tradutores.clone())
            .unwrap_or_default(),
        numero_edicao: idx
            .iter()
            .find_map(|i| registros[*i].edicao.as_deref().and_then(numero_edicao)),
        volume: idx.iter().find_map(|i| {
            volume_de(
                &registros[*i],
                normalizar_titulo(&registros[*i].titulo).volume.as_ref(),
            )
        }),
        formatos,
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn r(id: &str, titulo: &str, autor: &str) -> RegistroBibliografico {
        RegistroBibliografico {
            id: id.into(),
            titulo: titulo.into(),
            autores: vec![autor.into()],
            ..Default::default()
        }
    }

    fn completo(id: &str, fmt: FormatoLivro, hash: &str) -> RegistroBibliografico {
        RegistroBibliografico {
            editora: Some("Companhia das Letras".into()),
            ano: Some(2010),
            idioma: Some("por".into()),
            formato: Some(fmt),
            hash: Some(hash.into()),
            tamanho: 1000,
            paginas: Some(300),
            ..r(id, "Dom Casmurro", "Machado de Assis")
        }
    }

    #[test]
    fn titulo_sozinho_nunca_basta() {
        let a = r("1", "Introdução", "Autor Um");
        let b = r("2", "Introdução", "Autor Dois");
        assert_eq!(comparar(&a, &b).relacao, Relacao::Distintos);
        let c = RegistroBibliografico {
            id: "3".into(),
            titulo: "Introdução".into(),
            ..Default::default()
        };
        let v = comparar(&a, &c);
        assert_eq!(v.relacao, Relacao::Incerta);
        assert_eq!(v.risco, Risco::Alto);
    }

    #[test]
    fn mesma_edicao_por_metadados_completos() {
        let a = completo("1", FormatoLivro::Pdf, "h1");
        let mut b = completo("2", FormatoLivro::Epub, "h2");
        b.paginas = None;
        let v = comparar(&a, &b);
        assert_eq!(v.relacao, Relacao::MesmaEdicao, "{v:?}");
        assert_eq!(v.tipo, Some(TipoDuplicidade::MesmaEdicao));
    }

    #[test]
    fn faltando_dados_da_edicao_e_incerta() {
        let a = completo("1", FormatoLivro::Pdf, "h1");
        let mut b = completo("2", FormatoLivro::Pdf, "h2");
        b.editora = None;
        let v = comparar(&a, &b);
        assert_eq!(v.relacao, Relacao::Incerta);
        assert!(v.contraevidencias.iter().any(|c| c.contains("editora")));
    }

    #[test]
    fn idiomas_diferentes_sao_edicoes_diferentes_da_mesma_obra() {
        // CA-B05
        let a = completo("1", FormatoLivro::Pdf, "h1");
        let mut b = completo("2", FormatoLivro::Pdf, "h2");
        b.idioma = Some("eng".into());
        let v = comparar(&a, &b);
        assert_eq!(v.relacao, Relacao::MesmaObra);
        assert_eq!(v.tipo, Some(TipoDuplicidade::MesmaObra));
    }

    #[test]
    fn traducao_e_edicao_separada_da_mesma_obra() {
        let mut a = completo("1", FormatoLivro::Pdf, "h1");
        let mut b = completo("2", FormatoLivro::Pdf, "h2");
        a.tradutores = vec!["Maria Souza".into()];
        b.tradutores = vec!["Pedro Lima".into()];
        assert_eq!(comparar(&a, &b).relacao, Relacao::MesmaObra);
        b.tradutores.clear();
        b.editora = Some("Outra Editora".into());
        assert_eq!(comparar(&a, &b).relacao, Relacao::MesmaObra);
    }

    #[test]
    fn edicao_ano_e_numero_diferentes_sao_mesma_obra() {
        let a = completo("1", FormatoLivro::Pdf, "h1");
        let mut b = completo("2", FormatoLivro::Pdf, "h2");
        b.ano = Some(2020);
        assert_eq!(comparar(&a, &b).relacao, Relacao::MesmaObra);
        let mut c = completo("3", FormatoLivro::Pdf, "h3");
        let mut d = completo("4", FormatoLivro::Pdf, "h4");
        c.edicao = Some("2ª edição".into());
        d.edicao = Some("3ª edição".into());
        assert_eq!(comparar(&c, &d).relacao, Relacao::MesmaObra);
    }

    #[test]
    fn volumes_1_e_2_nunca_agrupam() {
        let mut a = completo("1", FormatoLivro::Pdf, "h1");
        let mut b = completo("2", FormatoLivro::Pdf, "h2");
        a.titulo = "Cálculo Vol. 1".into();
        b.titulo = "Cálculo Vol. 2".into();
        let v = comparar(&a, &b);
        assert_eq!(v.relacao, Relacao::Distintos);
        assert!(v.contraevidencias[0].contains("volumes"));
        // Via campo explícito e via romano.
        a.titulo = "Cálculo".into();
        b.titulo = "Cálculo".into();
        a.volume = Some("1".into());
        b.volume = Some("II".into());
        assert_eq!(comparar(&a, &b).relacao, Relacao::Distintos);
        // Mesmo ISBN não salva volumes diferentes (ISBN errado de coleção).
        a.isbns = vec!["9788535902778".into()];
        b.isbns = vec!["9788535902778".into()];
        assert_eq!(comparar(&a, &b).relacao, Relacao::Distintos);
    }

    #[test]
    fn quadrinhos_com_numero_diferente_nunca_agrupam() {
        let mut a = r("1", "Homem-Aranha #12", "Stan Lee");
        let mut b = r("2", "Homem-Aranha #13", "Stan Lee");
        a.quadrinho = true;
        b.quadrinho = true;
        assert_eq!(comparar(&a, &b).relacao, Relacao::Distintos);
        // Número só de um lado: nunca agrupa (no máximo revisão).
        let c = RegistroBibliografico {
            quadrinho: true,
            ..r("3", "Homem-Aranha", "Stan Lee")
        };
        assert_ne!(comparar(&a, &c).relacao, Relacao::MesmaEdicao);
        assert_ne!(comparar(&a, &c).relacao, Relacao::MesmaObra);
        // Índice de série.
        let mut d = r("4", "Turma da Mônica", "Mauricio de Sousa");
        let mut e = r("5", "Turma da Mônica", "Mauricio de Sousa");
        d.serie = Some("Turma".into());
        e.serie = Some("Turma".into());
        d.indice_serie = Some(1.0);
        e.indice_serie = Some(2.0);
        assert_eq!(comparar(&d, &e).relacao, Relacao::Distintos);
        // CBZ sem marcar `quadrinho` também conta.
        let mut f = r("6", "Saga", "Brian Vaughan");
        let mut g = r("7", "Saga", "Brian Vaughan");
        f.formato = Some(FormatoLivro::Cbz);
        g.formato = Some(FormatoLivro::Cbz);
        f.volume = Some("1".into());
        assert_ne!(comparar(&f, &g).relacao, Relacao::MesmaEdicao);
    }

    #[test]
    fn isbn_validado_mesma_edicao_mas_nao_prova_isolada() {
        let mut a = r("1", "Dom Casmurro", "Machado de Assis");
        let mut b = r("2", "Dom Casmurro (edição de bolso)", "Assis, Machado de");
        a.isbns = vec!["978-85-359-0277-8".into()];
        b.isbns = vec!["8535902775".into()]; // mesmo ISBN em formato 10
        assert_eq!(comparar(&a, &b).relacao, Relacao::MesmaEdicao);
        // ISBN igual com título e autor totalmente diferentes: revisão, não agrupamento.
        let c = RegistroBibliografico {
            isbns: vec!["9788535902778".into()],
            ..r("3", "Receitas da Vovó", "Fulana de Tal")
        };
        let v = comparar(&a, &c);
        assert_eq!(v.relacao, Relacao::Incerta);
        // ISBN inválido é ignorado (CA-B09).
        let d = RegistroBibliografico {
            isbns: vec!["9788535902779".into()],
            ..r("4", "Dom Casmurro", "Machado de Assis")
        };
        let e = RegistroBibliografico {
            isbns: vec!["9788535902779".into()],
            ..r("5", "Dom Casmurro", "Machado de Assis")
        };
        assert_eq!(comparar(&d, &e).relacao, Relacao::Incerta);
    }

    #[test]
    fn mesmo_isbn_idiomas_diferentes_vai_para_revisao() {
        let mut a = completo("1", FormatoLivro::Pdf, "h1");
        let mut b = completo("2", FormatoLivro::Pdf, "h2");
        a.isbns = vec!["9788535902778".into()];
        b.isbns = vec!["9788535902778".into()];
        b.idioma = Some("eng".into());
        assert_eq!(comparar(&a, &b).relacao, Relacao::Incerta);
    }

    #[test]
    fn hash_igual_e_mesma_edicao() {
        let a = completo("1", FormatoLivro::Pdf, "hx");
        let mut b = completo("2", FormatoLivro::Pdf, "hx");
        b.titulo = "xxxx".into();
        assert_eq!(comparar(&a, &b).relacao, Relacao::MesmaEdicao);
    }

    #[test]
    fn autores_homonimos_e_sufixo_nao_agrupam() {
        let a = completo("1", FormatoLivro::Pdf, "h1");
        let mut b = completo("2", FormatoLivro::Pdf, "h2");
        b.autores = vec!["Machado de Assis Filho".into()];
        assert_eq!(comparar(&a, &b).relacao, Relacao::Incerta);
        b.autores = vec!["M. de Assis".into()];
        assert_eq!(comparar(&a, &b).relacao, Relacao::Incerta);
    }

    #[test]
    fn ca_b06_pdf_e_epub_da_mesma_edicao_um_agrupamento_zero_economia() {
        let mut pdf = completo("pdf", FormatoLivro::Pdf, "hp");
        pdf.tamanho = 50_000_000;
        let mut epub = completo("epub", FormatoLivro::Epub, "he");
        epub.tamanho = 2_000_000;
        epub.paginas = None;
        let g = agrupar(&[pdf, epub]);
        assert_eq!(g.obras.len(), 1);
        assert_eq!(g.obras[0].edicoes.len(), 1);
        let ed = &g.obras[0].edicoes[0].edicao;
        assert_eq!(ed.formatos.len(), 2, "dois Formatos preservados");
        assert_eq!(ed.economia_bytes(), 0);
        assert!(g.revisoes.is_empty());
    }

    #[test]
    fn copias_exatas_no_mesmo_formato_geram_economia() {
        let a = completo("a", FormatoLivro::Pdf, "hx");
        let b = completo("b", FormatoLivro::Pdf, "hx");
        let c = completo("c", FormatoLivro::Epub, "he");
        let g = agrupar(&[a, b, c]);
        assert_eq!(g.obras[0].edicoes.len(), 1);
        assert_eq!(g.obras[0].edicoes[0].edicao.economia_bytes(), 1000);
    }

    #[test]
    fn traducao_vira_edicao_separada_na_mesma_obra() {
        // CA-B05 / WOW-05
        let pt = completo("pt", FormatoLivro::Pdf, "h1");
        let mut en = completo("en", FormatoLivro::Epub, "h2");
        en.idioma = Some("eng".into());
        en.editora = Some("Penguin".into());
        let mut pt2 = completo("pt2", FormatoLivro::Mobi, "h3");
        pt2.paginas = None;
        let g = agrupar(&[pt, en, pt2]);
        assert_eq!(g.obras.len(), 1);
        assert_eq!(g.obras[0].edicoes.len(), 2);
        let tamanhos: Vec<usize> = g.obras[0]
            .edicoes
            .iter()
            .map(|e| e.registros.len())
            .collect();
        assert!(tamanhos.contains(&2) && tamanhos.contains(&1));
    }

    #[test]
    fn volumes_nao_sao_agrupados_nem_pela_obra() {
        let mut v1 = completo("1", FormatoLivro::Pdf, "h1");
        let mut v2 = completo("2", FormatoLivro::Pdf, "h2");
        v1.titulo = "Cálculo Vol. 1".into();
        v2.titulo = "Cálculo Vol. 2".into();
        v1.autores = vec!["James Stewart".into()];
        v2.autores = vec!["James Stewart".into()];
        let g = agrupar(&[v1, v2]);
        assert_eq!(g.obras.len(), 2);
    }

    #[test]
    fn transitividade_nao_une_distintos() {
        // A ~ B (mesma edição), B ~ C (mesma edição), mas A e C são volumes diferentes.
        let mut a = completo("a", FormatoLivro::Pdf, "h1");
        a.titulo = "Série Vol. 1".into();
        let mut b = completo("b", FormatoLivro::Pdf, "h2");
        b.titulo = "Série".into();
        let mut c = completo("c", FormatoLivro::Pdf, "h3");
        c.titulo = "Série Vol. 2".into();
        for x in [&mut a, &mut b, &mut c] {
            x.autores = vec!["Autor Z".into()];
        }
        let g = agrupar(&[a, b, c]);
        assert!(g.obras.len() >= 2, "{g:?}");
        assert!(
            !g.obras
                .iter()
                .any(|o| o.registros.contains(&0) && o.registros.contains(&2))
        );
    }

    #[test]
    fn agrupar_e_deterministico_e_idempotente() {
        let regs = vec![
            completo("1", FormatoLivro::Pdf, "h1"),
            completo("2", FormatoLivro::Epub, "h2"),
            r("3", "Outro Livro", "Fulano"),
        ];
        let a = agrupar(&regs);
        let b = agrupar(&regs);
        assert_eq!(a, b);
        // Reprocessar 3 vezes o mesmo arquivo não duplica (CA-B11).
        let triplo = vec![regs[0].clone(), regs[0].clone(), regs[0].clone()];
        let g = agrupar(&triplo);
        assert_eq!(g.obras.len(), 1);
        assert_eq!(g.obras[0].edicoes.len(), 1);
    }

    #[test]
    fn titulos_parecidos_com_mesmo_autor_vao_para_revisao() {
        let a = r("1", "Fundamentos de Física", "Halliday");
        let b = r("2", "Fundamentos de Física Moderna", "Halliday");
        let v = comparar(&a, &b);
        assert!(matches!(v.relacao, Relacao::Incerta | Relacao::Distintos));
        assert_ne!(v.relacao, Relacao::MesmaEdicao);
    }
}
