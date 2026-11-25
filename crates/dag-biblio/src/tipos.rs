//! Tipos compartilhados da Fase B. Mudanças aqui são só aditivas (campos com `#[serde(default)]`).

use serde::{Deserialize, Serialize};

/// Papel de uma biblioteca Calibre conhecida pelo DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PapelBiblioteca {
    /// Biblioteca Homologada: só leitura para o DAG.
    Homologada,
    /// A Classificar: biblioteca Calibre separada, criada pelo DAG.
    AClassificar,
}

/// Formato bibliográfico (o tipo do arquivo de uma Edição), como o Calibre o nomeia (`EPUB`, `PDF`...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatoLivro {
    Pdf,
    Epub,
    Mobi,
    Azw,
    Azw3,
    Kfx,
    Djvu,
    Fb2,
    Cbz,
    Cbr,
    Cb7,
    Txt,
    Rtf,
    Docx,
    Html,
    Outro,
}

impl FormatoLivro {
    /// Pela extensão (sem ponto, qualquer caixa) ou pelo nome de formato do Calibre.
    #[must_use]
    pub fn da_extensao(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "pdf" => Self::Pdf,
            "epub" | "kepub" => Self::Epub,
            "mobi" | "prc" => Self::Mobi,
            "azw" => Self::Azw,
            "azw3" => Self::Azw3,
            "kfx" => Self::Kfx,
            "djvu" | "djv" => Self::Djvu,
            "fb2" => Self::Fb2,
            "cbz" => Self::Cbz,
            "cbr" => Self::Cbr,
            "cb7" => Self::Cb7,
            "txt" => Self::Txt,
            "rtf" => Self::Rtf,
            "docx" => Self::Docx,
            "html" | "htm" | "htmlz" => Self::Html,
            _ => return None,
        })
    }

    /// Nome do formato como o Calibre grava em `data.format` (maiúsculas).
    #[must_use]
    pub const fn nome_calibre(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Epub => "EPUB",
            Self::Mobi => "MOBI",
            Self::Azw => "AZW",
            Self::Azw3 => "AZW3",
            Self::Kfx => "KFX",
            Self::Djvu => "DJVU",
            Self::Fb2 => "FB2",
            Self::Cbz => "CBZ",
            Self::Cbr => "CBR",
            Self::Cb7 => "CB7",
            Self::Txt => "TXT",
            Self::Rtf => "RTF",
            Self::Docx => "DOCX",
            Self::Html => "HTML",
            Self::Outro => "OUTRO",
        }
    }

    /// Quadrinhos (não misturar números diferentes).
    #[must_use]
    pub const fn e_quadrinho(self) -> bool {
        matches!(self, Self::Cbz | Self::Cbr | Self::Cb7)
    }
}

/// Origem de uma evidência, em ordem decrescente de força (regra de classificação da spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrigemEvidencia {
    /// Hash Completo idêntico a um Formato homologado.
    HashVerificado,
    /// ISBN/DOI validado por dígito e corroborado.
    Identificador,
    /// Ficha catalográfica (CIP) ou folha de rosto.
    FichaCatalografica,
    /// Sumário/índice, texto do miolo ou capa.
    Conteudo,
    /// OCR de página sem texto.
    Ocr,
    /// Metadados internos do arquivo (OPF, Info do PDF, EXTH, ComicInfo).
    MetadadoInterno,
    /// Sugestão de modelo de IA (sempre exige outra evidência para alta confiança).
    Ia,
    /// Biblioteca Homologada (vocabulário, autor existente).
    Homologada,
    /// Nome do arquivo ou da pasta.
    NomeArquivo,
    /// Decisão humana.
    Humano,
}

/// Um trecho que sustenta um valor: de onde veio, em que página e com que confiança.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidencia {
    pub origem: OrigemEvidencia,
    /// Página (1-based) quando aplicável.
    #[serde(default)]
    pub pagina: Option<u32>,
    /// Trecho literal (curto, até ~500 caracteres) que sustenta o valor.
    #[serde(default)]
    pub trecho: Option<String>,
    /// Detalhe livre: nome do campo OPF, chave EXTH, modelo de IA...
    #[serde(default)]
    pub detalhe: Option<String>,
    /// 0.0..=1.0.
    pub confianca: f32,
}

/// Campos bibliográficos que o DAG sabe sugerir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Campo {
    Titulo,
    Subtitulo,
    Autores,
    Editora,
    Edicao,
    Data,
    Idioma,
    Isbn,
    Doi,
    Serie,
    IndiceSerie,
    Volume,
    Resumo,
    Tags,
    Estante,
    Capa,
    Paginas,
}

/// Valor proposto para um [`Campo`], com evidências. `valor = None` é "desconhecido" explícito.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValorSugerido {
    pub campo: Campo,
    /// Texto (listas, como autores e tags, separadas por `&` como no Calibre; use [`Self::lista`]).
    #[serde(default)]
    pub valor: Option<String>,
    #[serde(default)]
    pub lista: Vec<String>,
    pub confianca: f32,
    #[serde(default)]
    pub evidencias: Vec<Evidencia>,
}

