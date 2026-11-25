//! XML sem surpresas: sem DTD (nenhuma entidade definida pelo documento), com limite de nós.
//! Rust puro (`roxmltree`), nada é executado nem buscado fora do documento.

use dag_biblio::{Evidencia, OrigemEvidencia, PaginaTexto};
use roxmltree::{Document, Node, ParsingOptions};

use crate::util;

/// Tamanho máximo de um XML de metadados lido (OPF, NCX, container, ComicInfo...).
pub(crate) const TETO_XML: u64 = 8 << 20;

/// Interpreta `bytes` como XML e aplica `f` ao documento.
pub(crate) fn analisar<T>(bytes: &[u8], f: impl FnOnce(&Document<'_>) -> T) -> Result<T, String> {
    let texto = util::decodificar_documento(bytes);
    let limpo = util::sanear_xml(&texto);
    let opcoes = ParsingOptions {
        allow_dtd: false,
        nodes_limit: 2_000_000,
        ..ParsingOptions::default()
    };
    let doc = Document::parse_with_options(&limpo, opcoes).map_err(|e| e.to_string())?;
    Ok(f(&doc))
}

/// Elementos filhos com o nome local `nome`.
pub(crate) fn filhos<'a, 'i: 'a>(
    n: Node<'a, 'i>,
    nome: &'a str,
) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == nome)
}

pub(crate) fn filho<'a, 'i>(n: Node<'a, 'i>, nome: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == nome)
}

/// Atributo pelo nome local, qualquer espaço de nomes (`opf:role`, `epub:type`, `role`).
pub(crate) fn atributo<'a>(n: Node<'a, '_>, local: &str) -> Option<&'a str> {
    n.attributes()
        .find(|a| a.name() == local)
        .map(|a| a.value())
}

/// Todo o texto dos descendentes, com espaços colapsados.
pub(crate) fn texto(n: Node<'_, '_>) -> String {
    let mut s = String::new();
    for d in n.descendants().filter(|d| d.is_text()) {
        if let Some(t) = d.text() {
            s.push_str(t);
            s.push(' ');
        }
    }
    util::colapsar(&s)
}

/// Texto do primeiro filho com o nome dado.
pub(crate) fn texto_de(n: Node<'_, '_>, nome: &str) -> Option<String> {
    filho(n, nome).map(texto).filter(|t| !t.is_empty())
}

/// Evidência de um ISBN achado no texto: ficha catalográfica se a página tem a ficha.
pub(crate) fn evidencia_isbn_texto(p: &PaginaTexto, com_ficha: bool, isbn_txt: &str) -> Evidencia {
    let trecho = p
        .texto
        .lines()
        .find(|l| l.to_ascii_lowercase().contains("isbn") || l.contains(isbn_txt))
        .map(|l| util::truncar(l.trim(), 300));
    Evidencia {
        origem: if com_ficha {
            OrigemEvidencia::FichaCatalografica
        } else {
            OrigemEvidencia::Conteudo
        },
        pagina: Some(p.numero),
        trecho,
        detalhe: Some("isbn no texto".to_owned()),
        confianca: if com_ficha { 0.85 } else { 0.7 },
    }
}
