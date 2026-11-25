//! Metadados nativos (Info do PDF, XMP, meta do DjVu) viram `ValorSugerido` com origem
//! `MetadadoInterno`. Valores que parecem lixo (nome de arquivo, "untitled", scanner) ficam com
//! confiança baixa.

use std::collections::BTreeMap;

use dag_biblio::{Campo, Evidencia, OrigemEvidencia, ValorSugerido};

use crate::texto::normalizar;

/// Confiança de um metadado interno plausível.
pub const CONFIANCA_METADADO: f32 = 0.55;
/// Confiança de um metadado com cara de lixo.
pub const CONFIANCA_LIXO: f32 = 0.1;

/// O valor parece lixo de ferramenta (nome de arquivo, modelo, scanner)?
#[must_use]
pub fn parece_lixo(valor: &str) -> bool {
    let n = normalizar(valor);
    let n = n.trim();
    if n.len() < 2 || n.chars().all(|c| !c.is_alphabetic()) {
        return true;
    }
    const EXATOS: [&str; 16] = [
        "untitled",
        "unknown",
        "document",
        "documento",
        "sem titulo",
        "admin",
        "administrator",
        "user",
        "usuario",
        "owner",
        "scan",
        "scanned",
        "title",
        "author",
        "null",
        "none",
    ];
    const COMECA: [&str; 12] = [
        "microsoft word",
        "microsoft powerpoint",
        "microsoft excel",
        "untitled",
        "novo documento",
        "new document",
        "documento1",
        "img_",
        "img-",
        "scan_",
        "scan0",
        "camscanner",
    ];
    const TERMINA: [&str; 12] = [
        ".doc", ".docx", ".pdf", ".indd", ".qxd", ".tex", ".odt", ".ppt", ".pptx", ".rtf", ".tif",
        ".jpg",
    ];
    const CONTEM: [&str; 9] = [
        "scanner", "xerox", "canon ir", "epson", "hp scan", "ricoh", "konica", "brother", "fujitsu",
    ];
    EXATOS.contains(&n)
        || COMECA.iter().any(|p| n.starts_with(p))
        || TERMINA.iter().any(|s| n.ends_with(s))
        || CONTEM.iter().any(|c| n.contains(c))
}

pub(crate) fn decodificar_entidades(s: &str) -> String {
    let mut saida = String::with_capacity(s.len());
    let mut resto = s;
    while let Some(i) = resto.find('&') {
        saida.push_str(&resto[..i]);
        let depois = &resto[i..];
        if let Some(fim) = depois.find(';').filter(|f| *f <= 10) {
            let ent = &depois[1..fim];
            let c = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => ent
                    .strip_prefix("#x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            if let Some(c) = c {
                saida.push(c);
                resto = &depois[fim + 1..];
                continue;
            }
        }
        saida.push('&');
        resto = &depois[1..];
    }
    saida.push_str(resto);
    saida
}

/// Valores de um elemento XMP (`dc:title`, `dc:creator`...): lista de `<rdf:li>` ou texto simples.
#[must_use]
pub fn xmp_valores(xmp: &str, elemento: &str) -> Vec<String> {
    let abre = format!("<{elemento}");
    let fecha = format!("</{elemento}>");
    let Some(i) = xmp.find(&abre) else {
        return Vec::new();
    };
    let Some(f) = xmp[i..].find(&fecha) else {
        return Vec::new();
    };
    let corpo = &xmp[i..i + f];
    let Some(gt) = corpo.find('>') else {
        return Vec::new();
    };
    let corpo = &corpo[gt + 1..];
    let mut itens = Vec::new();
    let mut resto = corpo;
    while let Some(a) = resto.find("<rdf:li") {
        let Some(g) = resto[a..].find('>') else { break };
        let ini = a + g + 1;
        let Some(z) = resto[ini..].find("</rdf:li>") else {
            break;
        };
        itens.push(decodificar_entidades(resto[ini..ini + z].trim()));
        resto = &resto[ini + z + 9..];
    }
    if itens.is_empty() {
        let simples = decodificar_entidades(corpo.trim());
        if !simples.is_empty() && !simples.contains('<') {
            itens.push(simples);
        }
    }
    itens.retain(|s| !s.is_empty());
    itens
}

/// Campos do bloco de saída do `pdfinfo` (`Chave:   valor`), antes da linha `Pages:`.
#[must_use]
pub fn parse_pdfinfo(saida: &str) -> BTreeMap<String, String> {
    saida
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            let k = k.trim();
            (!k.is_empty() && !k.contains(' ') || k == "Page size" || k == "Metadata Stream")
                .then(|| (k.to_string(), v.trim().to_string()))
        })
        .collect()
}

