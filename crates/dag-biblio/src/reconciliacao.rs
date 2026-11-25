//! Reconciliação determinística de valores sugeridos por várias origens (B-19, parte sem IA).
//!
//! Ordem de evidência (spec): hash verificado > identificadores corroborados > conteúdo/índice/
//! capa > metadados internos > nome do arquivo. Decisão humana e metadados homologados vencem.
//! ISBN sozinho não é prova isolada. Divergência entre fontes fortes exige revisão. IA nunca
//! chega a alta confiança sem outra evidência.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::isbn;
use crate::normalizacao::{
    CompatAutor, comparar_autores_norm, normalizar_autor, normalizar_texto, normalizar_titulo,
};
use crate::tipos::{Campo, Evidencia, FaixaConfianca, OrigemEvidencia};

/// Um valor proposto por uma origem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidato {
    pub origem: OrigemEvidencia,
    #[serde(default)]
    pub valor: Option<String>,
    /// Para campos de lista (autores, tags).
    #[serde(default)]
    pub lista: Vec<String>,
    pub confianca: f32,
    #[serde(default)]
    pub evidencias: Vec<Evidencia>,
}

impl Candidato {
    #[must_use]
    pub fn texto(origem: OrigemEvidencia, valor: &str, confianca: f32) -> Self {
        Self {
            origem,
            valor: Some(valor.to_owned()),
            lista: vec![],
            confianca,
            evidencias: vec![],
        }
    }

    #[must_use]
    pub fn lista(origem: OrigemEvidencia, valores: &[&str], confianca: f32) -> Self {
        Self {
            origem,
            valor: None,
            lista: valores.iter().map(|s| (*s).to_owned()).collect(),
            confianca,
            evidencias: vec![],
        }
    }

    fn vazio(&self) -> bool {
        self.valor.as_deref().is_none_or(|v| v.trim().is_empty()) && self.lista.is_empty()
    }
}

/// Contexto da reconciliação.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextoReconciliacao {
    /// O ISBN foi corroborado por título/autor/capa de outra origem independente.
    pub isbn_corroborado: bool,
}

/// Valor final de um campo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampoReconciliado {
    pub campo: Campo,
    /// `None` = desconhecido (nunca inventar).
    pub valor: Option<String>,
    pub lista: Vec<String>,
    pub confianca: f32,
    pub faixa: FaixaConfianca,
    pub origem_vencedora: Option<OrigemEvidencia>,
    pub precisa_revisao: bool,
    /// "Precisa revisão porque...".
    pub motivos: Vec<String>,
    /// Valores divergentes (para comparação lado a lado).
    pub alternativas: Vec<Alternativa>,
    pub evidencias: Vec<Evidencia>,
}

/// Valor concorrente que não venceu.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alternativa {
    pub valor: Option<String>,
    pub lista: Vec<String>,
    pub origens: Vec<OrigemEvidencia>,
}

/// Resultado do conjunto de campos.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ResultadoReconciliacao {
    pub campos: Vec<CampoReconciliado>,
    pub precisa_revisao: bool,
    /// Resumo "campo: razão" de tudo que precisa revisão.
    pub razoes: Vec<String>,
}

/// Força da origem (maior = mais confiável).
#[must_use]
pub fn forca(o: OrigemEvidencia) -> u8 {
    match o {
        OrigemEvidencia::Humano => 100,
        OrigemEvidencia::Homologada => 90,
        OrigemEvidencia::HashVerificado => 80,
        OrigemEvidencia::Identificador => 70,
        OrigemEvidencia::FichaCatalografica => 65,
        OrigemEvidencia::Conteudo => 60,
        OrigemEvidencia::Ocr => 55,
        OrigemEvidencia::MetadadoInterno => 40,
        OrigemEvidencia::Ia => 30,
        OrigemEvidencia::NomeArquivo => 10,
    }
}

fn desconto(o: OrigemEvidencia) -> f32 {
    match o {
        OrigemEvidencia::Humano | OrigemEvidencia::Homologada | OrigemEvidencia::HashVerificado => {
            1.0
        }
        OrigemEvidencia::Identificador | OrigemEvidencia::FichaCatalografica => 0.95,
        OrigemEvidencia::Conteudo => 0.9,
        OrigemEvidencia::Ocr | OrigemEvidencia::MetadadoInterno => 0.8,
        OrigemEvidencia::Ia => 0.7,
        OrigemEvidencia::NomeArquivo => 0.5,
    }
}

