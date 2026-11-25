//! Pontos de encaixe da extração de livros e da IA: dois traits pequenos, com a implementação
//! real (PDF/DjVu via `dag-biblio-pdf`; EPUB/MOBI/FB2/CBZ/compactados via `dag-biblio-ebook`; IA
//! via `dag-biblio-ia`) e uma implementação "indisponível" que falha de forma explícita.
//!
//! Trocar o comportamento (testes, outro motor) é trocar uma linha em
//! `Agente::com_extrator_livro` / `Agente::com_classificador_ia`.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dag_biblio::{EstadoLeitura, FormatoLivro, ResultadoExtracao};
use dag_biblio_ebook::{Extraido, OpcoesExtracao};
use dag_biblio_ia::{
    Backend, Classificador, ConfigCliente, ConfigIa, EntradaClassificacao, PapelModelo,
    PerfilModelo, ResultadoClassificacao,
};
use dag_biblio_pdf::{ConfigExtracaoPdf, Extrator, Fonte};
use dag_core::ConfigBiblioteca;

/// Resultado de extrair um arquivo.
#[derive(Debug, Clone)]
pub struct SaidaExtracao {
    pub resultado: ResultadoExtracao,
    /// Páginas que precisaram de OCR (já em `resultado.paginas_ocr`, aqui só a contagem).
    pub paginas_ocr: u32,
    /// Capa (bytes da imagem) para a passada de visão da IA, quando houver.
    pub capa: Option<Vec<u8>>,
    pub avisos: Vec<String>,
}

/// Um livro extraído de um arquivo: o próprio arquivo (`caminho_interno` vazio) ou um Membro de
/// compactado (`a.zip!/x/livro.epub`). Um compactado com vários livros devolve um por livro.
#[derive(Debug, Clone)]
pub struct SaidaMembro {
    /// Vazio = o próprio arquivo.
    pub caminho_interno: String,
    pub formato: FormatoLivro,
    /// Tamanho do Membro (0 = o do arquivo).
    pub tamanho: u64,
    pub saida: SaidaExtracao,
}

/// Extrai metadados, texto, capa e índice de um livro, em leitura apenas (ADR 0006: nada é
/// extraído, montado ou gravado em disco; contêineres são lidos em memória).
pub trait ExtratorLivro: Send + Sync {
    /// Nome (para o histórico e os logs).
    fn nome(&self) -> String;

    /// Extrai o livro em `caminho`. `Err` = falha da própria ferramenta (ausente, travou): o job
    /// tenta de novo; formato ilegível, DRM e senha voltam em `resultado.estado`.
    fn extrair(
        &self,
        caminho: &Path,
        formato: FormatoLivro,
        cfg: &ConfigBiblioteca,
    ) -> Result<SaidaExtracao, String>;

    /// Extrai todos os livros do arquivo: um para arquivo comum; um por livro de compactado.
    /// A implementação padrão devolve o resultado de [`ExtratorLivro::extrair`].
    ///
    /// # Errors
    /// Falha da própria ferramenta (ausente, travou).
    fn extrair_membros(
        &self,
        caminho: &Path,
        formato: FormatoLivro,
        cfg: &ConfigBiblioteca,
    ) -> Result<Vec<SaidaMembro>, String> {
        self.extrair(caminho, formato, cfg).map(|saida| {
            vec![SaidaMembro {
                caminho_interno: String::new(),
                formato,
                tamanho: 0,
                saida,
            }]
        })
    }
}

/// Extrator "indisponível": todo formato vira `NaoSuportado` com o motivo.
#[derive(Debug, Clone)]
pub struct ExtratorIndisponivel {
    pub motivo: String,
}

impl ExtratorLivro for ExtratorIndisponivel {
    fn nome(&self) -> String {
        "indisponivel".into()
    }

