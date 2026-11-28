//! Pipeline de classificação: modelo primário, validador, segundo parecer e combinação por regras.
//!
//! A combinação nunca escolhe às cegas: concordância sobe a confiança; divergência deixa o campo
//! desconhecido e manda para revisão humana, com os dois candidatos à vista.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use dag_biblio::{Campo, Evidencia, ValorSugerido};
use serde::{Deserialize, Serialize};

use crate::cliente::ConfigCliente;
use crate::cliente::{ClienteLlm, InfoServidor, Metricas};
use crate::entrada::EntradaClassificacao;
use crate::erro::{ErroIa, Resultado};
use crate::esquema::{CAMPOS_CRITICOS, CAMPOS_IA, CAMPOS_VISAO, interpretar_saida};
use crate::modelo::{ConfigIa, PapelModelo};
use crate::prompt::{
    ConfigPrompt, VERSAO_PROMPT, VERSAO_PROMPT_VISAO, construir_prompt, construir_prompt_visao,
};
use crate::registro::{DecisaoValidador, RegistroInferencia};
use crate::validador::{
    Colaborador, ConfigValidador, Rejeicao, ResultadoValidacao, TagProposta, chave_comparacao,
    validar, validar_visao,
};

const LIMITE_BRUTO: usize = 16 * 1024;
/// Teto de confiança quando só um dos modelos sugere o valor.
const TETO_SO_UM: f32 = 0.74;

/// Por que um campo foi para revisão humana.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", rename_all = "snake_case")]
pub enum MotivoRevisao {
    /// O texto (verificado literalmente) e a leitura da imagem da página 1 discordam.
    DivergenciaVisao { texto: String, visao: String },
    /// Os dois modelos validados discordam.
    Divergencia { candidatos: Vec<String> },
    /// A IA discorda do metadado interno ou da Homologada.
    DivergenciaMetadado { ia: String, referencia: String },
    /// Campo crítico sugerido por um modelo só.
    UmSoSugere { modelo: String },
    /// Havia motivo para segundo parecer, mas ele não estava disponível ou falhou.
    SegundoParecerIndisponivel,
    /// Nenhum modelo respondeu de forma utilizável.
    ModeloIndisponivel { detalhe: String },
}

/// Item da fila de revisão humana.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisaoHumana {
    pub campo: Option<Campo>,
    pub motivo: MotivoRevisao,
}

/// Resultado final de classificar um arquivo.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResultadoClassificacao {
    /// Um por campo da IA; desconhecido = `valor None`.
    pub valores: Vec<ValorSugerido>,
    /// Tags fora do vocabulário, para decisão humana (nunca criadas).
    pub tags_novas_propostas: Vec<TagProposta>,
    pub rejeicoes: Vec<Rejeicao>,
    pub revisao_humana: Vec<RevisaoHumana>,
    pub segundo_parecer_consultado: bool,
    pub motivos_segundo_parecer: Vec<String>,
    pub avisos: Vec<String>,
    pub registros: Vec<RegistroInferencia>,
    /// Tradutores, ilustradores e outros: nunca entram em `autores`.
    #[serde(default)]
    pub colaboradores: Vec<Colaborador>,
}

/// Orquestra primário + segundo parecer.
#[derive(Debug)]
pub struct Classificador {
    pub primario: ClienteLlm,
    pub secundario: Option<ClienteLlm>,
    pub cfg_prompt: ConfigPrompt,
    pub cfg_validador: ConfigValidador,
    pub limiar_confianca: f32,
    /// Faz a passada de visão (página 1) quando o primário é multimodal e há imagem.
    pub usar_visao: bool,
    info_primario: OnceLock<Option<InfoServidor>>,
    info_secundario: OnceLock<Option<InfoServidor>>,
}

fn agora_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

fn recortar_bruto(s: &str) -> String {
    s.chars().take(LIMITE_BRUTO).collect()
}

impl Classificador {
    #[must_use]
    pub fn novo(primario: ClienteLlm, secundario: Option<ClienteLlm>) -> Self {
        Self {
            primario,
            secundario,
            cfg_prompt: ConfigPrompt::default(),
            cfg_validador: ConfigValidador::default(),
            limiar_confianca: 0.75,
            usar_visao: true,
            info_primario: OnceLock::new(),
            info_secundario: OnceLock::new(),
        }
    }

