//! Classificação final de duplicidade por arquivo novo (B-14).
//!
//! `Exata` exige hash e tamanho iguais; é a única que produz espaço recuperável. Hipótese
//! bibliográfica (mesma Edição, mesma Obra, incerta) nunca vira certeza nem economia.

use serde::{Deserialize, Serialize};

use crate::identidade::{RegistroBibliografico, Relacao, Risco, comparar};
use crate::tipos::TipoDuplicidade;

/// Arquivo novo a classificar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArquivoNovo {
    pub id: String,
    /// Hash Completo; `None` enquanto não calculado (então nunca é Exata).
    #[serde(default)]
    pub hash: Option<String>,
    pub tamanho: u64,
    pub registro: RegistroBibliografico,
}

/// Arquivo já conhecido (Formato homologado ou outro arquivo do Catálogo).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArquivoConhecido {
    pub id: String,
    #[serde(default)]
    pub hash: Option<String>,
    pub tamanho: u64,
    /// De um Formato da Biblioteca Homologada?
    #[serde(default)]
    pub homologado: bool,
    /// Identidade física (dispositivo, inode): se igual à do novo, é o Mesmo Arquivo Físico.
    #[serde(default)]
    pub id_fisico: Option<(u64, u64)>,
    pub registro: RegistroBibliografico,
}

/// Resultado para um arquivo novo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Classificacao {
    pub tipo: TipoDuplicidade,
    /// Arquivo conhecido que sustenta a classificação.
    pub alvo: String,
    pub alvo_homologado: bool,
    pub evidencias: Vec<String>,
    pub contraevidencias: Vec<String>,
    pub risco: Risco,
    /// Bytes recuperáveis ao descartar o arquivo novo. Só `Exata` pode ser > 0.
    pub espaco_recuperavel: u64,
    /// Arquivos conhecidos que também são cópia exata (para o relatório).
    pub outras_copias_exatas: Vec<String>,
}

fn ordem(t: TipoDuplicidade) -> u8 {
    match t {
        TipoDuplicidade::Exata => 0,
        TipoDuplicidade::MesmaEdicao => 1,
        TipoDuplicidade::MesmaObra => 2,
        TipoDuplicidade::Incerta => 3,
    }
}

/// Tamanho mínimo (bytes) para um arquivo participar de duplicidade Exata. Arquivos menores
/// (0 byte, truncados) casariam com milhares de vazios: são "formato quebrado".
pub const TAMANHO_MINIMO_EXATA: u64 = 1024;

/// Parâmetros da classificação.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParametrosDuplicidade {
    pub tamanho_minimo_exata: u64,
}

impl Default for ParametrosDuplicidade {
    fn default() -> Self {
        Self {
            tamanho_minimo_exata: TAMANHO_MINIMO_EXATA,
        }
    }
}

/// Resultado completo para um arquivo novo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "resultado", rename_all = "snake_case")]
pub enum ResultadoDuplicidade {
    /// Menor que o mínimo: não é comparado; vai para a fila de exceções.
    FormatoQuebrado {
        tamanho: u64,
        minimo: u64,
    },
    Duplicata(Box<Classificacao>),
    SemRelacao,
}

/// Como [`classificar`], mas informa o "formato quebrado" e aceita parâmetros.
#[must_use]
pub fn classificar_arquivo(
    novo: &ArquivoNovo,
    conhecidos: &[ArquivoConhecido],
    params: &ParametrosDuplicidade,
) -> ResultadoDuplicidade {
    if novo.tamanho < params.tamanho_minimo_exata {
        return ResultadoDuplicidade::FormatoQuebrado {
            tamanho: novo.tamanho,
            minimo: params.tamanho_minimo_exata,
        };
    }
    let uteis: Vec<ArquivoConhecido> = conhecidos
        .iter()
        .filter(|c| c.tamanho >= params.tamanho_minimo_exata)
        .cloned()
        .collect();
    match classificar_sem_minimo(novo, &uteis) {
        Some(c) => ResultadoDuplicidade::Duplicata(Box::new(c)),
        None => ResultadoDuplicidade::SemRelacao,
    }
}

/// Classifica com o limite padrão (1 KiB): arquivo pequeno demais devolve `None`.
#[must_use]
pub fn classificar(novo: &ArquivoNovo, conhecidos: &[ArquivoConhecido]) -> Option<Classificacao> {
    match classificar_arquivo(novo, conhecidos, &ParametrosDuplicidade::default()) {
        ResultadoDuplicidade::Duplicata(c) => Some(*c),
        _ => None,
    }
}

