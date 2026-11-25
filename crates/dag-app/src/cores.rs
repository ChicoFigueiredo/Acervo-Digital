//! Escalas de cor do Treemap (A-11): por tipo (categoria), finalidade, idade ou profundidade.
//! Puro (sem egui): a página converte [`Rgb`] em `Color32`. Cor nunca é o único sinal: o
//! tooltip e a legenda trazem os nomes.

/// Cor sRGB de 8 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Como colorir os blocos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ModoCor {
    #[default]
    Tipo,
    Finalidade,
    Idade,
    Profundidade,
}

impl ModoCor {
    pub const TODOS: [Self; 4] = [
        Self::Tipo,
        Self::Finalidade,
        Self::Idade,
        Self::Profundidade,
    ];

    #[must_use]
    pub fn rotulo(self) -> &'static str {
        match self {
            Self::Tipo => "Tipo",
            Self::Finalidade => "Finalidade",
            Self::Idade => "Idade",
            Self::Profundidade => "Profundidade",
        }
    }
}

/// Cinza dos blocos sem informação e do grupo "outros".
pub const CINZA: Rgb = Rgb(120, 126, 133);
/// Cor do bloco "outros".
pub const OUTROS: Rgb = Rgb(86, 92, 99);

/// Categorias de formato (as mesmas gravadas em `file_types.broad_category`) e a cor de cada.
pub const CATEGORIAS: [(&str, &str, Rgb); 14] = [
    ("documento", "Documento", Rgb(66, 133, 244)),
    ("imagem", "Imagem", Rgb(52, 168, 83)),
    ("audio", "Áudio", Rgb(171, 71, 188)),
    ("video", "Vídeo", Rgb(229, 57, 53)),
    ("compactado", "Compactado", Rgb(251, 140, 0)),
    ("executavel", "Executável", Rgb(0, 150, 136)),
    ("codigo", "Código", Rgb(121, 85, 72)),
    ("fonte", "Fonte", Rgb(158, 157, 36)),
    ("livro", "Livro", Rgb(236, 64, 122)),
    ("imagem_de_disco", "Imagem de disco", Rgb(94, 53, 177)),
    ("banco_de_dados", "Banco de dados", Rgb(0, 131, 143)),
    ("sistema", "Sistema", Rgb(84, 110, 122)),
    ("outro", "Outro", Rgb(141, 110, 99)),
    ("desconhecido", "Desconhecido", Rgb(144, 164, 174)),
];

/// Finalidades (A-08, nomes gravados em `classifications.category`) e a cor de cada.
pub const FINALIDADES: [(&str, &str, Rgb); 6] = [
    ("sistema", "Sistema", Rgb(84, 110, 122)),
    ("aplicativo", "Aplicativo", Rgb(0, 150, 136)),
    ("usuario", "Usuário", Rgb(66, 133, 244)),
    ("compartilhado", "Compartilhado", Rgb(251, 140, 0)),
    ("misto", "Misto", Rgb(171, 71, 188)),
    ("desconhecido", "Desconhecido", Rgb(144, 164, 174)),
];

/// Faixas de idade: limite superior em dias (exclusivo) e rótulo. A última não tem limite.
pub const FAIXAS_IDADE: [(u64, &str); 5] = [
    (30, "menos de 30 dias"),
    (365, "menos de 1 ano"),
    (3 * 365, "1 a 3 anos"),
    (5 * 365, "3 a 5 anos"),
    (u64::MAX, "5 anos ou mais"),
];

/// Cores das faixas de idade (do mais novo, azul claro, ao mais velho, vermelho escuro).
pub const CORES_IDADE: [Rgb; 5] = [
    Rgb(129, 212, 250),
    Rgb(77, 182, 172),
    Rgb(255, 213, 79),
    Rgb(255, 138, 101),
    Rgb(183, 28, 28),
];

/// Quantos níveis de profundidade têm cor própria (os mais fundos repetem a última).
pub const NIVEIS_PROFUNDIDADE: u32 = 8;

fn buscar(tabela: &[(&str, &str, Rgb)], chave: Option<&str>) -> Rgb {
    chave
        .and_then(|k| tabela.iter().find(|(c, _, _)| *c == k))
        .map_or(CINZA, |(_, _, cor)| *cor)
}