    /// Monta o classificador a partir da configuração (primário e, se houver, secundário).
    ///
    /// # Errors
    /// `Configuracao` sem modelo primário; `Endpoint` para endpoint inválido.
    pub fn da_config(cfg: &ConfigIa, cc: &ConfigCliente) -> Resultado<Self> {
        let p = cfg
            .com_papel(PapelModelo::Primario)
            .ok_or_else(|| ErroIa::Configuracao("nenhum modelo primário configurado".into()))?;
        let s = cfg.com_papel(PapelModelo::Secundario);
        let mut c = Self::novo(
            ClienteLlm::novo(p.clone(), cc.clone())?,
            s.map(|s| ClienteLlm::novo(s.clone(), cc.clone()))
                .transpose()?,
        );
        c.limiar_confianca = cfg.limiar_confianca;
        c.cfg_prompt.caracteres_por_token = cfg.caracteres_por_token;
        Ok(c)
    }

    /// Consulta só o modelo primário e valida (usado pelo benchmark); falhas ficam no registro.
    #[must_use]
    pub fn avaliar_modelo(
        &self,
        entrada: &EntradaClassificacao,
    ) -> (Option<ResultadoValidacao>, RegistroInferencia) {
        self.consultar(
            &self.primario,
            PapelModelo::Primario,
            entrada,
            &self.info_primario,
        )
    }

    /// Chama um modelo e valida. Nunca entra em pânico nem propaga erro: falhas ficam no registro.
    fn consultar(
        &self,
        cliente: &ClienteLlm,
        papel: PapelModelo,
        entrada: &EntradaClassificacao,
        cache: &OnceLock<Option<InfoServidor>>,
    ) -> (Option<ResultadoValidacao>, RegistroInferencia) {
        let perfil = cliente.perfil();
        let prompt = construir_prompt(entrada, perfil.contexto, &self.cfg_prompt);
        let info = cache.get_or_init(|| cliente.info_servidor().ok()).clone();
        let mut reg = RegistroInferencia {
            papel,
            modelo: perfil.nome.clone(),
            versao_modelo: info
                .as_ref()
                .and_then(|i| i.quantizacao.clone())
                .map_or_else(|| perfil.versao(), |q| format!("{} [{q}]", perfil.nome)),
            info_servidor: info,
            etapa: "texto".to_owned(),
            versao_prompt: VERSAO_PROMPT.to_owned(),
            hash_prompt: prompt.hash.clone(),
            seed: perfil.seed,
            temperatura: 0.0,
            metricas: Metricas::default(),
            tempo_ms: 0,
            resposta_bruta: None,
            erro: None,
            decisao: None,
            material_truncado: prompt.material.truncado,
            suspeitas_injecao: prompt
                .material
                .suspeitas_injecao
                .iter()
                .map(|s| format!("pagina={:?} padrao={}", s.pagina, s.padrao))
                .collect(),
            inicio_unix_ms: agora_ms(),
        };
        let resposta = match cliente.chat(&prompt.mensagens, Some(&prompt.esquema), &[]) {
            Ok(r) => r,
            Err(e) => {
                reg.erro = Some(e.to_string());
                return (None, reg);
            }
        };
        reg.metricas = resposta.metricas.clone();
        reg.tempo_ms = u64::try_from(resposta.tempo.as_millis()).unwrap_or(u64::MAX);
        reg.resposta_bruta = Some(recortar_bruto(&resposta.conteudo));
        let saida = match interpretar_saida(&resposta.conteudo) {
            Ok(s) => s,
            Err(e) => {
                reg.erro = Some(e.to_string());
                return (None, reg);
            }
        };
        let v = validar(
            &saida,
            &prompt.material,
            entrada,
            &perfil.nome,
            &self.cfg_validador,
        );
        reg.decisao = Some(DecisaoValidador {
            campos_aceitos: v
                .valores
                .iter()
                .filter(|x| e_conhecido(x))
                .map(|x| x.campo)
                .collect(),
            campos_desconhecidos: v
                .valores
                .iter()
                .filter(|x| !e_conhecido(x))
                .map(|x| x.campo)
                .collect(),
            rejeicoes: v.rejeicoes.clone(),
            tags_novas: v.tags_novas.iter().map(|t| t.tag.clone()).collect(),
            avisos: v.avisos.clone(),
        });
        (Some(v), reg)
    }