fn classificar_sem_minimo(
    novo: &ArquivoNovo,
    conhecidos: &[ArquivoConhecido],
) -> Option<Classificacao> {
    let mut exatas: Vec<&ArquivoConhecido> = Vec::new();
    let mut melhor: Option<Classificacao> = None;
    for c in conhecidos {
        if c.id == novo.id {
            continue;
        }
        let candidato = classificar_um(novo, c);
        let Some(cl) = candidato else { continue };
        if cl.tipo == TipoDuplicidade::Exata {
            exatas.push(c);
        }
        let melhor_que = match &melhor {
            None => true,
            Some(m) => (ordem(cl.tipo), !cl.alvo_homologado) < (ordem(m.tipo), !m.alvo_homologado),
        };
        if melhor_que {
            melhor = Some(cl);
        }
    }
    if let Some(m) = &mut melhor
        && m.tipo == TipoDuplicidade::Exata
    {
        m.outras_copias_exatas = exatas
            .iter()
            .filter(|c| c.id != m.alvo)
            .map(|c| c.id.clone())
            .collect();
    }
    melhor
}

fn classificar_um(novo: &ArquivoNovo, c: &ArquivoConhecido) -> Option<Classificacao> {
    let mut ev = Vec::new();
    let mut contra = Vec::new();
    if let (Some(a), Some(b)) = (&novo.hash, &c.hash)
        && a == b
    {
        if novo.tamanho == c.tamanho {
            ev.push("hash e tamanho iguais".to_owned());
            return Some(Classificacao {
                tipo: TipoDuplicidade::Exata,
                alvo: c.id.clone(),
                alvo_homologado: c.homologado,
                evidencias: ev,
                contraevidencias: contra,
                risco: Risco::Baixo,
                espaco_recuperavel: novo.tamanho,
                outras_copias_exatas: Vec::new(),
            });
        }
        // Hash igual com tamanho diferente é anomalia: nunca Exata.
        contra.push("hash igual mas tamanho diferente: anomalia, verificar".to_owned());
        return Some(Classificacao {
            tipo: TipoDuplicidade::Incerta,
            alvo: c.id.clone(),
            alvo_homologado: c.homologado,
            evidencias: ev,
            contraevidencias: contra,
            risco: Risco::Alto,
            espaco_recuperavel: 0,
            outras_copias_exatas: Vec::new(),
        });
    }
    let mut a = novo.registro.clone();
    let mut b = c.registro.clone();
    // O hash só entra na comparação bibliográfica se for o mesmo (tratado acima).
    a.hash = None;
    b.hash = None;
    let v = comparar(&a, &b);
    let tipo = match v.relacao {
        Relacao::MesmaEdicao => TipoDuplicidade::MesmaEdicao,
        Relacao::MesmaObra => TipoDuplicidade::MesmaObra,
        Relacao::Incerta => TipoDuplicidade::Incerta,
        Relacao::Distintos => return None,
    };
    Some(Classificacao {
        tipo,
        alvo: c.id.clone(),
        alvo_homologado: c.homologado,
        evidencias: v.evidencias,
        contraevidencias: v.contraevidencias,
        risco: v.risco,
        espaco_recuperavel: 0,
        outras_copias_exatas: Vec::new(),
    })
}

/// Cópia que só difere no caminho (hard link / mesmo inode) não é segunda cópia: não recupera
/// espaço. Use depois de [`classificar`] quando o catálogo conhece o inode.
#[must_use]
pub fn sem_espaco_se_mesmo_fisico(
    mut c: Classificacao,
    novo_fisico: Option<(u64, u64)>,
    alvo: &ArquivoConhecido,
) -> Classificacao {
    if c.tipo == TipoDuplicidade::Exata && novo_fisico.is_some() && novo_fisico == alvo.id_fisico {
        c.espaco_recuperavel = 0;
        c.contraevidencias
            .push("Mesmo Arquivo Físico (hard link): não há segunda cópia".into());
    }
    c
}

/// Formato de um livro da Homologada (para achar repetições internas).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatoHomologado {
    pub livro_id: i64,
    pub arquivo_id: i64,
    pub hash: Option<String>,
    pub tamanho: u64,
}