/// Cor da categoria de formato; categoria ausente ou desconhecida ao catálogo é cinza.
#[must_use]
pub fn cor_da_categoria(categoria: Option<&str>) -> Rgb {
    buscar(&CATEGORIAS, categoria)
}

/// Cor da finalidade.
#[must_use]
pub fn cor_da_finalidade(finalidade: Option<&str>) -> Rgb {
    buscar(&FINALIDADES, finalidade)
}

/// Índice da faixa de idade, ou `None` sem data.
#[must_use]
pub fn faixa_de_idade(dias: Option<u64>) -> Option<usize> {
    let d = dias?;
    FAIXAS_IDADE.iter().position(|(lim, _)| d < *lim)
}

/// Cor pela idade (cinza sem data).
#[must_use]
pub fn cor_da_idade(dias: Option<u64>) -> Rgb {
    faixa_de_idade(dias).map_or(CINZA, |i| CORES_IDADE[i])
}

/// Rampa do azul-claro ao roxo-escuro conforme a profundidade (0 = raiz).
#[must_use]
pub fn cor_da_profundidade(nivel: u32) -> Rgb {
    let t = f64::from(nivel.min(NIVEIS_PROFUNDIDADE)) / f64::from(NIVEIS_PROFUNDIDADE);
    let mistura = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
    // De (144, 202, 249) a (74, 20, 140).
    Rgb(mistura(144, 74), mistura(202, 20), mistura(249, 140))
}

/// Dados de um bloco que influenciam a cor.
#[derive(Debug, Clone, Copy)]
pub struct AtributosCor<'a> {
    pub categoria: Option<&'a str>,
    pub finalidade: Option<&'a str>,
    pub idade_dias: Option<u64>,
    pub profundidade: u32,
}

/// Cor do bloco no modo escolhido.
#[must_use]
pub fn cor_do_bloco(modo: ModoCor, a: &AtributosCor<'_>) -> Rgb {
    match modo {
        ModoCor::Tipo => cor_da_categoria(a.categoria),
        ModoCor::Finalidade => cor_da_finalidade(a.finalidade),
        ModoCor::Idade => cor_da_idade(a.idade_dias),
        ModoCor::Profundidade => cor_da_profundidade(a.profundidade),
    }
}

/// Itens da legenda do modo: rótulo e cor. Em `Tipo` e `Finalidade` lista tudo o que existe;
/// a página pode filtrar pelos presentes.
#[must_use]
pub fn legenda(modo: ModoCor) -> Vec<(String, Rgb)> {
    match modo {
        ModoCor::Tipo => CATEGORIAS
            .iter()
            .map(|(_, r, c)| ((*r).to_string(), *c))
            .collect(),
        ModoCor::Finalidade => FINALIDADES
            .iter()
            .map(|(_, r, c)| ((*r).to_string(), *c))
            .collect(),
        ModoCor::Idade => FAIXAS_IDADE
            .iter()
            .zip(CORES_IDADE)
            .map(|((_, r), c)| ((*r).to_string(), c))
            .chain(std::iter::once(("sem data".to_string(), CINZA)))
            .collect(),
        ModoCor::Profundidade => [0, 2, 4, 6, NIVEIS_PROFUNDIDADE]
            .iter()
            .map(|n| {
                let rotulo = if *n == NIVEIS_PROFUNDIDADE {
                    format!("nível {n}+")
                } else {
                    format!("nível {n}")
                };
                (rotulo, cor_da_profundidade(*n))
            })
            .collect(),
    }
}

/// Rótulo legível da categoria (`imagem_de_disco` → `Imagem de disco`).
#[must_use]
pub fn rotulo_da_categoria(categoria: &str) -> String {
    CATEGORIAS
        .iter()
        .find(|(c, _, _)| *c == categoria)
        .map_or_else(|| categoria.to_string(), |(_, r, _)| (*r).to_string())
}

/// Rótulo legível da finalidade.
#[must_use]
pub fn rotulo_da_finalidade(finalidade: &str) -> String {
    FINALIDADES
        .iter()
        .find(|(c, _, _)| *c == finalidade)
        .map_or_else(|| finalidade.to_string(), |(_, r, _)| (*r).to_string())
}