    /// Passada de visão com o modelo primário (se multimodal e houver imagem da página 1).
    fn consultar_visao(
        &self,
        entrada: &EntradaClassificacao,
    ) -> Option<(Option<ResultadoValidacao>, RegistroInferencia)> {
        let cliente = &self.primario;
        let perfil = cliente.perfil();
        let imagem = entrada
            .imagem_pagina1
            .as_ref()
            .or(entrada.extracao.capa.as_ref())?;
        if !perfil.multimodal || imagem.is_empty() || imagem.len() > LIMITE_IMAGEM {
            return None;
        }
        let (mensagens, esquema, hash) = construir_prompt_visao(entrada);
        let info = self
            .info_primario
            .get_or_init(|| cliente.info_servidor().ok())
            .clone();
        let mut reg = RegistroInferencia {
            papel: PapelModelo::Primario,
            modelo: perfil.nome.clone(),
            versao_modelo: perfil.versao(),
            info_servidor: info,
            etapa: "visao".to_owned(),
            versao_prompt: VERSAO_PROMPT_VISAO.to_owned(),
            hash_prompt: hash,
            seed: perfil.seed,
            temperatura: 0.0,
            metricas: Metricas::default(),
            tempo_ms: 0,
            resposta_bruta: None,
            erro: None,
            decisao: None,
            material_truncado: false,
            suspeitas_injecao: vec![],
            inicio_unix_ms: agora_ms(),
        };
        let b64 = base64(imagem);
        let resposta = match cliente.chat(&mensagens, Some(&esquema), &[b64]) {
            Ok(r) => r,
            Err(e) => {
                reg.erro = Some(e.to_string());
                return Some((None, reg));
            }
        };
        reg.metricas = resposta.metricas.clone();
        reg.tempo_ms = u64::try_from(resposta.tempo.as_millis()).unwrap_or(u64::MAX);
        reg.resposta_bruta = Some(recortar_bruto(&resposta.conteudo));
        let saida = match interpretar_saida(&resposta.conteudo) {
            Ok(s) => s,
            Err(e) => {
                reg.erro = Some(e.to_string());
                return Some((None, reg));
            }
        };
        let v = validar_visao(&saida, entrada, &perfil.nome, &self.cfg_validador);
        reg.decisao = Some(DecisaoValidador {
            campos_aceitos: v
                .valores
                .iter()
                .filter(|x| e_conhecido(x))
                .map(|x| x.campo)
                .collect(),
            campos_desconhecidos: v
                .valores
                .iter()
                .filter(|x| !e_conhecido(x))
                .map(|x| x.campo)
                .collect(),
            rejeicoes: v.rejeicoes.clone(),
            tags_novas: vec![],
            avisos: v.avisos.clone(),
        });
        Some((Some(v), reg))
    }

    /// Classifica um arquivo. Sempre devolve um resultado: falhas de modelo viram revisão humana.
    #[must_use]
    pub fn classificar(&self, entrada: &EntradaClassificacao) -> ResultadoClassificacao {
        match self.fase_primaria(entrada) {
            Fase::Pronto(out) => out,
            Fase::AguardaSecundario(p) => self.fase_secundaria(entrada, p),
        }
    }

    /// Classifica vários arquivos agrupando as chamadas por modelo: primeiro todos os primários
    /// (e a visão), depois o segundo parecer só dos que precisam. Cada item recebe o mesmo
    /// resultado de [`Self::classificar`]; só muda a ordem das chamadas (uma troca de modelo na
    /// GPU por lote, não por item).
    #[must_use]
    pub fn classificar_lote(
        &self,
        entradas: &[EntradaClassificacao],
    ) -> Vec<ResultadoClassificacao> {
        let fases: Vec<Fase> = entradas.iter().map(|e| self.fase_primaria(e)).collect();
        fases
            .into_iter()
            .zip(entradas)
            .map(|(f, e)| match f {
                Fase::Pronto(out) => out,
                Fase::AguardaSecundario(p) => self.fase_secundaria(e, p),
            })
            .collect()
    }

