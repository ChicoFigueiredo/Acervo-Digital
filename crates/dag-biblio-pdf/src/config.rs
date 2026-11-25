//! Configuração da extração, com padrões sensatos.

use std::time::Duration;

/// Limites duros de execução. Estourar qualquer um vira `EstadoLeitura::LimiteExcedido`.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitesExecucao {
    /// Tamanho máximo do arquivo (bytes). Padrão 2 GiB.
    pub max_bytes_arquivo: u64,
    /// Páginas que o documento pode declarar. Acima disso é rejeitado (PDF-bomba). Padrão 50 000.
    pub max_paginas_documento: u32,
    /// Tempo máximo de cada chamada de ferramenta, por página envolvida. Padrão 120 s.
    pub timeout_pagina: Duration,
    /// Tempo para as consultas de metadados/sumário (pdfinfo, pdftohtml). Padrão 60 s.
    pub timeout_metadados: Duration,
    /// Tempo total da extração. Estourado, devolve o que já tem com `LimiteExcedido`. Padrão 15 min.
    pub timeout_total: Duration,
    /// Máximo de bytes lidos do stdout de uma ferramenta (o processo é morto ao exceder). Padrão 64 MiB.
    pub max_bytes_saida: usize,
    /// Máximo de caracteres guardados por página (o excedente é cortado, com aviso). Padrão 200 000.
    pub max_caracteres_pagina: usize,
}

impl Default for LimitesExecucao {
    fn default() -> Self {
        Self {
            max_bytes_arquivo: 2 * 1024 * 1024 * 1024,
            max_paginas_documento: 50_000,
            timeout_pagina: Duration::from_secs(120),
            timeout_metadados: Duration::from_secs(60),
            timeout_total: Duration::from_secs(15 * 60),
            max_bytes_saida: 64 * 1024 * 1024,
            max_caracteres_pagina: 200_000,
        }
    }
}

/// Configuração da extração de PDF/DjVu.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigExtracaoPdf {
    /// Páginas da primeira fase da amostragem. Padrão 20.
    pub paginas_iniciais: u32,
    /// Teto de páginas amostradas no total (todas as fases). Padrão 60.
    pub teto_paginas: u32,
    /// Páginas do fim lidas na fase de expansão (índice tardio, colofão). Padrão 10.
    pub paginas_finais: u32,
    /// Amostras espalhadas pelo miolo na última fase de expansão. Padrão 10.
    pub amostras_meio: u32,
    /// Resolução do OCR. Padrão 300 dpi.
    pub dpi_ocr: u32,
    /// Idiomas do tesseract, em ordem de preferência. Padrão `por`, `eng`. Os ausentes são ignorados.
    pub idiomas_ocr: Vec<String>,
    /// Caracteres úteis mínimos para a página ser "texto nativo confiável". Padrão 80.
    pub min_caracteres_pagina: usize,
    /// Qualidade mínima (0..=1) do texto nativo. Padrão 0,85.
    pub qualidade_minima_texto: f32,
    /// Máximo de páginas passadas por OCR numa extração. Padrão 40.
    pub max_paginas_ocr: u32,
    /// Lado maior da capa renderizada, em pixels. Padrão 480.
    pub tamanho_capa: u32,
    pub limites: LimitesExecucao,
}

impl Default for ConfigExtracaoPdf {
    fn default() -> Self {
        Self {
            paginas_iniciais: 20,
            teto_paginas: 60,
            paginas_finais: 10,
            amostras_meio: 10,
            dpi_ocr: 300,
            idiomas_ocr: vec!["por".into(), "eng".into()],
            min_caracteres_pagina: 80,
            qualidade_minima_texto: 0.85,
            max_paginas_ocr: 40,
            tamanho_capa: 480,
            limites: LimitesExecucao::default(),
        }
    }
}
