//! Taxonomia inicial de assuntos (B-20): dicionário de palavras-chave pt/en usado só como
//! evidência auxiliar. Todo assunto é mapeado para tags que EXISTEM no vocabulário do usuário;
//! se não existir, vira proposta (nunca é aplicado).
//!
//! A hierarquia segue o que se observou na biblioteca real: Matemática > Cálculo >
//! Integral/Derivada, Álgebra, Álgebra Linear, Geometria, Geometria Analítica, Trigonometria,
//! Probabilidade, Estatística, Equações Diferenciais; níveis escolares são tags; Xadrez tem
//! subassuntos próprios. Sinônimos de tag (HQ/Gibi/História em Quadrinhos) são todos mapeados,
//! sem unificar nada.

use std::sync::OnceLock;

use crate::vocabulario::{PropostaTag, Vocabulario, tokens_chave};

/// Tipo do assunto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipoAssunto {
    Assunto,
    /// Nível escolar (Ensino Médio...): é tag, não assunto.
    NivelEscolar,
}

/// Entrada do dicionário.
#[derive(Debug, Clone, Copy)]
pub struct Assunto {
    pub nome: &'static str,
    pub pai: Option<&'static str>,
    pub tipo: TipoAssunto,
    /// Só vale se o pai também for detectado (ex.: "estratégia" só conta em xadrez).
    pub requer_pai: bool,
    /// Outros nomes de tag que o usuário pode usar para o mesmo assunto.
    pub sinonimos: &'static [&'static str],
    /// Palavras-chave pt/en separadas por `|` (sem acento não é preciso: são normalizadas).
    pub palavras: &'static str,
}

const fn a(
    nome: &'static str,
    pai: Option<&'static str>,
    sinonimos: &'static [&'static str],
    palavras: &'static str,
) -> Assunto {
    Assunto {
        nome,
        pai,
        tipo: TipoAssunto::Assunto,
        requer_pai: false,
        sinonimos,
        palavras,
    }
}

const fn filho(
    nome: &'static str,
    pai: &'static str,
    sinonimos: &'static [&'static str],
    palavras: &'static str,
) -> Assunto {
    Assunto {
        nome,
        pai: Some(pai),
        tipo: TipoAssunto::Assunto,
        requer_pai: true,
        sinonimos,
        palavras,
    }
}

const fn nivel(nome: &'static str, palavras: &'static str) -> Assunto {
    Assunto {
        nome,
        pai: None,
        tipo: TipoAssunto::NivelEscolar,
        requer_pai: false,
        sinonimos: &[],
        palavras,
    }
}