    /// Primário + visão. Resolve tudo que não depende do segundo parecer.
    fn fase_primaria(&self, entrada: &EntradaClassificacao) -> Fase {
        let mut out = ResultadoClassificacao::default();
        let (a, reg_a) = self.consultar(
            &self.primario,
            PapelModelo::Primario,
            entrada,
            &self.info_primario,
        );
        out.registros.push(reg_a);

        // Primário falhou: o secundário assume sozinho (e tudo vai para conferência).
        let Some(mut a) = a else {
            let detalhe = out.registros[0].erro.clone().unwrap_or_default();
            if self.secundario.is_some() {
                return Fase::AguardaSecundario(Pendente {
                    out,
                    a: None,
                    detalhe_primario: detalhe,
                });
            }
            return Fase::Pronto(falha_total(out, detalhe));
        };

        if let Some((v, reg)) = self
            .usar_visao
            .then(|| self.consultar_visao(entrada))
            .flatten()
        {
            out.registros.push(reg);
            if let Some(v) = v {
                reconciliar_visao(&mut a, &v, &mut out);
            }
        }

        let motivos = motivos_segundo_parecer(&a, entrada, self.limiar_confianca);
        if motivos.is_empty() {
            combinar(&mut out, a, None, entrada, self.limiar_confianca);
            return Fase::Pronto(out);
        }
        out.motivos_segundo_parecer.clone_from(&motivos);
        if self.secundario.is_none() {
            out.revisao_humana.push(RevisaoHumana {
                campo: None,
                motivo: MotivoRevisao::SegundoParecerIndisponivel,
            });
            combinar(&mut out, a, None, entrada, self.limiar_confianca);
            return Fase::Pronto(out);
        }
        Fase::AguardaSecundario(Pendente {
            out,
            a: Some(a),
            detalhe_primario: String::new(),
        })
    }

    /// Segundo parecer e combinação final de um item que ficou pendente.
    fn fase_secundaria(
        &self,
        entrada: &EntradaClassificacao,
        p: Pendente,
    ) -> ResultadoClassificacao {
        let Pendente {
            mut out,
            a,
            detalhe_primario,
        } = p;
        let Some(sec) = &self.secundario else {
            // Inalcançável (só há pendente com secundário), mas mantém o contrato.
            return falha_total(out, detalhe_primario);
        };
        let (rb, reg_b) =
            self.consultar(sec, PapelModelo::Secundario, entrada, &self.info_secundario);
        out.registros.push(reg_b);
        out.segundo_parecer_consultado = true;
        match a {
            None => {
                out.motivos_segundo_parecer
                    .push(format!("primário indisponível: {detalhe_primario}"));
                if let Some(b) = rb {
                    absorver(&mut out, b, entrada, self.limiar_confianca);
                    out.revisao_humana.push(RevisaoHumana {
                        campo: None,
                        motivo: MotivoRevisao::ModeloIndisponivel {
                            detalhe: format!("primário: {detalhe_primario}"),
                        },
                    });
                    return out;
                }
                falha_total(out, detalhe_primario)
            }
            Some(a) => {
                if rb.is_none() {
                    out.revisao_humana.push(RevisaoHumana {
                        campo: None,
                        motivo: MotivoRevisao::SegundoParecerIndisponivel,
                    });
                }
                combinar(&mut out, a, rb, entrada, self.limiar_confianca);
                out
            }
        }
    }
}

/// Estado intermediário de um item entre a fase do primário e a do segundo parecer.
struct Pendente {
    out: ResultadoClassificacao,
    /// `None` quando o primário falhou.
    a: Option<ResultadoValidacao>,
    detalhe_primario: String,
}

enum Fase {
    Pronto(ResultadoClassificacao),
    AguardaSecundario(Pendente),
}

fn falha_total(mut out: ResultadoClassificacao, detalhe: String) -> ResultadoClassificacao {
    out.valores = CAMPOS_IA.iter().map(|c| desconhecido(*c)).collect();
    out.revisao_humana.push(RevisaoHumana {
        campo: None,
        motivo: MotivoRevisao::ModeloIndisponivel { detalhe },
    });
    out
}

