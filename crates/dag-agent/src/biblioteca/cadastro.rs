//! Cadastro na A Classificar e homologação controlada. A Homologada só muda por
//! `BibliotecaHomologar` com aprovação de pessoa (GUI ou CLI); o MCP só propõe.

use std::path::Path;

use dag_calibre::calibredb::{AcessoServidor, Alvo, CamposMetadados, NovoLivro, Segredo};
use dag_calibre::homologacao::{AprovacaoHomologacao, OpcoesHomologacao, homologar};
use dag_core::{
    Autor, DadosBiblioteca, HomologacaoInfo, ModoHomologacao, Resposta, ResultadoCadastro,
};
use dag_store::biblioteca as store;

use super::{blake3_arquivo, dados, hash_de_hex, hex, homologada_ativa};
use crate::agente::Agente;
use crate::erro::ErroAgente;

fn a_classificar_ativa(
    conn: &rusqlite::Connection,
) -> Result<store::BibliotecaCalibre, ErroAgente> {
    store::listar_bibliotecas(conn, Some(dag_biblio::PapelBiblioteca::AClassificar))?
        .into_iter()
        .find(|b| b.estado == store::EstadoBiblioteca::Ativa)
        .ok_or_else(|| {
            ErroAgente::biblio(
                "sem_a_classificar",
                "registre a biblioteca A Classificar primeiro",
            )
        })
}

fn info_hom(h: &store::Homologacao) -> HomologacaoInfo {
    HomologacaoInfo {
        id: h.id,
        cadastro_id: h.cadastro_id,
        estado: format!("{:?}", h.estado).to_lowercase(),
        aprovado_por: h.aprovado_por.clone(),
        aprovado_em: h.aprovado_em,
        calibre_id: h.calibre_id,
        erro: h.erro.clone(),
    }
}

/// Metadados do cadastro: melhor sugestão não rejeitada por campo; correção humana vence.
fn metadados_do_arquivo(
    conn: &rusqlite::Connection,
    file_id: i64,
) -> Result<(CamposMetadados, Option<f64>), ErroAgente> {
    use dag_biblio::Campo;
    let mut sugs = store::sugestoes_do_arquivo(conn, file_id)?;
    sugs.retain(|s| s.estado != store::EstadoSugestao::Rejeitada);
    let corrigidos: Vec<(Campo, String)> =
        store::decisoes_do_alvo(conn, store::AlvoDecisao::Arquivo(file_id))?
            .into_iter()
            .filter(|d| d.desfeita_em.is_none() && d.acao == store::AcaoDecisao::Corrigir)
            .filter_map(|d| d.valor_novo.map(|v| (d.campo, v)))
            .collect();
    let melhor = |c: Campo| {
        corrigidos
            .iter()
            .rev()
            .find(|(k, _)| *k == c)
            .map(|(_, v)| (vec![v.clone()], 1.0_f32))
            .or_else(|| {
                sugs.iter()
                    .filter(|s| s.campo == c)
                    .max_by(|a, b| a.confianca.total_cmp(&b.confianca))
                    .map(|s| {
                        let l = if s.lista.is_empty() {
                            s.valor.iter().cloned().collect()
                        } else {
                            s.lista.clone()
                        };
                        (l, s.confianca)
                    })
            })
    };
    let primeiro = |c: Campo| melhor(c).and_then(|(l, _)| l.into_iter().next());
    let mut m = CamposMetadados {
        titulo: primeiro(Campo::Titulo),
        editora: primeiro(Campo::Editora),
        serie: primeiro(Campo::Serie),
        ..CamposMetadados::default()
    };
    if let Some((l, _)) = melhor(Campo::Autores) {
        let autores: Vec<String> = l
            .iter()
            .flat_map(|x| x.split(" & ").map(|y| y.trim().to_owned()))
            .filter(|x| !x.is_empty())
            .collect();
        m.autores = (!autores.is_empty()).then_some(autores);
    }
    if let Some((l, _)) = melhor(Campo::Tags) {
        let t: Vec<String> = l
            .iter()
            .flat_map(|x| x.split(',').map(|y| y.trim().to_owned()))
            .filter(|x| !x.is_empty())
            .collect();
        m.tags = (!t.is_empty()).then_some(t);
    }
    if let Some(i) = primeiro(Campo::Idioma) {
        m.idiomas = Some(vec![i]);
    }
    if let Some(i) = primeiro(Campo::Isbn) {
        m.identificadores = Some(vec![("isbn".into(), i)]);
    }
    let conf = sugs
        .iter()
        .map(|s| f64::from(s.confianca))
        .fold(None, |a: Option<f64>, c| Some(a.map_or(c, |x| x.min(c))));
    Ok((m, conf))
}