fn e_forte(c: &Candidato) -> bool {
    forca(c.origem) >= 40 && forca(c.origem) < 90 && c.confianca >= 0.6
}

/// Chave de comparação do valor, por campo.
fn chave(campo: Campo, c: &Candidato) -> String {
    match campo {
        Campo::Autores | Campo::Tags => {
            let mut v: Vec<String> = c
                .lista
                .iter()
                .map(|x| {
                    if campo == Campo::Autores {
                        normalizar_autor(x).chave()
                    } else {
                        normalizar_texto(x)
                    }
                })
                .collect();
            if v.is_empty()
                && let Some(t) = &c.valor
            {
                v = t
                    .split('&')
                    .map(|x| {
                        if campo == Campo::Autores {
                            normalizar_autor(x).chave()
                        } else {
                            normalizar_texto(x)
                        }
                    })
                    .collect();
            }
            v.sort();
            v.join("|")
        }
        Campo::Titulo => normalizar_titulo(c.valor.as_deref().unwrap_or("")).principal,
        Campo::Isbn => {
            let l = isbn::limpar(c.valor.as_deref().unwrap_or(""));
            if isbn::valido_13(&l) {
                l
            } else {
                isbn::para_13(&l).unwrap_or_default()
            }
        }
        Campo::Idioma => normalizar_texto(c.valor.as_deref().unwrap_or(""))
            .chars()
            .take(2)
            .collect(),
        _ => normalizar_texto(c.valor.as_deref().unwrap_or("")),
    }
}

/// Autores com a mesma chave exata ou compatíveis por iniciais contam como o mesmo valor.
fn mesmo_valor(campo: Campo, a: &Candidato, b: &Candidato) -> bool {
    if chave(campo, a) == chave(campo, b) {
        return true;
    }
    if campo == Campo::Autores && a.lista.len() == b.lista.len() && !a.lista.is_empty() {
        return a.lista.iter().zip(&b.lista).all(|(x, y)| {
            comparar_autores_norm(&normalizar_autor(x), &normalizar_autor(y))
                >= CompatAutor::Compativel
        });
    }
    false
}

fn faixa_de(c: f32) -> FaixaConfianca {
    FaixaConfianca::de(c)
}