fn e_conhecido(v: &ValorSugerido) -> bool {
    v.valor.is_some() || !v.lista.is_empty()
}

fn desconhecido(campo: Campo) -> ValorSugerido {
    ValorSugerido {
        campo,
        valor: None,
        lista: vec![],
        confianca: 0.0,
        evidencias: vec![],
    }
}

fn rotulo(v: &ValorSugerido) -> String {
    v.valor.clone().unwrap_or_else(|| v.lista.join(" & "))
}

/// Compara com metadados internos e referências da Homologada: (valor da referência) se diverge.
fn divergencia_com_referencias(
    entrada: &EntradaClassificacao,
    v: &ValorSugerido,
) -> Option<String> {
    let chave = chave_comparacao(v)?;
    entrada
        .extracao
        .metadados
        .iter()
        .chain(entrada.referencias.iter())
        .filter(|r| r.campo == v.campo && !matches!(v.campo, Campo::Resumo | Campo::Idioma))
        .find_map(|r| {
            let k = chave_comparacao(r)?;
            (k != chave).then(|| rotulo(r))
        })
}

/// Razões para consultar o segundo modelo (vazio = não precisa).
fn motivos_segundo_parecer(
    a: &ResultadoValidacao,
    entrada: &EntradaClassificacao,
    limiar: f32,
) -> Vec<String> {
    let mut m = Vec::new();
    for v in &a.valores {
        let conhecido = e_conhecido(v);
        if CAMPOS_CRITICOS.contains(&v.campo) && conhecido && v.confianca + 1e-4 < limiar {
            m.push(format!(
                "{:?} com confiança {:.2} < {limiar:.2}",
                v.campo, v.confianca
            ));
        }
        if matches!(v.campo, Campo::Titulo | Campo::Autores) && !conhecido {
            m.push(format!("{:?} desconhecido", v.campo));
        }
        if let Some(ref_val) = divergencia_com_referencias(entrada, v).filter(|_| conhecido) {
            m.push(format!("{:?} diverge da referência (`{ref_val}`)", v.campo));
        }
    }
    for r in &a.rejeicoes {
        if r.campo.is_some_and(|c| CAMPOS_CRITICOS.contains(&c))
            && !matches!(
                r.motivo,
                crate::validador::MotivoRejeicao::TagForaDoVocabulario
            )
        {
            m.push(format!(
                "{:?} rejeitado pelo validador ({:?})",
                r.campo, r.motivo
            ));
        }
    }
    m
}

fn mesclar_evidencias(a: &[Evidencia], b: &[Evidencia], conf: f32) -> Vec<Evidencia> {
    let mut out: Vec<Evidencia> = Vec::new();
    for e in a.iter().chain(b) {
        if !out.iter().any(|x| {
            x.origem == e.origem
                && x.pagina == e.pagina
                && x.trecho == e.trecho
                && x.detalhe == e.detalhe
        }) {
            let mut e = e.clone();
            if e.origem == dag_biblio::OrigemEvidencia::Ia {
                e.confianca = conf;
            }
            out.push(e);
        }
    }
    out
}

/// Usa um único resultado validado (sem segundo parecer ou com o primário fora do ar).
fn absorver(
    out: &mut ResultadoClassificacao,
    a: ResultadoValidacao,
    entrada: &EntradaClassificacao,
    limiar: f32,
) {
    combinar(out, a, None, entrada, limiar);
}

