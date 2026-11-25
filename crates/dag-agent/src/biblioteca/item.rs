//! Processamento de UM item de revisão (um arquivo, ou um livro dentro de um compactado):
//! candidatos por campo (extração, nome do arquivo, decisões humanas, Homologada, vocabulário
//! aprendido, IA com contexto), reconciliação por ordem de evidência, resolução de autores,
//! Obra > Edição > Formato, sugestões e candidatos de identidade bibliográfica.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use dag_biblio::autores::{ContextoAutor, ResolucaoAutor};
use dag_biblio::identidade::{RegistroBibliografico, Relacao, Risco, comparar};
use dag_biblio::normalizacao::{author_sort, normalizar_texto};
use dag_biblio::reconciliacao::{CampoReconciliado, Candidato, ContextoReconciliacao, reconciliar};
use dag_biblio::vocabulario::LivroTreino;
use dag_biblio::{
    Campo, Evidencia, FormatoLivro, OrigemEvidencia, ResultadoExtracao, TipoDuplicidade,
    ValorSugerido, isbn,
};
use dag_biblio_ia::{EntradaClassificacao, VocabularioHomologado};
use dag_store::biblioteca as store;
use rusqlite::Connection;

use super::extracao::ClassificadorIa;
use super::vocab::CacheVocab;
use super::{ConfigBiblioteca, hash_de_hex, hex};
use crate::agente::Agente;
use crate::erro::ErroAgente;

/// Prefixo do `detalhe` das evidências que guardam "precisa revisão porque...".
pub(super) const PREFIXO_REVISAO: &str = "revisao: ";

/// Entrada de um item.
pub(super) struct EntradaItem<'a> {
    pub job_id: i64,
    pub info: &'a store::InfoArquivo,
    /// Caminho interno (vazio = o próprio arquivo).
    pub membro: &'a str,
    pub formato: FormatoLivro,
    /// Tamanho do livro (do Membro, ou do arquivo).
    pub tamanho: u64,
    pub hash_arquivo: [u8; 32],
    pub resultado: &'a ResultadoExtracao,
    /// Capa (imagem em memória) para a passada de visão da IA.
    pub capa: Option<Vec<u8>>,
}

/// O que o item produziu.
pub(super) struct SaidaItem {
    pub usou_ia: bool,
}

