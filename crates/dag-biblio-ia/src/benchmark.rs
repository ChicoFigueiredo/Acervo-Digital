//! Benchmark de modelos (CA-B15, CA-B20): roda casos com gabarito contra 1..N modelos e mede
//! tempo, tokens/s, VRAM e precisão por campo. Nada é estimado: o que não rodou aparece como
//! "NÃO EXECUTADO" e o que não foi medido como "n/d".

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use dag_biblio::{Campo, ValorSugerido};
use serde::{Deserialize, Serialize};

use crate::classificador::Classificador;
use crate::cliente::InfoServidor;
use crate::entrada::EntradaClassificacao;
use crate::erro::{ErroIa, Resultado};
use crate::esquema::{e_lista, nome_campo};
use crate::validador::chave_comparacao;

/// Valor esperado no gabarito.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GabaritoValor {
    Lista(Vec<String>),
    Texto(String),
}

/// Um caso: entrada + gabarito. `null` no gabarito significa "deve ficar desconhecido".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CasoBenchmark {
    #[serde(default)]
    pub id: String,
    pub entrada: EntradaClassificacao,
    pub gabarito: BTreeMap<Campo, Option<GabaritoValor>>,
}

/// Lê todos os `*.json` do diretório (ordem alfabética).
///
/// # Errors
/// `Io`/`Configuracao` se o diretório ou algum JSON for ilegível.
pub fn carregar_casos(dir: &Path) -> Resultado<Vec<CasoBenchmark>> {
    let mut caminhos: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    caminhos.sort();
    let mut casos = Vec::new();
    for p in caminhos {
        let t = std::fs::read_to_string(&p)?;
        let mut c: CasoBenchmark = serde_json::from_str(&t)
            .map_err(|e| ErroIa::Configuracao(format!("{}: {e}", p.display())))?;
        if c.id.is_empty() {
            c.id = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        c.entrada.id.clone_from(&c.id);
        casos.push(c);
    }
    Ok(casos)
}

/// Uma leitura de VRAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeituraVram {
    pub usada_mib: u64,
    pub total_mib: u64,
}

/// Fonte de medição de VRAM (injetável para teste).
pub trait MedidorVram: Send + Sync + std::fmt::Debug {
    fn ler(&self) -> Option<LeituraVram>;
}

/// `nvidia-smi --query-gpu=memory.used,memory.total` (primeira GPU).
#[derive(Debug, Default, Clone, Copy)]
pub struct NvidiaSmi;

impl MedidorVram for NvidiaSmi {
    fn ler(&self) -> Option<LeituraVram> {
        let s = Command::new("nvidia-smi")
            .args([
                "--query-gpu=memory.used,memory.total",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        let t = String::from_utf8_lossy(&s.stdout);
        let linha = t.lines().next()?;
        let mut p = linha.split(',').map(|x| x.trim().parse::<u64>());
        Some(LeituraVram {
            usada_mib: p.next()?.ok()?,
            total_mib: p.next()?.ok()?,
        })
    }
}

/// Avaliação de um campo contra o gabarito.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Avaliacao {
    Correto,
    Errado,
    /// Havia resposta no gabarito e o modelo deixou desconhecido.
    Omitido,
    /// O gabarito diz desconhecido e o modelo preencheu (invenção, CA-B09).
    Inventado,
    /// Gabarito desconhecido e o modelo deixou desconhecido.
    DesconhecidoCorreto,
}

/// Resultado de um caso para um modelo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultadoItem {
    pub id: String,
    pub chamada_ok: bool,
    pub erro: Option<String>,
    pub tempo_ms: u64,
    pub tokens_entrada: Option<u64>,
    pub tokens_saida: Option<u64>,
    pub tokens_por_segundo: Option<f64>,
    pub paginas_na_entrada: usize,
    pub ms_por_pagina: Option<f64>,
    pub vram_antes_mib: Option<u64>,
    pub vram_pico_mib: Option<u64>,
    /// Ollama `/api/ps`: VRAM do modelo após o item.
    pub vram_modelo_servidor_mib: Option<u64>,
    pub campos: BTreeMap<Campo, Avaliacao>,
}

