//! Processamento da fila: duplicata exata antes de qualquer extração, extração reaproveitável
//! (CA-B11), uma sugestão reconciliada por campo, Obra > Edição > Formato idempotentes e
//! candidatos de duplicidade bibliográfica. Retomável: um job órfão volta à fila na abertura e a
//! extração já gravada é reaproveitada (CA-B13).
//!
//! Extração em paralelo: no máximo um arquivo por Dispositivo Físico (até
//! `max_discos_extracao` discos) em threads; tudo o que escreve no Catálogo, classifica e fala com
//! a IA acontece na thread do agente (escritor único, ADR 0004). Pausa e cancelamento valem entre
//! itens: nada novo sai da fila depois do pedido e o que já está lendo termina.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use dag_biblio::{
    Campo, EstadoLeitura, FormatoLivro, OrigemEvidencia, ResultadoExtracao, TipoDuplicidade,
    ValorSugerido,
};
use dag_core::{DadosBiblioteca, ProgressoBiblioteca, Resposta, ResumoProcessamento};
use dag_store::biblioteca as store;

use super::extracao::{ClassificadorIa, IaIndisponivel, IaLocal, SaidaMembro};
use super::fila::formato_para_fila;
use super::item::EntradaItem;
use super::vocab::CacheVocab;
use super::{ConfigBiblioteca, blake3_arquivo, dados, hex};
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
        membros: u64,
    },
    Excecao,
}

/// Um livro extraído (do arquivo ou de um Membro de compactado).
struct ItemExtraido {
    membro: String,
    formato: FormatoLivro,
    tamanho: u64,
    resultado: ResultadoExtracao,
    capa: Option<Vec<u8>>,
    ocr: u64,
}

/// O que falta fazer com um job depois dos passos baratos.
enum Preparo {
    Feito(Desfecho),
    Reusar {
        info: store::InfoArquivo,
        hash: [u8; 32],
        itens: Vec<ItemExtraido>,
    },
    Extrair {
        info: store::InfoArquivo,
        hash: [u8; 32],
        caminho: PathBuf,
        formato: FormatoLivro,
    },
}

/// Extração pronta, vinda de uma thread.
struct Pronto {
    job: store::Job,
    dispositivo: i64,
    info: store::InfoArquivo,
    hash: [u8; 32],
    saidas: Result<Vec<SaidaMembro>, String>,
    ms: u64,
}