/// Reconcilia as propostas de UM campo.
#[must_use]
pub fn reconciliar_campo(
    campo: Campo,
    candidatos: &[Candidato],
    ctx: ContextoReconciliacao,
) -> CampoReconciliado {
    let mut motivos: Vec<String> = Vec::new();
    let mut uteis: Vec<&Candidato> = Vec::new();
    for c in candidatos {
        if c.vazio() {
            continue;
        }
        if campo == Campo::Isbn && chave(campo, c).is_empty() {
            motivos.push(format!(
                "ISBN inválido ignorado ({}): dígito verificador não confere",
                c.valor.as_deref().unwrap_or("")
            ));
            continue;
        }
        uteis.push(c);
    }
    let critico = matches!(campo, Campo::Titulo | Campo::Autores);
    if uteis.is_empty() {
        let precisa = critico || !motivos.is_empty();
        motivos.push("sem evidência: campo desconhecido".into());
        return CampoReconciliado {
            campo,
            valor: None,
            lista: vec![],
            confianca: 0.0,
            faixa: FaixaConfianca::Baixa,
            origem_vencedora: None,
            precisa_revisao: precisa,
            motivos,
            alternativas: vec![],
            evidencias: vec![],
        };
    }

    // Humano vence; depois a Homologada.
    for alvo in [OrigemEvidencia::Humano, OrigemEvidencia::Homologada] {
        if let Some(v) = uteis.iter().find(|c| c.origem == alvo) {
            let divergentes: Vec<Alternativa> = agrupar(campo, &uteis)
                .into_iter()
                .filter(|g| !mesmo_valor(campo, g.0[0], v))
                .map(|g| alternativa(&g.0))
                .collect();
            let mut m = motivos;
            if !divergentes.is_empty() {
                m.push(format!(
                    "{} prevalece sobre {} sugestão(ões) divergente(s) de outras origens",
                    if alvo == OrigemEvidencia::Humano {
                        "decisão humana"
                    } else {
                        "metadado homologado"
                    },
                    divergentes.len()
                ));
            }
            return CampoReconciliado {
                campo,
                valor: v.valor.clone(),
                lista: v.lista.clone(),
                confianca: if alvo == OrigemEvidencia::Humano {
                    1.0
                } else {
                    0.99
                },
                faixa: FaixaConfianca::Alta,
                origem_vencedora: Some(alvo),
                precisa_revisao: false,
                motivos: m,
                alternativas: divergentes,
                evidencias: v.evidencias.clone(),
            };
        }
    }

    let grupos = agrupar(campo, &uteis);
    // Confiança de cada grupo: noisy-or por origem distinta, ponderada pelo desconto da origem.
    let mut avaliados: Vec<(Vec<&Candidato>, f32, u8)> = grupos
        .into_iter()
        .map(|(g, _)| {
            let mut melhor_por_origem: BTreeMap<u8, f32> = BTreeMap::new();
            for c in &g {
                let p = (c.confianca * desconto(c.origem)).clamp(0.0, 1.0);
                let e = melhor_por_origem.entry(forca(c.origem)).or_insert(0.0);
                if p > *e {
                    *e = p;
                }
            }
            let conf = 1.0 - melhor_por_origem.values().map(|p| 1.0 - p).product::<f32>();
            let topo = g.iter().map(|c| forca(c.origem)).max().unwrap_or(0);
            (g, conf.min(0.98), topo)
        })
        .collect();
    avaliados.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.total_cmp(&a.1)));

    let fortes: Vec<usize> = avaliados
        .iter()
        .enumerate()
        .filter(|(_, (g, _, _))| g.iter().any(|c| e_forte(c)))
        .map(|(i, _)| i)
        .collect();
    let (venc, mut confianca, _) = avaliados[0].clone();
    let origem_vencedora = venc.iter().map(|c| c.origem).max_by_key(|o| forca(*o));
    let mut precisa = !motivos.is_empty();

    if fortes.len() > 1 {
        precisa = true;
        confianca = confianca.min(0.75);
        motivos.push("fontes fortes divergem entre si".into());
    }
    let so_origens = |ors: &[OrigemEvidencia]| venc.iter().all(|c| ors.contains(&c.origem));
    if so_origens(&[OrigemEvidencia::NomeArquivo]) {
        confianca = confianca.min(0.4);
        precisa = true;
        motivos.push("só o nome do arquivo sustenta o valor".into());
    } else if so_origens(&[OrigemEvidencia::Ia, OrigemEvidencia::NomeArquivo]) {
        confianca = confianca.min(0.7);
        precisa = true;
        motivos.push("só IA sustenta o valor, sem outra evidência".into());
    }
    if campo == Campo::Isbn {
        if !ctx.isbn_corroborado {
            confianca = confianca.min(0.75);
            precisa = true;
            motivos.push("ISBN sem corroboração por título/autor: não é prova isolada".into());
        }
    } else if so_origens(&[OrigemEvidencia::Identificador]) && !ctx.isbn_corroborado {
        confianca = confianca.min(0.75);
        precisa = true;
        motivos.push("valor vindo só do ISBN, sem corroboração independente".into());
    }
    if confianca < 0.6 && !motivos.iter().any(|m| m.contains("confiança baixa")) {
        precisa = true;
        motivos.push("confiança baixa".into());
    }

    let alternativas: Vec<Alternativa> = avaliados[1..]
        .iter()
        .map(|(g, _, _)| alternativa(g))
        .collect();
    let evidencias: Vec<Evidencia> = venc.iter().flat_map(|c| c.evidencias.clone()).collect();
    // Representante: o candidato mais forte (e mais confiante) do grupo vencedor.
    let rep = venc
        .iter()
        .max_by(|a, b| {
            forca(a.origem)
                .cmp(&forca(b.origem))
                .then(a.confianca.total_cmp(&b.confianca))
        })
        .copied()
        .unwrap_or(venc[0]);
    CampoReconciliado {
        campo,
        valor: rep.valor.clone(),
        lista: rep.lista.clone(),
        confianca,
        faixa: faixa_de(confianca),
        origem_vencedora,
        precisa_revisao: precisa,
        motivos,
        alternativas,
        evidencias,
    }
}