fn dividir_lista(s: &str, separadores: &[&str]) -> Vec<String> {
    let mut partes = vec![s.to_string()];
    for sep in separadores {
        partes = partes
            .iter()
            .flat_map(|p| p.split(sep).map(str::to_string).collect::<Vec<_>>())
            .collect();
    }
    partes
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

fn sugestao(
    campo: Campo,
    valor: Option<String>,
    lista: Vec<String>,
    lixo: bool,
    detalhe: &str,
    trecho: &str,
) -> ValorSugerido {
    let confianca = if lixo {
        CONFIANCA_LIXO
    } else {
        CONFIANCA_METADADO
    };
    ValorSugerido {
        campo,
        valor,
        lista,
        confianca,
        evidencias: vec![Evidencia {
            origem: OrigemEvidencia::MetadadoInterno,
            pagina: None,
            trecho: Some(trecho.chars().take(300).collect()),
            detalhe: Some(detalhe.into()),
            confianca,
        }],
    }
}

/// Converte os campos nativos (chaves do `Info` e valores XMP) em valores sugeridos.
/// O XMP vence o Info quando os dois existem (o Info costuma ser cópia desatualizada).
#[must_use]
pub fn sugerir_de_metadados(info: &BTreeMap<String, String>, xmp: &str) -> Vec<ValorSugerido> {
    let mut saida = Vec::new();
    let escolhe = |xmp_elem: &str, info_chave: &str| -> (Option<String>, &'static str) {
        if let Some(v) = xmp_valores(xmp, xmp_elem).into_iter().next() {
            return (Some(v), "XMP");
        }
        (
            info.get(info_chave).filter(|v| !v.is_empty()).cloned(),
            "Info",
        )
    };

    if let (Some(t), fonte) = escolhe("dc:title", "Title") {
        saida.push(sugestao(
            Campo::Titulo,
            Some(t.clone()),
            Vec::new(),
            parece_lixo(&t),
            &format!("{fonte}: Title"),
            &t,
        ));
    }
    let autores_xmp = xmp_valores(xmp, "dc:creator");
    let (autores, fonte) = if autores_xmp.is_empty() {
        (
            info.get("Author")
                .map(|a| dividir_lista(a, &[";", " & ", " and "]))
                .unwrap_or_default(),
            "Info",
        )
    } else {
        (autores_xmp, "XMP")
    };
    if !autores.is_empty() {
        let junto = autores.join(" & ");
        let lixo = autores.iter().all(|a| parece_lixo(a));
        saida.push(sugestao(
            Campo::Autores,
            Some(junto.clone()),
            autores,
            lixo,
            &format!("{fonte}: Author"),
            &junto,
        ));
    }
    if let Some(e) = xmp_valores(xmp, "dc:publisher").into_iter().next() {
        saida.push(sugestao(
            Campo::Editora,
            Some(e.clone()),
            Vec::new(),
            parece_lixo(&e),
            "XMP: dc:publisher",
            &e,
        ));
    }
    if let Some(l) = xmp_valores(xmp, "dc:language").into_iter().next() {
        saida.push(sugestao(
            Campo::Idioma,
            Some(l.clone()),
            Vec::new(),
            false,
            "XMP: dc:language",
            &l,
        ));
    }
    if let (Some(d), fonte) = escolhe("dc:description", "Subject")
        && !parece_lixo(&d)
    {
        saida.push(sugestao(
            Campo::Resumo,
            Some(d.clone()),
            Vec::new(),
            false,
            &format!("{fonte}: Subject"),
            &d,
        ));
    }
    let tags = {
        let x = xmp_valores(xmp, "dc:subject");
        if x.is_empty() {
            info.get("Keywords")
                .map(|k| dividir_lista(k, &[",", ";"]))
                .unwrap_or_default()
        } else {
            x
        }
    };
    if !tags.is_empty() {
        let junto = tags.join(", ");
        saida.push(sugestao(
            Campo::Tags,
            Some(junto.clone()),
            tags,
            false,
            "Keywords",
            &junto,
        ));
    }
    if let Some(d) = xmp_valores(xmp, "dc:date").into_iter().next() {
        saida.push(sugestao(
            Campo::Data,
            Some(d.clone()),
            Vec::new(),
            false,
            "XMP: dc:date",
            &d,
        ));
    }
    saida
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn lixo_de_ferramenta_e_reconhecido() {
        for lixo in [
            "Microsoft Word - doc1.docx",
            "untitled",
            "Untitled Document",
            "scan_0001",
            "livro_final.pdf",
            "Xerox WorkCentre 7845",
            "12345",
            "admin",
            "CamScanner 01-02-2020",
        ] {
            assert!(parece_lixo(lixo), "{lixo}");
        }
        for bom in ["Dom Casmurro", "Machado de Assis", "O Senhor dos Anéis"] {
            assert!(!parece_lixo(bom), "{bom}");
        }
    }

    #[test]
    fn xmp_le_listas_e_entidades() {
        let xmp = r#"<rdf:RDF><dc:title><rdf:Alt><rdf:li xml:lang="x-default">Pão &amp; Circo</rdf:li></rdf:Alt></dc:title>
            <dc:creator><rdf:Seq><rdf:li>Ana</rdf:li><rdf:li>Bia</rdf:li></rdf:Seq></dc:creator>
            <dc:publisher>Editora X</dc:publisher></rdf:RDF>"#;
        assert_eq!(xmp_valores(xmp, "dc:title"), vec!["Pão & Circo"]);
        assert_eq!(xmp_valores(xmp, "dc:creator"), vec!["Ana", "Bia"]);
        assert_eq!(xmp_valores(xmp, "dc:publisher"), vec!["Editora X"]);
        assert!(xmp_valores(xmp, "dc:language").is_empty());
    }

    #[test]
    fn sugere_com_confianca_baixa_para_lixo() {
        let mut info = BTreeMap::new();
        info.insert("Title".into(), "Microsoft Word - doc1.docx".into());
        info.insert("Author".into(), "Silva, João; Souza, Maria".into());
        let v = sugerir_de_metadados(&info, "");
        let titulo = v.iter().find(|x| x.campo == Campo::Titulo).unwrap();
        assert!(titulo.confianca < 0.2);
        let autores = v.iter().find(|x| x.campo == Campo::Autores).unwrap();
        assert_eq!(autores.lista, vec!["Silva, João", "Souza, Maria"]);
        assert!((autores.confianca - CONFIANCA_METADADO).abs() < f32::EPSILON);
        assert_eq!(
            autores.evidencias[0].origem,
            OrigemEvidencia::MetadadoInterno
        );
    }
}
