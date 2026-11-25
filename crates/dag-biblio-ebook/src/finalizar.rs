//! Passos comuns depois de ter as páginas: ficha catalográfica e ISBN no texto.

use dag_biblio::{Campo, Evidencia, OrigemEvidencia, ResultadoExtracao, isbn};

use crate::util;
use crate::xml::evidencia_isbn_texto;

/// Registra um ISBN (já normalizado) com a evidência dada, sem repetir a mesma origem/página.
pub(crate) fn registrar_isbn(r: &mut ResultadoExtracao, isbn13: &str, ev: Evidencia) {
    let repetido = r
        .isbns
        .iter()
        .any(|(i, e)| i == isbn13 && e.origem == ev.origem && e.pagina == ev.pagina);
    if !repetido {
        r.isbns.push((isbn13.to_owned(), ev));
    }
}

/// ISBN vindo de um identificador interno (OPF, EXTH, FB2, ComicInfo). `None` se inválido.
pub(crate) fn isbn_de_identificador(r: &mut ResultadoExtracao, bruto: &str, detalhe: &str) -> bool {
    let Some(n) = isbn::normalizar(bruto.trim().trim_start_matches("urn:isbn:")) else {
        return false;
    };
    registrar_isbn(r, &n, util::evidencia(detalhe, 0.9));
    if !r
        .metadados
        .iter()
        .any(|m| m.campo == Campo::Isbn && m.valor.as_deref() == Some(n.as_str()))
    {
        r.metadados.push(util::valor(Campo::Isbn, &n, 0.9, detalhe));
    }
    true
}

/// Procura a ficha catalográfica e ISBNs nas páginas amostradas (`r.paginas`) e em textos
/// extras (trechos além das páginas amostradas, sem número de página).
pub(crate) fn analisar_texto(r: &mut ResultadoExtracao, extras: &[String]) {
    r.ficha_catalografica = util::achar_ficha(&r.paginas);
    let ficha_pag = r.ficha_catalografica.as_ref().map(|(p, _)| *p);
    let mut novos: Vec<(String, Evidencia)> = Vec::new();
    for p in &r.paginas {
        for i in isbn::encontrar(&p.texto) {
            let ev = evidencia_isbn_texto(p, ficha_pag == Some(p.numero), &i);
            novos.push((i, ev));
        }
    }
    for t in extras {
        for i in isbn::encontrar(t) {
            novos.push((
                i,
                Evidencia {
                    origem: OrigemEvidencia::Conteudo,
                    pagina: None,
                    trecho: None,
                    detalhe: Some("isbn no texto fora das páginas amostradas".to_owned()),
                    confianca: 0.6,
                },
            ));
        }
    }
    for (i, ev) in novos {
        registrar_isbn(r, &i, ev);
    }
}
