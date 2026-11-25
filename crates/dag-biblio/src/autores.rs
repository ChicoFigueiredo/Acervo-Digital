//! Resolução de autores contra a Biblioteca Homologada.
//!
//! Regra: autor existente só é reutilizado com confirmação de identidade. Homônimos NUNCA são
//! unidos automaticamente: havendo mais de um candidato, ou só o nome batendo sem outra
//! evidência (coautor, assunto, editora), o resultado é revisão com os candidatos pontuados.

use serde::{Deserialize, Serialize};

use crate::normalizacao::{CompatAutor, comparar_autores_norm, normalizar_autor, normalizar_texto};

/// Autor da Biblioteca Homologada com o que se sabe sobre ele.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AutorExistente {
    pub id: i64,
    pub nome: String,
    #[serde(default)]
    pub coautores: Vec<String>,
    /// Tags/assuntos dos livros dele.
    #[serde(default)]
    pub assuntos: Vec<String>,
    #[serde(default)]
    pub editoras: Vec<String>,
}

/// O que se sabe sobre o autor do livro novo.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContextoAutor {
    #[serde(default)]
    pub coautores: Vec<String>,
    #[serde(default)]
    pub assuntos: Vec<String>,
    #[serde(default)]
    pub editora: Option<String>,
}

/// Decisão humana anterior: este nome (chave normalizada) é este autor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmacaoAutor {
    /// Nome como foi confirmado (qualquer grafia: compara por normalização, exige `Igual`).
    pub nome: String,
    pub autor_id: i64,
}

/// Evidência além do nome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", content = "valor", rename_all = "snake_case")]
pub enum EvidenciaAutor {
    Coautor(String),
    Assunto(String),
    Editora(String),
    ConfirmacaoHumana,
}

/// Candidato pontuado.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidatoAutor {
    pub autor_id: i64,
    pub nome: String,
    pub compatibilidade: CompatAutorSerde,
    /// 0..=1.
    pub pontuacao: f32,
    pub evidencias: Vec<EvidenciaAutor>,
}

/// Espelho serializável de [`CompatAutor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatAutorSerde {
    Igual,
    Compativel,
}

/// Resultado da resolução.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "resultado", rename_all = "snake_case")]
pub enum ResolucaoAutor {
    /// Reaproveitar o autor existente (identidade confirmada).
    Reutilizar { candidato: CandidatoAutor },
    /// Humano precisa decidir. `motivo` explica por quê.
    Revisao {
        candidatos: Vec<CandidatoAutor>,
        motivo: String,
    },
    /// Ninguém compatível: proposta de autor novo (nada é criado sozinho).
    Novo,
}

fn pontuar(
    existente: &AutorExistente,
    compat: CompatAutor,
    ctx: &ContextoAutor,
    confirmado: bool,
) -> CandidatoAutor {
    let mut ev = Vec::new();
    let nz = |s: &str| normalizar_texto(s);
    for c in &ctx.coautores {
        let n = normalizar_autor(c);
        if existente
            .coautores
            .iter()
            .any(|e| comparar_autores_norm(&n, &normalizar_autor(e)) == CompatAutor::Igual)
        {
            ev.push(EvidenciaAutor::Coautor(c.clone()));
        }
    }
    for a in &ctx.assuntos {
        if existente.assuntos.iter().any(|e| nz(e) == nz(a)) {
            ev.push(EvidenciaAutor::Assunto(a.clone()));
        }
    }
    if let Some(ed) = &ctx.editora
        && existente.editoras.iter().any(|e| nz(e) == nz(ed))
    {
        ev.push(EvidenciaAutor::Editora(ed.clone()));
    }
    if confirmado {
        ev.push(EvidenciaAutor::ConfirmacaoHumana);
    }
    let base = if compat == CompatAutor::Igual {
        0.5
    } else {
        0.3
    };
    let extra = ev.len() as f32 * 0.2;
    CandidatoAutor {
        autor_id: existente.id,
        nome: existente.nome.clone(),
        compatibilidade: if compat == CompatAutor::Igual {
            CompatAutorSerde::Igual
        } else {
            CompatAutorSerde::Compativel
        },
        pontuacao: if confirmado {
            1.0
        } else {
            (base + extra).min(0.95)
        },
        evidencias: ev,
    }
}

