//! Processamento da fila: duplicata exata antes de qualquer extração, extração reaproveitável
//! (CA-B11), uma sugestão reconciliada por campo, Obra > Edição > Formato idempotentes e
//! candidatos de duplicidade bibliográfica. Retomável: um job órfão volta à fila na abertura e a
//! extração já gravada é reaproveitada (CA-B13).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use dag_biblio::identidade::{RegistroBibliografico, comparar};
use dag_biblio::{
    Campo, EstadoLeitura, FormatoLivro, OrigemEvidencia, ResultadoExtracao, TipoDuplicidade,
    ValorSugerido,
};
use dag_biblio_ia::EntradaClassificacao;
use dag_core::{DadosBiblioteca, ProgressoBiblioteca, Resposta, ResumoProcessamento};
use dag_store::biblioteca as store;
use rusqlite::Connection;

use super::extracao::{ClassificadorIa, IaIndisponivel, IaLocal};
use super::{ConfigBiblioteca, blake3_arquivo, dados, formato_do_nome, hash_de_hex, hex};
use crate::agente::Agente;
use crate::erro::ErroAgente;

/// Como um job terminou.
enum Desfecho {
    DuplicataExata {
        bytes: u64,
    },
    Revisao {
        reaproveitado: bool,
        ocr: u64,
        paginas: u64,
        ia: bool,
    },
    Excecao,
}

/// Valor escolhido por campo (o de maior confiança).
fn melhor_por_campo(valores: Vec<ValorSugerido>) -> Vec<ValorSugerido> {
    let mut v: Vec<ValorSugerido> = Vec::new();
    for x in valores {
        if x.valor.is_none() && x.lista.is_empty() {
            continue;
        }
        match v.iter_mut().find(|y| y.campo == x.campo) {
            Some(y) if x.confianca > y.confianca => *y = x,
            Some(_) => {}
            None => v.push(x),
        }
    }
    v
}

fn origem_de(v: &ValorSugerido) -> OrigemEvidencia {
    v.evidencias
        .iter()
        .max_by(|a, b| a.confianca.total_cmp(&b.confianca))
        .map_or(OrigemEvidencia::MetadadoInterno, |e| e.origem)
}

fn valores_da_extracao(r: &ResultadoExtracao) -> Vec<ValorSugerido> {
    let mut v = r.metadados.clone();
    if let Some((isbn, ev)) = r
        .isbns
        .iter()
        .max_by(|a, b| a.1.confianca.total_cmp(&b.1.confianca))
    {
        v.push(ValorSugerido {
            campo: Campo::Isbn,
            valor: Some(isbn.clone()),
            lista: Vec::new(),
            confianca: ev.confianca,
            evidencias: vec![ev.clone()],
        });
    }
    v
}

fn texto(v: &[ValorSugerido], c: Campo) -> Option<&str> {
    v.iter()
        .find(|x| x.campo == c)
        .and_then(|x| x.valor.as_deref())
}

fn lista(v: &[ValorSugerido], c: Campo) -> Vec<String> {
    v.iter()
        .find(|x| x.campo == c)
        .map(|x| {
            if x.lista.is_empty() {
                x.valor
                    .as_deref()
                    .map(|t| t.split(" & ").map(str::to_owned).collect())
                    .unwrap_or_default()
            } else {
                x.lista.clone()
            }
        })
        .unwrap_or_default()
}

fn nome_ia(cfg: &ConfigBiblioteca, ia: Option<&Arc<dyn ClassificadorIa>>) -> String {
    ia.map_or_else(|| cfg.ia.modelo_primario.clone(), |i| i.nome())
}

impl Agente {
    fn ia_ativa(
        &mut self,
        cfg: &ConfigBiblioteca,
        pedida: bool,
    ) -> Option<Arc<dyn ClassificadorIa>> {
        if !(pedida || cfg.ia.ativa) {
            return None;
        }
        if let Some(i) = &self.biblio.ia {
            return Some(Arc::clone(i));
        }
        let nova: Arc<dyn ClassificadorIa> = match IaLocal::da_config(cfg) {
            Ok(i) => Arc::new(i),
            Err(e) => Arc::new(IaIndisponivel { motivo: e }),
        };
        self.biblio.ia = Some(Arc::clone(&nova));
        Some(nova)
    }