    fn extrair(
        &self,
        _caminho: &Path,
        formato: FormatoLivro,
        _cfg: &ConfigBiblioteca,
    ) -> Result<SaidaExtracao, String> {
        Ok(SaidaExtracao {
            resultado: resultado_vazio(
                formato,
                EstadoLeitura::NaoSuportado {
                    motivo: self.motivo.clone(),
                },
            ),
            paginas_ocr: 0,
            capa: None,
            avisos: Vec::new(),
        })
    }
}

fn resultado_vazio(formato: FormatoLivro, estado: EstadoLeitura) -> ResultadoExtracao {
    ResultadoExtracao {
        formato,
        estado,
        metadados: Vec::new(),
        isbns: Vec::new(),
        sumario: Vec::new(),
        paginas: Vec::new(),
        total_paginas: None,
        ficha_catalografica: None,
        capa: None,
        capa_mime: None,
        paginas_ocr: Vec::new(),
        avisos: Vec::new(),
    }
}

/// Extrator real: PDF e DjVu pelo `dag-biblio-pdf` (com OCR seletivo), o resto pelo
/// `dag-biblio-ebook`. PDF/DjVu dentro de compactado vão da memória ao `dag-biblio-pdf`.
#[derive(Default)]
pub struct ExtratorPadrao {
    pdf: Mutex<Option<Arc<Extrator>>>,
}

impl std::fmt::Debug for ExtratorPadrao {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtratorPadrao").finish_non_exhaustive()
    }
}

fn config_pdf(cfg: &ConfigBiblioteca) -> ConfigExtracaoPdf {
    let mut c = ConfigExtracaoPdf {
        paginas_iniciais: cfg.ocr.paginas_iniciais.max(1),
        dpi_ocr: cfg.ocr.dpi.max(72),
        idiomas_ocr: cfg.ocr.idiomas.clone(),
        max_paginas_ocr: if cfg.ocr.ativo {
            cfg.ocr.max_paginas
        } else {
            0
        },
        ..ConfigExtracaoPdf::default()
    };
    c.limites.max_bytes_arquivo = cfg.max_bytes_arquivo;
    c
}

impl ExtratorPadrao {
    #[must_use]
    pub fn novo() -> Self {
        Self::default()
    }

    fn extrator_pdf(&self, cfg: &ConfigBiblioteca) -> Result<Arc<Extrator>, String> {
        let mut g = self.pdf.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = g.as_ref() {
            return Ok(Arc::clone(e));
        }
        let e = Arc::new(Extrator::detectar(config_pdf(cfg)).map_err(|e| e.to_string())?);
        *g = Some(Arc::clone(&e));
        Ok(e)
    }

    fn por_pdf(
        &self,
        fonte: Fonte<'_>,
        formato: FormatoLivro,
        cfg: &ConfigBiblioteca,
    ) -> Result<SaidaExtracao, String> {
        let ex = self.extrator_pdf(cfg)?;
        let rel = ex.extrair(fonte, formato);
        let mut r = rel.resultado;
        let capa = r.capa.take();
        let ocr = u32::try_from(r.paginas_ocr.len()).unwrap_or(u32::MAX);
        let avisos = r.avisos.clone();
        Ok(SaidaExtracao {
            resultado: r,
            paginas_ocr: ocr,
            capa,
            avisos,
        })
    }
}

impl ExtratorLivro for ExtratorPadrao {
    fn nome(&self) -> String {
        "dag-biblio-pdf+dag-biblio-ebook".into()
    }

    fn extrair(
        &self,
        caminho: &Path,
        formato: FormatoLivro,
        cfg: &ConfigBiblioteca,
    ) -> Result<SaidaExtracao, String> {
        let mut v = self.extrair_membros(caminho, formato, cfg)?;
        if v.is_empty() {
            return Err("nada extraído".into());
        }
        Ok(v.remove(0).saida)
    }

