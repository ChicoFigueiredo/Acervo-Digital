//! Texto e conversões puras das páginas da Biblioteca (sem egui): rótulos em português,
//! texto puro vindo dos livros, leitura do progresso do job e pequenas regras de confirmação.
//! Tudo aqui é testável sem janela.

use dag_core::{EstadoFilaInfo, MotivoFila, PapelCopiaInfo, PapelRegistro, ProgressoBiblioteca};

/// Prefixo das mensagens de progresso do job Biblioteca (`ProgressoBiblioteca::texto`).
pub const PREFIXO_PROGRESSO: &str = "biblioteca: ";
/// Prefixo das mensagens de alerta novo emitidas pelo agente durante um job.
pub const PREFIXO_ALERTA: &str = "alerta: ";

/// Texto que veio de um livro (título, trecho, evidência) pronto para exibir como texto puro:
/// remove caracteres de controle e de direção (que poderiam disfarçar ou inverter o que se lê),
/// junta quebras de linha em espaço e corta em `max` caracteres. Nunca é interpretado como
/// marcação: a GUI o desenha sempre em um rótulo simples.
#[must_use]
pub fn texto_puro(s: &str, max: usize) -> String {
    let mut saida = String::with_capacity(s.len().min(max + 4));
    let mut espaco = false;
    let mut n = 0usize;
    for c in s.chars() {
        let invisivel = matches!(
            c,
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
        );
        if invisivel {
            continue;
        }
        let c = if c.is_control() || c.is_whitespace() {
            ' '
        } else {
            c
        };
        if c == ' ' {
            espaco = !saida.is_empty();
            continue;
        }
        if espaco {
            saida.push(' ');
            espaco = false;
            n += 1;
        }
        if n >= max {
            saida.push('…');
            return saida;
        }
        saida.push(c);
        n += 1;
    }
    saida
}

#[must_use]
pub fn rotulo_papel(p: PapelRegistro) -> &'static str {
    match p {
        PapelRegistro::Homologada => "Homologada",
        PapelRegistro::AClassificar => "A Classificar",
    }
}

#[must_use]
pub fn rotulo_estado_fila(e: EstadoFilaInfo) -> &'static str {
    match e {
        EstadoFilaInfo::Pendente => "pendente",
        EstadoFilaInfo::EmAndamento => "em andamento",
        EstadoFilaInfo::Concluido => "concluído",
        EstadoFilaInfo::Falhou => "falhou",
        EstadoFilaInfo::Excecao => "exceção",
    }
}

#[must_use]
pub fn rotulo_motivo(m: MotivoFila) -> &'static str {
    match m {
        MotivoFila::Novo => "novo",
        MotivoFila::Alterado => "alterado",
        MotivoFila::Reprocessar => "reprocessar",
    }
}

/// Onde a cópia está, em palavras (a Cobertura é dita à parte).
#[must_use]
pub fn rotulo_papel_copia(p: PapelCopiaInfo) -> &'static str {
    match p {
        PapelCopiaInfo::EspelhoDeBiblioteca => "espelho de biblioteca",
        PapelCopiaInfo::DownloadOuAClassificar => "download ou A Classificar",
        PapelCopiaInfo::Outro => "outro local",
    }
}

/// Nome legível de um campo bibliográfico (`titulo`, `autores`...).
#[must_use]
pub fn rotulo_campo(campo: &str) -> String {
    match campo {
        "titulo" => "Título".into(),
        "subtitulo" => "Subtítulo".into(),
        "autores" => "Autores".into(),
        "editora" => "Editora".into(),
        "edicao" => "Edição".into(),
        "data" => "Data".into(),
        "idioma" => "Idioma".into(),
        "isbn" => "ISBN".into(),
        "doi" => "DOI".into(),
        "serie" => "Série".into(),
        "indice_serie" => "Índice na série".into(),
        "volume" => "Volume".into(),
        "resumo" => "Resumo".into(),
        "tags" => "Tags".into(),
        "estante" => "Estante".into(),
        "capa" => "Capa".into(),
        "paginas" => "Páginas".into(),
        outro => texto_puro(outro, 40),
    }
}

/// Nome legível do tipo de um alerta.
#[must_use]
pub fn rotulo_tipo_alerta(tipo: &str) -> String {
    match tipo {
        "ja_tem_este_livro" => "Você já tem este livro".into(),
        "duplicata_exata" => "Duplicata exata".into(),
        "mesma_edicao_outro_formato" => "Mesma edição em outro formato".into(),
        "possivel_mesma_obra" => "Possivelmente a mesma obra".into(),
        "extracao_falhou" => "Extração falhou".into(),
        "biblioteca_alterada" => "Biblioteca alterada".into(),
        "resumo_diario" => "Resumo diário".into(),
        outro => texto_puro(outro, 40),
    }
}

