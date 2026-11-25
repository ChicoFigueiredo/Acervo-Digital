//! Fila de revisão humana: sugestões lado a lado com o extraído e o homologado; decisões em lote
//! (desfazíveis). Decidir nunca escreve em arquivo nem em biblioteca Calibre.

use dag_biblio::{Campo, FaixaConfianca, ResultadoExtracao};
use dag_core::{
    AcaoRevisao, AlvoRevisao, Autor, CampoLadoALado, DadosBiblioteca, ItemDecisao, ItemRevisaoInfo,
    ParecidoHomologado, Resposta,
};
use dag_store::biblioteca as store;

use super::duplicatas::livro_ref;
use super::{cursor_i64, dados, hex, limite_pagina};
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

impl Agente {
    pub(super) fn revisao_biblioteca(
        &mut self,
        limite: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<Resposta, ErroAgente> {
        let conn = self.conexao();
        let pag = store::listar_revisao_pendente(conn, cursor_i64(cursor), limite_pagina(limite))?;
        let mut itens = Vec::with_capacity(pag.itens.len());
        for it in &pag.itens {
            let info = store::info_arquivo(conn, it.file_id)?;
            let extraido: Option<ResultadoExtracao> = conn
                .query_row(
                    "SELECT resultado FROM extracoes WHERE file_id = ?1 AND compressao = 'nenhuma'
                     AND resultado IS NOT NULL ORDER BY criado_em DESC LIMIT 1",
                    [it.file_id],
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
                    parecidos.push(ParecidoHomologado {
                        livro: livro_ref(&l),
                        motivo: c.evidencias_json.clone(),
                        relacao: Some(format!("{:?}", c.tipo).to_lowercase()),
                    });
                }
            }
            let sugs = store::sugestoes_do_arquivo(conn, it.file_id)?;
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
                            .map(|e| {
                                e.trecho
                                    .clone()
                                    .or_else(|| e.detalhe.clone())
                                    .unwrap_or_else(|| format!("{:?}", e.origem))
                            })
                            .collect(),
                        precisa_revisao: s.confianca < 0.9,
                        motivos: Vec::new(),
                        homologado,
                        estado: format!("{:?}", s.estado).to_lowercase(),
                        sugestao_id: Some(s.id),
                    }
                })
                .collect();
            let od = store::obra_edicao_do_arquivo(conn, it.file_id)?;
            itens.push(ItemRevisaoInfo {
                file_id: it.file_id,
                caminho: info.as_ref().map(|i| i.caminho.clone()),
                tamanho: info.as_ref().map(|i| i.tamanho),
                hash: info.as_ref().and_then(|i| i.hash).map(|h| hex(&h)),
                formato: info
                    .as_ref()
                    .and_then(|i| i.nome.rsplit('.').next().map(str::to_uppercase)),
                pendentes: it.pendentes,
                campos,
                homologados_parecidos: parecidos,
                obra_id: od.map(|x| x.0),
                edicao_id: od.map(|x| x.1),
                avisos: Vec::new(),
            });
        }
        Ok(dados(DadosBiblioteca::Revisao {
            itens,
            total_pendentes: store::contar_sugestoes_pendentes(conn)?,
            proximo_cursor: pag.proximo.map(|i| i.to_string()),
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
                        .filter(|s| s.campo == campo && s.estado == store::EstadoSugestao::Pendente)
                        .max_by(|a, b| a.confianca.total_cmp(&b.confianca));
                    (store::AlvoDecisao::Arquivo(f), s)
                }
                AlvoRevisao::Edicao(e) => (store::AlvoDecisao::Edicao(e), None),
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