/// Cor do texto (preto ou branco) com contraste suficiente sobre `fundo`.
#[must_use]
pub fn cor_do_texto(fundo: Rgb) -> Rgb {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lin(fundo.0) + 0.7152 * lin(fundo.1) + 0.0722 * lin(fundo.2);
    if l > 0.30 {
        Rgb(20, 24, 28)
    } else {
        Rgb(245, 247, 250)
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn categorias_conhecidas_tem_cor_propria_e_desconhecidas_cinza() {
        assert_ne!(cor_da_categoria(Some("imagem")), CINZA);
        assert_ne!(
            cor_da_categoria(Some("imagem")),
            cor_da_categoria(Some("video"))
        );
        assert_eq!(cor_da_categoria(None), CINZA);
        assert_eq!(cor_da_categoria(Some("nao_existe")), CINZA);
    }

    #[test]
    fn categorias_da_tabela_cobrem_o_core_e_cores_sao_distintas() {
        let mut cores: Vec<Rgb> = CATEGORIAS.iter().map(|c| c.2).collect();
        let n = cores.len();
        cores.sort_by_key(|c| (c.0, c.1, c.2));
        cores.dedup();
        assert_eq!(cores.len(), n, "cada categoria tem cor distinta");
        let nomes: Vec<&str> = CATEGORIAS.iter().map(|c| c.0).collect();
        for c in [
            "documento",
            "imagem",
            "audio",
            "video",
            "compactado",
            "codigo",
        ] {
            assert!(nomes.contains(&c));
        }
    }

    #[test]
    fn faixas_de_idade() {
        assert_eq!(faixa_de_idade(Some(0)), Some(0));
        assert_eq!(faixa_de_idade(Some(29)), Some(0));
        assert_eq!(faixa_de_idade(Some(30)), Some(1));
        assert_eq!(faixa_de_idade(Some(400)), Some(2));
        assert_eq!(faixa_de_idade(Some(1500)), Some(3));
        assert_eq!(faixa_de_idade(Some(2000)), Some(4));
        assert_eq!(faixa_de_idade(Some(100_000)), Some(4));
        assert_eq!(faixa_de_idade(None), None);
        assert_eq!(cor_da_idade(None), CINZA);
        assert_eq!(cor_da_idade(Some(1)), CORES_IDADE[0]);
    }

    #[test]
    fn profundidade_e_rampa_monotona_e_limitada() {
        assert_eq!(cor_da_profundidade(0), Rgb(144, 202, 249));
        assert_eq!(cor_da_profundidade(8), Rgb(74, 20, 140));
        assert_eq!(cor_da_profundidade(50), cor_da_profundidade(8));
        let a = cor_da_profundidade(1);
        let b = cor_da_profundidade(5);
        assert!(a.0 > b.0 && a.1 > b.1, "escurece com a profundidade");
    }

    #[test]
    fn cor_do_bloco_segue_o_modo() {
        let a = AtributosCor {
            categoria: Some("video"),
            finalidade: Some("usuario"),
            idade_dias: Some(10),
            profundidade: 3,
        };
        assert_eq!(
            cor_do_bloco(ModoCor::Tipo, &a),
            cor_da_categoria(Some("video"))
        );
        assert_eq!(
            cor_do_bloco(ModoCor::Finalidade, &a),
            cor_da_finalidade(Some("usuario"))
        );
        assert_eq!(cor_do_bloco(ModoCor::Idade, &a), CORES_IDADE[0]);
        assert_eq!(
            cor_do_bloco(ModoCor::Profundidade, &a),
            cor_da_profundidade(3)
        );
    }

    #[test]
    fn legenda_tem_itens_por_modo() {
        assert_eq!(legenda(ModoCor::Tipo).len(), 14);
        assert_eq!(legenda(ModoCor::Finalidade).len(), 6);
        assert_eq!(legenda(ModoCor::Idade).len(), 6);
        assert!(legenda(ModoCor::Idade).last().unwrap().0 == "sem data");
        assert_eq!(legenda(ModoCor::Profundidade).len(), 5);
        assert_eq!(rotulo_da_categoria("imagem_de_disco"), "Imagem de disco");
        assert_eq!(rotulo_da_finalidade("usuario"), "Usuário");
    }

    #[test]
    fn texto_contrasta_com_fundo() {
        assert_eq!(cor_do_texto(Rgb(255, 255, 255)), Rgb(20, 24, 28));
        assert_eq!(cor_do_texto(Rgb(10, 10, 10)), Rgb(245, 247, 250));
    }
}