impl Agente {
    pub(super) fn cadastrar_biblioteca(&mut self, itens: &[i64]) -> Result<Resposta, ErroAgente> {
        let cfg = self.config_biblioteca();
        let agora = self.agora_biblio();
        let conn = self.conexao();
        let homologada = homologada_ativa(conn)?.ok_or_else(|| {
            ErroAgente::biblio(
                "sem_homologada",
                "registre a Biblioteca Homologada primeiro",
            )
        })?;
        let destino = a_classificar_ativa(conn)?;
        let a_cls = dag_calibre::calibredb::BibliotecaAClassificar::abrir(
            Path::new(&destino.caminho),
            Path::new(&homologada.caminho),
        )?;
        let cli = self.cliente_calibre(&cfg, Some(Path::new(&homologada.caminho)))?;
        let mut resultados = Vec::new();
        let mut bloqueado = None;
        for &fid in itens {
            if !self.seguir_biblioteca() {
                break;
            }
            let conn = self.conexao();
            let Some(info) = store::info_arquivo(conn, fid)? else {
                resultados.push(ResultadoCadastro {
                    file_id: fid,
                    estado: "falhou".into(),
                    calibre_id: None,
                    erro: Some("arquivo fora do Catálogo".into()),
                });
                continue;
            };
            let hash = match info.hash {
                Some(h) => h,
                None => match blake3_arquivo(Path::new(&info.caminho)) {
                    Ok(h) => h,
                    Err(e) => {
                        resultados.push(ResultadoCadastro {
                            file_id: fid,
                            estado: "falhou".into(),
                            calibre_id: None,
                            erro: Some(e.to_string()),
                        });
                        continue;
                    }
                },
            };
            if !store::buscar_por_hash(conn, &hash, Some(info.tamanho))?.is_empty() {
                resultados.push(ResultadoCadastro {
                    file_id: fid,
                    estado: "ja_existe_na_homologada".into(),
                    calibre_id: None,
                    erro: None,
                });
                continue;
            }
            let edicao = store::obra_edicao_do_arquivo(conn, fid)?.map(|x| x.1);
            let (cad, _) = store::registrar_cadastro(conn, fid, edicao, destino.id, &hash, agora)?;
            if cad.estado == store::EstadoCadastro::Cadastrado {
                resultados.push(ResultadoCadastro {
                    file_id: fid,
                    estado: "cadastrado".into(),
                    calibre_id: cad.calibre_id,
                    erro: None,
                });
                continue;
            }
            let (metadados, confianca) = metadados_do_arquivo(conn, fid)?;
            let livro = NovoLivro {
                arquivo: Path::new(&info.caminho).to_path_buf(),
                hash_blake3: hex(&hash),
                capa: None,
                confianca,
                formatos_extras: Vec::new(),
                metadados,
            };
            match cli.adicionar(&a_cls, &livro) {
                Ok(r) => {
                    let conn = self.conexao();
                    store::marcar_cadastrando(conn, cad.id, agora)?;
                    store::marcar_cadastrado(conn, cad.id, r.id, agora)?;
                    resultados.push(ResultadoCadastro {
                        file_id: fid,
                        estado: "cadastrado".into(),
                        calibre_id: Some(r.id),
                        erro: None,
                    });
                }
                Err(
                    e @ (dag_calibre::ErroCalibre::CalibreOcupado { .. }
                    | dag_calibre::ErroCalibre::ServidorServindo { .. }),
                ) => {
                    // Trava: nada foi escrito; o cadastro continua pendente.
                    bloqueado = Some(e.to_string());
                    break;
                }
                Err(e) => {
                    let conn = self.conexao();
                    store::marcar_cadastro_falhou(conn, cad.id, &e.to_string(), agora)?;
                    resultados.push(ResultadoCadastro {
                        file_id: fid,
                        estado: "falhou".into(),
                        calibre_id: None,
                        erro: Some(e.to_string()),
                    });
                }
            }
        }
        if bloqueado.is_some() && resultados.is_empty() {
            return Err(ErroAgente::biblio(
                "calibre_ocupado",
                bloqueado.unwrap_or_default(),
            ));
        }
        Ok(dados(DadosBiblioteca::Cadastrada {
            resultados,
            bloqueado,
        }))
    }