static ASSUNTOS: &[Assunto] = &[
    // Matemática
    a(
        "Matemática",
        None,
        &["Matemática"],
        "matemática|mathematics|math|teorema|theorem|demonstração|proof|exercícios resolvidos",
    ),
    a(
        "Cálculo",
        Some("Matemática"),
        &["Calculus"],
        "cálculo|calculus|limite|limit|continuidade|série de taylor|taylor series|cálculo diferencial|cálculo integral",
    ),
    a(
        "Integral",
        Some("Cálculo"),
        &["Integrais"],
        "integral|integração|integration|antiderivada|integral definida",
    ),
    a(
        "Derivada",
        Some("Cálculo"),
        &["Derivadas"],
        "derivada|derivative|derivação|regra da cadeia|differentiation",
    ),
    a(
        "Álgebra",
        Some("Matemática"),
        &["Algebra"],
        "álgebra|algebra|polinômio|polynomial|anéis|ring theory|álgebra abstrata|abstract algebra|grupos",
    ),
    a(
        "Álgebra Linear",
        Some("Matemática"),
        &["Linear Algebra"],
        "álgebra linear|linear algebra|matriz|matrix|autovalor|autovetor|eigenvalue|espaço vetorial|vector space",
    ),
    a(
        "Geometria",
        Some("Matemática"),
        &["Geometry"],
        "geometria|geometry|triângulo|círculo|euclides|euclidiana|polígono",
    ),
    a(
        "Geometria Analítica",
        Some("Matemática"),
        &[],
        "geometria analítica|analytic geometry|coordenadas cartesianas|cônicas",
    ),
    a(
        "Trigonometria",
        Some("Matemática"),
        &[],
        "trigonometria|trigonometry|seno|cosseno|tangente|sine|cosine",
    ),
    a(
        "Probabilidade",
        Some("Matemática"),
        &["Probability"],
        "probabilidade|probability|variável aleatória|random variable|markov",
    ),
    a(
        "Estatística",
        Some("Matemática"),
        &["Statistics"],
        "estatística|statistics|regressão|regression|inferência|hipótese|amostragem|desvio padrão|distribuição normal|anova",
    ),
    a(
        "Equações Diferenciais",
        Some("Matemática"),
        &["EDO", "Equação Diferencial", "Differential Equations"],
        "equação diferencial|equações diferenciais|differential equations|edo|ode|edp|pde",
    ),
    a(
        "Análise",
        Some("Matemática"),
        &["Analysis"],
        "análise real|análise matemática|real analysis|análise complexa|complex analysis|análise funcional|functional analysis|espaço métrico",
    ),
    a(
        "Topologia",
        Some("Matemática"),
        &["Topology"],
        "topologia|topology|espaço topológico|homotopia|variedade|manifold",
    ),
    a(
        "Teoria dos Números",
        Some("Matemática"),
        &["Number Theory"],
        "teoria dos números|number theory|números primos|primes|congruência",
    ),
    a(
        "Lógica",
        Some("Matemática"),
        &["Logic"],
        "lógica|logic|lógica matemática|cálculo proposicional|predicate",
    ),
    a(
        "Combinatória",
        Some("Matemática"),
        &["Combinatorics"],
        "combinatória|combinatorics|análise combinatória|permutação|grafos|graph theory",
    ),
    // Xadrez
    a(
        "Xadrez",
        None,
        &["Chess"],
        "xadrez|chess|enxadrismo|gambito|gambit|tabuleiro de xadrez",
    ),
    filho(
        "Aberturas",
        "Xadrez",
        &["Abertura", "Openings"],
        "abertura|opening|defesa siciliana|sicilian|gambito da dama|ruy lopez|defesa francesa|caro kann|defesa índia|indian defense",
    ),
    filho(
        "Meio-Jogo",
        "Xadrez",
        &["Meio Jogo", "Middlegame"],
        "meio jogo|meio-jogo|middlegame|ataque ao rei|planejamento posicional",
    ),
    filho(
        "Finais",
        "Xadrez",
        &["Endgame", "Final"],
        "finais|final de partida|endgame|finais de torre|finais de peões",
    ),
    filho(
        "Estratégia",
        "Xadrez",
        &["Strategy"],
        "estratégia|strategy|estrutura de peões|pawn structure|posicional|positional play",
    ),
    filho(
        "Tática",
        "Xadrez",
        &["Táticas", "Tactics"],
        "tática|tactics|tactical|combinações|cravada|garfo|puzzles",
    ),
    filho(
        "Partidas",
        "Xadrez",
        &["Games"],
        "partidas|melhores partidas|partidas comentadas|annotated games|torneio",
    ),
    // Literatura
    a(
        "Ficção",
        None,
        &["Fiction"],
        "ficção|fiction|conto|short stories|novela|narrativa",
    ),
    a(
        "Romance",
        Some("Ficção"),
        &["Novel"],
        "romance|novel|folhetim",
    ),
    a(
        "Ficção Científica",
        Some("Ficção"),
        &["Sci-Fi", "Science Fiction"],
        "ficção científica|science fiction|sci fi|distopia|dystopia",
    ),
    a(
        "Fantasia",
        Some("Ficção"),
        &["Fantasy"],
        "fantasia|fantasy|dragões|dragons|magia|elfos",
    ),
    a(
        "Terror",
        Some("Ficção"),
        &["Horror"],
        "terror|horror|assombração|vampiro",
    ),
    a(
        "Policial",
        Some("Ficção"),
        &["Mistério", "Mystery"],
        "policial|detetive|detective|mystery|mistério|crime",
    ),
    a(
        "Poesia",
        None,
        &["Poemas", "Poetry"],
        "poesia|poemas|poetry|poems|sonetos",
    ),
    a(
        "Quadrinhos",
        None,
        &[
            "HQ",
            "Gibi",
            "História em Quadrinhos",
            "Histórias em Quadrinhos",
            "Comics",
        ],
        "quadrinhos|gibi|hq|comic|comics|graphic novel|mangá|manga|super-herói|superhero",
    ),
    // Outras áreas
    a(
        "Física",
        None,
        &["Physics"],
        "física|physics|mecânica|termodinâmica|eletromagnetismo|quântica|quantum|relatividade",
    ),
    a(
        "Química",
        None,
        &["Chemistry"],
        "química|chemistry|orgânica|reações químicas|estequiometria",
    ),
    a(
        "Biologia",
        None,
        &["Biology"],
        "biologia|biology|genética|genetics|evolução|ecologia|célula",
    ),
    a(
        "História",
        None,
        &["History"],
        "história|history|império|guerra mundial|revolução|idade média",
    ),
    a(
        "Filosofia",
        None,
        &["Philosophy"],
        "filosofia|philosophy|ética|metafísica|epistemologia|platão|aristóteles|kant",
    ),
    a(
        "Programação",
        None,
        &["Programming"],
        "programação|programming|python|rust|java|javascript|algoritmos|algorithms|software|código|compilador|flutter",
    ),
    a(
        "R",
        Some("Programação"),
        &["Programa R", "Linguagem R", "R Language"],
        "linguagem r|programa r|r programming|ggplot|tidyverse|rstudio|cran",
    ),
    a(
        "Direito",
        None,
        &["Law"],
        "direito|law|jurídico|constitucional|civil|penal|tributário|processo",
    ),
    a(
        "Economia",
        None,
        &["Economics"],
        "economia|economics|macroeconomia|microeconomia|mercado financeiro",
    ),
    a(
        "Administração",
        None,
        &["Management"],
        "administração|management|gestão|liderança|leadership|organizações",
    ),
    a(
        "Marketing",
        None,
        &["Marketing Digital"],
        "marketing|publicidade|branding|vendas|seo",
    ),
    a(
        "Auto-Ajuda",
        None,
        &["Autoajuda", "Self-Help"],
        "autoajuda|auto ajuda|self help|motivação|hábitos|sucesso pessoal",
    ),
    a(
        "Biografia",
        None,
        &["Biografias", "Biography"],
        "biografia|biography|autobiografia|memórias|memoir|vida de",
    ),
    a(
        "Inglês",
        None,
        &["Ingles", "English"],
        "inglês|english language|gramática inglesa|vocabulário inglês|toefl|ielts",
    ),
    a(
        "Português",
        None,
        &["Portugues"],
        "gramática|língua portuguesa|ortografia|redação|português",
    ),
    a(
        "Psicologia",
        None,
        &["Psychology"],
        "psicologia|psychology|psicanálise|terapia|cognitiva",
    ),
    a(
        "Medicina",
        None,
        &["Medicine"],
        "medicina|medicine|anatomia|fisiologia|clínica|patologia",
    ),
    a(
        "Culinária",
        None,
        &["Cooking"],
        "culinária|receitas|cooking|recipes|cozinha",
    ),
    a(
        "Música",
        None,
        &["Music"],
        "música|music|partitura|harmonia|violão|piano",
    ),
    a(
        "Arte",
        None,
        &["Art"],
        "arte|pintura|painting|escultura|história da arte",
    ),
    a(
        "Religião",
        None,
        &["Religion"],
        "religião|religion|bíblia|teologia|theology",
    ),
    // Níveis escolares
    nivel(
        "Ensino Médio",
        "ensino médio|vestibular|enem|segundo grau|high school",
    ),
    nivel(
        "Ensino Fundamental",
        "ensino fundamental|primeiro grau|fundamental ii|elementary school",
    ),
];