fn motivo_excecao(e: &EstadoLeitura) -> String {
    match e {
        EstadoLeitura::Drm => "drm".to_owned(),
        EstadoLeitura::Senha => "senha".to_owned(),
        EstadoLeitura::Corrompido { motivo } => format!("corrompido: {motivo}"),
        EstadoLeitura::NaoSuportado { motivo } => format!("não suportado: {motivo}"),
        EstadoLeitura::LimiteExcedido { limite } => format!("limite excedido: {limite}"),
        EstadoLeitura::Lido => String::new(),
    }
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
        let vocab = self.vocabulario(&cfg)?;
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
        let max_discos = cfg.max_discos_extracao.max(1) as usize;
        let (tx, rx): (Sender<Pronto>, Receiver<Pronto>) = channel();
        let mut ocupados: Vec<i64> = Vec::new();
        let mut em_voo = 0_usize;
        let mut pegos = 0_u64;
        loop {
            // Despacha enquanto couber: um arquivo por disco, até `max_discos` discos.
            let mut despachou = false;
            while pegos < maximo && em_voo < max_discos {
                if !self.seguir_biblioteca() {
                    r.interrompido = true;
                    break;
                }
                let agora = self.agora_biblio();
                let Some((job, disp)) =
                    store::pegar_proximo_job_fora(self.conexao(), agora, &ocupados)?
                else {
                    break;
                };
                pegos += 1;
                despachou = true;
                prog.item_atual =
                    store::info_arquivo(self.conexao(), job.file_id)?.map(|i| i.caminho);
                let t0 = Instant::now();
                let preparo = catch_unwind(AssertUnwindSafe(|| self.preparar(&job, &cfg)))
                    .unwrap_or_else(|_| {
                        Err(ErroAgente::biblio(
                            "pane",
                            "o preparo da extração falhou de forma inesperada",
                        ))
                    });
                match preparo {
                    Ok(Preparo::Extrair {
                        info,
                        hash,
                        caminho,
                        formato,
                    }) => {
                        ocupados.push(disp);
                        em_voo += 1;
                        let extrator = Arc::clone(&self.biblio.extrator);
                        let cfg2 = cfg.clone();
                        let tx2 = tx.clone();
                        std::thread::spawn(move || {
                            let t = Instant::now();
                            let saidas = catch_unwind(AssertUnwindSafe(|| {
                                extrator.extrair_membros(&caminho, formato, &cfg2)
                            }))
                            .unwrap_or_else(|_| {
                                Err("a extração falhou de forma inesperada".into())
                            });
                            let ms = u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX);
                            let _ = tx2.send(Pronto {
                                job,
                                dispositivo: disp,
                                info,
                                hash,
                                saidas,
                                ms,
                            });
                        });
                    }
                    Ok(Preparo::Feito(d)) => {
                        self.finalizar(
                            &job,
                            Ok(d),
                            &info_ou_none(self, job.file_id),
                            &cfg,
                            &mut r,
                            &mut prog,
                            t0,
                        )?;
                    }
                    Ok(Preparo::Reusar { info, hash, itens }) => {
                        let res = self.concluir_extracao(
                            &job,
                            &info,
                            hash,
                            itens,
                            true,
                            &cfg,
                            ia_cls.as_ref(),
                            vocab.as_ref(),
                        );
                        self.finalizar(&job, res, &Some(info), &cfg, &mut r, &mut prog, t0)?;
                    }
                    Err(e) => {
                        self.finalizar(
                            &job,
                            Err(e),
                            &info_ou_none(self, job.file_id),
                            &cfg,
                            &mut r,
                            &mut prog,
                            t0,
                        )?;
                    }
                }
            }
            if em_voo == 0 {
                if despachou {
                    continue; // o que saiu da fila já foi concluído sem extração
                }
                break;
            }
            // Espera uma extração (acordando para ver pausa/cancelamento).
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(p) => {
                    em_voo -= 1;
                    if let Some(i) = ocupados.iter().position(|d| *d == p.dispositivo) {
                        ocupados.swap_remove(i);
                    }
                    let t0 = Instant::now();
                    let Pronto {
                        job,
                        info,
                        hash,
                        saidas,
                        ms,
                        ..
                    } = p;
                    let res = match saidas {
                        Ok(s) => self.gravar_e_concluir(
                            &job,
                            &info,
                            hash,
                            s,
                            ms,
                            &cfg,
                            ia_cls.as_ref(),
                            vocab.as_ref(),
                        ),
                        Err(e) => Err(ErroAgente::biblio("extracao", e)),
                    };
                    self.finalizar(&job, res, &Some(info), &cfg, &mut r, &mut prog, t0)?;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        // Itens da Homologada (`!Catalogar` ou sem tag): só propostas, nada é escrito nela.
        if !r.interrompido && r.processados < maximo {
            let restante = maximo - r.processados;
            self.propostas_homologada(&cfg, vocab.as_ref(), restante, &mut r)?;
        }
        r.pendentes_restantes = store::contagem_jobs(self.conexao())?.pendentes;
        self.progresso_biblioteca(&prog, true);
        Ok(dados(DadosBiblioteca::Processada { resumo: r }))
    }

    /// Passos baratos de um job: arquivo e hash, duplicata exata (antes de qualquer extração) e
    /// extração já gravada. O que sobra é extrair.
    fn preparar(
        &mut self,
        job: &store::Job,
        cfg: &ConfigBiblioteca,
    ) -> Result<Preparo, ErroAgente> {
        let agora = self.agora_biblio();
        let conn = self.conexao();
        let info = store::info_arquivo(conn, job.file_id)?
            .ok_or_else(|| ErroAgente::biblio("arquivo_inexistente", "arquivo fora do Catálogo"))?;
        let caminho = Path::new(&info.caminho).to_path_buf();
        let formato = formato_para_fila(&info.nome)
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
                let titulo = a.livro.titulo.clone();
                store::gravar_checkpoint(conn, job.id, "duplicata_exata", None, agora)?;
                let bytes = info.tamanho;
                if criado {
                    self.mensagem(format!("alerta: você já tem este livro — {titulo}"));
                }
                return Ok(Preparo::Feito(Desfecho::DuplicataExata { bytes }));
            }
        }
        // 2. Extração: reaproveita a vigente (mesmo hash e regras) ou extrai.
        let vigentes =
            store::extracoes_vigentes_do_arquivo(conn, job.file_id, &cfg.versao_regras, &hash)?;
        if !vigentes.is_empty() {
            let mut itens = Vec::with_capacity(vigentes.len());
            let mut completo = true;
            for (membro, ex) in &vigentes {
                match ex.resultado_tipado().ok().flatten() {
                    Some(r) => itens.push(ItemExtraido {
                        membro: membro.clone(),
                        formato: r.formato,
                        tamanho: if membro.is_empty() { info.tamanho } else { 0 },
                        resultado: r,
                        capa: None,
                        ocr: 0,
                    }),
                    None => completo = false,
                }
            }
            if completo {
                return Ok(Preparo::Reusar { info, hash, itens });
            }
        }
        store::gravar_checkpoint(conn, job.id, "extraindo", None, agora)?;
        Ok(Preparo::Extrair {
            info,
            hash,
            caminho,
            formato,
        })
    }

    /// Grava a extração de cada livro e conclui o job.
    #[allow(clippy::too_many_arguments)]
    fn gravar_e_concluir(
        &mut self,
        job: &store::Job,
        info: &store::InfoArquivo,
        hash: [u8; 32],
        saidas: Vec<SaidaMembro>,
        ms: u64,
        cfg: &ConfigBiblioteca,
        ia: Option<&Arc<dyn ClassificadorIa>>,
        vocab: Option<&Arc<CacheVocab>>,
    ) -> Result<Desfecho, ErroAgente> {
        let agora = self.agora_biblio();
        let mut itens = Vec::with_capacity(saidas.len());
        for s in saidas {
            let versao = store::versao_do_item(&cfg.versao_regras, &s.caminho_interno);
            store::gravar_resultado_extracao(
                self.conexao(),
                Some(job.id),
                job.file_id,
                &s.saida.resultado,
                Some(hash),
                &versao,
                ms,
                agora,
            )?;
            itens.push(ItemExtraido {
                membro: s.caminho_interno,
                formato: s.formato,
                tamanho: if s.tamanho == 0 {
                    info.tamanho
                } else {
                    s.tamanho
                },
                resultado: s.saida.resultado,
                capa: s.saida.capa,
                ocr: u64::from(s.saida.paginas_ocr),
            });
        }
        store::gravar_checkpoint(self.conexao(), job.id, "extraido", None, agora)?;
        self.concluir_extracao(job, info, hash, itens, false, cfg, ia, vocab)
    }

    /// Cada livro extraído vira um item de revisão (um por Membro de compactado).
    #[allow(clippy::too_many_arguments)]
    fn concluir_extracao(
        &mut self,
        job: &store::Job,
        info: &store::InfoArquivo,
        hash: [u8; 32],
        itens: Vec<ItemExtraido>,
        reaproveitado: bool,
        cfg: &ConfigBiblioteca,
        ia: Option<&Arc<dyn ClassificadorIa>>,
        vocab: Option<&Arc<CacheVocab>>,
    ) -> Result<Desfecho, ErroAgente> {
        let agora = self.agora_biblio();
        let (mut ocr, mut paginas, mut membros) = (0_u64, 0_u64, 0_u64);
        let (mut revisao, mut usou_ia) = (0_u64, false);
        let mut motivos: Vec<String> = Vec::new();
        for it in itens {
            paginas += it.resultado.paginas.len() as u64;
            ocr += it.ocr;
            // Exceções: DRM, senha, ilegível, limite. Nunca contorna; vai para revisão humana.
            if !matches!(it.resultado.estado, EstadoLeitura::Lido) {
                let motivo = motivo_excecao(&it.resultado.estado);
                let _ = store::emitir_alerta(
                    self.conexao(),
                    &store::NovoAlerta {
                        chave: Some(format!("excecao:{}:{}", job.file_id, it.membro)),
                        tipo: store::TipoAlerta::ExtracaoFalhou,
                        file_id: Some(job.file_id),
                        dados_json: serde_json::json!({
                            "motivo": motivo,
                            "membro": (!it.membro.is_empty()).then_some(&it.membro),
                        })
                        .to_string(),
                    },
                    agora,
                );
                motivos.push(if it.membro.is_empty() {
                    motivo
                } else {
                    format!("{}: {motivo}", it.membro)
                });
                continue;
            }
            let entrada = EntradaItem {
                job_id: job.id,
                info,
                membro: &it.membro,
                formato: it.formato,
                tamanho: it.tamanho,
                hash_arquivo: hash,
                resultado: &it.resultado,
                capa: it.capa.clone(),
            };
            let s = self.processar_item(&entrada, cfg, ia, vocab)?;
            usou_ia |= s.usou_ia;
            revisao += 1;
            membros += u64::from(!it.membro.is_empty());
        }
        let conn = self.conexao();
        if revisao == 0 {
            store::marcar_excecao(conn, job.id, &motivos.join("; "), agora)?;
            return Ok(Desfecho::Excecao);
        }
        store::gravar_checkpoint(conn, job.id, "revisao_pendente", None, agora)?;
        Ok(Desfecho::Revisao {
            reaproveitado,
            ocr,
            paginas,
            ia: usou_ia,
            membros,
        })
    }

    /// Contabiliza e persiste o desfecho de um job (concluído, exceção ou falha).
    #[allow(clippy::too_many_arguments)]
    fn finalizar(
        &mut self,
        job: &store::Job,
        res: Result<Desfecho, ErroAgente>,
        info: &Option<store::InfoArquivo>,
        cfg: &ConfigBiblioteca,
        r: &mut ResumoProcessamento,
        prog: &mut ProgressoBiblioteca,
        t0: Instant,
    ) -> Result<(), ErroAgente> {
        let agora = self.agora_biblio();
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
                membros,
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
                r.membros_de_conteiner += membros;
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
        if let Some(i) = info {
            r.bytes += i.tamanho;
            prog.bytes += i.tamanho;
        }
        prog.itens_feitos = r.processados;
        let forcar = t0.elapsed().as_secs() >= 1;
        self.progresso_biblioteca(prog, forcar);
        Ok(())
    }

    /// Livros da Homologada com `!Catalogar` ou sem nenhuma tag: propostas de tags e estante
    /// (WOW-03/WOW-06), gravadas como sugestões do arquivo ligado ao livro. NADA é escrito na
    /// Homologada: aplicar exige uma homologação aprovada por pessoa.
    fn propostas_homologada(
        &mut self,
        cfg: &ConfigBiblioteca,
        vocab: Option<&Arc<CacheVocab>>,
        restante: u64,
        r: &mut ResumoProcessamento,
    ) -> Result<(), ErroAgente> {
        let Some(vocab) = vocab else { return Ok(()) };
        let Some(h) = super::homologada_ativa(self.conexao())? else {
            return Ok(());
        };
        let agora = self.agora_biblio();
        let versao = format!("{}|h", cfg.versao_regras);
        let mut apos = 0_i64;
        let mut feitas = 0_u64;
        'tudo: loop {
            let lote = store::livros_a_catalogar(self.conexao(), h.id, apos, 200)?;
            if lote.is_empty() {
                break;
            }
            for l in lote {
                apos = l.id;
                if feitas >= restante || !self.seguir_biblioteca() {
                    break 'tudo;
                }
                let conn = self.conexao();
                let Some(file_id) = store::arquivo_do_livro(conn, l.id)? else {
                    continue; // sem arquivo no Catálogo não há onde ligar a proposta
                };
                if store::tem_sugestao_da_versao(conn, file_id, &versao)? {
                    continue;
                }
                let treino = dag_biblio::vocabulario::LivroTreino {
                    id: l.id,
                    titulo: l.titulo.clone(),
                    autores: l.autores.clone(),
                    editora: l.editora.clone(),
                    serie: l.serie.clone(),
                    ..Default::default()
                };
                let s = vocab.sugerir(&treino);
                let nota =
                    "proposta para a Homologada: nada é escrito nela sem homologação aprovada";
                let mut manter = Vec::new();
                let mut gravou = false;
                if !s.tags.is_empty() {
                    let lista: Vec<String> = s.tags.iter().map(|t| t.tag.clone()).collect();
                    let media =
                        s.tags.iter().map(|t| t.confianca).sum::<f32>() / s.tags.len() as f32;
                    let mut evidencias: Vec<dag_biblio::Evidencia> = s
                        .tags
                        .iter()
                        .map(|t| dag_biblio::Evidencia {
                            origem: OrigemEvidencia::Homologada,
                            pagina: None,
                            trecho: Some(t.tag.clone()),
                            detalhe: Some(vocab.explicar(conn, &t.exemplos).join("; ")),
                            confianca: t.confianca,
                        })
                        .collect();
                    evidencias.push(dag_biblio::Evidencia {
                        origem: OrigemEvidencia::MetadadoInterno,
                        pagina: None,
                        trecho: None,
                        detalhe: Some(format!("{}{nota}", super::item::PREFIXO_REVISAO)),
                        confianca: 0.0,
                    });
                    let v = ValorSugerido {
                        campo: Campo::Tags,
                        valor: Some(lista.join("|")),
                        lista,
                        confianca: media,
                        evidencias,
                    };
                    let nova = store::NovaSugestao::de_valor(
                        file_id,
                        &v,
                        OrigemEvidencia::Conteudo,
                        "",
                        &versao,
                    );
                    manter.push(store::gravar_sugestao(conn, &nova, agora)?);
                    gravou = true;
                }
                if l.estante.is_none()
                    && let Some(est) = &s.estante
                {
                    let v = ValorSugerido {
                        campo: Campo::Estante,
                        valor: Some(est.estante.clone()),
                        lista: Vec::new(),
                        confianca: est.confianca,
                        evidencias: vec![
                            dag_biblio::Evidencia {
                                origem: OrigemEvidencia::Homologada,
                                pagina: None,
                                trecho: Some(est.estante.clone()),
                                detalhe: Some(vocab.explicar(conn, &est.exemplos).join("; ")),
                                confianca: est.confianca,
                            },
                            dag_biblio::Evidencia {
                                origem: OrigemEvidencia::MetadadoInterno,
                                pagina: None,
                                trecho: None,
                                detalhe: Some(format!("{}{nota}", super::item::PREFIXO_REVISAO)),
                                confianca: 0.0,
                            },
                        ],
                    };
                    let nova = store::NovaSugestao::de_valor(
                        file_id,
                        &v,
                        OrigemEvidencia::Conteudo,
                        "",
                        &versao,
                    );
                    manter.push(store::gravar_sugestao(conn, &nova, agora)?);
                    gravou = true;
                }
                if gravou {
                    store::limpar_pendentes_exceto(conn, file_id, &versao, &manter)?;
                    r.propostas_homologada += 1;
                    feitas += 1;
                }
            }
        }
        Ok(())
    }
}

fn info_ou_none(a: &Agente, file_id: i64) -> Option<store::InfoArquivo> {
    store::info_arquivo(a.conexao(), file_id).ok().flatten()
}