fn combinar(
    out: &mut ResultadoClassificacao,
    a: ResultadoValidacao,
    b: Option<ResultadoValidacao>,
    entrada: &EntradaClassificacao,
    _limiar: f32,
) {
    out.rejeicoes.extend(a.rejeicoes.iter().cloned());
    out.avisos.extend(a.avisos.iter().cloned());
    let mut tags_novas = a.tags_novas.clone();
    if let Some(b) = &b {
        out.rejeicoes.extend(b.rejeicoes.iter().cloned());
        out.avisos.extend(b.avisos.iter().cloned());
        for t in &b.tags_novas {
            if let Some(x) = tags_novas.iter_mut().find(|x| {
                crate::normaliza::normalizar(&x.tag) == crate::normaliza::normalizar(&t.tag)
            }) {
                x.evidencias.extend(t.evidencias.iter().cloned());
            } else {
                tags_novas.push(t.clone());
            }
        }
    }
    out.tags_novas_propostas = tags_novas;
    for c in a
        .colaboradores
        .iter()
        .chain(b.iter().flat_map(|b| b.colaboradores.iter()))
    {
        let chave = |x: &Colaborador| (crate::normaliza::normalizar(&x.nome), x.papel);
        if !out.colaboradores.iter().any(|o| chave(o) == chave(c)) {
            out.colaboradores.push(c.clone());
        }
    }

    for c in CAMPOS_IA {
        let va = a
            .valores
            .iter()
            .find(|v| v.campo == c)
            .cloned()
            .unwrap_or_else(|| desconhecido(c));
        let vb = b.as_ref().map(|b| {
            b.valores
                .iter()
                .find(|v| v.campo == c)
                .cloned()
                .unwrap_or_else(|| desconhecido(c))
        });
        let mut final_v = match vb {
            None => va,
            Some(vb) => combinar_campo(out, c, va, vb),
        };
        if let Some(ref_val) = divergencia_com_referencias(entrada, &final_v) {
            final_v.confianca = final_v.confianca.min(TETO_SO_UM);
            for e in &mut final_v.evidencias {
                e.confianca = e.confianca.min(TETO_SO_UM);
            }
            out.revisao_humana.push(RevisaoHumana {
                campo: Some(c),
                motivo: MotivoRevisao::DivergenciaMetadado {
                    ia: rotulo(&final_v),
                    referencia: ref_val,
                },
            });
        }
        out.valores.push(final_v);
    }
}

fn combinar_campo(
    out: &mut ResultadoClassificacao,
    c: Campo,
    va: ValorSugerido,
    vb: ValorSugerido,
) -> ValorSugerido {
    let (ka, kb) = (e_conhecido(&va), e_conhecido(&vb));
    match (ka, kb) {
        (false, false) => va,
        (true, true) => {
            if matches!(c, Campo::Resumo | Campo::Idioma) {
                return va; // não comparáveis literalmente: vale o primário, com seu teto
            }
            if c == Campo::Tags {
                return combinar_tags(va, &vb);
            }
            if chave_comparacao(&va) == chave_comparacao(&vb) {
                let mx = va.confianca.max(vb.confianca);
                let conf = (mx + (1.0 - mx) * 0.5).min(0.97);
                let evidencias = mesclar_evidencias(&va.evidencias, &vb.evidencias, conf);
                ValorSugerido {
                    confianca: conf,
                    evidencias,
                    ..va
                }
            } else {
                out.revisao_humana.push(RevisaoHumana {
                    campo: Some(c),
                    motivo: MotivoRevisao::Divergencia {
                        candidatos: vec![rotulo(&va), rotulo(&vb)],
                    },
                });
                desconhecido(c)
            }
        }
        (true, false) | (false, true) => {
            let (mut v, modelo_so) = if ka {
                (va, "primario")
            } else {
                (vb, "secundario")
            };
            v.confianca = v.confianca.min(TETO_SO_UM);
            for e in &mut v.evidencias {
                e.confianca = e.confianca.min(TETO_SO_UM);
            }
            if CAMPOS_CRITICOS.contains(&c) {
                out.revisao_humana.push(RevisaoHumana {
                    campo: Some(c),
                    motivo: MotivoRevisao::UmSoSugere {
                        modelo: modelo_so.to_owned(),
                    },
                });
            }
            v
        }
    }
}

fn combinar_tags(va: ValorSugerido, vb: &ValorSugerido) -> ValorSugerido {
    let iguais = chave_comparacao(&va) == chave_comparacao(vb);
    let mut lista = va.lista.clone();
    for t in &vb.lista {
        if !lista
            .iter()
            .any(|x| crate::normaliza::normalizar(x) == crate::normaliza::normalizar(t))
        {
            lista.push(t.clone());
        }
    }
    let mx = va.confianca.max(vb.confianca);
    let conf = if iguais {
        (mx + (1.0 - mx) * 0.5).min(0.9)
    } else {
        va.confianca.min(vb.confianca).min(TETO_SO_UM)
    };
    ValorSugerido {
        campo: Campo::Tags,
        valor: Some(lista.join(" & ")),
        lista,
        confianca: conf,
        evidencias: mesclar_evidencias(&va.evidencias, &vb.evidencias, conf),
    }
}