/// Faixa de confiança em palavras e marcador (nunca só cor): `alta`, `media`, `baixa`. Sem a
/// faixa da resposta, calcula pelos mesmos limites do agente (0,90 e 0,60).
#[must_use]
pub fn faixa_de_confianca(faixa: Option<&str>, confianca: Option<f32>) -> &'static str {
    match faixa {
        Some("alta") => "alta",
        Some("media" | "média") => "média",
        Some("baixa") => "baixa",
        _ => match confianca {
            Some(c) if c >= 0.90 => "alta",
            Some(c) if c >= 0.60 => "média",
            Some(_) => "baixa",
            None => "—",
        },
    }
}

/// `87%` para uma confiança de 0 a 1; `—` sem valor.
#[must_use]
pub fn percentual_de_confianca(c: Option<f32>) -> String {
    c.map_or_else(
        || "—".into(),
        |c| format!("{:.0}%", (c.clamp(0.0, 1.0) * 100.0).round()),
    )
}

/// Hexadecimal minúsculo.
#[must_use]
pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

/// Extensão do caminho em maiúsculas (`EPUB`), ou `—`.
#[must_use]
pub fn formato_do_caminho(caminho: &str) -> String {
    let nome = caminho.rsplit('/').next().unwrap_or(caminho);
    match nome.rsplit_once('.') {
        Some((base, ext)) if !base.is_empty() && !ext.is_empty() && ext.len() <= 8 => {
            ext.to_uppercase()
        }
        _ => "—".into(),
    }
}

/// Nome do arquivo (último componente do caminho).
#[must_use]
pub fn nome_do_arquivo(caminho: &str) -> &str {
    caminho.rsplit('/').next().unwrap_or(caminho)
}

/// Raízes digitadas, uma por linha (ou separadas por `;`): sem brancos, sem barra final, sem
/// repetição e só caminhos absolutos.
#[must_use]
pub fn raizes_de_texto(texto: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for linha in texto.split(['\n', ';']) {
        let l = linha.trim();
        if !l.starts_with('/') {
            continue;
        }
        let l = if l.len() > 1 {
            l.trim_end_matches('/')
        } else {
            l
        };
        let l = if l.is_empty() { "/" } else { l };
        if !v.iter().any(|x| x == l) {
            v.push(l.to_string());
        }
    }
    v
}

/// Lista editável (`a | b`) para os valores separados por `|` que o agente espera.
#[must_use]
pub fn lista_para_valor(partes: &[String]) -> String {
    partes.join("|")
}

/// Valor de uma sugestão para mostrar: o texto, ou a lista separada por ` & `.
#[must_use]
pub fn texto_de_sugestao(valor: Option<&str>, lista: &[String]) -> Option<String> {
    match valor {
        Some(v) if !v.trim().is_empty() => Some(v.to_string()),
        _ if !lista.is_empty() => Some(lista.join(" & ")),
        _ => None,
    }
}

/// O que se digita para corrigir um campo: a sugestão atual, com as listas separadas por `|`.
#[must_use]
pub fn texto_inicial_de_correcao(
    valor: Option<&str>,
    lista: &[String],
    extraido: Option<&str>,
) -> String {
    if !lista.is_empty() {
        return lista_para_valor(lista);
    }
    valor
        .filter(|v| !v.trim().is_empty())
        .or(extraido)
        .unwrap_or_default()
        .to_string()
}

/// Dupla confirmação da homologação: o título digitado precisa ser o do livro (sem diferenciar
/// maiúsculas nem espaços repetidos) e é preciso dizer quem aprova.
#[must_use]
pub fn confirma_homologacao(titulo: &str, digitado: &str, quem: &str) -> bool {
    let norma = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let t = norma(titulo);
    !t.is_empty() && norma(digitado) == t && !quem.trim().is_empty()
}

fn numero_no_inicio(s: &str) -> Option<u64> {
    let fim = s
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map_or(s.len(), |(i, _)| i);
    s.get(..fim)?.parse().ok()
}

/// Lê de volta o texto de [`ProgressoBiblioteca::texto`] (a mensagem que o agente envia no
/// progresso do job). `None` se a mensagem não é de progresso.
#[must_use]
pub fn parse_progresso(texto: &str) -> Option<ProgressoBiblioteca> {
    let resto = texto.strip_prefix(PREFIXO_PROGRESSO)?;
    let (corpo, item) = match resto.split_once(" — ") {
        Some((c, i)) => (c, Some(i.to_string())),
        None => (resto, None),
    };
    let partes: Vec<&str> = corpo.split(", ").collect();
    if partes.len() < 7 {
        return None;
    }
    let (feitos, total) = partes[0].split_once('/')?;
    let ocr = partes[2]
        .split_once('(')
        .and_then(|(_, r)| numero_no_inicio(r))
        .unwrap_or(0);
    Some(ProgressoBiblioteca {
        itens_feitos: numero_no_inicio(feitos)?,
        itens_total: numero_no_inicio(total)?,
        bytes: numero_no_inicio(partes[1])?,
        paginas: numero_no_inicio(partes[2])?,
        paginas_ocr: ocr,
        falhas: numero_no_inicio(partes[3])?,
        excecoes: numero_no_inicio(partes[4])?,
        duplicatas_exatas: numero_no_inicio(partes[5])?,
        espaco_potencial_bytes: numero_no_inicio(partes[6])?,
        item_atual: item,
    })
}