/// Alerta próprio: o mesmo conteúdo aparece em livros DIFERENTES da Homologada. Sempre para
/// revisão humana (talvez juntar os livros no Calibre); nunca é sugestão de descarte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LivroRepetidoNaHomologada {
    pub hash: String,
    pub tamanho: u64,
    pub livros: Vec<i64>,
    pub arquivos: Vec<i64>,
}

/// Acha conteúdo idêntico em livros diferentes da Homologada. Formatos menores que `minimo`
/// são ignorados; vários formatos do mesmo livro não contam como repetição.
#[must_use]
pub fn livros_repetidos_na_homologada(
    formatos: &[FormatoHomologado],
    minimo: u64,
) -> Vec<LivroRepetidoNaHomologada> {
    let mut mapa: std::collections::BTreeMap<(String, u64), Vec<&FormatoHomologado>> =
        Default::default();
    for f in formatos.iter().filter(|f| f.tamanho >= minimo) {
        if let Some(h) = &f.hash {
            mapa.entry((h.clone(), f.tamanho)).or_default().push(f);
        }
    }
    mapa.into_iter()
        .filter_map(|((hash, tamanho), v)| {
            let mut livros: Vec<i64> = v.iter().map(|f| f.livro_id).collect();
            livros.sort_unstable();
            livros.dedup();
            (livros.len() > 1).then(|| {
                let mut arquivos: Vec<i64> = v.iter().map(|f| f.arquivo_id).collect();
                arquivos.sort_unstable();
                LivroRepetidoNaHomologada {
                    hash,
                    tamanho,
                    livros,
                    arquivos,
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::tipos::FormatoLivro;

    #[test]
    fn arquivos_pequenos_sao_formato_quebrado_e_nunca_casam() {
        let vazio = novo("n", Some("e3b0"), 0);
        let k = conhecido("k", Some("e3b0"), 0, true);
        let r = classificar_arquivo(
            &vazio,
            std::slice::from_ref(&k),
            &ParametrosDuplicidade::default(),
        );
        assert_eq!(
            r,
            ResultadoDuplicidade::FormatoQuebrado {
                tamanho: 0,
                minimo: 1024
            }
        );
        assert!(classificar(&vazio, &[k]).is_none());
        let k0 = conhecido("k0", Some("h"), 0, true);
        assert_eq!(
            classificar_arquivo(
                &novo("n", Some("h"), 5000),
                &[k0],
                &ParametrosDuplicidade::default()
            ),
            ResultadoDuplicidade::SemRelacao
        );
        let p = ParametrosDuplicidade {
            tamanho_minimo_exata: 10,
        };
        assert!(matches!(
            classificar_arquivo(
                &novo("n", Some("h"), 50),
                &[conhecido("k", Some("h"), 50, true)],
                &p
            ),
            ResultadoDuplicidade::Duplicata(_)
        ));
    }

    #[test]
    fn livro_repetido_na_homologada_e_alerta_proprio() {
        let f = |l, a, h: &str, t| FormatoHomologado {
            livro_id: l,
            arquivo_id: a,
            hash: Some(h.into()),
            tamanho: t,
        };
        let r = livros_repetidos_na_homologada(
            &[
                f(1, 10, "x", 5000),
                f(2, 20, "x", 5000),
                f(3, 30, "y", 5000),
                f(3, 31, "y", 5000),
                f(4, 40, "z", 0),
                f(5, 50, "z", 0),
            ],
            1024,
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].livros, [1, 2]);
        assert_eq!(r[0].arquivos, [10, 20]);
    }

    fn reg(id: &str, fmt: FormatoLivro) -> RegistroBibliografico {
        RegistroBibliografico {
            id: id.into(),
            titulo: "Dom Casmurro".into(),
            autores: vec!["Machado de Assis".into()],
            editora: Some("Ática".into()),
            ano: Some(2000),
            idioma: Some("por".into()),
            formato: Some(fmt),
            ..Default::default()
        }
    }

    fn novo(id: &str, hash: Option<&str>, tam: u64) -> ArquivoNovo {
        ArquivoNovo {
            id: id.into(),
            hash: hash.map(Into::into),
            tamanho: tam,
            registro: reg(id, FormatoLivro::Pdf),
        }
    }

    fn conhecido(id: &str, hash: Option<&str>, tam: u64, homologado: bool) -> ArquivoConhecido {
        ArquivoConhecido {
            id: id.into(),
            hash: hash.map(Into::into),
            tamanho: tam,
            homologado,
            id_fisico: None,
            registro: reg(id, FormatoLivro::Pdf),
        }
    }

    #[test]
    fn exata_por_hash_e_tamanho_e_unica_com_espaco() {
        let c = classificar(
            &novo("n", Some("h"), 5000),
            &[conhecido("k", Some("h"), 5000, true)],
        )
        .unwrap();
        assert_eq!(c.tipo, TipoDuplicidade::Exata);
        assert_eq!(c.espaco_recuperavel, 5000);
        assert!(c.alvo_homologado);
        assert_eq!(c.risco, Risco::Baixo);
    }

    #[test]
    fn hash_igual_tamanho_diferente_nunca_e_exata() {
        let c = classificar(
            &novo("n", Some("h"), 5000),
            &[conhecido("k", Some("h"), 5001, true)],
        )
        .unwrap();
        assert_eq!(c.tipo, TipoDuplicidade::Incerta);
        assert_eq!(c.espaco_recuperavel, 0);
    }

    #[test]
    fn sem_hash_nao_ha_exata_nem_espaco() {
        let c = classificar(
            &novo("n", None, 5000),
            &[conhecido("k", Some("h"), 5000, true)],
        )
        .unwrap();
        assert_ne!(c.tipo, TipoDuplicidade::Exata);
        assert_eq!(c.espaco_recuperavel, 0);
    }

    #[test]
    fn outro_formato_da_mesma_edicao_nao_gera_espaco() {
        let mut n = novo("n", Some("a"), 9000);
        n.registro.formato = Some(FormatoLivro::Epub);
        let c = classificar(&n, &[conhecido("k", Some("b"), 50000, true)]).unwrap();
        assert_eq!(c.tipo, TipoDuplicidade::MesmaEdicao);
        assert_eq!(c.espaco_recuperavel, 0);
    }

    #[test]
    fn mesma_obra_e_incerta_nao_geram_espaco() {
        let mut k = conhecido("k", Some("b"), 50000, true);
        k.registro.ano = Some(1990);
        let c = classificar(&novo("n", Some("a"), 9000), &[k]).unwrap();
        assert_eq!(c.tipo, TipoDuplicidade::MesmaObra);
        assert_eq!(c.espaco_recuperavel, 0);
        let mut k2 = conhecido("k2", Some("b"), 50000, true);
        k2.registro.editora = None;
        let c2 = classificar(&novo("n", Some("a"), 9000), &[k2]).unwrap();
        assert_eq!(c2.tipo, TipoDuplicidade::Incerta);
        assert_eq!(c2.espaco_recuperavel, 0);
    }

    #[test]
    fn distintos_nao_classificam() {
        let mut k = conhecido("k", Some("b"), 50000, true);
        k.registro.titulo = "Cálculo".into();
        k.registro.autores = vec!["James Stewart".into()];
        assert!(classificar(&novo("n", Some("a"), 9000), &[k]).is_none());
    }

    #[test]
    fn exata_vence_e_homologado_vence_em_empate() {
        let conhecidos = [
            conhecido("a", Some("x"), 2000, false),
            conhecido("b", Some("h"), 2000, false),
            conhecido("c", Some("h"), 2000, true),
        ];
        let c = classificar(&novo("n", Some("h"), 2000), &conhecidos).unwrap();
        assert_eq!(c.tipo, TipoDuplicidade::Exata);
        assert_eq!(c.alvo, "c");
        assert_eq!(c.outras_copias_exatas, ["b"]);
    }

    #[test]
    fn mesmo_arquivo_fisico_nao_recupera_espaco() {
        let k = ArquivoConhecido {
            id_fisico: Some((1, 42)),
            ..conhecido("k", Some("h"), 2000, true)
        };
        let c = classificar(&novo("n", Some("h"), 2000), std::slice::from_ref(&k)).unwrap();
        let c = sem_espaco_se_mesmo_fisico(c, Some((1, 42)), &k);
        assert_eq!(c.espaco_recuperavel, 0);
    }

    #[test]
    fn ignora_a_si_mesmo() {
        assert!(
            classificar(
                &novo("n", Some("h"), 2000),
                &[conhecido("n", Some("h"), 2000, false)]
            )
            .is_none()
        );
    }
}