/// Contagens por campo.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResumoCampo {
    pub corretos: u32,
    pub errados: u32,
    pub omitidos: u32,
    pub inventados: u32,
    pub desconhecidos_corretos: u32,
    /// corretos / (corretos + errados + inventados): de tudo que o modelo afirmou, quanto estava certo.
    pub precisao: Option<f64>,
    /// afirmados / esperados com valor.
    pub cobertura: Option<f64>,
}

/// Resumo de um modelo.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResumoModelo {
    pub itens: usize,
    pub itens_ok: usize,
    pub tempo_medio_ms: Option<f64>,
    pub tokens_por_segundo_medio: Option<f64>,
    pub ms_por_pagina_medio: Option<f64>,
    pub vram_pico_mib: Option<u64>,
    pub por_campo: BTreeMap<Campo, ResumoCampo>,
    pub precisao_global: Option<f64>,
}

/// Status de um modelo no benchmark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "estado", rename_all = "snake_case")]
pub enum StatusModelo {
    Executado,
    NaoExecutado { motivo: String },
}

/// Relatório de um modelo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelatorioModelo {
    pub nome: String,
    pub versao: String,
    pub info_servidor: Option<InfoServidor>,
    pub status: StatusModelo,
    pub itens: Vec<ResultadoItem>,
    pub resumo: Option<ResumoModelo>,
}

/// Relatório completo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelatorioBenchmark {
    pub gerado_em_unix_ms: u128,
    pub casos: usize,
    pub vram_medida: bool,
    pub gpu_total_mib: Option<u64>,
    pub modelos: Vec<RelatorioModelo>,
    pub observacoes: Vec<String>,
}

/// Opções.
#[derive(Debug)]
pub struct OpcoesBenchmark {
    /// `None` = tenta `nvidia-smi`; ausente, VRAM fica "n/d".
    pub medidor: Option<Arc<dyn MedidorVram>>,
    pub intervalo_amostra: Duration,
}

impl Default for OpcoesBenchmark {
    fn default() -> Self {
        Self {
            medidor: Some(Arc::new(NvidiaSmi)),
            intervalo_amostra: Duration::from_millis(250),
        }
    }
}

fn avaliar(
    campo: Campo,
    esperado: &Option<GabaritoValor>,
    obtido: Option<&ValorSugerido>,
) -> Avaliacao {
    let conhecido = obtido.is_some_and(|v| v.valor.is_some() || !v.lista.is_empty());
    let Some(esp) = esperado else {
        return if conhecido {
            Avaliacao::Inventado
        } else {
            Avaliacao::DesconhecidoCorreto
        };
    };
    if !conhecido {
        return Avaliacao::Omitido;
    }
    let ref_v = match esp {
        GabaritoValor::Texto(t) if !e_lista(campo) => ValorSugerido {
            campo,
            valor: Some(t.clone()),
            lista: vec![],
            confianca: 1.0,
            evidencias: vec![],
        },
        GabaritoValor::Texto(t) => ValorSugerido {
            campo,
            valor: None,
            lista: vec![t.clone()],
            confianca: 1.0,
            evidencias: vec![],
        },
        GabaritoValor::Lista(l) => ValorSugerido {
            campo,
            valor: None,
            lista: l.clone(),
            confianca: 1.0,
            evidencias: vec![],
        },
    };
    let (Some(ka), Some(kb)) = (chave_comparacao(&ref_v), obtido.and_then(chave_comparacao)) else {
        return Avaliacao::Errado;
    };
    if ka == kb {
        Avaliacao::Correto
    } else {
        Avaliacao::Errado
    }
}