    fn extrair_membros(
        &self,
        caminho: &Path,
        formato: FormatoLivro,
        cfg: &ConfigBiblioteca,
    ) -> Result<Vec<SaidaMembro>, String> {
        let um = |saida: SaidaExtracao| {
            vec![SaidaMembro {
                caminho_interno: String::new(),
                formato,
                tamanho: 0,
                saida,
            }]
        };
        if matches!(formato, FormatoLivro::Pdf | FormatoLivro::Djvu) {
            return self.por_pdf(Fonte::Caminho(caminho), formato, cfg).map(um);
        }
        let opc = OpcoesExtracao {
            reter_dados_nao_tratados: true,
            ..OpcoesExtracao::default()
        };
        // `Outro` (compactado) não é dica: a assinatura decide.
        let dica = (formato != FormatoLivro::Outro).then_some(formato);
        let extraido =
            dag_biblio_ebook::extrair_arquivo(caminho, dica, &opc).map_err(|e| e.to_string())?;
        match extraido {
            Extraido::Livro(mut r) => {
                let capa = r.capa.take();
                let avisos = r.avisos.clone();
                Ok(um(SaidaExtracao {
                    resultado: r,
                    paginas_ocr: 0,
                    capa,
                    avisos,
                }))
            }
            Extraido::Compactado(c) => {
                let mut avisos = c.avisos.clone();
                avisos.push(format!(
                    "compactado ({}): {} livro(s) dentro",
                    c.conteiner,
                    c.membros.len()
                ));
                for (nome, motivo) in &c.rejeitados {
                    avisos.push(format!("membro rejeitado {nome}: {motivo}"));
                }
                // Todos os livros do compactado, cada um com o seu caminho interno. PDF/DjVu
                // vão da memória (memfd) ao `dag-biblio-pdf`; nada toca o disco (ADR 0006).
                let mut saidas = Vec::new();
                for m in &c.membros {
                    let saida = if let Some(dados) = &m.dados
                        && matches!(m.formato, FormatoLivro::Pdf | FormatoLivro::Djvu)
                    {
                        match self.por_pdf(Fonte::Bytes(dados), m.formato, cfg) {
                            Ok(s) => s,
                            // A falha de um Membro não derruba os outros livros do compactado.
                            Err(e) => SaidaExtracao {
                                resultado: resultado_vazio(
                                    m.formato,
                                    EstadoLeitura::NaoSuportado { motivo: e },
                                ),
                                paginas_ocr: 0,
                                capa: None,
                                avisos: Vec::new(),
                            },
                        }
                    } else {
                        let mut r = m.resultado.clone();
                        let capa = r.capa.take();
                        let av = r.avisos.clone();
                        SaidaExtracao {
                            resultado: r,
                            paginas_ocr: 0,
                            capa,
                            avisos: av,
                        }
                    };
                    let mut saida = saida;
                    saida.avisos.extend(avisos.clone());
                    saida.resultado.avisos.extend(avisos.clone());
                    saida
                        .resultado
                        .avisos
                        .push(format!("membro: {}", m.caminho_interno));
                    saidas.push(SaidaMembro {
                        caminho_interno: m.caminho_interno.clone(),
                        formato: m.formato,
                        tamanho: m.tamanho,
                        saida,
                    });
                }
                if saidas.is_empty() {
                    let estado = if matches!(c.estado, EstadoLeitura::Lido) {
                        EstadoLeitura::NaoSuportado {
                            motivo: "compactado sem livro legível".into(),
                        }
                    } else {
                        c.estado.clone()
                    };
                    let mut r = resultado_vazio(formato, estado);
                    r.avisos = avisos.clone();
                    saidas.push(SaidaMembro {
                        caminho_interno: String::new(),
                        formato,
                        tamanho: 0,
                        saida: SaidaExtracao {
                            resultado: r,
                            paginas_ocr: 0,
                            capa: None,
                            avisos,
                        },
                    });
                }
                Ok(saidas)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// IA
// ---------------------------------------------------------------------------------------------

/// Classifica livros com IA local; nunca decide nada sozinha (só sugere, com evidência).
pub trait ClassificadorIa: Send + Sync {
    fn nome(&self) -> String;

    /// `Err` = IA indisponível (serviço fora do ar, modelo ausente): o pipeline segue sem ela.
    /// Falhas do modelo viram revisão humana dentro do resultado.
    fn classificar(&self, entrada: &EntradaClassificacao)
    -> Result<ResultadoClassificacao, String>;

    /// Classifica vários livros de uma vez, na ordem das entradas. A implementação padrão chama
    /// `classificar` item a item; `IaLocal` agrupa as chamadas por modelo (menos trocas na GPU).
    fn classificar_lote(
        &self,
        entradas: &[EntradaClassificacao],
    ) -> Vec<Result<ResultadoClassificacao, String>> {
        entradas.iter().map(|e| self.classificar(e)).collect()
    }
}

/// IA indisponível.
#[derive(Debug, Clone)]
pub struct IaIndisponivel {
    pub motivo: String,
}

impl ClassificadorIa for IaIndisponivel {
    fn nome(&self) -> String {
        "indisponivel".into()
    }

    fn classificar(&self, _: &EntradaClassificacao) -> Result<ResultadoClassificacao, String> {
        Err(self.motivo.clone())
    }
}

/// IA real: `dag_biblio_ia::Classificador` (Ollama ou llama-server em loopback).
pub struct IaLocal {
    inner: Classificador,
    nome: String,
}

impl std::fmt::Debug for IaLocal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IaLocal").field("nome", &self.nome).finish()
    }
}

impl IaLocal {
    /// Monta a partir da configuração (primário e, se houver, segundo parecer).
    ///
    /// # Errors
    /// Configuração inválida (endpoint fora do loopback, por exemplo).
    pub fn da_config(cfg: &ConfigBiblioteca) -> Result<Self, String> {
        let mut primario = PerfilModelo::ollama(
            &cfg.ia.modelo_primario,
            cfg.ia.usar_visao,
            PapelModelo::Primario,
        );
        primario.endpoint.clone_from(&cfg.ia.endpoint);
        primario.backend = Backend::Ollama;
        let mut modelos = vec![primario];
        if let Some(s) = &cfg.ia.modelo_secundario {
            let mut p = PerfilModelo::ollama(s, false, PapelModelo::Secundario);
            p.endpoint.clone_from(&cfg.ia.endpoint);
            // Segundo parecer com contexto menor: cabe na VRAM junto dos outros programas.
            p.contexto = 4096;
            modelos.push(p);
        }
        // Qwen3 (e outros com "thinking") respondem direto no JSON Schema só com `think: false`.
        for m in &mut modelos {
            if m.nome.to_lowercase().starts_with("qwen3") {
                m.extras
                    .insert("think".into(), serde_json::Value::Bool(false));
            }
        }
        let ia_cfg = ConfigIa {
            modelos,
            ..ConfigIa::default()
        };
        let cliente = ConfigCliente {
            timeout: Duration::from_secs(cfg.ia.timeout_s.max(5)),
            ..ConfigCliente::default()
        };
        let inner = Classificador::da_config(&ia_cfg, &cliente).map_err(|e| e.to_string())?;
        Ok(Self {
            inner,
            nome: cfg.ia.modelo_primario.clone(),
        })
    }
}

impl ClassificadorIa for IaLocal {
    fn nome(&self) -> String {
        self.nome.clone()
    }

    fn classificar(
        &self,
        entrada: &EntradaClassificacao,
    ) -> Result<ResultadoClassificacao, String> {
        // `classificar` nunca entra em pânico nem devolve erro: falhas viram revisão humana.
        Ok(self.inner.classificar(entrada))
    }

    fn classificar_lote(
        &self,
        entradas: &[EntradaClassificacao],
    ) -> Vec<Result<ResultadoClassificacao, String>> {
        self.inner
            .classificar_lote(entradas)
            .into_iter()
            .map(Ok)
            .collect()
    }
}