/// Resolve `nome` contra os autores existentes.
#[must_use]
pub fn resolver(
    nome: &str,
    ctx: &ContextoAutor,
    existentes: &[AutorExistente],
    confirmacoes: &[ConfirmacaoAutor],
) -> ResolucaoAutor {
    let n = normalizar_autor(nome);
    let mut candidatos: Vec<CandidatoAutor> = existentes
        .iter()
        .filter_map(|e| {
            let c = comparar_autores_norm(&n, &normalizar_autor(&e.nome));
            // Sufixo ambíguo ("Silva" x "Silva Filho") é outra pessoa provável: nem candidato.
            (c >= CompatAutor::Compativel).then(|| {
                let confirmado = confirmacoes.iter().any(|k| {
                    k.autor_id == e.id
                        && comparar_autores_norm(&n, &normalizar_autor(&k.nome))
                            == CompatAutor::Igual
                });
                pontuar(e, c, ctx, confirmado)
            })
        })
        .collect();
    candidatos.sort_by(|a, b| {
        b.pontuacao
            .total_cmp(&a.pontuacao)
            .then(a.autor_id.cmp(&b.autor_id))
    });
    if candidatos.is_empty() {
        return ResolucaoAutor::Novo;
    }
    // Confirmação humana anterior de exatamente um autor decide.
    let confirmados: Vec<&CandidatoAutor> = candidatos
        .iter()
        .filter(|c| c.evidencias.contains(&EvidenciaAutor::ConfirmacaoHumana))
        .collect();
    if confirmados.len() == 1 {
        return ResolucaoAutor::Reutilizar {
            candidato: confirmados[0].clone(),
        };
    }
    if candidatos.len() > 1 {
        let motivo = format!(
            "{} autores existentes com o mesmo nome ou nome compatível (homônimos): escolha manual",
            candidatos.len()
        );
        return ResolucaoAutor::Revisao { candidatos, motivo };
    }
    let unico = candidatos.remove(0);
    let evidencias_alem_do_nome = unico
        .evidencias
        .iter()
        .filter(|e| **e != EvidenciaAutor::ConfirmacaoHumana)
        .count();
    if evidencias_alem_do_nome == 0 {
        return ResolucaoAutor::Revisao {
            motivo: "só o nome coincide, sem coautor, assunto ou editora em comum: identidade não confirmada"
                .into(),
            candidatos: vec![unico],
        };
    }
    if unico.compatibilidade != CompatAutorSerde::Igual {
        return ResolucaoAutor::Revisao {
            motivo: "nome só compatível por iniciais: identidade não confirmada".into(),
            candidatos: vec![unico],
        };
    }
    ResolucaoAutor::Reutilizar { candidato: unico }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn existente(id: i64, nome: &str) -> AutorExistente {
        AutorExistente {
            id,
            nome: nome.into(),
            ..Default::default()
        }
    }

    #[test]
    fn so_o_nome_nao_basta_nem_com_um_unico_candidato() {
        let ex = [existente(1, "Assis, Machado de")];
        let r = resolver("Machado de Assis", &ContextoAutor::default(), &ex, &[]);
        match r {
            ResolucaoAutor::Revisao { candidatos, motivo } => {
                assert_eq!(candidatos.len(), 1);
                assert!(motivo.contains("só o nome"));
            }
            outro => panic!("{outro:?}"),
        }
    }

    #[test]
    fn reutiliza_com_evidencia_adicional() {
        let mut e = existente(1, "Stewart, James");
        e.assuntos = vec!["Cálculo".into()];
        e.editoras = vec!["Cengage".into()];
        let ctx = ContextoAutor {
            assuntos: vec!["calculo".into()],
            editora: Some("CENGAGE".into()),
            ..Default::default()
        };
        match resolver("James Stewart", &ctx, &[e], &[]) {
            ResolucaoAutor::Reutilizar { candidato } => {
                assert_eq!(candidato.autor_id, 1);
                assert_eq!(candidato.evidencias.len(), 2);
                assert!(candidato.pontuacao > 0.8);
            }
            outro => panic!("{outro:?}"),
        }
    }

    #[test]
    fn homonimos_nunca_sao_unidos_mesmo_com_evidencia() {
        let mut a = existente(1, "Silva, João");
        a.assuntos = vec!["Direito".into()];
        let mut b = existente(2, "João Silva");
        b.assuntos = vec!["Direito".into()];
        let ctx = ContextoAutor {
            assuntos: vec!["Direito".into()],
            ..Default::default()
        };
        match resolver("João Silva", &ctx, &[a, b], &[]) {
            ResolucaoAutor::Revisao { candidatos, motivo } => {
                assert_eq!(candidatos.len(), 2);
                assert!(motivo.contains("homônimos"));
                assert!(candidatos[0].pontuacao >= candidatos[1].pontuacao);
            }
            outro => panic!("{outro:?}"),
        }
    }

    #[test]
    fn coautor_conta_como_evidencia() {
        let mut e = existente(7, "Kernighan, Brian");
        e.coautores = vec!["Dennis Ritchie".into()];
        let ctx = ContextoAutor {
            coautores: vec!["Ritchie, Dennis".into()],
            ..Default::default()
        };
        assert!(matches!(
            resolver("Brian Kernighan", &ctx, &[e], &[]),
            ResolucaoAutor::Reutilizar { .. }
        ));
    }

    #[test]
    fn confirmacao_humana_anterior_decide_entre_homonimos() {
        let ex = [existente(1, "Silva, João"), existente(2, "João Silva")];
        let conf = [ConfirmacaoAutor {
            nome: "Silva, João".into(),
            autor_id: 2,
        }];
        match resolver("João Silva", &ContextoAutor::default(), &ex, &conf) {
            ResolucaoAutor::Reutilizar { candidato } => assert_eq!(candidato.autor_id, 2),
            outro => panic!("{outro:?}"),
        }
    }

    #[test]
    fn iniciais_exigem_revisao_mesmo_com_evidencia() {
        let mut e = existente(1, "Tolkien, John Ronald Reuel");
        e.editoras = vec!["Martins Fontes".into()];
        let ctx = ContextoAutor {
            editora: Some("Martins Fontes".into()),
            ..Default::default()
        };
        assert!(matches!(
            resolver("J. R. R. Tolkien", &ctx, &[e], &[]),
            ResolucaoAutor::Revisao { .. }
        ));
    }

    #[test]
    fn sufixo_e_nome_diferente_nao_viram_candidato() {
        let ex = [
            existente(1, "João Silva Filho"),
            existente(2, "Pedro Santos"),
        ];
        assert_eq!(
            resolver("João Silva", &ContextoAutor::default(), &ex, &[]),
            ResolucaoAutor::Novo
        );
    }

    #[test]
    fn sem_correspondencia_e_novo() {
        assert_eq!(
            resolver(
                "Fulano Raro",
                &ContextoAutor::default(),
                &[existente(1, "Outro Nome")],
                &[]
            ),
            ResolucaoAutor::Novo
        );
    }
}
