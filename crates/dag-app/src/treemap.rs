//! Layout *squarified* (Bruls, Huizing e van Wijk) para o Treemap. Puro: trabalha com `f64` e
//! não conhece egui.
//!
//! Garantias (testadas): os retângulos preenchem a área exatamente (sem lacunas nem
//! sobreposição), cada área é proporcional ao valor, a saída segue a ordem da entrada e a
//! ordenação interna é estável (valores iguais mantêm a ordem original). Itens de valor zero
//! recebem retângulo vazio. Blocos pequenos demais para desenhar são agrupados em "outros" por
//! [`agrupar_pequenos`].

/// Retângulo em coordenadas de tela (origem no canto superior esquerdo).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Ret {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Ret {
    #[must_use]
    pub const fn novo(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    #[must_use]
    pub fn area(&self) -> f64 {
        self.w * self.h
    }

    #[must_use]
    pub fn contem(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    fn vazio(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
}

/// Pior razão de aspecto de uma fileira de áreas `areas` sobre um lado de comprimento `lado`.
fn pior_razao(areas: &[f64], lado: f64) -> f64 {
    let soma: f64 = areas.iter().sum();
    if soma <= 0.0 || lado <= 0.0 {
        return f64::INFINITY;
    }
    let (mut mn, mut mx) = (f64::INFINITY, 0.0_f64);
    for &a in areas {
        mn = mn.min(a);
        mx = mx.max(a);
    }
    let l2 = lado * lado;
    let s2 = soma * soma;
    (l2 * mx / s2).max(s2 / (l2 * mn))
}

/// Distribui os retângulos de `valores` (qualquer ordem) dentro de `area`.
///
/// O resultado tem o mesmo tamanho e a mesma ordem de `valores`.
#[must_use]
pub fn squarify(valores: &[u64], area: Ret) -> Vec<Ret> {
    let mut saida = vec![
        Ret {
            x: area.x,
            y: area.y,
            w: 0.0,
            h: 0.0
        };
        valores.len()
    ];
    let total: f64 = valores.iter().map(|&v| v as f64).sum();
    if total <= 0.0 || area.vazio() {
        return saida;
    }
    // Ordem de desenho: maior primeiro; empate pela posição original (estável).
    let mut ordem: Vec<usize> = (0..valores.len()).filter(|&i| valores[i] > 0).collect();
    ordem.sort_by(|&a, &b| valores[b].cmp(&valores[a]).then(a.cmp(&b)));
    let escala = area.area() / total;
    let areas: Vec<f64> = ordem.iter().map(|&i| valores[i] as f64 * escala).collect();

    let mut resto = area;
    let mut inicio = 0;
    while inicio < ordem.len() {
        let lado = resto.w.min(resto.h);
        let mut fim = inicio + 1;
        let mut atual = pior_razao(&areas[inicio..fim], lado);
        while fim < ordem.len() {
            let candidata = pior_razao(&areas[inicio..=fim], lado);
            if candidata <= atual {
                atual = candidata;
                fim += 1;
            } else {
                break;
            }
        }
        let ultima = fim == ordem.len();
        resto = colocar_fileira(
            &areas[inicio..fim],
            &ordem[inicio..fim],
            resto,
            ultima,
            &mut saida,
        );
        inicio = fim;
    }
    saida
}

/// Posiciona uma fileira ao longo do lado menor de `resto` e devolve o que sobra.
/// Na última fileira a espessura é o que restou (fecha o preenchimento sem erro acumulado).
fn colocar_fileira(
    areas: &[f64],
    indices: &[usize],
    resto: Ret,
    ultima: bool,
    saida: &mut [Ret],
) -> Ret {
    let soma: f64 = areas.iter().sum();
    if resto.w >= resto.h {
        // Faixa vertical à esquerda; itens empilhados de cima para baixo.
        let espessura = if ultima { resto.w } else { soma / resto.h };
        let mut y = resto.y;
        for (k, (&a, &i)) in areas.iter().zip(indices).enumerate() {
            let fim_k = k + 1 == areas.len();
            let h = if fim_k {
                resto.y + resto.h - y
            } else {
                a / espessura
            };
            saida[i] = Ret::novo(resto.x, y, espessura, h);
            y += h;
        }
        Ret::novo(
            resto.x + espessura,
            resto.y,
            (resto.w - espessura).max(0.0),
            resto.h,
        )
    } else {
        // Faixa horizontal no topo; itens lado a lado.
        let espessura = if ultima { resto.h } else { soma / resto.w };
        let mut x = resto.x;
        for (k, (&a, &i)) in areas.iter().zip(indices).enumerate() {
            let fim_k = k + 1 == areas.len();
            let w = if fim_k {
                resto.x + resto.w - x
            } else {
                a / espessura
            };
            saida[i] = Ret::novo(x, resto.y, w, espessura);
            x += w;
        }
        Ret::novo(
            resto.x,
            resto.y + espessura,
            resto.w,
            (resto.h - espessura).max(0.0),
        )
    }
}

/// Resultado de [`agrupar_pequenos`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agrupamento {
    /// Índices (em ordem) dos itens que ganham bloco próprio.
    pub visiveis: Vec<usize>,
    /// Índices dos itens pequenos demais, que entram no bloco "outros".
    pub agrupados: Vec<usize>,
    /// Soma dos bytes dos agrupados.
    pub bytes_agrupados: u64,
}

/// Separa os itens cujo bloco ocuparia menos de `area_minima` pixels quadrados quando `valores`
/// (mais `bytes_extras` já presentes em "outros") preenchem `area`. Itens de valor zero somem.
#[must_use]
pub fn agrupar_pequenos(
    valores: &[u64],
    bytes_extras: u64,
    area: Ret,
    area_minima: f64,
) -> Agrupamento {
    let total: f64 = valores.iter().map(|&v| v as f64).sum::<f64>() + bytes_extras as f64;
    let mut r = Agrupamento {
        visiveis: Vec::new(),
        agrupados: Vec::new(),
        bytes_agrupados: 0,
    };
    for (i, &v) in valores.iter().enumerate() {
        if v == 0 {
            continue;
        }
        let a = if total > 0.0 {
            v as f64 / total * area.area()
        } else {
            0.0
        };
        if a < area_minima {
            r.agrupados.push(i);
            r.bytes_agrupados += v;
        } else {
            r.visiveis.push(i);
        }
    }
    r
}

#[cfg(test)]
mod testes {
    use super::*;

    const AREA: Ret = Ret::novo(10.0, 20.0, 400.0, 300.0);

    fn soma_areas(rs: &[Ret]) -> f64 {
        rs.iter().map(Ret::area).sum()
    }

    fn sobrepoe(a: &Ret, b: &Ret) -> bool {
        const E: f64 = 1e-6;
        a.x + a.w > b.x + E && b.x + b.w > a.x + E && a.y + a.h > b.y + E && b.y + b.h > a.y + E
    }

    fn confere(valores: &[u64], area: Ret) -> Vec<Ret> {
        let rs = squarify(valores, area);
        assert_eq!(rs.len(), valores.len());
        let total: u64 = valores.iter().sum();
        if total > 0 {
            assert!(
                (soma_areas(&rs) - area.area()).abs() < 1e-6 * area.area(),
                "preenche a área: {} vs {}",
                soma_areas(&rs),
                area.area()
            );
            for (r, &v) in rs.iter().zip(valores) {
                let esperado = v as f64 / total as f64 * area.area();
                assert!(
                    (r.area() - esperado).abs() < 1e-6 * area.area(),
                    "proporcional"
                );
                if v > 0 {
                    assert!(r.x >= area.x - 1e-9 && r.y >= area.y - 1e-9);
                    assert!(r.x + r.w <= area.x + area.w + 1e-6);
                    assert!(r.y + r.h <= area.y + area.h + 1e-6);
                }
            }
            for i in 0..rs.len() {
                for j in (i + 1)..rs.len() {
                    assert!(!sobrepoe(&rs[i], &rs[j]), "{i} x {j} sobrepõem");
                }
            }
        }
        rs
    }

    #[test]
    fn preenche_a_area_exatamente_e_e_proporcional() {
        confere(&[600, 600, 400, 300, 200, 100, 100], AREA);
        confere(&[1], AREA);
        confere(&[5, 5], AREA);
        confere(&[1_000_000, 3, 2, 1], AREA);
        confere(&[7; 25], Ret::novo(0.0, 0.0, 123.4, 987.6));
    }

    #[test]
    fn muitas_formas_de_entrada_nao_deixam_lacunas() {
        // Pseudo-aleatório determinístico.
        let mut s = 0x1234_5678_u64;
        for n in [2usize, 3, 7, 19, 64, 200] {
            let v: Vec<u64> = (0..n)
                .map(|_| {
                    s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    (s >> 40) % 10_000 + 1
                })
                .collect();
            confere(&v, Ret::novo(0.0, 0.0, 1000.0, 600.0));
            confere(&v, Ret::novo(5.0, 5.0, 90.0, 700.0));
        }
    }

    #[test]
    fn saida_segue_a_ordem_da_entrada_mesmo_desordenada() {
        let rs = confere(&[100, 900, 300], AREA);
        assert!(rs[1].area() > rs[2].area() && rs[2].area() > rs[0].area());
    }

    #[test]
    fn ordem_e_estavel_com_valores_iguais() {
        let a = squarify(&[50, 50, 50, 50], AREA);
        let b = squarify(&[50, 50, 50, 50], AREA);
        assert_eq!(a, b);
        // Iguais: o primeiro da entrada vem antes (mais perto da origem do layout).
        assert!(a[0].x <= a[1].x + 1e-9 && a[0].y <= a[1].y + 1e-9);
        assert!((a[0].area() - a[3].area()).abs() < 1e-6);
    }

    #[test]
    fn zero_e_entrada_vazia() {
        assert!(squarify(&[], AREA).is_empty());
        let rs = squarify(&[0, 0], AREA);
        assert!(rs.iter().all(|r| r.area() == 0.0));
        let rs = confere(&[0, 10, 0, 30], AREA);
        assert_eq!(rs[0].area(), 0.0);
        assert_eq!(rs[2].area(), 0.0);
        let vazia = squarify(&[1, 2], Ret::novo(0.0, 0.0, 0.0, 10.0));
        assert!(vazia.iter().all(|r| r.area() == 0.0));
    }

    #[test]
    fn razoes_de_aspecto_ficam_razoaveis() {
        let rs = squarify(&[400, 300, 200, 100, 90, 80, 70, 60, 50, 40], AREA);
        for r in rs {
            let razao = (r.w / r.h).max(r.h / r.w);
            assert!(razao < 8.0, "bloco muito esticado: {r:?}");
        }
    }

    #[test]
    fn contem_usa_intervalo_semiaberto() {
        let r = Ret::novo(0.0, 0.0, 10.0, 10.0);
        assert!(r.contem(0.0, 0.0) && r.contem(9.99, 9.99));
        assert!(!r.contem(10.0, 5.0));
    }

    #[test]
    fn itens_pequenos_sao_agrupados_em_outros() {
        // Área 400x300 = 120.000 px²; limite 100 px² = 0,083%.
        let valores = [1_000_000, 500_000, 90, 50, 0, 10];
        let g = agrupar_pequenos(&valores, 0, AREA, 100.0);
        assert_eq!(g.visiveis, [0, 1]);
        assert_eq!(g.agrupados, [2, 3, 5]);
        assert_eq!(g.bytes_agrupados, 150);
        // Bytes extras ("outros" do nível) diluem os blocos e podem empurrar mais itens.
        let g2 = agrupar_pequenos(&valores, 100_000_000, AREA, 100.0);
        assert!(g2.visiveis.len() < g.visiveis.len() || g2.agrupados.len() >= g.agrupados.len());
        // Nada some: visíveis + agrupados + zeros = todos.
        let total = g.visiveis.len() + g.agrupados.len();
        assert_eq!(total, valores.iter().filter(|&&v| v > 0).count());
    }

    #[test]
    fn layout_depois_de_agrupar_ainda_fecha_a_area() {
        let valores = [1_000_000u64, 500_000, 90, 50, 10];
        let g = agrupar_pequenos(&valores, 7, AREA, 100.0);
        let mut v: Vec<u64> = g.visiveis.iter().map(|&i| valores[i]).collect();
        v.push(g.bytes_agrupados + 7);
        confere(&v, AREA);
    }
}