/// Dicionário inicial completo.
#[must_use]
pub fn assuntos() -> &'static [Assunto] {
    ASSUNTOS
}

fn compilados() -> &'static Vec<Vec<Vec<String>>> {
    static C: OnceLock<Vec<Vec<Vec<String>>>> = OnceLock::new();
    C.get_or_init(|| {
        ASSUNTOS
            .iter()
            .map(|a| {
                a.palavras
                    .split('|')
                    .map(tokens_chave)
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .collect()
    })
}

fn contem(toks: &[String], frase: &[String]) -> bool {
    !frase.is_empty() && toks.windows(frase.len()).any(|w| w == frase)
}

/// Assunto detectado por palavras-chave.
#[derive(Debug, Clone, PartialEq)]
pub struct AssuntoDetectado {
    pub nome: &'static str,
    /// 0..=1.
    pub pontuacao: f32,
    pub palavras: Vec<String>,
}

/// Procura palavras-chave no título (peso dobrado) e no resto do texto.
#[must_use]
pub fn detectar(titulo: &str, texto: &str) -> Vec<AssuntoDetectado> {
    let tt = tokens_chave(titulo);
    let tx = tokens_chave(texto);
    let mut brutos: Vec<(usize, f32, Vec<String>)> = Vec::new();
    for (i, frases) in compilados().iter().enumerate() {
        let mut pontos = 0.0f32;
        let mut achadas = Vec::new();
        for f in frases {
            let mut p = 0.0;
            if contem(&tt, f) {
                p += 2.0;
            }
            if contem(&tx, f) {
                p += 1.0;
            }
            if p > 0.0 {
                pontos += p;
                achadas.push(f.join(" "));
            }
        }
        if pontos > 0.0 {
            brutos.push((i, pontos, achadas));
        }
    }
    let detectados: Vec<&str> = brutos.iter().map(|(i, _, _)| ASSUNTOS[*i].nome).collect();
    brutos
        .iter()
        .filter(|(i, _, _)| {
            let a = &ASSUNTOS[*i];
            !a.requer_pai || a.pai.is_some_and(|p| detectados.contains(&p))
        })
        .map(|(i, p, w)| AssuntoDetectado {
            nome: ASSUNTOS[*i].nome,
            pontuacao: 1.0 - (-p / 2.5).exp(),
            palavras: w.clone(),
        })
        .collect()
}

/// Tag do vocabulário que corresponde a um assunto.
#[derive(Debug, Clone, PartialEq)]
pub struct TagMapeada {
    /// Grafia homologada.
    pub tag: String,
    pub pontuacao: f32,
    pub assunto: String,
    /// Veio de um ancestral do assunto detectado (Derivada -> Cálculo -> Matemática).
    pub via_ancestral: bool,
}

/// Resultado do mapeamento para o vocabulário do usuário.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mapeamento {
    pub tags: Vec<TagMapeada>,
    /// Assuntos sem tag correspondente: só propostas.
    pub propostas: Vec<PropostaTag>,
}