#[allow(clippy::cast_precision_loss)]
fn media(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

#[allow(clippy::cast_precision_loss)]
fn resumir(itens: &[ResultadoItem]) -> ResumoModelo {
    let ok: Vec<&ResultadoItem> = itens.iter().filter(|i| i.chamada_ok).collect();
    let mut por_campo: BTreeMap<Campo, ResumoCampo> = BTreeMap::new();
    for i in &ok {
        for (c, a) in &i.campos {
            let r = por_campo.entry(*c).or_default();
            match a {
                Avaliacao::Correto => r.corretos += 1,
                Avaliacao::Errado => r.errados += 1,
                Avaliacao::Omitido => r.omitidos += 1,
                Avaliacao::Inventado => r.inventados += 1,
                Avaliacao::DesconhecidoCorreto => r.desconhecidos_corretos += 1,
            }
        }
    }
    let (mut tc, mut ta) = (0u32, 0u32);
    for r in por_campo.values_mut() {
        let afirmados = r.corretos + r.errados + r.inventados;
        let esperados = r.corretos + r.errados + r.omitidos;
        r.precisao = (afirmados > 0).then(|| f64::from(r.corretos) / f64::from(afirmados));
        r.cobertura =
            (esperados > 0).then(|| f64::from(r.corretos + r.errados) / f64::from(esperados));
        tc += r.corretos;
        ta += afirmados;
    }
    let col =
        |f: fn(&ResultadoItem) -> Option<f64>| ok.iter().filter_map(|i| f(i)).collect::<Vec<_>>();
    ResumoModelo {
        itens: itens.len(),
        itens_ok: ok.len(),
        tempo_medio_ms: media(&ok.iter().map(|i| i.tempo_ms as f64).collect::<Vec<_>>()),
        tokens_por_segundo_medio: media(&col(|i| i.tokens_por_segundo)),
        ms_por_pagina_medio: media(&col(|i| i.ms_por_pagina)),
        vram_pico_mib: itens.iter().filter_map(|i| i.vram_pico_mib).max(),
        por_campo,
        precisao_global: (ta > 0).then(|| f64::from(tc) / f64::from(ta)),
    }
}

/// Mede a VRAM em segundo plano enquanto a chamada roda.
struct Amostrador {
    parar: Arc<AtomicBool>,
    pico: Arc<AtomicU64>,
    fio: Option<std::thread::JoinHandle<()>>,
}

impl Amostrador {
    fn iniciar(m: Option<Arc<dyn MedidorVram>>, intervalo: Duration) -> Self {
        let parar = Arc::new(AtomicBool::new(false));
        let pico = Arc::new(AtomicU64::new(0));
        let fio = m.map(|m| {
            let (p, k) = (parar.clone(), pico.clone());
            std::thread::spawn(move || {
                loop {
                    if let Some(l) = m.ler() {
                        k.fetch_max(l.usada_mib + 1, Ordering::Relaxed); // +1: 0 = sem leitura
                    }
                    if p.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(intervalo);
                }
            })
        });
        Self { parar, pico, fio }
    }

    fn encerrar(mut self) -> Option<u64> {
        self.parar.store(true, Ordering::Relaxed);
        if let Some(f) = self.fio.take() {
            let _ = f.join();
        }
        self.pico.load(Ordering::Relaxed).checked_sub(1)
    }
}

/// Roda os casos contra cada classificador (um por modelo; use `Classificador::novo(cliente, None)`).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn executar_benchmark(
    casos: &[CasoBenchmark],
    modelos: &[Classificador],
    opcoes: &OpcoesBenchmark,
) -> RelatorioBenchmark {
    let leitura0 = opcoes.medidor.as_ref().and_then(|m| m.ler());
    let mut relatorio = RelatorioBenchmark {
        gerado_em_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis()),
        casos: casos.len(),
        vram_medida: leitura0.is_some(),
        gpu_total_mib: leitura0.map(|l| l.total_mib),
        modelos: Vec::new(),
        observacoes: Vec::new(),
    };
    if leitura0.is_none() {
        relatorio
            .observacoes
            .push("VRAM não medida: nvidia-smi ausente ou sem resposta (campos n/d).".into());
    }
    if casos.is_empty() {
        relatorio
            .observacoes
            .push("Nenhum caso encontrado: nada foi executado.".into());
    }

    for c in modelos {
        let perfil = c.primario.perfil();
        let mut rm = RelatorioModelo {
            nome: perfil.nome.clone(),
            versao: perfil.versao(),
            info_servidor: None,
            status: StatusModelo::Executado,
            itens: Vec::new(),
            resumo: None,
        };
        if casos.is_empty() {
            rm.status = StatusModelo::NaoExecutado {
                motivo: "sem casos".into(),
            };
            relatorio.modelos.push(rm);
            continue;
        }
        if let Err(e) = c.primario.saude() {
            rm.status = StatusModelo::NaoExecutado {
                motivo: format!("servidor inacessível: {e}"),
            };
            relatorio.modelos.push(rm);
            continue;
        }
        match c.primario.info_servidor() {
            Ok(i) => rm.info_servidor = Some(i),
            Err(e @ ErroIa::ModeloAusente(_)) => {
                rm.status = StatusModelo::NaoExecutado {
                    motivo: e.to_string(),
                };
                relatorio.modelos.push(rm);
                continue;
            }
            Err(e) => relatorio.observacoes.push(format!(
                "{}: info do servidor indisponível ({e})",
                perfil.nome
            )),
        }
        for caso in casos {
            let antes = opcoes
                .medidor
                .as_ref()
                .and_then(|m| m.ler())
                .map(|l| l.usada_mib);
            let amostra = Amostrador::iniciar(opcoes.medidor.clone(), opcoes.intervalo_amostra);
            let (val, reg) = c.avaliar_modelo(&caso.entrada);
            let pico = amostra.encerrar();
            let paginas = caso.entrada.extracao.paginas.len();
            let campos = if let Some(v) = &val {
                caso.gabarito
                    .iter()
                    .map(|(campo, esp)| {
                        (
                            *campo,
                            avaliar(*campo, esp, v.valores.iter().find(|x| x.campo == *campo)),
                        )
                    })
                    .collect()
            } else {
                BTreeMap::new()
            };
            let tps = reg.metricas.tokens_por_segundo.or_else(|| {
                match (reg.metricas.tokens_saida, reg.tempo_ms) {
                    (Some(n), t) if t > 0 => Some(n as f64 / (t as f64 / 1000.0)),
                    _ => None,
                }
            });
            rm.itens.push(ResultadoItem {
                id: caso.id.clone(),
                chamada_ok: val.is_some(),
                erro: reg.erro.clone(),
                tempo_ms: reg.tempo_ms,
                tokens_entrada: reg.metricas.tokens_entrada,
                tokens_saida: reg.metricas.tokens_saida,
                tokens_por_segundo: tps,
                paginas_na_entrada: paginas,
                ms_por_pagina: (paginas > 0 && val.is_some())
                    .then(|| reg.tempo_ms as f64 / paginas as f64),
                vram_antes_mib: antes,
                vram_pico_mib: pico,
                vram_modelo_servidor_mib: c.primario.vram_ollama().map(|b| b / (1024 * 1024)),
                campos,
            });
        }
        rm.resumo = Some(resumir(&rm.itens));
        relatorio.modelos.push(rm);
    }
    relatorio
}