    pub(super) fn processar_biblioteca(
        &mut self,
        limite: Option<u32>,
        ia: bool,
    ) -> Result<Resposta, ErroAgente> {
        let cfg = self.config_biblioteca();
        let agora = self.agora_biblio();
        let orfaos = store::retomar_orfaos(self.conexao(), agora)?;
        let ia_cls = self.ia_ativa(&cfg, ia);
        let maximo = limite.map_or(u64::MAX, u64::from);
        let conta = store::contagem_jobs(self.conexao())?;
        let mut r = ResumoProcessamento {
            orfaos_retomados: orfaos,
            ..ResumoProcessamento::default()
        };
        let mut prog = ProgressoBiblioteca {
            itens_total: u64::try_from(conta.pendentes).unwrap_or(0).min(maximo),
            ..ProgressoBiblioteca::default()
        };
        while r.processados < maximo {
            if !self.seguir_biblioteca() {
                r.interrompido = true;
                break;
            }
            let agora = self.agora_biblio();
            let Some(job) = store::pegar_proximo_job(self.conexao(), agora)? else {
                break;
            };
            let info = store::info_arquivo(self.conexao(), job.file_id)?;
            prog.item_atual = info.as_ref().map(|i| i.caminho.clone());
            let t0 = Instant::now();
            let res = catch_unwind(AssertUnwindSafe(|| {
                self.processar_um(&job, &cfg, ia_cls.as_ref(), &mut r)
            }))
            .unwrap_or_else(|_| {
                Err(ErroAgente::biblio(
                    "pane",
                    "a extração falhou de forma inesperada",
                ))
            });
            let conn = self.conexao();
            match res {
                Ok(Desfecho::DuplicataExata { bytes }) => {
                    store::concluir_job(conn, job.id, Some(&cfg.versao_regras), agora)?;
                    r.duplicatas_exatas += 1;
                    r.espaco_potencial_bytes += bytes;
                    prog.duplicatas_exatas += 1;
                    prog.espaco_potencial_bytes += bytes;
                }
                Ok(Desfecho::Revisao {
                    reaproveitado,
                    ocr,
                    paginas,
                    ia: usou,
                }) => {
                    store::concluir_job(conn, job.id, Some(&cfg.versao_regras), agora)?;
                    r.revisao_pendente += 1;
                    if reaproveitado {
                        r.reaproveitados += 1;
                    } else {
                        r.extraidos += 1;
                    }
                    r.paginas += paginas;
                    r.paginas_ocr += ocr;
                    r.consultas_ia += u64::from(usou);
                    prog.paginas += paginas;
                    prog.paginas_ocr += ocr;
                }
                Ok(Desfecho::Excecao) => {
                    r.excecoes += 1;
                    prog.excecoes += 1;
                }
                Err(e) => {
                    let estado =
                        store::falhar_job(conn, job.id, &e.to_string(), cfg.tentativas_max, agora)?;
                    if estado == store::EstadoJob::Falhou {
                        r.falhas += 1;
                        prog.falhas += 1;
                        let _ = store::emitir_alerta(
                            conn,
                            &store::NovoAlerta {
                                chave: Some(format!("extracao_falhou:{}", job.file_id)),
                                tipo: store::TipoAlerta::ExtracaoFalhou,
                                file_id: Some(job.file_id),
                                dados_json: serde_json::json!({"erro": e.to_string()}).to_string(),
                            },
                            agora,
                        );
                    }
                }
            }
            r.processados += 1;
            if let Some(i) = &info {
                r.bytes += i.tamanho;
                prog.bytes += i.tamanho;
            }
            prog.itens_feitos = r.processados;
            let forcar = t0.elapsed().as_secs() >= 1;
            self.progresso_biblioteca(&prog, forcar);
        }
        r.pendentes_restantes = store::contagem_jobs(self.conexao())?.pendentes;
        self.progresso_biblioteca(&prog, true);
        Ok(dados(DadosBiblioteca::Processada { resumo: r }))
    }