fn nomes_do_assunto(nome: &str) -> Option<&'static Assunto> {
    ASSUNTOS.iter().find(|a| a.nome == nome)
}

fn mapear_nomes(
    a: &Assunto,
    pontuacao: f32,
    via_ancestral: bool,
    origem: &str,
    vocab: &Vocabulario,
    m: &mut Mapeamento,
) -> bool {
    let mut achou = false;
    for n in std::iter::once(a.nome).chain(a.sinonimos.iter().copied()) {
        if let Some(t) = vocab.resolver_tag(n)
            && !m.tags.iter().any(|x| x.tag == t)
        {
            m.tags.push(TagMapeada {
                tag: t.to_owned(),
                pontuacao,
                assunto: origem.to_owned(),
                via_ancestral,
            });
            achou = true;
        } else if vocab.resolver_tag(n).is_some() {
            achou = true;
        }
    }
    achou
}

/// Mapeia assuntos detectados para tags existentes (incluindo ancestrais, com peso menor, e
/// todos os sinônimos que o usuário tenha). O que não existir vira proposta.
#[must_use]
pub fn mapear(detectados: &[AssuntoDetectado], vocab: &Vocabulario) -> Mapeamento {
    let mut m = Mapeamento::default();
    for d in detectados {
        let Some(a) = nomes_do_assunto(d.nome) else {
            continue;
        };
        let achou = mapear_nomes(a, d.pontuacao, false, a.nome, vocab, &mut m);
        if !achou && !m.propostas.iter().any(|p| p.valor == a.nome) {
            m.propostas.push(PropostaTag {
                valor: a.nome.to_owned(),
                motivo: format!(
                    "assunto detectado por palavras-chave ({}) sem tag correspondente no vocabulário",
                    d.palavras.join(", ")
                ),
            });
        }
        let mut pai = a.pai;
        let mut fator = 0.8;
        while let Some(p) = pai.and_then(nomes_do_assunto) {
            mapear_nomes(p, d.pontuacao * fator, true, a.nome, vocab, &mut m);
            fator *= 0.8;
            pai = p.pai;
        }
    }
    m.tags
        .sort_by(|x, y| y.pontuacao.total_cmp(&x.pontuacao).then(x.tag.cmp(&y.tag)));
    m
}

/// Estante existente (valor de `#pasta`) para o assunto raiz mais forte, se houver.
#[must_use]
pub fn estante_por_assunto<'v>(
    detectados: &[AssuntoDetectado],
    vocab: &'v Vocabulario,
) -> Option<&'v str> {
    let mut raizes: Vec<&AssuntoDetectado> = detectados.iter().collect();
    raizes.sort_by(|x, y| y.pontuacao.total_cmp(&x.pontuacao));
    for d in raizes {
        let mut a = nomes_do_assunto(d.nome)?;
        while let Some(p) = a.pai.and_then(nomes_do_assunto) {
            a = p;
        }
        for n in std::iter::once(a.nome).chain(a.sinonimos.iter().copied()) {
            if let Some(e) = vocab.resolver_estante(n) {
                return Some(e);
            }
        }
    }
    None
}