fn fmt_opt(v: Option<f64>, casas: usize, sufixo: &str) -> String {
    v.map_or_else(|| "n/d".to_owned(), |x| format!("{x:.casas$}{sufixo}"))
}

fn fmt_pct(v: Option<f64>) -> String {
    v.map_or_else(|| "n/d".to_owned(), |x| format!("{:.1}%", x * 100.0))
}

impl RelatorioBenchmark {
    /// JSON legível.
    ///
    /// # Errors
    /// Falha de serialização (não esperada).
    pub fn para_json(&self) -> Resultado<String> {
        serde_json::to_string_pretty(self).map_err(|e| ErroIa::Configuracao(e.to_string()))
    }

    /// Relatório em Markdown.
    #[must_use]
    pub fn para_markdown(&self) -> String {
        let mut s = String::from("# Benchmark de modelos de classificação bibliográfica\n\n");
        s.push_str(&format!(
            "- Casos: {}\n- VRAM medida: {}{}\n\n",
            self.casos,
            if self.vram_medida {
                "sim (nvidia-smi)"
            } else {
                "NÃO (n/d)"
            },
            self.gpu_total_mib
                .map_or(String::new(), |t| format!(", GPU com {t} MiB")),
        ));
        for o in &self.observacoes {
            s.push_str(&format!("> {o}\n"));
        }
        for m in &self.modelos {
            s.push_str(&format!("\n## {}\n\n", m.versao));
            if let Some(i) = &m.info_servidor {
                s.push_str(&format!(
                    "Servidor: quantização {}, contexto {}, parâmetros {}, versão {}\n\n",
                    i.quantizacao.as_deref().unwrap_or("n/d"),
                    i.n_ctx.map_or("n/d".to_owned(), |n| n.to_string()),
                    i.parametros.as_deref().unwrap_or("n/d"),
                    i.versao_servidor.as_deref().unwrap_or("n/d"),
                ));
            }
            if let StatusModelo::NaoExecutado { motivo } = &m.status {
                s.push_str(&format!(
                    "**NÃO EXECUTADO**: {motivo}. Nenhum número é apresentado.\n"
                ));
                continue;
            }
            let Some(r) = &m.resumo else { continue };
            s.push_str(&format!(
                "Itens: {} ({} com resposta utilizável). Tempo médio: {}. Tokens/s médio: {}. ms/página médio: {}. VRAM pico: {}. Precisão global: {}.\n\n",
                r.itens,
                r.itens_ok,
                fmt_opt(r.tempo_medio_ms, 0, " ms"),
                fmt_opt(r.tokens_por_segundo_medio, 1, ""),
                fmt_opt(r.ms_por_pagina_medio, 0, ""),
                r.vram_pico_mib.map_or("n/d".to_owned(), |v| format!("{v} MiB")),
                fmt_pct(r.precisao_global),
            ));
            s.push_str("| Campo | Corretos | Errados | Omitidos | Inventados | Precisão | Cobertura |\n|---|---:|---:|---:|---:|---:|---:|\n");
            for (c, f) in &r.por_campo {
                s.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} | {} |\n",
                    nome_campo(*c),
                    f.corretos,
                    f.errados,
                    f.omitidos,
                    f.inventados,
                    fmt_pct(f.precisao),
                    fmt_pct(f.cobertura),
                ));
            }
            s.push_str("\n| Caso | OK | Tempo (ms) | Tokens/s | VRAM antes | VRAM pico | Erro |\n|---|---|---:|---:|---:|---:|---|\n");
            for i in &m.itens {
                s.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} | {} |\n",
                    i.id,
                    if i.chamada_ok { "sim" } else { "não" },
                    i.tempo_ms,
                    fmt_opt(i.tokens_por_segundo, 1, ""),
                    i.vram_antes_mib.map_or("n/d".into(), |v| v.to_string()),
                    i.vram_pico_mib.map_or("n/d".into(), |v| v.to_string()),
                    i.erro.as_deref().unwrap_or(""),
                ));
            }
        }
        s
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn vs(c: Campo, v: &str) -> ValorSugerido {
        ValorSugerido {
            campo: c,
            valor: Some(v.into()),
            lista: vec![],
            confianca: 0.8,
            evidencias: vec![],
        }
    }

    #[test]
    fn avaliacao_cobre_os_cinco_casos() {
        let t = |s: &str| Some(GabaritoValor::Texto(s.into()));
        assert_eq!(
            avaliar(
                Campo::Titulo,
                &t("Dom Casmurro"),
                Some(&vs(Campo::Titulo, "dom  casmurro"))
            ),
            Avaliacao::Correto
        );
        assert_eq!(
            avaliar(Campo::Titulo, &t("A"), Some(&vs(Campo::Titulo, "B"))),
            Avaliacao::Errado
        );
        assert_eq!(avaliar(Campo::Titulo, &t("A"), None), Avaliacao::Omitido);
        assert_eq!(
            avaliar(Campo::Isbn, &None, Some(&vs(Campo::Isbn, "9788535902778"))),
            Avaliacao::Inventado
        );
        assert_eq!(
            avaliar(Campo::Isbn, &None, None),
            Avaliacao::DesconhecidoCorreto
        );
    }

    #[test]
    fn autores_como_lista_ignoram_ordem_dos_nomes() {
        let obtido = ValorSugerido {
            campo: Campo::Autores,
            valor: Some("Stewart, James".into()),
            lista: vec!["Stewart, James".into()],
            confianca: 0.8,
            evidencias: vec![],
        };
        let esp = Some(GabaritoValor::Lista(vec!["James Stewart".into()]));
        assert_eq!(
            avaliar(Campo::Autores, &esp, Some(&obtido)),
            Avaliacao::Correto
        );
    }
}