/// Faixa de confiança usada na revisão.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaixaConfianca {
    Baixa,
    Media,
    Alta,
}

impl FaixaConfianca {
    /// Alta >= 0.90, média >= 0.60.
    #[must_use]
    pub fn de(c: f32) -> Self {
        if c >= 0.90 {
            Self::Alta
        } else if c >= 0.60 {
            Self::Media
        } else {
            Self::Baixa
        }
    }
}

/// Um item de sumário/índice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSumario {
    pub titulo: String,
    #[serde(default)]
    pub nivel: u8,
    #[serde(default)]
    pub pagina: Option<u32>,
}

/// Texto de uma página e como foi obtido.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaginaTexto {
    /// 1-based.
    pub numero: u32,
    pub texto: String,
    pub via_ocr: bool,
    /// Qualidade 0..=1 (confiança média do OCR, ou proporção de caracteres "bons" no texto nativo).
    pub qualidade: f32,
    #[serde(default)]
    pub idioma_ocr: Option<String>,
}

/// Situação de leitura de um arquivo bibliográfico.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "estado", rename_all = "snake_case")]
pub enum EstadoLeitura {
    Lido,
    /// DRM detectado: não é contornado.
    Drm,
    /// Protegido por senha.
    Senha,
    Corrompido {
        motivo: String,
    },
    NaoSuportado {
        motivo: String,
    },
    /// Ultrapassou limite de tamanho/tempo/expansão.
    LimiteExcedido {
        limite: String,
    },
}

/// Tudo o que a extração tirou de um arquivo, antes de qualquer IA.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultadoExtracao {
    pub formato: FormatoLivro,
    pub estado: EstadoLeitura,
    /// Metadados internos já normalizados em valores sugeridos (origem `MetadadoInterno`).
    #[serde(default)]
    pub metadados: Vec<ValorSugerido>,
    /// ISBNs válidos encontrados (normalizados para ISBN-13), com evidência.
    #[serde(default)]
    pub isbns: Vec<(String, Evidencia)>,
    #[serde(default)]
    pub sumario: Vec<ItemSumario>,
    /// Páginas amostradas (amostragem progressiva).
    #[serde(default)]
    pub paginas: Vec<PaginaTexto>,
    #[serde(default)]
    pub total_paginas: Option<u32>,
    /// Texto da ficha catalográfica (CIP), se achada.
    #[serde(default)]
    pub ficha_catalografica: Option<(u32, String)>,
    /// Capa em memória (JPEG/PNG), sem gravar em disco.
    #[serde(default, skip_serializing)]
    pub capa: Option<Vec<u8>>,
    #[serde(default)]
    pub capa_mime: Option<String>,
    /// Páginas sem texto nativo que precisaram de OCR.
    #[serde(default)]
    pub paginas_ocr: Vec<u32>,
    /// Avisos não fatais.
    #[serde(default)]
    pub avisos: Vec<String>,
}

/// Tipo de candidato a duplicidade (spec: EXATA/EQUIVALENTE/MESMA_OBRA).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoDuplicidade {
    /// Mesmo Hash Completo e tamanho: mesmo conteúdo. Única que pode gerar espaço recuperável.
    Exata,
    /// Mesma Edição em outro arquivo (outro formato, outra digitalização): nunca é descartável
    /// automaticamente e não gera economia.
    MesmaEdicao,
    /// Mesma Obra, Edição diferente (tradução, revisão, edição): preservar.
    MesmaObra,
    /// Hipótese fraca, vai para revisão.
    Incerta,
}

/// Autor como o Calibre o guarda.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AutorRef {
    pub nome: String,
    /// `author_sort` do Calibre, quando conhecido.
    #[serde(default)]
    pub ordenacao: Option<String>,
}

/// Livro da Biblioteca Homologada (uma linha de `books` e o que pende dela).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct LivroCalibre {
    /// `books.id`.
    pub id: i64,
    pub uuid: Option<String>,
    pub titulo: String,
    pub titulo_ordenacao: Option<String>,
    pub autores: Vec<String>,
    pub autor_ordenacao: Option<String>,
    pub tags: Vec<String>,
    pub serie: Option<String>,
    pub indice_serie: Option<f64>,
    pub editora: Option<String>,
    pub idiomas: Vec<String>,
    /// (tipo, valor), p. ex. ("isbn", "9788535902778").
    pub identificadores: Vec<(String, String)>,
    pub data_publicacao: Option<String>,
    pub comentarios: Option<String>,
    pub avaliacao: Option<i64>,
    /// Caminho relativo da pasta do livro na biblioteca (`books.path`).
    pub caminho: String,
    pub tem_capa: bool,
    /// Valores de colunas personalizadas: (rótulo sem `#`, valor serializado).
    pub personalizados: Vec<(String, serde_json::Value)>,
    pub formatos: Vec<FormatoCalibre>,
}

/// Um arquivo de formato de um livro Calibre (`data`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatoCalibre {
    pub formato: String,
    /// `data.name` (nome do arquivo sem extensão).
    pub nome: String,
    pub tamanho: u64,
    /// Caminho absoluto do arquivo, montado a partir da biblioteca.
    pub caminho_absoluto: String,
}