    /// `item` é o id do cadastro. Sem aprovação (ou vindo do MCP) só PROPÕE; com aprovação de
    /// pessoa (GUI/CLI) aplica na Homologada e registra o rollback.
    pub(super) fn homologar_biblioteca(
        &mut self,
        item: i64,
        aprovacao: Option<dag_core::AprovacaoHumana>,
    ) -> Result<Resposta, ErroAgente> {
        let agora = self.agora_biblio();
        let autor = self.autor;
        if autor == Autor::Mcp && aprovacao.is_some() {
            return Err(ErroAgente::biblio(
                "aprovacao_humana_exigida",
                "o MCP só propõe a homologação; a aprovação é de uma pessoa, pela GUI ou CLI",
            ));
        }
        let cfg = self.config_biblioteca();
        let conn = self.conexao();
        let cad = store::obter_cadastro(conn, item)?.ok_or_else(|| {
            ErroAgente::biblio("cadastro_inexistente", format!("cadastro {item}"))
        })?;
        if cad.estado != store::EstadoCadastro::Cadastrado {
            return Err(ErroAgente::biblio(
                "cadastro_nao_pronto",
                "o livro ainda não foi cadastrado na A Classificar",
            ));
        }
        let homologada = homologada_ativa(conn)?.ok_or_else(|| {
            ErroAgente::biblio(
                "sem_homologada",
                "registre a Biblioteca Homologada primeiro",
            )
        })?;
        let existente = store::homologacoes_do_cadastro(conn, cad.id)?
            .into_iter()
            .find(|h| h.estado == store::EstadoHomologacao::Proposta);
        let hom_id = match existente {
            Some(h) => h.id,
            None => store::propor_homologacao(
                conn,
                cad.id,
                homologada.id,
                None,
                Some(&serde_json::json!({"calibre_id": cad.calibre_id}).to_string()),
                agora,
            )?,
        };
        let Some(ap) = aprovacao else {
            let h = store::obter_homologacao(conn, hom_id)?
                .ok_or_else(|| ErroAgente::biblio("homologacao_inexistente", "sumiu"))?;
            return Ok(dados(DadosBiblioteca::Homologacao {
                homologacao: info_hom(&h),
            }));
        };
        if ap.quem.trim().is_empty() {
            return Err(ErroAgente::biblio(
                "aprovacao_invalida",
                "a aprovação precisa de quem aprovou",
            ));
        }
        let acao = match autor {
            Autor::Gui => store::AutorAcao::Gui,
            _ => store::AutorAcao::Cli,
        };
        store::aprovar_homologacao(conn, hom_id, &ap.quem, acao, agora)?;
        let destino = a_classificar_ativa(conn)?;
        let a_cls = dag_calibre::calibredb::BibliotecaAClassificar::abrir(
            Path::new(&destino.caminho),
            Path::new(&homologada.caminho),
        )?;
        let hash_txt = cad
            .chave_idempotencia
            .strip_prefix("dag:")
            .unwrap_or_default()
            .to_owned();
        let _ = hash_de_hex(&hash_txt);
        let cli = self.cliente_calibre(&cfg, Some(Path::new(&homologada.caminho)))?;
        let alvo = match cfg.homologacao.modo {
            ModoHomologacao::Local => Alvo::Local(Path::new(&homologada.caminho).to_path_buf()),
            ModoHomologacao::Servidor => {
                let senha = cfg
                    .homologacao
                    .senha_env
                    .as_deref()
                    .and_then(|v| std::env::var(v).ok())
                    .unwrap_or_default();
                Alvo::Servidor(AcessoServidor {
                    url: cfg.homologacao.url.clone().unwrap_or_default(),
                    id_biblioteca: cfg.homologacao.id_biblioteca.clone().unwrap_or_default(),
                    usuario: cfg.homologacao.usuario.clone().unwrap_or_default(),
                    senha: Segredo::novo(senha),
                })
            }
        };
        let aprov = AprovacaoHomologacao {
            aprovado_por: ap.quem.clone(),
            aprovado_em_unix: u64::try_from(ap.quando.unwrap_or(agora)).unwrap_or(0),
            hash_blake3: hash_txt,
            motivo: ap
                .motivo
                .clone()
                .unwrap_or_else(|| "homologação aprovada".into()),
        };
        let r = homologar(
            &cli,
            &a_cls,
            cad.calibre_id.unwrap_or(0),
            Path::new(&homologada.caminho),
            &alvo,
            &aprov,
            &OpcoesHomologacao::default(),
        );
        let conn = self.conexao();
        match r {
            Ok(reg) => {
                let rollback = serde_json::to_string(&reg).unwrap_or_default();
                store::registrar_aplicacao(conn, hom_id, reg.id_criado, &rollback, agora)?;
            }
            Err(e) => {
                store::registrar_homologacao_falhou(conn, hom_id, &e.to_string(), agora)?;
                return Err(e.into());
            }
        }
        let h = store::obter_homologacao(conn, hom_id)?
            .ok_or_else(|| ErroAgente::biblio("homologacao_inexistente", "sumiu"))?;
        Ok(dados(DadosBiblioteca::Homologacao {
            homologacao: info_hom(&h),
        }))
    }
}
