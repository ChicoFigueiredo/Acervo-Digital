//! Fila de revisão humana: sugestões lado a lado com o extraído e o homologado; decisões em lote
//! (desfazíveis). Decidir nunca escreve em arquivo nem em biblioteca Calibre.

use dag_biblio::{Campo, FaixaConfianca, ResultadoExtracao};
use dag_core::{
    AcaoRevisao, AlvoRevisao, Autor, CampoLadoALado, DadosBiblioteca, ItemDecisao, ItemRevisaoInfo,
    ParecidoHomologado, Resposta,
};
use dag_store::biblioteca as store;

use super::duplicatas::livro_ref;
use super::item::PREFIXO_REVISAO;
use super::{dados, hex, limite_pagina};
use crate::agente::Agente;
use crate::erro::ErroAgente;

pub(super) fn nome_campo(c: Campo) -> String {
    serde_json::to_value(c)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn campo_de(nome: &str) -> Option<Campo> {
    serde_json::from_value(serde_json::Value::String(nome.to_owned())).ok()
}

fn autor_acao(a: Autor) -> store::AutorAcao {
    match a {
        Autor::Gui => store::AutorAcao::Gui,
        Autor::Cli => store::AutorAcao::Cli,
        Autor::Mcp => store::AutorAcao::Mcp,
    }
}

fn texto_do_valor(v: &dag_biblio::ValorSugerido) -> Option<String> {
    v.valor.clone().or_else(|| {
        if v.lista.is_empty() {
            None
        } else {
            Some(v.lista.join(" & "))
        }
    })
}

/// Cursor da revisão: `file_id` ou `file_id:membro`.
fn cursor_item(c: Option<&str>) -> Option<(i64, String)> {
    let c = c?;
    match c.split_once(':') {
        Some((f, m)) => Some((f.parse().ok()?, m.to_owned())),
        None => Some((c.parse().ok()?, String::new())),
    }
}

fn texto_cursor(f: i64, m: &str) -> String {
    if m.is_empty() {
        f.to_string()
    } else {
        format!("{f}:{m}")
    }
}

impl Agente {
    pub(super) fn revisao_biblioteca(
        &mut self,
        limite: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<Resposta, ErroAgente> {
        let conn = self.conexao();
        let pag = store::listar_revisao_itens(conn, cursor_item(cursor), limite_pagina(limite))?;
        let mut itens = Vec::with_capacity(pag.itens.len());
        for it in &pag.itens {
            let info = store::info_arquivo(conn, it.file_id)?;
            let extraido: Option<ResultadoExtracao> = conn
                .query_row(
                    "SELECT resultado FROM extracoes WHERE file_id = ?1 AND compressao = 'nenhuma'
                     AND resultado IS NOT NULL
                     AND CASE WHEN instr(versao_regras, '|m:') > 0
                              THEN substr(versao_regras, instr(versao_regras, '|m:') + 3) ELSE '' END = ?2
                     ORDER BY criado_em DESC LIMIT 1",
                    rusqlite::params![it.file_id, it.membro],
                    |l| l.get::<_, Vec<u8>>(0),
                )
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            let mut parecidos = Vec::new();
            for c in store::candidatos_do_arquivo(conn, it.file_id)? {
                if let store::AlvoDuplicidade::FormatoHomologado(f) = c.alvo
                    && let Ok(livro_id) = conn.query_row(
                        "SELECT livro_id FROM formatos_homologados WHERE id = ?1",
                        [f],
                        |l| l.get::<_, i64>(0),
                    )
                    && let Some(l) = store::obter_livro(conn, livro_id)?
                {
                    // Candidato de um Membro só vai ao item dele.
                    let do_membro = serde_json::from_str::<serde_json::Value>(&c.evidencias_json)
                        .ok()
                        .and_then(|v| v.get("membro").and_then(|m| m.as_str().map(str::to_owned)))
                        .unwrap_or_default();
                    if do_membro != it.membro {
                        continue;
                    }
                    parecidos.push(ParecidoHomologado {
                        livro: livro_ref(&l),
                        motivo: c.evidencias_json.clone(),
                        relacao: Some(format!("{:?}", c.tipo).to_lowercase()),
                    });
                }
            }
            let sugs = store::sugestoes_do_item(conn, it.file_id, &it.membro)?;
            let campos = sugs
                .iter()
                .map(|s| {
                    let nome = nome_campo(s.campo);
                    let ext = extraido.as_ref().and_then(|r| {
                        r.metadados
                            .iter()
                            .find(|m| m.campo == s.campo)
                            .and_then(texto_do_valor)
                    });
                    let homologado = parecidos.first().and_then(|p| match s.campo {
                        Campo::Titulo => Some(p.livro.titulo.clone()),
                        Campo::Autores => Some(p.livro.autores.join(" & ")),
                        Campo::Editora => p.livro.editora.clone(),
                        Campo::Idioma => p.livro.idioma.clone(),
                        Campo::Isbn => p.livro.isbn13.clone(),
                        _ => None,
                    });
                    let motivos: Vec<String> = s
                        .evidencias
                        .iter()
                        .filter_map(|e| {
                            e.detalhe
                                .as_deref()
                                .and_then(|d| d.strip_prefix(PREFIXO_REVISAO))
                                .map(str::to_owned)
                        })
                        .collect();
                    CampoLadoALado {
                        campo: nome,
                        extraido: ext,
                        sugerido: s.valor.clone(),
                        sugerido_lista: s.lista.clone(),
                        confianca: Some(s.confianca),
                        faixa: Some(
                            format!("{:?}", FaixaConfianca::de(s.confianca)).to_lowercase(),
                        ),
                        origem: Some(format!("{:?}", s.origem)),
                        modelo: (!s.modelo.is_empty()).then(|| s.modelo.clone()),
                        evidencias: s
                            .evidencias
                            .iter()
                            .filter(|e| {
                                !e.detalhe
                                    .as_deref()
                                    .is_some_and(|d| d.starts_with(PREFIXO_REVISAO))
                            })
                            .map(|e| match (&e.trecho, &e.detalhe) {
                                (Some(t), Some(d)) => format!("{t}: {d}"),
                                (Some(x), None) | (None, Some(x)) => x.clone(),
                                (None, None) => format!("{:?}", e.origem),
                            })
                            .collect(),
                        precisa_revisao: s.confianca < 0.9 || !motivos.is_empty(),
                        motivos,
                        homologado,
                        estado: format!("{:?}", s.estado).to_lowercase(),
                        sugestao_id: Some(s.id),
                    }
                })
                .collect();
            let od = sugs
                .iter()
                .find_map(|s| s.edicao_id)
                .map(|e| store::obter_edicao(conn, e).map(|x| x.map(|ed| (ed.obra_id, ed.id))))
                .transpose()?
                .flatten()
                .or(store::obra_edicao_do_arquivo(conn, it.file_id)?);
            let proposta = if it.membro.is_empty() && store::e_formato_homologado(conn, it.file_id)?
            {
                store::livro_do_arquivo(conn, it.file_id)?.map(|l| livro_ref(&l))
            } else {
                None
            };
            itens.push(ItemRevisaoInfo {
                file_id: it.file_id,
                caminho: info.as_ref().map(|i| i.caminho.clone()),
                tamanho: info.as_ref().map(|i| i.tamanho),
                hash: info.as_ref().and_then(|i| i.hash).map(|h| hex(&h)),
                formato: if it.membro.is_empty() {
                    info.as_ref()
                        .and_then(|i| i.nome.rsplit('.').next().map(str::to_uppercase))
                } else {
                    it.membro.rsplit('.').next().map(str::to_uppercase)
                },
                pendentes: it.pendentes,
                campos,
                homologados_parecidos: parecidos,
                obra_id: od.map(|x| x.0),
                edicao_id: od.map(|x| x.1),
                avisos: avisos_da_extracao(extraido.as_ref()),
                paginas: paginas_resumidas(extraido.as_ref()),
                paginas_ocr: extraido.as_ref().map_or(0, paginas_com_ocr),
                membro: (!it.membro.is_empty()).then(|| it.membro.clone()),
                proposta_homologada: proposta,
            });
        }
        Ok(dados(DadosBiblioteca::Revisao {
            itens,
            total_pendentes: store::contar_sugestoes_pendentes(conn)?,
            proximo_cursor: pag.proximo.map(|(f, m)| texto_cursor(f, &m)),
        }))
    }

    pub(super) fn decidir_biblioteca(
        &mut self,
        itens: &[ItemDecisao],
        lote: Option<&str>,
    ) -> Result<Resposta, ErroAgente> {
        let agora = self.agora_biblio();
        let autor = autor_acao(self.autor);
        let lote_id = lote.map_or_else(|| format!("lote-{agora}-{}", itens.len()), str::to_owned);
        let conn = self.conexao();
        let tx = conn.unchecked_transaction()?;
        let (mut aplicadas, mut ignoradas) = (0_u64, Vec::new());
        for it in itens {
            let Some(campo) = campo_de(&it.campo) else {
                ignoradas.push(format!("campo desconhecido: {}", it.campo));
                continue;
            };
            let acao = match it.acao {
                AcaoRevisao::Aceitar => store::AcaoDecisao::Aceitar,
                AcaoRevisao::Corrigir => store::AcaoDecisao::Corrigir,
                AcaoRevisao::Rejeitar => store::AcaoDecisao::Rejeitar,
            };
            if acao == store::AcaoDecisao::Corrigir && it.valor.is_none() {
                ignoradas.push(format!("{}: corrigir exige o valor", it.campo));
                continue;
            }
            let (alvo, sugestao) = match it.alvo {
                AlvoRevisao::Arquivo(f) => {
                    let s = store::sugestoes_do_arquivo(&tx, f)?
                        .into_iter()
                        .filter(|s| {
                            s.campo == campo
                                && s.estado == store::EstadoSugestao::Pendente
                                && store::membro_da_versao(&s.versao_regras).is_empty()
                        })
                        .max_by(|a, b| a.confianca.total_cmp(&b.confianca));
                    (store::AlvoDecisao::Arquivo(f), s)
                }
                // Um Membro de compactado se decide pela Edição dele: as sugestões pendentes
                // da Edição são as do livro (o arquivo é o do Contêiner, compartilhado).
                AlvoRevisao::Edicao(e) => {
                    let s = store::sugestoes_pendentes_da_edicao(&tx, e)?
                        .into_iter()
                        .filter(|s| s.campo == campo)
                        .max_by(|a, b| a.confianca.total_cmp(&b.confianca));
                    (store::AlvoDecisao::Edicao(e), s)
                }
                AlvoRevisao::Obra(o) => (store::AlvoDecisao::Obra(o), None),
            };
            if sugestao.is_none() && acao != store::AcaoDecisao::Corrigir {
                ignoradas.push(format!("{}: nenhuma sugestão pendente", it.campo));
                continue;
            }
            let anterior = sugestao.as_ref().and_then(|s| s.valor.clone());
            let novo = match acao {
                store::AcaoDecisao::Aceitar => anterior.clone(),
                store::AcaoDecisao::Corrigir => it.valor.clone(),
                store::AcaoDecisao::Rejeitar => None,
            };
            store::registrar_decisao(
                &tx,
                &store::NovaDecisao {
                    lote_id: Some(lote_id.clone()),
                    alvo,
                    sugestao_id: sugestao.map(|s| s.id),
                    campo,
                    acao,
                    valor_anterior: anterior,
                    valor_novo: novo,
                    autor,
                },
                agora,
            )?;
            aplicadas += 1;
        }
        tx.commit()?;
        Ok(dados(DadosBiblioteca::Decidida {
            lote: lote_id,
            aplicadas,
            ignoradas,
        }))
    }

    pub(super) fn desfazer_biblioteca(&mut self, lote: &str) -> Result<Resposta, ErroAgente> {
        let agora = self.agora_biblio();
        let n = store::desfazer_lote(self.conexao(), lote, agora)?;
        Ok(dados(DadosBiblioteca::Desfeita {
            lote: lote.to_owned(),
            desfeitas: n,
        }))
    }
}

/// Páginas lidas com OCR (a lista `paginas_ocr` do resultado, ou as páginas marcadas `via_ocr`).
fn paginas_com_ocr(r: &ResultadoExtracao) -> u32 {
    let marcadas = r.paginas.iter().filter(|p| p.via_ocr).count();
    u32::try_from(r.paginas_ocr.len().max(marcadas)).unwrap_or(u32::MAX)
}

/// Resumo das páginas extraídas: número, se veio de OCR, qualidade e um trecho curto.
fn paginas_resumidas(r: Option<&ResultadoExtracao>) -> Vec<dag_core::biblioteca::PaginaResumo> {
    r.map(|r| {
        r.paginas
            .iter()
            .map(|p| dag_core::biblioteca::PaginaResumo {
                numero: p.numero,
                via_ocr: p.via_ocr,
                qualidade: p.qualidade,
                trecho: p
                    .texto
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(160)
                    .collect(),
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Avisos da extração, com a marca de OCR em primeiro lugar (a CLI distingue PDF imagem de PDF
/// textual por ela).
fn avisos_da_extracao(r: Option<&ResultadoExtracao>) -> Vec<String> {
    let Some(r) = r else { return Vec::new() };
    let mut v = Vec::new();
    let ocr = paginas_com_ocr(r);
    if ocr > 0 {
        let total = r
            .total_paginas
            .map_or(String::new(), |t| format!(" de {t}"));
        v.push(format!("ocr: {ocr}{total} páginas lidas por OCR"));
    }
    for a in &r.avisos {
        if !v.contains(a) {
            v.push(a.clone());
        }
    }
    v
}