/// Separa uma lista como o humano ou o extrator a escreveu.
pub(super) fn dividir_lista(campo: Campo, texto: &str) -> Vec<String> {
    let alt: &[char] = if campo == Campo::Autores {
        &['&']
    } else {
        &[',']
    };
    let partes: Vec<&str> = if texto.contains('|') {
        texto.split('|').collect()
    } else {
        texto.split(|c| alt.contains(&c)).collect()
    };
    partes
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn juntar_lista(campo: Campo, lista: &[String]) -> String {
    lista.join(if campo == Campo::Autores { " & " } else { "|" })
}

fn e_lista(c: Campo) -> bool {
    matches!(c, Campo::Autores | Campo::Tags)
}

fn evidencia_revisao(motivo: &str) -> Evidencia {
    Evidencia {
        origem: OrigemEvidencia::MetadadoInterno,
        pagina: None,
        trecho: None,
        detalhe: Some(format!("{PREFIXO_REVISAO}{motivo}")),
        confianca: 0.0,
    }
}

fn origem_de(v: &ValorSugerido) -> OrigemEvidencia {
    v.evidencias
        .iter()
        .max_by(|a, b| a.confianca.total_cmp(&b.confianca))
        .map_or(OrigemEvidencia::MetadadoInterno, |e| e.origem)
}

fn candidato_de(v: &ValorSugerido, origem: OrigemEvidencia) -> Option<Candidato> {
    if v.valor.as_deref().is_none_or(|t| t.trim().is_empty()) && v.lista.is_empty() {
        return None;
    }
    let mut c = Candidato {
        origem,
        valor: v.valor.clone(),
        lista: v.lista.clone(),
        confianca: v.confianca.clamp(0.0, 1.0),
        evidencias: v.evidencias.clone(),
    };
    if e_lista(v.campo)
        && c.lista.is_empty()
        && let Some(t) = c.valor.take()
    {
        c.lista = dividir_lista(v.campo, &t);
    }
    Some(c)
}

fn texto_do_candidato(c: &Candidato) -> String {
    if c.lista.is_empty() {
        c.valor.clone().unwrap_or_default()
    } else {
        c.lista.join("|")
    }
}

fn chave_rejeicao(campo: Campo, t: &str) -> String {
    if e_lista(campo) {
        let mut v: Vec<String> = dividir_lista(campo, t)
            .iter()
            .map(|x| normalizar_texto(x))
            .collect();
        v.sort();
        v.join("|")
    } else {
        normalizar_texto(t)
    }
}

fn texto_de(c: &[CampoReconciliado], campo: Campo) -> Option<&str> {
    c.iter()
        .find(|x| x.campo == campo)
        .and_then(|x| x.valor.as_deref())
        .filter(|t| !t.trim().is_empty())
}

fn lista_de(c: &[CampoReconciliado], campo: Campo) -> Vec<String> {
    c.iter()
        .find(|x| x.campo == campo)
        .map(lista_final)
        .unwrap_or_default()
}

fn lista_final(c: &CampoReconciliado) -> Vec<String> {
    if c.lista.is_empty() {
        c.valor
            .as_deref()
            .map(|t| dividir_lista(c.campo, t))
            .unwrap_or_default()
    } else {
        c.lista.clone()
    }
}

/// Nome do arquivo (ou do Membro) como título provável.
fn titulo_do_nome(nome: &str) -> String {
    let base = nome.rsplit(['/', '!']).next().unwrap_or(nome);
    let base = base.rsplit_once('.').map_or(base, |(a, _)| a);
    base.replace(['_', '.'], " ").trim().to_owned()
}

fn registro(
    id: String,
    titulo: &str,
    autores: &[String],
    editora: Option<&str>,
    idioma: Option<&str>,
    isbn: Option<&str>,
    serie: Option<&str>,
) -> RegistroBibliografico {
    RegistroBibliografico {
        id,
        titulo: titulo.to_owned(),
        autores: autores.to_vec(),
        editora: editora.map(str::to_owned),
        idioma: idioma.map(str::to_owned),
        isbns: isbn.map(str::to_owned).into_iter().collect(),
        serie: serie.map(str::to_owned),
        ..RegistroBibliografico::default()
    }
}

fn registro_do_livro(l: &store::LivroHomologado) -> RegistroBibliografico {
    RegistroBibliografico {
        id: format!("livro:{}", l.id),
        titulo: l.titulo.clone(),
        autores: l.autores.clone(),
        editora: l.editora.clone(),
        idioma: l.idioma.clone(),
        isbns: l.isbn13.clone().into_iter().collect(),
        serie: l.serie.clone(),
        indice_serie: l.indice,
        ..RegistroBibliografico::default()
    }
}

/// Livros da Homologada que podem ser o mesmo livro: ISBN, título normalizado e título
/// principal, cada um por índice e com teto. Nunca compara todos contra todos.
pub(super) fn livros_candidatos(
    conn: &Connection,
    isbn13: Option<&str>,
    titulo: &str,
) -> Result<Vec<store::LivroHomologado>, ErroAgente> {
    const TETO: usize = 20;
    let mut v: Vec<store::LivroHomologado> = Vec::new();
    let mut vistos = HashSet::new();
    let mut add = |ls: Vec<store::LivroHomologado>, v: &mut Vec<store::LivroHomologado>| {
        for l in ls.into_iter().take(TETO) {
            if vistos.insert(l.id) {
                v.push(l);
            }
        }
    };
    if let Some(i) = isbn13 {
        add(store::buscar_livros_por_isbn(conn, i)?, &mut v);
    }
    if !titulo.trim().is_empty() {
        add(
            store::livros_por_titulo_norm(conn, &store::normalizar_texto(titulo), TETO)?,
            &mut v,
        );
        add(
            store::livros_por_titulo_principal(conn, &store::titulo_principal(titulo), TETO)?,
            &mut v,
        );
    }
    Ok(v)
}

fn risco_store(r: Risco) -> store::RiscoDuplicidade {
    match r {
        Risco::Baixo => store::RiscoDuplicidade::Baixo,
        Risco::Medio => store::RiscoDuplicidade::Medio,
        Risco::Alto => store::RiscoDuplicidade::Alto,
    }
}

/// Resultado de `reconciliar` para o conjunto de candidatos.
fn reconciliar_mapa(
    cand: &BTreeMap<Campo, Vec<Candidato>>,
    corroborado: bool,
) -> Vec<CampoReconciliado> {
    let entrada: Vec<(Campo, Vec<Candidato>)> = cand.iter().map(|(c, v)| (*c, v.clone())).collect();
    reconciliar(
        &entrada,
        ContextoReconciliacao {
            isbn_corroborado: corroborado,
        },
    )
    .campos
}

/// O ISBN é corroborado quando duas origens independentes o trazem, ou um livro da Homologada
/// com o mesmo ISBN tem o mesmo título principal.
fn isbn_corroborado(
    cand: &BTreeMap<Campo, Vec<Candidato>>,
    homologados: &[store::LivroHomologado],
    titulo: Option<&str>,
) -> bool {
    let mut por_chave: HashMap<String, HashSet<u8>> = HashMap::new();
    for c in cand.get(&Campo::Isbn).into_iter().flatten() {
        if let Some(i) = c.valor.as_deref().and_then(isbn::normalizar) {
            por_chave
                .entry(i)
                .or_default()
                .insert(dag_biblio::reconciliacao::forca(c.origem));
        }
    }
    if por_chave.values().any(|o| o.len() >= 2) {
        return true;
    }
    let Some(t) = titulo else { return false };
    let principal = store::titulo_principal(t);
    por_chave.keys().any(|i| {
        homologados.iter().any(|l| {
            l.isbn13.as_deref() == Some(i) && store::titulo_principal(&l.titulo) == principal
        })
    })
}

impl Agente {
    /// Processa o item. Grava Obra/Edição/Formato, sugestões, inferências e candidatos.
    #[allow(clippy::too_many_lines)]
    pub(super) fn processar_item(
        &mut self,
        e: &EntradaItem<'_>,
        cfg: &ConfigBiblioteca,
        ia: Option<&Arc<dyn ClassificadorIa>>,
        vocab: Option<&Arc<CacheVocab>>,
    ) -> Result<SaidaItem, ErroAgente> {
        let agora = self.agora_biblio();
        let file_id = e.info.file_id;
        let versao_item = store::versao_do_item(&cfg.versao_regras, e.membro);
        let nome_alvo = if e.membro.is_empty() {
            e.info.nome.as_str()
        } else {
            e.membro
        };

        // 1. Candidatos da extração, do nome do arquivo e das decisões humanas.
        let mut cand: BTreeMap<Campo, Vec<Candidato>> = BTreeMap::new();
        let mut tags_extraidas: Vec<String> = Vec::new();
        for v in &e.resultado.metadados {
            if v.campo == Campo::Tags {
                tags_extraidas.extend(if v.lista.is_empty() {
                    v.valor
                        .as_deref()
                        .map(|t| dividir_lista(Campo::Tags, t))
                        .unwrap_or_default()
                } else {
                    v.lista.clone()
                });
                continue;
            }
            if let Some(c) = candidato_de(v, origem_de(v)) {
                cand.entry(v.campo).or_default().push(c);
            }
        }
        for (i, ev) in &e.resultado.isbns {
            cand.entry(Campo::Isbn).or_default().push(Candidato {
                origem: ev.origem,
                valor: Some(i.clone()),
                lista: Vec::new(),
                confianca: ev.confianca,
                evidencias: vec![ev.clone()],
            });
        }
        let nome_titulo = titulo_do_nome(nome_alvo);
        if !nome_titulo.is_empty() {
            cand.entry(Campo::Titulo).or_default().push(Candidato {
                origem: OrigemEvidencia::NomeArquivo,
                valor: Some(nome_titulo),
                lista: Vec::new(),
                confianca: 0.4,
                evidencias: vec![Evidencia {
                    origem: OrigemEvidencia::NomeArquivo,
                    pagina: None,
                    trecho: Some(nome_alvo.to_owned()),
                    detalhe: Some("nome do arquivo".into()),
                    confianca: 0.4,
                }],
            });
        }
        let decisoes = store::decisoes_do_item(self.conexao(), file_id, &versao_item)?;
        let mut humanos: HashMap<Campo, store::AcaoDecisao> = HashMap::new();
        let mut rejeitados: HashMap<Campo, Vec<String>> = HashMap::new();
        for d in &decisoes {
            match d.acao {
                store::AcaoDecisao::Rejeitar => {
                    if let Some(v) = &d.valor_anterior {
                        rejeitados
                            .entry(d.campo)
                            .or_default()
                            .push(chave_rejeicao(d.campo, v));
                    }
                }
                acao => {
                    if humanos.contains_key(&d.campo) {
                        continue; // a mais nova (primeira) vale
                    }
                    let Some(v) = d.valor_novo.as_deref().filter(|t| !t.trim().is_empty()) else {
                        continue;
                    };
                    let (valor, lista) = if e_lista(d.campo) {
                        (None, dividir_lista(d.campo, v))
                    } else {
                        (Some(v.to_owned()), Vec::new())
                    };
                    cand.entry(d.campo).or_default().push(Candidato {
                        origem: OrigemEvidencia::Humano,
                        valor,
                        lista,
                        confianca: 1.0,
                        evidencias: vec![Evidencia {
                            origem: OrigemEvidencia::Humano,
                            pagina: None,
                            trecho: Some(v.to_owned()),
                            detalhe: Some("decisão humana".into()),
                            confianca: 1.0,
                        }],
                    });
                    humanos.insert(d.campo, acao);
                }
            }
        }
        // O que o humano rejeitou não volta como sugestão.
        let podar = |cand: &mut BTreeMap<Campo, Vec<Candidato>>| {
            for (campo, ch) in &rejeitados {
                if let Some(v) = cand.get_mut(campo) {
                    v.retain(|c| {
                        c.origem == OrigemEvidencia::Humano
                            || !ch.contains(&chave_rejeicao(*campo, &texto_do_candidato(c)))
                    });
                }
            }
        };
        podar(&mut cand);

        // 2. Rascunho (sem vocabulário nem IA): título, autores, editora, ISBN...
        let rascunho = reconciliar_mapa(&cand, false);
        let titulo_rasc = texto_de(&rascunho, Campo::Titulo).map(str::to_owned);
        let autores_rasc = lista_de(&rascunho, Campo::Autores);
        let isbn_rasc = texto_de(&rascunho, Campo::Isbn)
            .and_then(isbn::normalizar)
            .or_else(|| texto_de(&rascunho, Campo::Isbn).map(str::to_owned));
        let editora_rasc = texto_de(&rascunho, Campo::Editora).map(str::to_owned);
        let idioma_rasc = texto_de(&rascunho, Campo::Idioma).map(str::to_owned);
        let serie_rasc = texto_de(&rascunho, Campo::Serie).map(str::to_owned);

        // 3. Livros homologados parecidos: pré-filtro por índice. Mesma Edição reconhecida =
        //    metadado homologado (vence na reconciliação).
        let homologados = livros_candidatos(
            self.conexao(),
            isbn_rasc.as_deref(),
            titulo_rasc.as_deref().unwrap_or(""),
        )?;
        let mut referencias: Vec<ValorSugerido> = Vec::new();
        if let Some(t) = &titulo_rasc {
            let a = registro(
                file_id.to_string(),
                t,
                &autores_rasc,
                editora_rasc.as_deref(),
                idioma_rasc.as_deref(),
                isbn_rasc.as_deref(),
                serie_rasc.as_deref(),
            );
            for l in &homologados {
                let v = comparar(&a, &registro_do_livro(l));
                if v.relacao != Relacao::MesmaEdicao {
                    continue;
                }
                let ev = |trecho: &str| Evidencia {
                    origem: OrigemEvidencia::Homologada,
                    pagina: None,
                    trecho: Some(trecho.to_owned()),
                    detalhe: Some(format!("livro homologado #{} (mesma Edição)", l.id)),
                    confianca: 1.0,
                };
                let mut push = |campo: Campo, valor: Option<String>, lista: Vec<String>| {
                    if valor.is_none() && lista.is_empty() {
                        return;
                    }
                    let tr = valor.clone().unwrap_or_else(|| lista.join(" & "));
                    let vs = ValorSugerido {
                        campo,
                        valor: valor.clone(),
                        lista: lista.clone(),
                        confianca: 1.0,
                        evidencias: vec![ev(&tr)],
                    };
                    if let Some(c) = candidato_de(&vs, OrigemEvidencia::Homologada) {
                        cand.entry(campo).or_default().push(c);
                    }
                    referencias.push(vs);
                };
                push(Campo::Titulo, Some(l.titulo.clone()), Vec::new());
                push(Campo::Autores, None, l.autores.clone());
                push(Campo::Editora, l.editora.clone(), Vec::new());
                push(Campo::Idioma, l.idioma.clone(), Vec::new());
                push(Campo::Serie, l.serie.clone(), Vec::new());
                break;
            }
            podar(&mut cand);
        }

        // 4. Vocabulário e estante aprendidos da Homologada (e das decisões humanas).
        let rasc2 = reconciliar_mapa(&cand, false);
        let titulo_ref = texto_de(&rasc2, Campo::Titulo).map(str::to_owned);
        let autores_ref = lista_de(&rasc2, Campo::Autores);
        let treino = LivroTreino {
            id: 0,
            titulo: titulo_ref.clone().unwrap_or_default(),
            autores: autores_ref.clone(),
            comentarios: texto_de(&rasc2, Campo::Resumo).map(str::to_owned),
            editora: texto_de(&rasc2, Campo::Editora).map(str::to_owned),
            serie: texto_de(&rasc2, Campo::Serie).map(str::to_owned),
            tags: Vec::new(),
            estante: None,
        };
        let mut extras: Vec<(Option<Campo>, String)> = Vec::new();
        let mut sugestoes = dag_biblio::vocabulario::Sugestoes::default();
        if let Some(v) = vocab {
            sugestoes = v.sugerir(&treino);
            let conn = self.conexao();
            if !sugestoes.tags.is_empty() {
                let lista: Vec<String> = sugestoes.tags.iter().map(|t| t.tag.clone()).collect();
                let media = sugestoes.tags.iter().map(|t| t.confianca).sum::<f32>()
                    / sugestoes.tags.len() as f32;
                let evidencias = sugestoes
                    .tags
                    .iter()
                    .map(|t| {
                        let porque = v.explicar(conn, &t.exemplos);
                        Evidencia {
                            origem: OrigemEvidencia::Homologada,
                            pagina: None,
                            trecho: Some(t.tag.clone()),
                            detalhe: Some(if porque.is_empty() {
                                format!("{:?} ({:.2})", t.origem, t.confianca)
                            } else {
                                porque.join("; ")
                            }),
                            confianca: t.confianca,
                        }
                    })
                    .collect();
                cand.entry(Campo::Tags).or_default().push(Candidato {
                    origem: OrigemEvidencia::Conteudo,
                    valor: None,
                    lista,
                    confianca: media,
                    evidencias,
                });
            }
            if let Some(est) = &sugestoes.estante {
                let porque = v.explicar(conn, &est.exemplos);
                cand.entry(Campo::Estante).or_default().push(Candidato {
                    origem: OrigemEvidencia::Conteudo,
                    valor: Some(est.estante.clone()),
                    lista: Vec::new(),
                    confianca: est.confianca,
                    evidencias: vec![Evidencia {
                        origem: OrigemEvidencia::Homologada,
                        pagina: None,
                        trecho: Some(est.estante.clone()),
                        detalhe: Some(if porque.is_empty() {
                            "vizinhos da Homologada".into()
                        } else {
                            porque.join("; ")
                        }),
                        confianca: est.confianca,
                    }],
                });
            } else if let Some(est) =
                v.estante_por_taxonomia(&treino.titulo, treino.comentarios.as_deref().unwrap_or(""))
            {
                cand.entry(Campo::Estante).or_default().push(Candidato {
                    origem: OrigemEvidencia::Conteudo,
                    valor: Some(est.clone()),
                    lista: Vec::new(),
                    confianca: 0.45,
                    evidencias: vec![Evidencia {
                        origem: OrigemEvidencia::Conteudo,
                        pagina: None,
                        trecho: Some(est),
                        detalhe: Some("taxonomia inicial (evidência auxiliar)".into()),
                        confianca: 0.45,
                    }],
                });
            }
            // Tags do próprio arquivo (assuntos do OPF...): só as que existem no vocabulário.
            if !tags_extraidas.is_empty() {
                let (ok, propostas) = v.filtrar_candidatas(&tags_extraidas);
                if !ok.is_empty() {
                    cand.entry(Campo::Tags).or_default().push(Candidato {
                        origem: OrigemEvidencia::MetadadoInterno,
                        valor: None,
                        lista: ok,
                        confianca: 0.7,
                        evidencias: vec![Evidencia {
                            origem: OrigemEvidencia::MetadadoInterno,
                            pagina: None,
                            trecho: None,
                            detalhe: Some("assuntos do metadado interno".into()),
                            confianca: 0.7,
                        }],
                    });
                }
                for p in propostas {
                    extras.push((
                        Some(Campo::Tags),
                        format!("tag nova proposta «{}»: {}", p.valor, p.motivo),
                    ));
                }
            }
            for p in &sugestoes.propostas {
                extras.push((
                    Some(Campo::Tags),
                    format!("tag nova proposta «{}»: {}", p.valor, p.motivo),
                ));
            }
        } else if !tags_extraidas.is_empty() {
            extras.push((
                Some(Campo::Tags),
                "sem Biblioteca Homologada: tags do arquivo não conferidas com o vocabulário"
                    .into(),
            ));
        }

        // 5. IA com contexto (vocabulário candidato, estantes, autores parecidos, few-shot).
        let mut usou_ia = false;
        let mut modelo = String::new();
        if let Some(c) = ia {
            let (vocabulario, exemplos) = match vocab {
                Some(v) => (
                    VocabularioHomologado {
                        tags: v.tags_candidatas(&sugestoes),
                        estantes: v.estantes(),
                        autores_parecidos: v.autores_parecidos(&autores_ref),
                    },
                    v.exemplos_ia(self.conexao(), &sugestoes),
                ),
                None => (VocabularioHomologado::default(), Vec::new()),
            };
            let entrada = EntradaClassificacao {
                id: file_id.to_string(),
                nome_arquivo: nome_alvo.to_owned(),
                extracao: e.resultado.clone(),
                vocabulario,
                exemplos,
                referencias,
                imagem_pagina1: e.capa.clone(),
            };
            match c.classificar(&entrada) {
                Ok(res) => {
                    usou_ia = true;
                    modelo = c.nome();
                    for reg in &res.registros {
                        let h = hash_de_hex(&reg.hash_prompt).unwrap_or_else(|| {
                            *blake3::hash(reg.hash_prompt.as_bytes()).as_bytes()
                        });
                        let clamp =
                            |x: Option<u64>| u32::try_from(x.unwrap_or(0)).unwrap_or(u32::MAX);
                        store::gravar_inferencia(
                            self.conexao(),
                            &store::NovaInferencia {
                                job_id: Some(e.job_id),
                                file_id: Some(file_id),
                                modelo: reg.modelo.clone(),
                                versao_modelo: reg.versao_modelo.clone(),
                                hash_prompt: h,
                                tokens_entrada: clamp(reg.metricas.tokens_entrada),
                                tokens_saida: clamp(reg.metricas.tokens_saida),
                                tempo_ms: reg.tempo_ms,
                                bruto_json: reg.resposta_bruta.clone(),
                            },
                            agora,
                        )?;
                    }
                    for v in &res.valores {
                        if let Some(c) = candidato_de(v, OrigemEvidencia::Ia) {
                            cand.entry(v.campo).or_default().push(c);
                        }
                    }
                    for t in &res.tags_novas_propostas {
                        extras.push((
                            Some(Campo::Tags),
                            format!(
                                "tag nova proposta pela IA «{}»: precisa de aprovação",
                                t.tag
                            ),
                        ));
                    }
                    for r in &res.revisao_humana {
                        extras.push((r.campo, format!("IA: {:?}", r.motivo)));
                    }
                    podar(&mut cand);
                }
                Err(err) => {
                    self.mensagem(format!(
                        "IA indisponível ({err}); seguindo só com a extração"
                    ));
                }
            }
        }

        // 6. Reconciliação final por ordem de evidência.
        let corroborado = isbn_corroborado(&cand, &homologados, titulo_ref.as_deref());
        let mut finais = reconciliar_mapa(&cand, corroborado);

        // 7. Autores contra a Homologada (nunca une homônimos sozinho) e `author_sort`.
        let mut notas_autor: Vec<String> = Vec::new();
        if let Some(c) = finais.iter_mut().find(|c| c.campo == Campo::Autores) {
            let nomes = lista_final(c);
            let tags_final: Vec<String> = finais_tags(&cand, &sugestoes);
            let editora = editora_rasc.clone();
            let mut novos = Vec::with_capacity(nomes.len());
            for n in &nomes {
                let ctx = ContextoAutor {
                    coautores: nomes.iter().filter(|o| *o != n).cloned().collect(),
                    assuntos: tags_final.clone(),
                    editora: editora.clone(),
                };
                match vocab.map(|v| v.resolver_autor(n, &ctx)) {
                    Some(ResolucaoAutor::Reutilizar { candidato }) => {
                        if candidato.nome != *n {
                            notas_autor.push(format!(
                                "autor «{n}» é «{}» na Homologada: grafia dela adotada",
                                candidato.nome
                            ));
                        }
                        novos.push(candidato.nome);
                    }
                    Some(ResolucaoAutor::Revisao { candidatos, motivo }) => {
                        let nomes_c: Vec<String> =
                            candidatos.iter().map(|k| k.nome.clone()).collect();
                        notas_autor.push(format!(
                            "autor «{n}»: {motivo} (candidatos: {})",
                            nomes_c.join("; ")
                        ));
                        c.precisa_revisao = true;
                        novos.push(n.clone());
                    }
                    Some(ResolucaoAutor::Novo) | None => novos.push(n.clone()),
                }
            }
            if !novos.is_empty() {
                c.valor = Some(juntar_lista(Campo::Autores, &novos));
                c.lista = novos;
            }
        }

        // 8. Obra > Edição > Formato (idempotentes por chave).
        let titulo = texto_de(&finais, Campo::Titulo)
            .unwrap_or(nome_alvo)
            .to_owned();
        let autores = lista_de(&finais, Campo::Autores);
        let isbn_final = texto_de(&finais, Campo::Isbn)
            .and_then(isbn::normalizar)
            .or_else(|| texto_de(&finais, Campo::Isbn).map(str::to_owned));
        let editora = texto_de(&finais, Campo::Editora).map(str::to_owned);
        let idioma = texto_de(&finais, Campo::Idioma).map(str::to_owned);
        let serie = texto_de(&finais, Campo::Serie).map(str::to_owned);
        let tnorm = store::normalizar_texto(&titulo);
        let anorm = autores
            .first()
            .map(|a| store::normalizar_texto(a))
            .unwrap_or_default();
        let chave_obra = format!("t:{tnorm}|a:{anorm}");
        let chave_edicao = isbn_final
            .as_ref()
            .map_or_else(|| chave_obra.clone(), |i| format!("isbn:{i}"));
        let conn = self.conexao();
        let obra = store::garantir_obra(
            conn,
            &store::NovaObra {
                chave: &chave_obra,
                titulo: &titulo,
                idioma_original: None,
            },
            agora,
        )?;
        let edicao = store::garantir_edicao(
            conn,
            &store::NovaEdicao {
                obra_id: obra,
                chave: &chave_edicao,
                titulo: &titulo,
                editora: editora.as_deref(),
                idioma: idioma.as_deref(),
                isbn13: isbn_final.as_deref(),
                ..store::NovaEdicao::default()
            },
            agora,
        )?;
        for (i, a) in autores.iter().enumerate() {
            let sort = author_sort(a);
            let id = store::garantir_autor(
                conn,
                &store::NovoAutor {
                    nome: a,
                    ordenacao: Some(&sort),
                    ..store::NovoAutor::default()
                },
                agora,
            )?;
            store::vincular_autoria(conn, edicao, id, store::PapelAutoria::Autor, i as i64)?;
        }
        // Formato de Membro: a chave é o hash do Contêiner somado ao caminho interno.
        let hash_formato = if e.membro.is_empty() {
            e.hash_arquivo
        } else {
            let mut h = blake3::Hasher::new();
            h.update(&e.hash_arquivo);
            h.update(&[0]);
            h.update(e.membro.as_bytes());
            *h.finalize().as_bytes()
        };
        store::garantir_formato_biblio(
            conn,
            &store::NovoFormatoBiblio {
                edicao_id: edicao,
                formato: e.formato,
                file_id: Some(file_id),
                hash: hash_formato,
                tamanho: e.tamanho,
                estado: store::EstadoFormatoBiblio::Candidato,
            },
            agora,
        )?;

        // 9. Sugestões: uma por campo (a reconciliada), com motivos e alternativas.
        let mut manter = Vec::new();
        for c in &finais {
            if c.valor.as_deref().is_none_or(|t| t.trim().is_empty()) && c.lista.is_empty() {
                continue;
            }
            let lista = if e_lista(c.campo) {
                lista_final(c)
            } else {
                Vec::new()
            };
            let valor = if e_lista(c.campo) {
                Some(juntar_lista(c.campo, &lista))
            } else {
                c.valor.clone()
            };
            let origem = c
                .origem_vencedora
                .unwrap_or(OrigemEvidencia::MetadadoInterno);
            let mut evidencias = c.evidencias.clone();
            let mut motivos = c.motivos.clone();
            if c.campo == Campo::Autores {
                motivos.extend(notas_autor.iter().cloned());
            }
            for (campo, m) in &extras {
                if campo.is_none_or(|k| k == c.campo) {
                    motivos.push(m.clone());
                }
            }
            if c.precisa_revisao {
                for a in &c.alternativas {
                    let t = a.valor.clone().unwrap_or_else(|| a.lista.join(" & "));
                    motivos.push(format!("alternativa «{t}» ({:?})", a.origens));
                }
            }
            evidencias.extend(motivos.iter().map(|m| evidencia_revisao(m)));
            let v = ValorSugerido {
                campo: c.campo,
                valor,
                lista,
                confianca: c.confianca.clamp(0.0, 1.0),
                evidencias,
            };
            let m = if origem == OrigemEvidencia::Ia {
                modelo.as_str()
            } else {
                ""
            };
            let mut s = store::NovaSugestao::de_valor(file_id, &v, origem, m, &versao_item);
            s.edicao_id = Some(edicao);
            if origem == OrigemEvidencia::Humano {
                // O humano já decidiu: guarda o valor dele como decidido e não pede de novo.
                let ja = store::sugestoes_do_item(conn, file_id, e.membro)?
                    .iter()
                    .any(|x| {
                        x.campo == c.campo
                            && matches!(
                                x.estado,
                                store::EstadoSugestao::Aceita | store::EstadoSugestao::Corrigida
                            )
                    });
                if ja {
                    continue;
                }
                let id = store::gravar_sugestao(conn, &s, agora)?;
                let estado = match humanos.get(&c.campo) {
                    Some(store::AcaoDecisao::Aceitar) => store::EstadoSugestao::Aceita,
                    _ => store::EstadoSugestao::Corrigida,
                };
                store::marcar_estado_sugestao(conn, id, estado, agora)?;
                continue;
            }
            manter.push(store::gravar_sugestao(conn, &s, agora)?);
        }
        store::limpar_pendentes_exceto(conn, file_id, &versao_item, &manter)?;

        // 10. Identidade bibliográfica: Mesma Edição / Mesma Obra / Incerta contra o espelho.
        let finais_livros = livros_candidatos(conn, isbn_final.as_deref(), &titulo)?;
        let a = RegistroBibliografico {
            formato: Some(e.formato),
            hash: e.membro.is_empty().then(|| hex(&e.hash_arquivo)),
            tamanho: e.tamanho,
            ..registro(
                if e.membro.is_empty() {
                    file_id.to_string()
                } else {
                    format!("{file_id}!{}", e.membro)
                },
                &titulo,
                &autores,
                editora.as_deref(),
                idioma.as_deref(),
                isbn_final.as_deref(),
                serie.as_deref(),
            )
        };
        for l in &finais_livros {
            let v = comparar(&a, &registro_do_livro(l));
            let Some(tipo) = v.tipo else { continue };
            if tipo == TipoDuplicidade::Exata {
                continue; // exata só por hash, já tratada antes
            }
            let formato_ref: Option<i64> = conn
                .query_row(
                    "SELECT id FROM formatos_homologados WHERE livro_id = ?1 ORDER BY id LIMIT 1",
                    [l.id],
                    |r| r.get(0),
                )
                .ok();
            let Some(alvo) = formato_ref else { continue };
            store::registrar_candidato(
                conn,
                &store::NovoCandidato {
                    file_id,
                    alvo: store::AlvoDuplicidade::FormatoHomologado(alvo),
                    tipo,
                    evidencias_json: serde_json::json!({
                        "evidencias": v.evidencias, "contra": v.contraevidencias,
                        "similaridade_titulo": v.similaridade_titulo, "livro": l.titulo,
                        "membro": (!e.membro.is_empty()).then_some(e.membro),
                    })
                    .to_string(),
                    risco: risco_store(v.risco),
                    bytes: 0,
                },
                agora,
            )?;
            let tipo_alerta = match tipo {
                TipoDuplicidade::MesmaEdicao => Some(store::TipoAlerta::MesmaEdicaoOutroFormato),
                TipoDuplicidade::MesmaObra => Some(store::TipoAlerta::PossivelMesmaObra),
                _ => None,
            };
            if let Some(tipo_alerta) = tipo_alerta {
                store::emitir_alerta(
                    conn,
                    &store::NovoAlerta {
                        chave: Some(format!("bib:{file_id}:{}:{}", e.membro, l.id)),
                        tipo: tipo_alerta,
                        file_id: Some(file_id),
                        dados_json: serde_json::json!({
                            "livro_id": l.id, "titulo": l.titulo,
                            "membro": (!e.membro.is_empty()).then_some(e.membro),
                        })
                        .to_string(),
                    },
                    agora,
                )?;
            }
        }
        self.impressao_textual(e, agora)?;
        Ok(SaidaItem { usou_ia })
    }

    /// B-14: impressão digital do texto das páginas e hipótese de mesma Edição/Obra contra
    /// itens parecidos já processados (pré-filtro LSH por índice, com teto: nunca quadrático).
    /// É só evidência: o candidato nunca é Exata, tem `bytes = 0` e não autoriza descarte.
    fn impressao_textual(&mut self, e: &EntradaItem<'_>, agora: i64) -> Result<(), ErroAgente> {
        use dag_biblio::impressao::{calcular, hipotese, jaccard_estimado};
        const TETO_CANDIDATOS: usize = 20;
        const MIN_BANDAS: u32 = 3;
        let Some(imp) = calcular(e.resultado.paginas.iter().map(|p| p.texto.as_str())) else {
            return Ok(());
        };
        let file_id = e.info.file_id;
        let conn = self.conexao();
        store::gravar_impressao(conn, file_id, e.membro, &imp, agora)?;
        for c in store::candidatos_por_impressao(conn, file_id, &imp, MIN_BANDAS, TETO_CANDIDATOS)?
        {
            let j = jaccard_estimado(&imp, &c.impressao);
            let Some(h) = hipotese(j) else { continue };
            store::registrar_candidato(
                conn,
                &store::NovoCandidato {
                    file_id,
                    alvo: store::AlvoDuplicidade::Arquivo(c.file_id),
                    tipo: h.tipo,
                    evidencias_json: serde_json::json!({
                        "evidencias": [h.evidencia],
                        "origem": "impressao_textual",
                        "jaccard_estimado": j,
                        "bandas_em_comum": c.bandas_em_comum,
                        "membro": (!e.membro.is_empty()).then_some(e.membro),
                        "membro_alvo": (!c.membro.is_empty()).then_some(c.membro.as_str()),
                    })
                    .to_string(),
                    risco: risco_store(h.risco),
                    bytes: 0,
                },
                agora,
            )?;
        }
        Ok(())
    }
}

/// Tags já candidatas (sugestões do vocabulário), para dar contexto à resolução de autores.
fn finais_tags(
    cand: &BTreeMap<Campo, Vec<Candidato>>,
    s: &dag_biblio::vocabulario::Sugestoes,
) -> Vec<String> {
    let mut v: Vec<String> = s.tags.iter().map(|t| t.tag.clone()).collect();
    for c in cand.get(&Campo::Tags).into_iter().flatten() {
        for t in &c.lista {
            if !v.contains(t) {
                v.push(t.clone());
            }
        }
    }
    v
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn listas_dividem_por_barra_ou_separador_do_campo() {
        assert_eq!(dividir_lista(Campo::Tags, "a|b"), vec!["a", "b"]);
        assert_eq!(dividir_lista(Campo::Tags, "a, b"), vec!["a", "b"]);
        assert_eq!(dividir_lista(Campo::Autores, "A & B"), vec!["A", "B"]);
        assert_eq!(
            juntar_lista(Campo::Autores, &["A".into(), "B".into()]),
            "A & B"
        );
    }

    #[test]
    fn titulo_do_nome_tira_extensao_e_separadores() {
        assert_eq!(titulo_do_nome("Dom_Casmurro.epub"), "Dom Casmurro");
        assert_eq!(titulo_do_nome("a.zip!/x/Livro Bom.pdf"), "Livro Bom");
    }
}