    fn processar_um(
        &mut self,
        job: &store::Job,
        cfg: &ConfigBiblioteca,
        ia: Option<&Arc<dyn ClassificadorIa>>,
        resumo: &mut ResumoProcessamento,
    ) -> Result<Desfecho, ErroAgente> {
        let agora = self.agora_biblio();
        let conn = self.conexao();
        let info = store::info_arquivo(conn, job.file_id)?
            .ok_or_else(|| ErroAgente::biblio("arquivo_inexistente", "arquivo fora do Catálogo"))?;
        let caminho = Path::new(&info.caminho).to_path_buf();
        let formato = formato_do_nome(&info.nome)
            .ok_or_else(|| ErroAgente::biblio("formato_nao_bibliografico", info.nome.clone()))?;
        if !caminho.is_file() {
            return Err(ErroAgente::biblio(
                "arquivo_ausente",
                format!("{} não existe mais", caminho.display()),
            ));
        }
        // Hash: o do Catálogo se a evidência vale; senão lê o arquivo (só leitura).
        let hash = match info.hash {
            Some(h) => h,
            None => blake3_arquivo(&caminho).map_err(|e| {
                ErroAgente::biblio("leitura", format!("{}: {e}", caminho.display()))
            })?,
        };
        // Já mudou desde a última extração? Sugestões do conteúdo antigo saem (CA-B12).
        if let Some(antigo) = store::hash_da_extracao(conn, job.file_id)?
            && antigo != hash
        {
            store::desassociar_conteudo(conn, job.file_id)?;
        }
        // 1. Duplicata exata ANTES de qualquer extração/OCR (CA-B04).
        if info.tamanho >= cfg.tamanho_minimo_duplicata {
            let achados = store::buscar_por_hash(conn, &hash, Some(info.tamanho))?;
            if let Some(a) = achados.first() {
                store::registrar_candidato(
                    conn,
                    &store::NovoCandidato {
                        file_id: info.file_id,
                        alvo: store::AlvoDuplicidade::FormatoHomologado(a.formato.id),
                        tipo: TipoDuplicidade::Exata,
                        evidencias_json:
                            serde_json::json!({"hash": hex(&hash), "tamanho": info.tamanho})
                                .to_string(),
                        risco: store::RiscoDuplicidade::Baixo,
                        bytes: info.tamanho,
                    },
                    agora,
                )?;
                let (_, criado) = store::emitir_alerta(
                    conn,
                    &store::NovoAlerta {
                        chave: Some(format!("ja_tem:{}:{}", info.file_id, a.formato.id)),
                        tipo: store::TipoAlerta::JaTemEsteLivro,
                        file_id: Some(info.file_id),
                        dados_json: serde_json::json!({
                            "livro": a.livro.titulo, "livro_id": a.livro.id,
                            "formato_id": a.formato.id, "caminho": info.caminho,
                        })
                        .to_string(),
                    },
                    agora,
                )?;
                if criado {
                    self.mensagem(format!(
                        "alerta: você já tem este livro — {}",
                        a.livro.titulo
                    ));
                }
                store::gravar_checkpoint(self.conexao(), job.id, "duplicata_exata", None, agora)?;
                return Ok(Desfecho::DuplicataExata {
                    bytes: info.tamanho,
                });
            }
        }
        // 2. Extração: reaproveita a vigente (mesmo hash e regras) ou extrai.
        let conn = self.conexao();
        let vigente = store::extracao_vigente(conn, job.file_id, &cfg.versao_regras, &hash)?;
        let (rext, reaproveitado, capa, ocr): (ResultadoExtracao, bool, Option<Vec<u8>>, u64) =
            if let Some(Some(r)) = vigente
                .as_ref()
                .map(|e| e.resultado_tipado().ok().flatten())
            {
                (r, true, None, 0)
            } else {
                store::gravar_checkpoint(conn, job.id, "extraindo", None, agora)?;
                let t0 = Instant::now();
                let s = self
                    .biblio
                    .extrator
                    .extrair(&caminho, formato, cfg)
                    .map_err(|e| ErroAgente::biblio("extracao", e))?;
                let ms = u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX);
                store::gravar_resultado_extracao(
                    self.conexao(),
                    Some(job.id),
                    job.file_id,
                    &s.resultado,
                    Some(hash),
                    &cfg.versao_regras,
                    ms,
                    agora,
                )?;
                store::gravar_checkpoint(self.conexao(), job.id, "extraido", None, agora)?;
                (s.resultado, false, s.capa, u64::from(s.paginas_ocr))
            };
        let conn = self.conexao();
        let paginas = rext.paginas.len() as u64;
        // 3. Exceções: DRM, senha, ilegível, limite. Nunca contorna; vai para revisão humana.
        if !matches!(rext.estado, EstadoLeitura::Lido) {
            let motivo = match &rext.estado {
                EstadoLeitura::Drm => "drm".to_owned(),
                EstadoLeitura::Senha => "senha".to_owned(),
                EstadoLeitura::Corrompido { motivo } => format!("corrompido: {motivo}"),
                EstadoLeitura::NaoSuportado { motivo } => format!("não suportado: {motivo}"),
                EstadoLeitura::LimiteExcedido { limite } => format!("limite excedido: {limite}"),
                EstadoLeitura::Lido => String::new(),
            };
            store::marcar_excecao(conn, job.id, &motivo, agora)?;
            let _ = store::emitir_alerta(
                conn,
                &store::NovoAlerta {
                    chave: Some(format!("excecao:{}", job.file_id)),
                    tipo: store::TipoAlerta::ExtracaoFalhou,
                    file_id: Some(job.file_id),
                    dados_json: serde_json::json!({"motivo": motivo}).to_string(),
                },
                agora,
            );
            return Ok(Desfecho::Excecao);
        }
        // 4. Valores: extração + (opcional) IA; um por campo, o de maior confiança.
        let mut valores = valores_da_extracao(&rext);
        let mut usou_ia = false;
        let mut modelo = String::new();
        if let Some(c) = ia {
            let entrada = EntradaClassificacao {
                id: job.file_id.to_string(),
                nome_arquivo: info.nome.clone(),
                extracao: rext.clone(),
                vocabulario: dag_biblio_ia::VocabularioHomologado::default(),
                exemplos: Vec::new(),
                referencias: Vec::new(),
                imagem_pagina1: capa,
            };
            match c.classificar(&entrada) {
                Ok(res) => {
                    usou_ia = true;
                    resumo.consultas_ia += 0;
                    modelo = nome_ia(cfg, ia);
                    for reg in &res.registros {
                        if let Some(h) = hash_de_hex(&reg.hash_prompt) {
                            let _ = store::gravar_inferencia(
                                self.conexao(),
                                &store::NovaInferencia {
                                    job_id: Some(job.id),
                                    file_id: Some(job.file_id),
                                    modelo: reg.modelo.clone(),
                                    versao_modelo: reg.versao_modelo.clone(),
                                    hash_prompt: h,
                                    tokens_entrada: 0,
                                    tokens_saida: 0,
                                    tempo_ms: 0,
                                    bruto_json: None,
                                },
                                agora,
                            );
                        }
                    }
                    valores.extend(res.valores);
                }
                Err(e) => {
                    self.mensagem(format!("IA indisponível ({e}); seguindo só com a extração"))
                }
            }
        }
        let escolhidos = melhor_por_campo(valores);
        // 5. Obra > Edição > Formato (idempotentes por chave).
        let conn = self.conexao();
        let titulo = texto(&escolhidos, Campo::Titulo)
            .unwrap_or(&info.nome)
            .to_owned();
        let autores = lista(&escolhidos, Campo::Autores);
        let isbn = texto(&escolhidos, Campo::Isbn).map(str::to_owned);
        let tnorm = store::normalizar_texto(&titulo);
        let anorm = autores
            .first()
            .map(|a| store::normalizar_texto(a))
            .unwrap_or_default();
        let chave_obra = format!("t:{tnorm}|a:{anorm}");
        let chave_edicao = isbn
            .as_ref()
            .map_or_else(|| chave_obra.clone(), |i| format!("isbn:{i}"));
        let obra = store::garantir_obra(
            conn,
            &store::NovaObra {
                chave: &chave_obra,
                titulo: &titulo,
                idioma_original: None,
            },
            agora,
        )?;
        let editora = texto(&escolhidos, Campo::Editora).map(str::to_owned);
        let idioma = texto(&escolhidos, Campo::Idioma).map(str::to_owned);
        let edicao = store::garantir_edicao(
            conn,
            &store::NovaEdicao {
                obra_id: obra,
                chave: &chave_edicao,
                titulo: &titulo,
                editora: editora.as_deref(),
                idioma: idioma.as_deref(),
                isbn13: isbn.as_deref(),
                ..store::NovaEdicao::default()
            },
            agora,
        )?;
        for (i, a) in autores.iter().enumerate() {
            let id = store::garantir_autor(
                conn,
                &store::NovoAutor {
                    nome: a,
                    ..store::NovoAutor::default()
                },
                agora,
            )?;
            store::vincular_autoria(conn, edicao, id, store::PapelAutoria::Autor, i as i64)?;
        }
        store::garantir_formato_biblio(
            conn,
            &store::NovoFormatoBiblio {
                edicao_id: edicao,
                formato: formato_para(formato),
                file_id: Some(job.file_id),
                hash,
                tamanho: info.tamanho,
                estado: store::EstadoFormatoBiblio::Candidato,
            },
            agora,
        )?;
        // 6. Sugestões (uma por campo; as pendentes antigas fora da escolha saem).
        let mut manter = Vec::new();
        for v in &escolhidos {
            let mut s = store::NovaSugestao::de_valor(
                job.file_id,
                v,
                origem_de(v),
                &modelo,
                &cfg.versao_regras,
            );
            s.edicao_id = Some(edicao);
            manter.push(store::gravar_sugestao(conn, &s, agora)?);
        }
        store::limpar_pendentes_exceto(conn, job.file_id, &cfg.versao_regras, &manter)?;
        // 7. Candidatos bibliográficos contra a Homologada.
        candidatos_bibliograficos(
            conn,
            &info,
            &titulo,
            &autores,
            editora.as_deref(),
            idioma.as_deref(),
            isbn.as_deref(),
            formato,
            &hash,
            agora,
        )?;
        store::gravar_checkpoint(conn, job.id, "revisao_pendente", None, agora)?;
        Ok(Desfecho::Revisao {
            reaproveitado,
            ocr,
            paginas,
            ia: usou_ia,
        })
    }
}