fn agrupar<'a>(campo: Campo, cs: &[&'a Candidato]) -> Vec<(Vec<&'a Candidato>, ())> {
    let mut grupos: Vec<(Vec<&Candidato>, ())> = Vec::new();
    for c in cs {
        match grupos
            .iter_mut()
            .find(|(g, ())| mesmo_valor(campo, g[0], c))
        {
            Some((g, ())) => g.push(c),
            None => grupos.push((vec![*c], ())),
        }
    }
    grupos
}

fn alternativa(g: &[&Candidato]) -> Alternativa {
    let rep = g[0];
    Alternativa {
        valor: rep.valor.clone(),
        lista: rep.lista.clone(),
        origens: g.iter().map(|c| c.origem).collect(),
    }
}

/// Reconcilia vários campos de uma vez.
#[must_use]
pub fn reconciliar(
    entrada: &[(Campo, Vec<Candidato>)],
    ctx: ContextoReconciliacao,
) -> ResultadoReconciliacao {
    let mut r = ResultadoReconciliacao::default();
    for (campo, cands) in entrada {
        let c = reconciliar_campo(*campo, cands, ctx);
        if c.precisa_revisao {
            r.precisa_revisao = true;
            for m in &c.motivos {
                r.razoes.push(format!("{:?}: {m}", c.campo));
            }
        }
        r.campos.push(c);
    }
    r
}

#[cfg(test)]
mod testes {
    use super::*;
    use OrigemEvidencia as O;

    fn t(o: O, v: &str, c: f32) -> Candidato {
        Candidato::texto(o, v, c)
    }

    fn rec(campo: Campo, cs: &[Candidato]) -> CampoReconciliado {
        reconciliar_campo(campo, cs, ContextoReconciliacao::default())
    }

    #[test]
    fn ordem_de_evidencia() {
        assert!(forca(O::HashVerificado) > forca(O::Identificador));
        assert!(forca(O::Identificador) > forca(O::Conteudo));
        assert!(forca(O::Conteudo) > forca(O::MetadadoInterno));
        assert!(forca(O::MetadadoInterno) > forca(O::NomeArquivo));
        assert!(forca(O::Humano) > forca(O::Homologada));
        assert!(forca(O::Homologada) > forca(O::HashVerificado));
    }

    #[test]
    fn humano_vence_tudo() {
        let r = rec(
            Campo::Titulo,
            &[
                t(O::Conteudo, "Cálculo", 0.95),
                t(O::Homologada, "Cálculo Vol. 1", 0.9),
                t(O::Humano, "Cálculo I", 1.0),
            ],
        );
        assert_eq!(r.valor.as_deref(), Some("Cálculo I"));
        assert_eq!(r.origem_vencedora, Some(O::Humano));
        assert!(!r.precisa_revisao);
        assert_eq!(r.faixa, FaixaConfianca::Alta);
        assert!(!r.alternativas.is_empty());
    }

    #[test]
    fn homologada_vence_sugestoes_automaticas() {
        let r = rec(
            Campo::Editora,
            &[
                t(O::Conteudo, "Atlas", 0.95),
                t(O::Ia, "Atlas S.A.", 0.9),
                t(O::Homologada, "Editora Atlas", 1.0),
            ],
        );
        assert_eq!(r.valor.as_deref(), Some("Editora Atlas"));
        assert!(!r.precisa_revisao);
    }

    #[test]
    fn fontes_fortes_divergentes_exigem_revisao() {
        let r = rec(
            Campo::Titulo,
            &[
                t(O::Conteudo, "Álgebra Linear", 0.9),
                t(O::MetadadoInterno, "Linear Algebra Notes", 0.85),
            ],
        );
        assert!(r.precisa_revisao);
        assert!(r.confianca <= 0.75);
        assert_eq!(r.valor.as_deref(), Some("Álgebra Linear"));
        assert_eq!(r.alternativas.len(), 1);
        assert!(r.motivos.iter().any(|m| m.contains("divergem")));
    }

    #[test]
    fn concordancia_de_fontes_aumenta_confianca() {
        let uma = rec(Campo::Titulo, &[t(O::MetadadoInterno, "Dom Casmurro", 0.8)]);
        let duas = rec(
            Campo::Titulo,
            &[
                t(O::MetadadoInterno, "Dom Casmurro", 0.8),
                t(O::Conteudo, "DOM CASMURRO", 0.85),
                t(O::NomeArquivo, "dom_casmurro", 0.5),
            ],
        );
        assert!(duas.confianca > uma.confianca);
        assert!(!duas.precisa_revisao);
        assert_eq!(duas.faixa, FaixaConfianca::Alta);
    }