const LIMITE_IMAGEM: usize = 12 * 1024 * 1024;

/// Base64 padrão (RFC 4648) sem dependência externa.
fn base64(dados: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(dados.len().div_ceil(3) * 4);
    for bloco in dados.chunks(3) {
        let n = (u32::from(bloco[0]) << 16)
            | (u32::from(*bloco.get(1).unwrap_or(&0)) << 8)
            | u32::from(*bloco.get(2).unwrap_or(&0));
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if bloco.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if bloco.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

/// Texto e visão dizem a mesma coisa? Igualdade canônica ou um contido no outro (subtítulo,
/// sobrenome na capa, autores a mais no texto).
fn equivalentes_visao(campo: Campo, a: &ValorSugerido, b: &ValorSugerido) -> bool {
    if chave_comparacao(a) == chave_comparacao(b) {
        return true;
    }
    let conjunto = |v: &ValorSugerido| -> Vec<String> {
        if v.lista.is_empty() {
            vec![crate::normaliza::normalizar(
                v.valor.as_deref().unwrap_or(""),
            )]
        } else {
            v.lista
                .iter()
                .map(|i| {
                    let mut t = crate::normaliza::tokens(i);
                    t.sort();
                    t.join(" ")
                })
                .collect()
        }
    };
    let (ca, cb) = (conjunto(a), conjunto(b));
    if campo == Campo::Autores {
        let sub = |x: &[String], y: &[String]| x.iter().all(|i| y.contains(i));
        return sub(&ca, &cb) || sub(&cb, &ca);
    }
    if campo == Campo::Isbn || campo == Campo::Data {
        return false;
    }
    let (na, nb) = (&ca[0], &cb[0]);
    !na.is_empty() && !nb.is_empty() && (na.contains(nb.as_str()) || nb.contains(na.as_str()))
}

/// Concordância texto×visão sobe a confiança; divergência vai para revisão; visão sozinha só
/// vale com teto de "média" e é sinalizada nos campos críticos.
fn reconciliar_visao(
    a: &mut ResultadoValidacao,
    v: &ResultadoValidacao,
    out: &mut ResultadoClassificacao,
) {
    out.rejeicoes.extend(v.rejeicoes.iter().cloned());
    for campo in CAMPOS_VISAO {
        let Some(vv) = v
            .valores
            .iter()
            .find(|x| x.campo == campo)
            .filter(|x| e_conhecido(x))
        else {
            continue;
        };
        let Some(va) = a.valores.iter_mut().find(|x| x.campo == campo) else {
            continue;
        };
        if e_conhecido(va) {
            if equivalentes_visao(campo, va, vv) {
                va.confianca = (va.confianca + 0.15).min(0.95);
                for e in &mut va.evidencias {
                    if e.origem == dag_biblio::OrigemEvidencia::Ia {
                        e.confianca = va.confianca;
                    }
                }
                va.evidencias.extend(vv.evidencias.iter().cloned());
            } else {
                va.confianca = va.confianca.min(TETO_SO_UM);
                out.revisao_humana.push(RevisaoHumana {
                    campo: Some(campo),
                    motivo: MotivoRevisao::DivergenciaVisao {
                        texto: rotulo(va),
                        visao: rotulo(vv),
                    },
                });
            }
        } else {
            let mut novo = vv.clone();
            novo.confianca = novo.confianca.min(0.6);
            for e in &mut novo.evidencias {
                e.confianca = novo.confianca;
            }
            *va = novo;
            if CAMPOS_CRITICOS.contains(&campo) {
                out.revisao_humana.push(RevisaoHumana {
                    campo: Some(campo),
                    motivo: MotivoRevisao::UmSoSugere {
                        modelo: "visao".to_owned(),
                    },
                });
            }
        }
    }
}