fn formato_para(f: FormatoLivro) -> FormatoLivro {
    f
}

#[allow(clippy::too_many_arguments)]
fn candidatos_bibliograficos(
    conn: &Connection,
    info: &store::InfoArquivo,
    titulo: &str,
    autores: &[String],
    editora: Option<&str>,
    idioma: Option<&str>,
    isbn: Option<&str>,
    formato: FormatoLivro,
    hash: &[u8; 32],
    agora: i64,
) -> Result<(), ErroAgente> {
    let mut vistos = Vec::new();
    let mut livros = Vec::new();
    if let Some(i) = isbn {
        livros.extend(store::buscar_livros_por_isbn(conn, i)?);
    }
    livros.extend(store::livros_por_titulo_norm(
        conn,
        &store::normalizar_texto(titulo),
        5,
    )?);
    let a = RegistroBibliografico {
        id: info.file_id.to_string(),
        titulo: titulo.to_owned(),
        autores: autores.to_vec(),
        editora: editora.map(str::to_owned),
        idioma: idioma.map(str::to_owned),
        isbns: isbn.map(str::to_owned).into_iter().collect(),
        formato: Some(formato),
        hash: Some(hex(hash)),
        tamanho: info.tamanho,
        ..RegistroBibliografico::default()
    };
    for l in livros {
        if vistos.contains(&l.id) {
            continue;
        }
        vistos.push(l.id);
        let b = RegistroBibliografico {
            id: format!("livro:{}", l.id),
            titulo: l.titulo.clone(),
            autores: l.autores.clone(),
            editora: l.editora.clone(),
            idioma: l.idioma.clone(),
            isbns: l.isbn13.clone().into_iter().collect(),
            serie: l.serie.clone(),
            indice_serie: l.indice,
            ..RegistroBibliografico::default()
        };
        let v = comparar(&a, &b);
        let Some(tipo) = v.tipo else { continue };
        if tipo == TipoDuplicidade::Exata {
            continue; // exata só por hash, já tratada antes
        }
        let risco = match v.risco {
            dag_biblio::identidade::Risco::Baixo => store::RiscoDuplicidade::Baixo,
            dag_biblio::identidade::Risco::Medio => store::RiscoDuplicidade::Medio,
            dag_biblio::identidade::Risco::Alto => store::RiscoDuplicidade::Alto,
        };
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
                file_id: info.file_id,
                alvo: store::AlvoDuplicidade::FormatoHomologado(alvo),
                tipo,
                evidencias_json: serde_json::json!({
                    "evidencias": v.evidencias, "contra": v.contraevidencias,
                    "similaridade_titulo": v.similaridade_titulo, "livro": l.titulo,
                })
                .to_string(),
                risco,
                bytes: 0,
            },
            agora,
        )?;
        let tipo_alerta = match tipo {
            TipoDuplicidade::MesmaEdicao => store::TipoAlerta::MesmaEdicaoOutroFormato,
            _ => store::TipoAlerta::PossivelMesmaObra,
        };
        store::emitir_alerta(
            conn,
            &store::NovoAlerta {
                chave: Some(format!("bib:{}:{}", info.file_id, l.id)),
                tipo: tipo_alerta,
                file_id: Some(info.file_id),
                dados_json: serde_json::json!({"livro_id": l.id, "titulo": l.titulo}).to_string(),
            },
            agora,
        )?;
    }
    Ok(())
}