    #[test]
    fn isbn_sozinho_nao_e_prova() {
        let c = [t(O::Identificador, "9788535902778", 0.99)];
        let r = rec(Campo::Isbn, &c);
        assert!(r.precisa_revisao);
        assert!(r.confianca <= 0.75);
        assert_ne!(r.faixa, FaixaConfianca::Alta);
        let r2 = reconciliar_campo(
            Campo::Isbn,
            &c,
            ContextoReconciliacao {
                isbn_corroborado: true,
            },
        );
        assert!(!r2.precisa_revisao);
        assert_eq!(r2.faixa, FaixaConfianca::Alta);
        // Título obtido só pelo ISBN também é limitado.
        let t1 = rec(Campo::Titulo, &[t(O::Identificador, "Dom Casmurro", 0.95)]);
        assert!(t1.precisa_revisao && t1.confianca <= 0.75);
    }

    #[test]
    fn isbn_invalido_e_ignorado_ca_b09() {
        let r = rec(Campo::Isbn, &[t(O::MetadadoInterno, "9788535902779", 0.9)]);
        assert_eq!(r.valor, None);
        assert!(r.precisa_revisao);
        assert!(r.motivos.iter().any(|m| m.contains("inválido")));
        // ISBN-10 válido é normalizado e comparado com o 13.
        let r = reconciliar_campo(
            Campo::Isbn,
            &[
                t(O::MetadadoInterno, "8535902775", 0.9),
                t(O::Conteudo, "978-85-359-0277-8", 0.9),
            ],
            ContextoReconciliacao {
                isbn_corroborado: true,
            },
        );
        assert!(!r.precisa_revisao, "{r:?}");
    }

    #[test]
    fn ia_e_nome_de_arquivo_sozinhos_nao_chegam_a_alta() {
        let r = rec(Campo::Titulo, &[t(O::Ia, "Algo", 0.99)]);
        assert!(r.confianca <= 0.7 && r.precisa_revisao);
        let r = rec(Campo::Titulo, &[t(O::NomeArquivo, "algo", 0.9)]);
        assert!(r.confianca <= 0.4 && r.precisa_revisao);
        let r = rec(
            Campo::Titulo,
            &[t(O::Ia, "Algo", 0.99), t(O::Conteudo, "Algo", 0.9)],
        );
        assert!(!r.precisa_revisao);
    }

    #[test]
    fn sem_evidencia_e_desconhecido_nunca_inventado() {
        let r = rec(Campo::Autores, &[]);
        assert_eq!(r.valor, None);
        assert!(r.lista.is_empty());
        assert!(r.precisa_revisao);
        let r = rec(Campo::Editora, &[t(O::Ia, "   ", 0.9)]);
        assert_eq!(r.valor, None);
        assert!(!r.precisa_revisao);
    }

    #[test]
    fn autores_em_formas_diferentes_concordam() {
        let r = rec(
            Campo::Autores,
            &[
                Candidato::lista(O::MetadadoInterno, &["Assis, Machado de"], 0.8),
                Candidato::lista(O::Conteudo, &["Machado de Assis"], 0.85),
            ],
        );
        assert!(!r.precisa_revisao, "{r:?}");
        assert!(r.alternativas.is_empty());
        let d = rec(
            Campo::Autores,
            &[
                Candidato::lista(O::MetadadoInterno, &["Ana Silva"], 0.8),
                Candidato::lista(O::Conteudo, &["João Silva"], 0.85),
            ],
        );
        assert!(d.precisa_revisao);
    }

    #[test]
    fn conjunto_de_campos_lista_razoes() {
        let r = reconciliar(
            &[
                (
                    Campo::Titulo,
                    vec![t(O::Conteudo, "A", 0.9), t(O::Conteudo, "A", 0.9)],
                ),
                (
                    Campo::Isbn,
                    vec![t(O::Identificador, "9788535902778", 0.99)],
                ),
            ],
            ContextoReconciliacao::default(),
        );
        assert!(r.precisa_revisao);
        assert!(r.razoes.iter().any(|x| x.starts_with("Isbn")));
        assert_eq!(r.campos.len(), 2);
    }
}
