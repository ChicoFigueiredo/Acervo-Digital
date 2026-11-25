//! Tipos próprios deste crate: o relatório que acompanha o `ResultadoExtracao` compartilhado.

use std::collections::BTreeMap;

use dag_biblio::ResultadoExtracao;
use serde::{Deserialize, Serialize};

use crate::texto::AlertaConteudo;

/// Como a página foi classificada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassePagina {
    /// Texto nativo confiável (ou curto e limpo): sem OCR.
    Nativa,
    /// Sem texto e com imagem: OCR.
    Imagem,
    /// Pouco texto nativo (ou texto ilegível) mais imagem: OCR e fica o melhor dos dois.
    Hibrida,
    /// Sem texto e sem imagem (página em branco): sem OCR.
    Vazia,
}

/// Medição de uma página, para o benchmark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MedicaoPagina {
    pub pagina: u32,
    pub classe: ClassePagina,
    pub caracteres_uteis: usize,
    pub qualidade_nativa: f32,
    /// Tempo do `pdftotext`/`djvutxt` (em lote: média por página do lote), em milissegundos.
    pub tempo_texto_ms: f64,
    /// Renderização mais tesseract, em milissegundos (só se houve OCR).
    pub tempo_ocr_ms: Option<f64>,
    pub qualidade_ocr: Option<f32>,
}

/// Resultado completo: o contrato compartilhado mais o que é só deste crate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelatorioExtracao {
    pub resultado: ResultadoExtracao,
    pub medicoes: Vec<MedicaoPagina>,
    /// Trechos do livro que parecem instruções a um modelo. Marcados, nunca obedecidos.
    pub alertas_conteudo: Vec<AlertaConteudo>,
    /// Campos brutos do `Info` (Creator, Producer, datas...), sem interpretação.
    pub metadados_brutos: BTreeMap<String, String>,
    /// Idiomas efetivamente usados no OCR (`por+eng`), se houve OCR.
    pub idiomas_ocr: Option<String>,
    /// Fases da amostragem que rodaram (`inicial`, `final`, `meio`).
    pub fases: Vec<String>,
    pub duracao_total_ms: f64,
}

impl RelatorioExtracao {
    pub(crate) fn vazio(resultado: ResultadoExtracao) -> Self {
        Self {
            resultado,
            medicoes: Vec::new(),
            alertas_conteudo: Vec::new(),
            metadados_brutos: BTreeMap::new(),
            idiomas_ocr: None,
            fases: Vec::new(),
            duracao_total_ms: 0.0,
        }
    }
}