#[cfg(test)]
mod testes {
    use super::*;

    fn nomes(d: &[AssuntoDetectado]) -> Vec<&str> {
        d.iter().map(|x| x.nome).collect()
    }

    #[test]
    fn matematica_hierarquia() {
        let d = detectar("Cálculo: derivadas e integrais", "");
        let n = nomes(&d);
        assert!(
            n.contains(&"Cálculo") && n.contains(&"Derivada") && n.contains(&"Integral"),
            "{n:?}"
        );
        let g = detectar("Geometria Analítica", "");
        assert!(nomes(&g).contains(&"Geometria Analítica"));
        let e = detectar("Equações Diferenciais Ordinárias", "");
        assert!(nomes(&e).contains(&"Equações Diferenciais"));
        let al = detectar("Linear Algebra Done Right", "");
        assert!(nomes(&al).contains(&"Álgebra Linear"));
    }

    #[test]
    fn xadrez_subassuntos_so_com_xadrez() {
        let d = detectar("Xadrez: estratégia e finais de torre", "");
        let n = nomes(&d);
        assert!(
            n.contains(&"Xadrez") && n.contains(&"Estratégia") && n.contains(&"Finais"),
            "{n:?}"
        );
        // "estratégia" sem xadrez não conta como subassunto de xadrez.
        let e = detectar("Estratégia Empresarial", "");
        assert!(!nomes(&e).contains(&"Estratégia"));
        let ab = detectar("Chess Openings: the Sicilian", "");
        assert!(nomes(&ab).contains(&"Aberturas"));
    }

    #[test]
    fn ficcao_quadrinhos_e_niveis() {
        assert!(nomes(&detectar("", "um romance de ficção")).contains(&"Romance"));
        assert!(nomes(&detectar("Gibi do Cascão", "")).contains(&"Quadrinhos"));
        assert!(nomes(&detectar("Matemática para o Ensino Médio", "")).contains(&"Ensino Médio"));
        assert!(nomes(&detectar("Manual de Direito Civil", "")).contains(&"Direito"));
    }

    #[test]
    fn mapeia_so_para_tags_existentes_e_propoe_o_resto() {
        let v = Vocabulario::novo(
            [
                "Matemática",
                "Cálculo",
                "HQ",
                "Gibi",
                "História em Quadrinhos",
                "!Catalogar",
            ],
            ["Matemática", "Gibi"],
        );
        let d = detectar("Cálculo: derivadas", "");
        let m = mapear(&d, &v);
        let tags: Vec<&str> = m.tags.iter().map(|t| t.tag.as_str()).collect();
        assert!(
            tags.contains(&"Cálculo") && tags.contains(&"Matemática"),
            "{tags:?}"
        );
        // "Derivada" não existe: proposta; ancestrais existentes vêm com peso menor.
        assert!(m.propostas.iter().any(|p| p.valor == "Derivada"));
        let calc = m.tags.iter().find(|t| t.tag == "Matemática").unwrap();
        assert!(calc.via_ancestral);
        // Todos os sinônimos que o usuário tem entram, sem unificar.
        let q = mapear(&detectar("Turma da Mônica - gibi", ""), &v);
        let tq: Vec<&str> = q.tags.iter().map(|t| t.tag.as_str()).collect();
        for t in ["HQ", "Gibi", "História em Quadrinhos"] {
            assert!(tq.contains(&t), "{tq:?}");
        }
        assert!(q.propostas.is_empty());
        assert!(m.tags.iter().all(|t| !t.tag.starts_with('!')));
    }

    #[test]
    fn estante_do_assunto_raiz() {
        let v = Vocabulario::novo(Vec::<String>::new(), ["Matemática", "Gibi", "Literatura"]);
        let d = detectar("Álgebra Linear", "");
        assert_eq!(estante_por_assunto(&d, &v), Some("Matemática"));
        let q = detectar("Gibi do Chico Bento", "");
        assert_eq!(estante_por_assunto(&q, &v), Some("Gibi"));
        // Estante que não existe nunca é inventada.
        let x = detectar("Xadrez de Torneio", "");
        assert_eq!(estante_por_assunto(&x, &v), None);
    }

    #[test]
    fn dicionario_tem_hierarquia_valida() {
        for a in assuntos() {
            if let Some(p) = a.pai {
                assert!(nomes_do_assunto(p).is_some(), "pai inexistente: {p}");
            }
            assert!(!a.palavras.is_empty());
        }
    }
}