/// Fração 0..=1 do progresso, se o total é conhecido.
#[must_use]
pub fn fracao_do_progresso(p: &ProgressoBiblioteca) -> Option<f32> {
    (p.itens_total > 0).then(|| (p.itens_feitos as f32 / p.itens_total as f32).clamp(0.0, 1.0))
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn texto_puro_tira_controle_e_direcao_e_corta() {
        assert_eq!(texto_puro("a\u{202E}b\nc\t d", 50), "ab c d");
        assert_eq!(texto_puro("  \u{0007}x  ", 50), "x");
        let longo = "x".repeat(300);
        let t = texto_puro(&longo, 10);
        assert_eq!(t.chars().count(), 11);
        assert!(t.ends_with('…'));
        // Marcação continua sendo só texto.
        assert_eq!(texto_puro("<b>**oi**</b>", 50), "<b>**oi**</b>");
    }

    #[test]
    fn faixas_de_confianca_seguem_os_limites_do_agente() {
        assert_eq!(faixa_de_confianca(Some("alta"), None), "alta");
        assert_eq!(faixa_de_confianca(Some("media"), Some(0.1)), "média");
        assert_eq!(faixa_de_confianca(None, Some(0.9)), "alta");
        assert_eq!(faixa_de_confianca(None, Some(0.6)), "média");
        assert_eq!(faixa_de_confianca(None, Some(0.59)), "baixa");
        assert_eq!(faixa_de_confianca(None, None), "—");
        assert_eq!(percentual_de_confianca(Some(0.874)), "87%");
        assert_eq!(percentual_de_confianca(None), "—");
    }

    #[test]
    fn progresso_ida_e_volta() {
        let p = ProgressoBiblioteca {
            itens_feitos: 12,
            itens_total: 340,
            bytes: 5_000,
            paginas: 77,
            paginas_ocr: 9,
            falhas: 1,
            excecoes: 2,
            duplicatas_exatas: 3,
            espaco_potencial_bytes: 123_456,
            item_atual: Some("/livros/a — b.pdf".into()),
        };
        assert_eq!(parse_progresso(&p.texto()), Some(p.clone()));
        let sem_item = ProgressoBiblioteca {
            item_atual: None,
            ..p
        };
        assert_eq!(parse_progresso(&sem_item.texto()), Some(sem_item.clone()));
        assert_eq!(parse_progresso("alerta: você já tem este livro — X"), None);
        assert_eq!(parse_progresso("biblioteca: lixo"), None);
        let f = fracao_do_progresso(&sem_item).unwrap();
        assert!((f - 12.0 / 340.0).abs() < 1e-6);
        assert_eq!(fracao_do_progresso(&ProgressoBiblioteca::default()), None);
    }

    #[test]
    fn raizes_normalizam_e_descartam_relativas() {
        let r = raizes_de_texto("/a/b/\n/a/b\n  /c ;relativo\n\n/");
        assert_eq!(r, ["/a/b", "/c", "/"]);
        assert!(raizes_de_texto("  \n").is_empty());
    }

    #[test]
    fn formato_e_nome_do_caminho() {
        assert_eq!(formato_do_caminho("/x/livro.epub"), "EPUB");
        assert_eq!(formato_do_caminho("/x/.oculto"), "—");
        assert_eq!(formato_do_caminho("/x/semextensao"), "—");
        assert_eq!(nome_do_arquivo("/x/y/z.pdf"), "z.pdf");
    }

    #[test]
    fn homologacao_exige_titulo_e_quem_aprova() {
        assert!(confirma_homologacao(
            "O Livro  Grande",
            "o livro grande",
            "Chico"
        ));
        assert!(!confirma_homologacao("O Livro", "O Livr", "Chico"));
        assert!(!confirma_homologacao("O Livro", "O Livro", "  "));
        assert!(!confirma_homologacao("", "", "Chico"));
    }

    #[test]
    fn listas_e_correcao_inicial() {
        assert_eq!(lista_para_valor(&["A".into(), "B".into()]), "A|B");
        assert_eq!(
            texto_de_sugestao(None, &["A".into(), "B".into()]),
            Some("A & B".into())
        );
        assert_eq!(texto_de_sugestao(Some(" "), &[]), None);
        assert_eq!(
            texto_inicial_de_correcao(None, &["A".into(), "B".into()], None),
            "A|B"
        );
        assert_eq!(texto_inicial_de_correcao(None, &[], Some("ext")), "ext");
    }

    #[test]
    fn rotulos_em_portugues() {
        assert_eq!(rotulo_campo("indice_serie"), "Índice na série");
        assert_eq!(rotulo_campo("x\u{202E}y"), "xy");
        assert_eq!(
            rotulo_tipo_alerta("ja_tem_este_livro"),
            "Você já tem este livro"
        );
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
        assert_eq!(
            rotulo_estado_fila(EstadoFilaInfo::EmAndamento),
            "em andamento"
        );
    }
}
