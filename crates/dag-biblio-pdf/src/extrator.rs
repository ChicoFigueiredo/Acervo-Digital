//! O [`Extrator`]: ponto de entrada público, e a consolidação do resultado (comum a PDF e DjVu).

use std::collections::BTreeMap;
use std::time::Instant;

use dag_biblio::{
    Campo, EstadoLeitura, Evidencia, FormatoLivro, ItemSumario, OrigemEvidencia, ResultadoExtracao,
    ValorSugerido,
};

use crate::config::ConfigExtracaoPdf;
use crate::ferramentas::{ErroFerramentas, Ferramentas};
use crate::fonte::Fonte;
use crate::motor::{Analise, ConfigOcr, Evidencias, estado_final};
use crate::relatorio::RelatorioExtracao;

/// Extrai metadados, texto, capa, ficha catalográfica, ISBN e sumário de PDF e DjVu.
#[derive(Debug, Clone)]
pub struct Extrator {
    pub(crate) ferramentas: Ferramentas,
    pub(crate) config: ConfigExtracaoPdf,
}

impl Extrator {
    #[must_use]
    pub fn novo(ferramentas: Ferramentas, config: ConfigExtracaoPdf) -> Self {
        Self {
            ferramentas,
            config,
        }
    }

    /// Detecta as ferramentas no `PATH` (erro claro se faltar alguma obrigatória).
    pub fn detectar(config: ConfigExtracaoPdf) -> Result<Self, ErroFerramentas> {
        Ok(Self::novo(Ferramentas::detectar()?, config))
    }

    #[must_use]
    pub fn ferramentas(&self) -> &Ferramentas {
        &self.ferramentas
    }

    #[must_use]
    pub fn config(&self) -> &ConfigExtracaoPdf {
        &self.config
    }

    /// Extrai conforme o formato. Só `Pdf` e `Djvu` são tratados aqui; outros viram `NaoSuportado`.
    #[must_use]
    pub fn extrair(&self, fonte: Fonte<'_>, formato: FormatoLivro) -> RelatorioExtracao {
        match formato {
            FormatoLivro::Pdf => self.extrair_pdf(fonte),
            FormatoLivro::Djvu => self.extrair_djvu(fonte),
            outro => {
                let mut r = resultado_base(
                    outro,
                    EstadoLeitura::NaoSuportado {
                        motivo: format!(
                            "{} não é tratado por dag-biblio-pdf",
                            outro.nome_calibre()
                        ),
                    },
                );
                r.avisos
                    .push("formato fora do escopo deste extrator".into());
                RelatorioExtracao::vazio(r)
            }
        }
    }

