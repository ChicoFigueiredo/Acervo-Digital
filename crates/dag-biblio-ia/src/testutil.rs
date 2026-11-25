//! Construtores de fixtures para testes unitários.

use dag_biblio::{EstadoLeitura, FormatoLivro, PaginaTexto, ResultadoExtracao};

use crate::entrada::EntradaClassificacao;

pub fn extracao_basica(paginas: Vec<PaginaTexto>) -> ResultadoExtracao {
    ResultadoExtracao {
        formato: FormatoLivro::Pdf,
        estado: EstadoLeitura::Lido,
        metadados: vec![],
        isbns: vec![],
        sumario: vec![],
        paginas,
        total_paginas: None,
        ficha_catalografica: None,
        capa: None,
        capa_mime: None,
        paginas_ocr: vec![],
        avisos: vec![],
    }
}

pub fn pagina(n: u32, t: &str) -> PaginaTexto {
    PaginaTexto {
        numero: n,
        texto: t.into(),
        via_ocr: false,
        qualidade: 1.0,
        idioma_ocr: None,
    }
}

pub fn entrada(paginas: Vec<PaginaTexto>) -> EntradaClassificacao {
    EntradaClassificacao {
        id: "x".into(),
        nome_arquivo: "livro.pdf".into(),
        extracao: extracao_basica(paginas),
        vocabulario: Default::default(),
        exemplos: vec![],
        referencias: vec![],
        imagem_pagina1: None,
    }
}