    /// OCR resolvido: tesseract presente e ao menos um idioma pedido instalado.
    pub(crate) fn config_ocr(&self, avisos: &mut Vec<String>) -> Option<ConfigOcr<'_>> {
        let Some(t) = &self.ferramentas.tesseract else {
            avisos.push("tesseract ausente: sem OCR".into());
            return None;
        };
        let Some(idiomas) = self
            .ferramentas
            .idiomas_ocr_efetivos(&self.config.idiomas_ocr)
        else {
            avisos.push(format!(
                "nenhum dos idiomas de OCR pedidos ({}) está instalado no tesseract (há: {})",
                self.config.idiomas_ocr.join("+"),
                self.ferramentas.idiomas_tesseract.join(", ")
            ));
            return None;
        };
        let faltam: Vec<&String> = self
            .config
            .idiomas_ocr
            .iter()
            .filter(|i| !self.ferramentas.idiomas_tesseract.contains(i))
            .collect();
        if !faltam.is_empty() {
            avisos.push(format!(
                "idiomas de OCR ausentes e ignorados: {}",
                faltam
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Some(ConfigOcr {
            tesseract: t,
            idiomas,
        })
    }
}

/// `ResultadoExtracao` vazio com o estado dado.
#[must_use]
pub(crate) fn resultado_base(formato: FormatoLivro, estado: EstadoLeitura) -> ResultadoExtracao {
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

/// Junta metadados nativos, evidências do texto e medições no relatório final.
pub(crate) fn consolidar(
    rel: &mut RelatorioExtracao,
    analise: Analise<'_>,
    ev: Evidencias,
    isbns_nativos: Vec<(String, Evidencia)>,
    sumario_nativo: Vec<ItemSumario>,
    inicio: Instant,
) {
    let ilegivel = analise.texto_ilegivel();
    let r = &mut rel.resultado;
    r.paginas = analise.paginas.into_values().collect();
    r.paginas_ocr = analise.paginas_ocr;
    r.paginas_ocr.sort_unstable();
    r.avisos.extend(analise.avisos);
    rel.alertas_conteudo = analise.alertas;
    rel.fases = analise.fases;
    rel.medicoes = analise.medicoes.into_values().collect();

    // ISBN: nativo e do texto, uma evidência (a mais forte) por número.
    let mut por_isbn: BTreeMap<String, Evidencia> = BTreeMap::new();
    for (i, e) in isbns_nativos.into_iter().chain(ev.isbns) {
        match por_isbn.get(&i) {
            Some(atual) if atual.confianca >= e.confianca => {}
            _ => {
                por_isbn.insert(i, e);
            }
        }
    }
    let mut isbns: Vec<(String, Evidencia)> = por_isbn.into_iter().collect();
    isbns.sort_by(|a, b| b.1.confianca.total_cmp(&a.1.confianca).then(a.0.cmp(&b.0)));
    if let Some((i, e)) = isbns.first() {
        r.metadados.push(ValorSugerido {
            campo: Campo::Isbn,
            valor: Some(i.clone()),
            lista: isbns.iter().map(|(n, _)| n.clone()).collect(),
            confianca: e.confianca,
            evidencias: vec![e.clone()],
        });
    }
    r.isbns = isbns;

    r.metadados.extend(ev.valores_ficha);
    if let Some(l) = ev.idioma {
        r.metadados.push(ValorSugerido {
            campo: Campo::Idioma,
            valor: Some(l.to_string()),
            lista: Vec::new(),
            confianca: 0.6,
            evidencias: vec![Evidencia {
                origem: OrigemEvidencia::Conteudo,
                pagina: None,
                trecho: None,
                detalhe: Some("palavras funcionais do texto amostrado".into()),
                confianca: 0.6,
            }],
        });
    }
    r.ficha_catalografica = ev.ficha;
    r.sumario = if sumario_nativo.is_empty() {
        if !ev.sumario_texto.is_empty() {
            r.avisos
                .push("sumário obtido do texto das páginas (o arquivo não tem marcadores)".into());
        }
        ev.sumario_texto
    } else {
        sumario_nativo
    };

    if matches!(r.estado, EstadoLeitura::Lido) {
        r.estado = if ilegivel && analise.limite.is_none() {
            EstadoLeitura::Corrompido {
                motivo: "nenhuma página pôde ser lida pelas ferramentas".into(),
            }
        } else {
            estado_final(analise.limite)
        };
    }
    rel.duracao_total_ms = inicio.elapsed().as_secs_f64() * 1000.0;
    let ocr_ms = rel
        .medicoes
        .iter()
        .filter_map(|m| m.tempo_ocr_ms)
        .fold(0.0, |a, b| a + b);
    let texto_ms = rel.medicoes.iter().fold(0.0, |a, m| a + m.tempo_texto_ms);
    rel.resultado.avisos.push(format!(
        "medicao: paginas={} ocr={} texto_ms={texto_ms:.0} ocr_ms={ocr_ms:.0} total_ms={:.0}",
        rel.medicoes.len(),
        rel.resultado.paginas_ocr.len(),
        rel.duracao_total_ms
    ));
}
