//! Saída legível (pt-BR) das respostas da Curadoria Bibliográfica (`DadosBiblioteca`).

use dag_core::formato::{bytes_humanos, data_hora_utc, inteiro_pt_br};
use dag_core::{
    AlertaInfo, BibliotecaInfo, CampoLadoALado, ConfigBiblioteca, ContagemFila, DadosBiblioteca,
    GrupoDuplicata, ItemFilaInfo, ItemRevisaoInfo, PapelCopiaInfo, PapelRegistro, PerfilResumo,
    RelatorioEspacoInfo, ResumoBiblioteca, ResumoEnfileiramento, ResumoProcessamento,
    ResumoSincronizacao,
};

use crate::saida::tabela;

fn corta(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn papel(p: PapelRegistro) -> &'static str {
    match p {
        PapelRegistro::Homologada => "Homologada",
        PapelRegistro::AClassificar => "A Classificar",
    }
}

fn quando(s: Option<i64>) -> String {
    s.map_or_else(|| "-".to_string(), data_hora_utc)
}

fn sim_nao(b: bool) -> &'static str {
    if b { "sim" } else { "não" }
}

fn n(v: u64) -> String {
    inteiro_pt_br(v)
}

fn ni(v: i64) -> String {
    inteiro_pt_br(u64::try_from(v).unwrap_or(0))
}

/// Orientação para os erros bibliográficos de código estável.
#[must_use]
pub fn dica_de_erro(codigo: &str) -> Option<&'static str> {
    Some(match codigo {
        "calibre_ocupado" => {
            "O Calibre (ou o calibre-server) está com a biblioteca aberta e trava a escrita. Feche o Calibre (ou pare o servidor), espere alguns segundos e rode o mesmo comando de novo: o que sobrou ficou pendente e nada é duplicado."
        }
        "calibredb_ausente" => {
            "O programa `calibredb` não foi encontrado. Instale o Calibre (por exemplo `sudo apt install calibre`) e confira com `calibredb --version`."
        }
        "aprovacao_humana_exigida" => {
            "A homologação precisa da aprovação de uma pessoa, pela GUI ou por `dag biblioteca homologar --aprovado-por NOME`."
        }
        "aprovacao_invalida" => "Informe quem aprova com --aprovado-por NOME.",
        "biblioteca_nao_registrada" => {
            "Veja as registradas com `dag biblioteca listar` ou registre com `dag biblioteca registrar CAMINHO --papel homologada`."
        }
        "sem_homologada" => {
            "Registre a Biblioteca Homologada: `dag biblioteca registrar /mnt/e/Livros --papel homologada`."
        }
        "sem_a_classificar" => {
            "Crie a A Classificar: `dag biblioteca registrar /mnt/e2/Livros_A_Classificar --papel a-classificar`."
        }
        "cadastro_inexistente" => {
            "Esse cadastro não existe. O número vem do resultado de `dag biblioteca cadastrar FILE_ID`."
        }
        "cadastro_nao_pronto" => {
            "O livro ainda não foi cadastrado na A Classificar: rode `dag biblioteca cadastrar FILE_ID` antes de homologar."
        }
        "ja_existe_na_homologada" => {
            "Esse livro já está na Biblioteca Homologada; não há o que homologar. Veja `dag biblioteca duplicatas`."
        }
        "config_invalida" => {
            "Valor recusado. Veja os nomes e valores atuais com `dag biblioteca config`; o endereço da IA precisa ser local (127.0.0.1)."
        }
        "papel_invalido" => "Use --papel homologada ou --papel a-classificar.",
        _ => return None,
    })
}

pub fn bibliotecas(itens: &[BibliotecaInfo]) -> String {
    if itens.is_empty() {
        return "Nenhuma biblioteca registrada. Registre com `dag biblioteca registrar CAMINHO --papel homologada`.\n".into();
    }
    let linhas: Vec<Vec<String>> = itens
        .iter()
        .map(|b| {
            vec![
                b.id.to_string(),
                papel(b.papel).to_string(),
                b.nome.clone(),
                ni(b.livros),
                ni(b.formatos),
                b.estado.clone(),
                quando(b.ultima_sincronizacao),
                b.caminho.clone(),
            ]
        })
        .collect();
    tabela(
        &[
            "ID",
            "PAPEL",
            "NOME",
            "LIVROS",
            "FORMATOS",
            "ESTADO",
            "SINCRONIZADA",
            "CAMINHO",
        ],
        &[true, false, false, true, true, false, false, false],
        &linhas,
    )
}

fn registrada(b: &BibliotecaInfo, criada: bool, avisos: &[String]) -> String {
    let mut s = format!(
        "Biblioteca {} registrada: #{} \"{}\" em {}\n",
        papel(b.papel),
        b.id,
        b.nome,
        b.caminho
    );
    s.push_str(&format!(
        "  estado: {}; Calibre {}; {} livro(s), {} formato(s)\n",
        b.estado,
        b.versao_calibre.as_deref().unwrap_or("-"),
        ni(b.livros),
        ni(b.formatos)
    ));
    if criada {
        s.push_str("  A pasta não era uma biblioteca: o DAG a criou agora (via calibredb).\n");
    }
    if b.papel == PapelRegistro::Homologada {
        s.push_str("  Lida só em modo leitura: nada foi criado nem alterado nela.\n");
        if b.livros == 0 {
            s.push_str("  As contagens aparecem depois da primeira sincronização.\n");
        }
    }
    for a in avisos {
        s.push_str(&format!("  aviso: {a}\n"));
    }
    if b.papel == PapelRegistro::Homologada {
        s.push_str("Próximo passo: `dag biblioteca sincronizar ");
        s.push_str(&b.id.to_string());
        s.push_str("` (somente leitura).\n");
    }
    s
}

fn perfil(b: &BibliotecaInfo, p: &PerfilResumo, campos: &[dag_core::CampoInfo]) -> String {
    let mut s = format!(
        "Perfil da biblioteca {} #{} \"{}\" ({})\n",
        papel(b.papel),
        b.id,
        b.nome,
        b.caminho
    );
    s.push_str(&format!(
        "  esquema do metadata.db: {}; Calibre {}\n",
        p.versao_esquema.map_or("-".into(), |v| v.to_string()),
        b.versao_calibre.as_deref().unwrap_or("-")
    ));
    s.push_str(&format!(
        "  {} livro(s), {} autor(es), {} tag(s), {} série(s), {} editora(s), {} arquivo(s) de formato\n",
        n(p.total_livros),
        n(p.total_autores),
        n(p.total_tags),
        n(p.total_series),
        n(p.total_editoras),
        n(p.total_arquivos_formato)
    ));
    if !p.formatos.is_empty() {
        let f: Vec<String> = p
            .formatos
            .iter()
            .map(|(f, c)| format!("{f} {}", n(*c)))
            .collect();
        s.push_str(&format!("  formatos: {}\n", f.join(", ")));
    }
    s.push_str("\nColunas (campos do Calibre):\n");
    if campos.is_empty() {
        s.push_str("  (nenhuma coluna personalizada)\n");
    } else {
        let linhas: Vec<Vec<String>> = campos
            .iter()
            .map(|c| {
                vec![
                    c.rotulo.clone(),
                    c.nome.clone(),
                    c.datatype.clone(),
                    sim_nao(c.multiplo).into(),
                    if c.clonavel {
                        "sim".into()
                    } else {
                        format!(
                            "não: {}",
                            c.motivo.as_deref().unwrap_or("sem motivo informado")
                        )
                    },
                ]
            })
            .collect();
        s.push_str(&tabela(
            &["RÓTULO", "NOME", "TIPO", "MÚLTIPLO", "CLONÁVEL"],
            &[false; 5],
            &linhas,
        ));
    }
    s.push_str("\nEstantes candidatas (a coluna que organiza os livros):\n");
    if p.candidatos_estante.is_empty() {
        s.push_str("  nenhuma encontrada\n");
    } else {
        for (nome, pontos, razoes) in &p.candidatos_estante {
            s.push_str(&format!(
                "  {nome}  ({pontos} ponto(s)): {}\n",
                razoes.join("; ")
            ));
        }
    }
    s.push_str("\nTags mais usadas:\n");
    if p.tags_principais.is_empty() {
        s.push_str("  nenhuma\n");
    } else {
        let t: Vec<String> = p
            .tags_principais
            .iter()
            .take(30)
            .map(|(t, c)| format!("{t} ({})", n(*c)))
            .collect();
        s.push_str(&format!("  {}\n", t.join(", ")));
    }
    if let Some(controle) = p
        .perfil_bruto
        .get("tags_controle")
        .and_then(|v| v.as_array())
    {
        let t: Vec<&str> = controle.iter().filter_map(|v| v.as_str()).collect();
        if !t.is_empty() {
            s.push_str(&format!("Tags de controle: {}\n", t.join(", ")));
        }
    }
    let clonaveis = campos.iter().filter(|c| c.clonavel).count();
    s.push_str(&format!(
        "\nCompatibilidade: {clonaveis} de {} coluna(s) clonáveis para a A Classificar.\n",
        campos.len()
    ));
    if p.nao_clonaveis.is_empty() {
        s.push_str("  Nada deixa de ser clonado.\n");
    } else {
        s.push_str("  Não são clonados (nada se perde em silêncio):\n");
        for x in &p.nao_clonaveis {
            s.push_str(&format!("   - {x}\n"));
        }
    }
    s
}

fn sincronizada(r: &ResumoSincronizacao) -> String {
    let mut s = format!(
        "Sincronização da biblioteca #{}: {} livro(s) ({} removido(s) do espelho), {} formato(s) em {} ms\n",
        r.biblioteca,
        n(r.livros),
        n(r.removidos),
        n(r.formatos),
        n(r.duracao_ms)
    );
    s.push_str(&format!(
        "  ligados ao Catálogo: {} de {}; hashes importados do Catálogo: {}; calculados agora: {}\n",
        n(r.formatos_ligados_ao_catalogo),
        n(r.formatos),
        n(r.hashes_importados),
        n(r.hashes_calculados)
    ));
    if r.formatos_sem_arquivo_no_catalogo > 0 {
        s.push_str(&format!(
            "  {} formato(s) ainda sem arquivo no Catálogo (rode um scan da pasta da biblioteca)\n",
            n(r.formatos_sem_arquivo_no_catalogo)
        ));
    }
    if r.formatos_sem_hash > 0 {
        s.push_str(&format!(
            "  {} formato(s) sem hash (as duplicatas só aparecem depois do Full Scan)\n",
            n(r.formatos_sem_hash)
        ));
    }
    let cmp = match r.comparacao_metadata_db.as_str() {
        "primeiro" => "primeira leitura (não há como comparar com antes)",
        "igual" => "igual à da sincronização anterior",
        "alterado" => {
            "mudou desde a sincronização anterior (você ou o Calibre alteraram a biblioteca; o DAG não)"
        }
        "incompativel" => "esquema incompatível com o esperado",
        o => o,
    };
    s.push_str(&format!("  metadata.db: {cmp}\n"));
    if r.biblioteca_intacta {
        s.push_str("PROVA DE NÃO ALTERAÇÃO: OK. O instantâneo da biblioteca antes e depois da leitura é idêntico; o DAG só leu (mode=ro).\n");
    } else {
        s.push_str("ATENÇÃO: o instantâneo da biblioteca mudou DURANTE a leitura. O DAG só lê (mode=ro); provavelmente o Calibre estava gravando. Rode de novo com o Calibre parado.\n");
        for d in &r.diferencas {
            s.push_str(&format!("   - {d}\n"));
        }
    }
    s
}

fn onde_copia(c: &dag_core::CopiaDuplicata) -> String {
    format!("{}: {}", c.volume, c.caminho)
}

fn papel_copia(p: PapelCopiaInfo) -> &'static str {
    match p {
        PapelCopiaInfo::EspelhoDeBiblioteca => "espelho de biblioteca",
        PapelCopiaInfo::DownloadOuAClassificar => "download/a classificar",
        PapelCopiaInfo::Outro => "outra pasta",
    }
}

fn duplicatas(
    grupos: &[GrupoDuplicata],
    total: u64,
    sugeridos: u64,
    alertas: u64,
    proximo: Option<&str>,
) -> String {
    if grupos.is_empty() {
        return "Nenhuma duplicata exata de livro da Homologada encontrada (fora dela).\n".into();
    }
    let mut s =
        String::from("VOCÊ JÁ TEM ESTES LIVROS: cópias exatas fora da Biblioteca Homologada\n\n");
    let linhas: Vec<Vec<String>> = grupos
        .iter()
        .map(|g| {
            let formatos: Vec<&str> = g
                .formatos_homologados
                .iter()
                .map(|f| f.formato.as_str())
                .collect();
            let volumes: Vec<&str> = {
                let mut v: Vec<&str> = g.copias.iter().map(|c| c.volume.as_str()).collect();
                v.sort_unstable();
                v.dedup();
                v
            };
            let cobertas = g.copias.iter().filter(|c| c.coberta).count();
            let sugerir = g.copias.iter().filter(|c| c.decisao.sugerir).count();
            vec![
                corta(&g.livro.titulo, 40),
                corta(&g.livro.autores.join(" & "), 24),
                formatos.join(","),
                bytes_humanos(g.bytes_totais),
                g.copias.len().to_string(),
                corta(&volumes.join(","), 20),
                format!("{cobertas}/{}", g.copias.len()),
                format!("{sugerir} sugerir / {} bloqueado", g.copias.len() - sugerir),
            ]
        })
        .collect();
    s.push_str(&tabela(
        &[
            "TÍTULO",
            "AUTOR",
            "FORMATO",
            "BYTES",
            "CÓPIAS",
            "ONDE",
            "COBERTURA",
            "DECISÃO SUGERIDA",
        ],
        &[false, false, false, true, true, false, true, false],
        &linhas,
    ));
    s.push_str("\nDetalhe das cópias:\n");
    for g in grupos {
        s.push_str(&format!(
            "\n* {} — você já tem este livro em {}\n",
            g.livro.titulo, g.livro.caminho
        ));
        for c in &g.copias {
            let decisao = if c.decisao.sugerir {
                "sugerir"
            } else {
                "bloqueado"
            };
            s.push_str(&format!(
                "    arquivo {} ({}, {}) {}; Cobertura {}; {}\n",
                c.file_id,
                c.formato,
                bytes_humanos(c.tamanho),
                papel_copia(c.papel),
                if c.coberta { "sim" } else { "NÃO" },
                decisao
            ));
            s.push_str(&format!("      {}\n", onde_copia(c)));
            if !c.decisao.razoes.is_empty() {
                s.push_str(&format!("      razões: {}\n", c.decisao.razoes.join("; ")));
            }
            if c.decisao.sugerir && !c.decisao.passos_obrigatorios.is_empty() {
                s.push_str(&format!(
                    "      antes de mover: {}\n",
                    c.decisao.passos_obrigatorios.join("; ")
                ));
            }
        }
    }
    s.push_str(&format!(
        "\nTotal desta página: {} em {} livro(s); {} só das cópias que o DAG sugeriria mover. É uma estimativa: nada foi movido nem apagado.\n",
        bytes_humanos(total),
        grupos.len(),
        bytes_humanos(sugeridos)
    ));
    s.push_str("Para liberar espaço use o fluxo da Lixeira (`dag lixeira mover`), com dupla confirmação.\n");
    if alertas > 0 {
        s.push_str(&format!(
            "{alertas} alerta(s) novo(s): `dag biblioteca alertas`.\n"
        ));
    }
    if let Some(c) = proximo {
        s.push_str(&format!(
            "Próxima página: dag biblioteca duplicatas --cursor {c}\n"
        ));
    }
    s
}

fn espaco(r: &RelatorioEspacoInfo) -> String {
    if r.volumes.is_empty() {
        return format!(
            "Nenhuma duplicata de livro com espaço a medir.\n{}\n",
            r.observacao
        );
    }
    let linhas: Vec<Vec<String>> = r
        .volumes
        .iter()
        .map(|v| {
            vec![
                v.volume_id.to_string(),
                n(v.caminhos),
                n(v.arquivos_fisicos),
                bytes_humanos(v.bytes_logicos),
                bytes_humanos(v.bytes_alocados),
                bytes_humanos(v.estimativa_recuperavel),
                bytes_humanos(v.economia_incerta),
                bytes_humanos(v.efetivamente_recuperado),
            ]
        })
        .collect();
    let mut s = String::from("Espaço das duplicatas de livros, por Volume:\n");
    s.push_str(&tabela(
        &[
            "VOLUME",
            "CAMINHOS",
            "ARQ. FÍSICOS",
            "LÓGICO",
            "ALOCADO",
            "ESTIMADO",
            "INCERTO",
            "RECUPERADO",
        ],
        &[true; 8],
        &linhas,
    ));
    let ignorados: u64 = r
        .volumes
        .iter()
        .map(|v| v.caminhos_hardlink_ignorados)
        .sum();
    if ignorados > 0 {
        s.push_str(&format!(
            "({} caminho(s) são hard links do mesmo arquivo físico e não contam como espaço.)\n",
            n(ignorados)
        ));
    }
    if !r.grupos.is_empty() {
        s.push_str(&format!(
            "\n{} grupo(s) de cópias idênticas; os maiores:\n",
            r.grupos.len()
        ));
        let mut gs: Vec<_> = r.grupos.iter().collect();
        gs.sort_by_key(|g| std::cmp::Reverse(g.bytes_estimados + g.bytes_incertos));
        let linhas: Vec<Vec<String>> = gs
            .iter()
            .take(10)
            .map(|g| {
                vec![
                    g.hash.chars().take(12).collect(),
                    bytes_humanos(g.tamanho_logico),
                    g.redundantes.len().to_string(),
                    bytes_humanos(g.bytes_estimados),
                    bytes_humanos(g.bytes_incertos),
                ]
            })
            .collect();
        s.push_str(&tabela(
            &["HASH", "TAMANHO", "REDUNDANTES", "ESTIMADO", "INCERTO"],
            &[false, true, true, true, true],
            &linhas,
        ));
    }
    s.push_str(&format!("\n{}\n", r.observacao));
    s
}

fn enfileirada(r: &ResumoEnfileiramento) -> String {
    format!(
        "Fila: {} examinado(s), {} enfileirado(s), {} já estavam na fila, {} ignorado(s){}.\nPróximo passo: `dag biblioteca processar` (acrescente --ia para consultar a IA local).\n",
        n(r.examinados),
        n(r.enfileirados),
        n(r.ja_na_fila),
        n(r.ignorados),
        if r.interrompido {
            "; INTERROMPIDO (rode de novo para continuar)"
        } else {
            ""
        }
    )
}

fn contagem(c: &ContagemFila) -> String {
    format!(
        "pendentes {}, em andamento {}, concluídos {}, falhos {}, exceções {}",
        ni(c.pendentes),
        ni(c.em_andamento),
        ni(c.concluidos),
        ni(c.falhos),
        ni(c.excecoes)
    )
}

fn fila(c: &ContagemFila, itens: &[ItemFilaInfo], proximo: Option<&str>) -> String {
    let mut s = format!("Fila bibliográfica: {}\n", contagem(c));
    if !itens.is_empty() {
        let linhas: Vec<Vec<String>> = itens
            .iter()
            .map(|i| {
                vec![
                    i.id.to_string(),
                    i.file_id.to_string(),
                    format!("{:?}", i.estado).to_lowercase(),
                    i.etapa.clone().unwrap_or_else(|| "-".into()),
                    format!("{:?}", i.motivo).to_lowercase(),
                    i.prioridade.to_string(),
                    i.tentativas.to_string(),
                    corta(
                        &i.erro
                            .clone()
                            .or_else(|| i.caminho.clone())
                            .unwrap_or_else(|| "-".into()),
                        70,
                    ),
                ]
            })
            .collect();
        s.push_str(&tabela(
            &[
                "ID",
                "ARQUIVO",
                "ESTADO",
                "ETAPA",
                "MOTIVO",
                "PRIO",
                "TENT.",
                "CAMINHO / ERRO",
            ],
            &[true, true, false, false, false, true, true, false],
            &linhas,
        ));
    }
    if let Some(p) = proximo {
        s.push_str(&format!(
            "Próxima página: dag biblioteca fila --cursor {p}\n"
        ));
    }
    s
}

fn processada(r: &ResumoProcessamento) -> String {
    let mut s = format!(
        "Processamento {}: {} item(ns), {} duplicata(s) exata(s) (sem extração), {} extraído(s), {} reaproveitado(s)\n",
        if r.interrompido {
            "PAUSADO"
        } else {
            "concluído"
        },
        n(r.processados),
        n(r.duplicatas_exatas),
        n(r.extraidos),
        n(r.reaproveitados)
    );
    s.push_str(&format!(
        "  {} para revisar, {} exceção(ões), {} falha(s), {} consulta(s) à IA\n",
        n(r.revisao_pendente),
        n(r.excecoes),
        n(r.falhas),
        n(r.consultas_ia)
    ));
    s.push_str(&format!(
        "  {} lido(s), {} página(s) ({} com OCR); espaço potencial das duplicatas: {}\n",
        bytes_humanos(r.bytes),
        n(r.paginas),
        n(r.paginas_ocr),
        bytes_humanos(r.espaco_potencial_bytes)
    ));
    if r.orfaos_retomados > 0 {
        s.push_str(&format!(
            "  {} item(ns) de uma execução interrompida foram retomados\n",
            n(r.orfaos_retomados)
        ));
    }
    if r.interrompido || r.pendentes_restantes > 0 {
        s.push_str(&format!(
            "  restam {} na fila: rode `dag biblioteca processar` de novo para continuar de onde parou\n",
            ni(r.pendentes_restantes)
        ));
    }
    if r.revisao_pendente > 0 {
        s.push_str("Próximo passo: `dag biblioteca revisar`.\n");
    }
    s
}

fn faixa(c: &CampoLadoALado) -> String {
    match (c.confianca, c.faixa.as_deref()) {
        (Some(p), Some(f)) => format!("{} {:.2}", f.to_uppercase(), p).replace('.', ","),
        (Some(p), None) => format!("{p:.2}").replace('.', ","),
        _ => "-".into(),
    }
}

fn sugestao_do_campo(c: &CampoLadoALado) -> String {
    let v = c
        .sugerido
        .clone()
        .or_else(|| (!c.sugerido_lista.is_empty()).then(|| c.sugerido_lista.join(" & ")))
        .unwrap_or_else(|| "(desconhecido)".into());
    format!("{} [{}]", corta(&v, 38), faixa(c))
}

fn item_revisao(i: &ItemRevisaoInfo) -> String {
    let mut s = format!(
        "\nArquivo {} — {} {} — {} sugestão(ões) pendente(s)\n  {}\n",
        i.file_id,
        i.formato.as_deref().unwrap_or("?"),
        i.tamanho.map(bytes_humanos).unwrap_or_default(),
        i.pendentes,
        i.caminho.as_deref().unwrap_or("(sem caminho)")
    );
    if let (Some(o), Some(e)) = (i.obra_id, i.edicao_id) {
        s.push_str(&format!("  Obra {o} > Edição {e}\n"));
    }
    for a in &i.avisos {
        s.push_str(&format!("  aviso: {a}\n"));
    }
    let linhas: Vec<Vec<String>> = i
        .campos
        .iter()
        .map(|c| {
            vec![
                format!("{}{}", c.campo, if c.precisa_revisao { " *" } else { "" }),
                corta(c.extraido.as_deref().unwrap_or("-"), 34),
                sugestao_do_campo(c),
                corta(c.homologado.as_deref().unwrap_or("-"), 34),
                c.estado.clone(),
            ]
        })
        .collect();
    s.push_str(&tabela(
        &[
            "CAMPO",
            "EXTRAÍDO",
            "SUGESTÃO [CONFIANÇA]",
            "HOMOLOGADO PARECIDO",
            "ESTADO",
        ],
        &[false; 5],
        &linhas,
    ));
    for c in &i.campos {
        if c.evidencias.is_empty() && c.motivos.is_empty() {
            continue;
        }
        let origem = [c.origem.as_deref(), c.modelo.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" / ");
        s.push_str(&format!("  {} ({origem}):\n", c.campo));
        for e in c.evidencias.iter().take(3) {
            s.push_str(&format!("    evidência: {}\n", corta(e, 160)));
        }
        for m in &c.motivos {
            s.push_str(&format!("    revisar porque: {m}\n"));
        }
    }
    for p in &i.homologados_parecidos {
        s.push_str(&format!(
            "  Já homologado parecido: \"{}\" ({}) — {}{}\n",
            p.livro.titulo,
            p.livro.autores.join(" & "),
            p.relacao.as_deref().unwrap_or("parecido"),
            if p.motivo.is_empty() {
                String::new()
            } else {
                format!(" ({})", corta(&p.motivo, 60))
            }
        ));
    }
    s
}

fn revisao(itens: &[ItemRevisaoInfo], total: i64, proximo: Option<&str>) -> String {
    if itens.is_empty() {
        return "Nada para revisar.\n".into();
    }
    let mut s = format!(
        "{} item(ns) nesta página; {} sugestão(ões) pendente(s) no total. (* = confiança baixa: confira)\n",
        itens.len(),
        ni(total)
    );
    for i in itens {
        s.push_str(&item_revisao(i));
    }
    s.push_str("\nDecida com `dag biblioteca decidir --alvo arquivo:ID --campo CAMPO --aceitar|--corrigir VALOR|--rejeitar` ou em lote com --de-arquivo.\n");
    if let Some(p) = proximo {
        s.push_str(&format!(
            "Próxima página: dag biblioteca revisar --cursor {p}\n"
        ));
    }
    s
}

fn alerta(a: &AlertaInfo) -> Vec<String> {
    let destaque = a.tipo == "ja_tem_este_livro";
    let detalhe = match &a.dados {
        serde_json::Value::Null => String::new(),
        d => d
            .get("titulo")
            .or_else(|| d.get("caminho"))
            .and_then(|v| v.as_str())
            .map_or_else(|| corta(&d.to_string(), 60), |t| corta(t, 60)),
    };
    vec![
        a.id.to_string(),
        if destaque {
            "VOCÊ JÁ TEM ESTE LIVRO".into()
        } else {
            a.tipo.clone()
        },
        a.file_id.map_or("-".into(), |f| f.to_string()),
        data_hora_utc(a.criado_em),
        if a.visto_em.is_some() {
            "visto".into()
        } else {
            "novo".into()
        },
        detalhe,
    ]
}

fn alertas(itens: &[AlertaInfo], nao_vistos: i64, proximo: Option<&str>) -> String {
    let mut s = format!("{} alerta(s) não visto(s).\n", ni(nao_vistos));
    if !itens.is_empty() {
        let linhas: Vec<Vec<String>> = itens.iter().map(alerta).collect();
        s.push_str(&tabela(
            &[
                "ID",
                "TIPO",
                "ARQUIVO",
                "QUANDO (UTC)",
                "SITUAÇÃO",
                "DETALHE",
            ],
            &[true, false, true, false, false, false],
            &linhas,
        ));
        s.push_str("Marque como vistos com `dag biblioteca visto ID...`.\n");
    }
    if let Some(p) = proximo {
        s.push_str(&format!(
            "Próxima página: dag biblioteca alertas --cursor {p}\n"
        ));
    }
    s
}

fn resumo(r: &ResumoBiblioteca) -> String {
    let mut s = format!(
        "Novidades da biblioteca de {} a {} (UTC)\n",
        data_hora_utc(r.de),
        data_hora_utc(r.ate)
    );
    s.push_str(&format!(
        "  livros novos na fila: {}\n  fila: {}\n  revisões pendentes: {}\n  candidatos a duplicidade abertos: {}\n  espaço recuperável (duplicatas exatas): {}\n  alertas não vistos: {}\n",
        ni(r.livros_novos_na_fila),
        contagem(&r.fila),
        ni(r.revisoes_pendentes),
        ni(r.candidatos_duplicidade_abertos),
        bytes_humanos(r.bytes_recuperaveis_exatos),
        ni(r.alertas_nao_vistos)
    ));
    if !r.alertas_por_tipo.is_empty() {
        s.push_str("  alertas por tipo (total / não vistos):\n");
        for (t, total, nv) in &r.alertas_por_tipo {
            s.push_str(&format!("    {t}: {} / {}\n", ni(*total), ni(*nv)));
        }
    }
    s
}

fn achatar(prefixo: &str, v: &serde_json::Value, saida: &mut Vec<(String, String)>) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, x) in m {
                let p = if prefixo.is_empty() {
                    k.clone()
                } else {
                    format!("{prefixo}.{k}")
                };
                achatar(&p, x, saida);
            }
        }
        serde_json::Value::String(t) => saida.push((prefixo.into(), t.clone())),
        outro => saida.push((prefixo.into(), outro.to_string())),
    }
}

fn config(c: &ConfigBiblioteca) -> String {
    let mut pares = Vec::new();
    achatar("", &serde_json::to_value(c).unwrap_or_default(), &mut pares);
    let mut s = String::from("Configuração da curadoria bibliográfica:\n");
    for (k, v) in pares {
        s.push_str(&format!("  {k} = {v}\n"));
    }
    s.push_str("Altere com `dag biblioteca config definir CHAVE=VALOR` (ex.: ia.ativa=true).\n");
    s
}

/// Texto de uma resposta bibliográfica.
#[must_use]
pub fn texto(d: &DadosBiblioteca) -> String {
    match d {
        DadosBiblioteca::Registrada {
            biblioteca,
            criada,
            avisos,
        } => registrada(biblioteca, *criada, avisos),
        DadosBiblioteca::Bibliotecas { itens } => bibliotecas(itens),
        DadosBiblioteca::Perfil {
            biblioteca,
            perfil: p,
            campos,
        } => perfil(biblioteca, p, campos),
        DadosBiblioteca::Sincronizada { resumo } => sincronizada(resumo),
        DadosBiblioteca::Duplicatas {
            grupos,
            bytes_totais_pagina,
            bytes_sugeridos_pagina,
            alertas_emitidos,
            proximo_cursor,
        } => duplicatas(
            grupos,
            *bytes_totais_pagina,
            *bytes_sugeridos_pagina,
            *alertas_emitidos,
            proximo_cursor.as_deref(),
        ),
        DadosBiblioteca::Enfileirada { resumo } => enfileirada(resumo),
        DadosBiblioteca::Fila {
            contagem,
            itens,
            proximo_cursor,
        } => fila(contagem, itens, proximo_cursor.as_deref()),
        DadosBiblioteca::Processada { resumo } => processada(resumo),
        DadosBiblioteca::Revisao {
            itens,
            total_pendentes,
            proximo_cursor,
        } => revisao(itens, *total_pendentes, proximo_cursor.as_deref()),
        DadosBiblioteca::Decidida {
            lote,
            aplicadas,
            ignoradas,
        } => {
            let mut s = format!(
                "Lote {lote}: {} decisão(ões) aplicada(s); a Homologada não foi tocada.\n",
                n(*aplicadas)
            );
            for i in ignoradas {
                s.push_str(&format!("  ignorada: {i}\n"));
            }
            s.push_str(&format!("Para desfazer: dag biblioteca desfazer {lote}\n"));
            s
        }
        DadosBiblioteca::Desfeita { lote, desfeitas } => {
            format!("Lote {lote}: {} decisão(ões) desfeita(s).\n", n(*desfeitas))
        }
        DadosBiblioteca::Cadastros {
            itens,
            proximo_cursor,
        } => format!(
            "{} cadastro(s) da A Classificar, {} pronto(s) para cadastrar.{}",
            itens.len(),
            itens.iter().filter(|c| c.pronto_para_cadastrar).count(),
            if proximo_cursor.is_some() {
                " Há mais (use --cursor)."
            } else {
                ""
            }
        ),
        DadosBiblioteca::Cadastrada {
            resultados,
            bloqueado,
        } => cadastrada(resultados, bloqueado.as_deref()),
        DadosBiblioteca::Homologacao { homologacao: h } => homologacao(h),
        DadosBiblioteca::Alertas {
            itens,
            nao_vistos_total,
            proximo_cursor,
        } => alertas(itens, *nao_vistos_total, proximo_cursor.as_deref()),
        DadosBiblioteca::Vistos { marcados } => {
            format!("{} alerta(s) marcado(s) como visto(s).\n", n(*marcados))
        }
        DadosBiblioteca::Resumo { resumo: r } => resumo(r),
        DadosBiblioteca::Espaco { relatorio } => espaco(relatorio),
        DadosBiblioteca::Cobertura { resumo: r } => cobertura(r),
        DadosBiblioteca::Config { config: c } => config(c),
    }
}

fn cobertura(r: &dag_core::ResumoCobertura) -> String {
    let mut s = format!(
        "Cobertura da Biblioteca Homologada, verificada em {}:\n",
        quando(Some(r.verificado_em))
    );
    s.push_str(&format!(
        "  Formatos: {} no total, {} com Cobertura, {} sem.\n  Manifesto: {} arquivo(s); metadata.db BLAKE3 {}.\n",
        n(r.formatos_total),
        n(r.formatos_cobertos),
        n(r.formatos_sem_cobertura),
        n(r.arquivos_no_manifesto),
        r.hash_metadata_db
    ));
    s.push_str(&format!(
        "  Amostra relida (BLAKE3, sem copiar): {} de {} pedida(s): {} ok, {} com falha.\n",
        n(u64::from(r.amostra_verificada)),
        n(u64::from(r.amostra_pedida)),
        n(u64::from(r.amostra_ok)),
        n(u64::from(r.amostra_falha))
    ));
    for f in &r.falhas {
        s.push_str(&format!("    falha: {f}\n"));
    }
    for f in r.sem_cobertura.iter().take(10) {
        s.push_str(&format!(
            "    sem Cobertura: {} ({})\n",
            f.caminho, f.motivo
        ));
    }
    if r.sem_cobertura.len() > 10 {
        s.push_str(&format!(
            "    ... e mais {} exemplo(s) no JSON\n",
            r.sem_cobertura.len() - 10
        ));
    }
    s.push_str(&format!(
        "Verificação {} (o descarte exige uma aprovada nos últimos {} dia(s)).\n",
        if r.aprovada {
            "APROVADA"
        } else {
            "NÃO aprovada"
        },
        r.validade_dias
    ));
    s
}

fn cadastrada(r: &[dag_core::ResultadoCadastro], bloqueado: Option<&str>) -> String {
    let mut s = String::new();
    if r.is_empty() {
        s.push_str("Nenhum arquivo cadastrado.\n");
    } else {
        let linhas: Vec<Vec<String>> = r
            .iter()
            .map(|c| {
                vec![
                    c.file_id.to_string(),
                    c.estado.clone(),
                    c.calibre_id.map_or("-".into(), |i| i.to_string()),
                    corta(c.erro.as_deref().unwrap_or("-"), 80),
                ]
            })
            .collect();
        s.push_str(&tabela(
            &["ARQUIVO", "ESTADO", "CALIBRE ID", "ERRO"],
            &[true, false, true, false],
            &linhas,
        ));
    }
    if let Some(b) = bloqueado {
        s.push_str(&format!("\nBLOQUEADO: {b}\n"));
        s.push_str(dica_de_erro("calibre_ocupado").unwrap_or_default());
        s.push('\n');
    }
    if r.iter()
        .any(|c| matches!(c.estado.as_str(), "cadastrado" | "ja_cadastrado"))
    {
        s.push_str("Próximo passo: `dag biblioteca homologar CADASTRO --aprovado-por NOME`.\n");
    }
    s
}

fn homologacao(h: &dag_core::HomologacaoInfo) -> String {
    let mut s = format!(
        "Homologação {} do cadastro {}: {}\n",
        h.id, h.cadastro_id, h.estado
    );
    if let Some(q) = &h.aprovado_por {
        s.push_str(&format!(
            "  aprovada por {q} em {}\n",
            quando(h.aprovado_em)
        ));
    }
    if let Some(c) = h.calibre_id {
        s.push_str(&format!("  livro {c} no Calibre\n"));
    }
    if let Some(e) = &h.erro {
        s.push_str(&format!("  erro: {e}\n"));
    }
    s.push_str(match h.estado.as_str() {
        "proposta" => "  Nada foi escrito na Biblioteca Homologada.\n",
        "aplicada" => "  Escrito na Biblioteca Homologada (registrado no Catálogo).\n",
        "falhou" => "  Não foi escrito; veja o erro e tente de novo.\n",
        "revertida" => "  Revertida.\n",
        _ => "",
    });
    s
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod testes {
    use super::*;
    use dag_core::{
        AlertaInfo, CampoInfo, CopiaDuplicata, DecisaoDescarteInfo, EspacoVolumeInfo,
        FormatoHomologadoRef, GrupoEspacoInfo, HomologacaoInfo, LivroRef, ParecidoHomologado,
        ResultadoCadastro,
    };

    fn livro() -> LivroRef {
        LivroRef {
            id: 1,
            calibre_id: 7,
            titulo: "O Jogo da Vida".into(),
            autores: vec!["Fulano".into(), "Beltrano".into()],
            isbn13: None,
            editora: None,
            idioma: None,
            estante: None,
            tags: vec![],
            caminho: "/mnt/e/Livros/Fulano/O Jogo da Vida".into(),
        }
    }

    fn copia(id: i64, sugerir: bool, coberta: bool, razoes: &[&str]) -> CopiaDuplicata {
        CopiaDuplicata {
            file_id: id,
            caminho: format!("/mnt/d/baixados/livro{id}.pdf"),
            volume: "D".into(),
            volume_id: 2,
            dispositivo_id: Some(1),
            tamanho: 3 * 1024 * 1024,
            formato: "PDF".into(),
            hash: "ab".repeat(32),
            papel: PapelCopiaInfo::DownloadOuAClassificar,
            coberta,
            decisao: DecisaoDescarteInfo {
                sugerir,
                copia_preservada: Some(1),
                razoes: razoes.iter().map(|r| (*r).to_string()).collect(),
                passos_obrigatorios: if sugerir {
                    vec!["Marcação para Deleção".into()]
                } else {
                    vec![]
                },
            },
        }
    }

    fn grupo() -> GrupoDuplicata {
        GrupoDuplicata {
            livro: livro(),
            formatos_homologados: vec![FormatoHomologadoRef {
                formato_id: 1,
                formato: "PDF".into(),
                caminho: "/mnt/e/Livros/x.pdf".into(),
                tamanho: 3 * 1024 * 1024,
                hash: "ab".repeat(32),
                file_id: Some(1),
            }],
            copias: vec![
                copia(10, true, true, &[]),
                copia(11, false, false, &["sem Cobertura em outro dispositivo"]),
            ],
            bytes_totais: 6 * 1024 * 1024,
            bytes_sugeridos: 3 * 1024 * 1024,
        }
    }

    #[test]
    fn duplicatas_destacam_o_livro_e_mostram_decisao_e_razoes() {
        let t = texto(&DadosBiblioteca::Duplicatas {
            grupos: vec![grupo()],
            bytes_totais_pagina: 6 * 1024 * 1024,
            bytes_sugeridos_pagina: 3 * 1024 * 1024,
            alertas_emitidos: 2,
            proximo_cursor: Some("42".into()),
        });
        assert!(t.contains("VOCÊ JÁ TEM ESTES LIVROS"), "{t}");
        assert!(t.contains("você já tem este livro em /mnt/e/Livros"), "{t}");
        assert!(
            t.contains("O Jogo da Vida") && t.contains("Fulano & Beltrano"),
            "{t}"
        );
        assert!(t.contains("PDF") && t.contains("6,0 MiB"), "{t}");
        assert!(
            t.contains("1/2") && t.contains("1 sugerir / 1 bloqueado"),
            "{t}"
        );
        assert!(
            t.contains("bloqueado") && t.contains("razões: sem Cobertura"),
            "{t}"
        );
        assert!(t.contains("Cobertura NÃO"), "{t}");
        assert!(t.contains("antes de mover: Marcação para Deleção"), "{t}");
        assert!(
            t.contains("Total desta página: 6,0 MiB") && t.contains("3,0 MiB só das cópias"),
            "{t}"
        );
        assert!(t.contains("nada foi movido nem apagado"), "{t}");
        assert!(
            t.contains("--cursor 42") && t.contains("2 alerta(s) novo(s)"),
            "{t}"
        );
        assert!(
            texto(&DadosBiblioteca::Duplicatas {
                grupos: vec![],
                bytes_totais_pagina: 0,
                bytes_sugeridos_pagina: 0,
                alertas_emitidos: 0,
                proximo_cursor: None
            })
            .contains("Nenhuma duplicata")
        );
    }

    fn campo(nome: &str, extraido: &str, sug: &str, conf: f32, faixa: &str) -> CampoLadoALado {
        CampoLadoALado {
            campo: nome.into(),
            extraido: Some(extraido.into()),
            sugerido: Some(sug.into()),
            sugerido_lista: vec![],
            confianca: Some(conf),
            faixa: Some(faixa.into()),
            origem: Some("Ia".into()),
            modelo: Some("gemma3:12b".into()),
            evidencias: vec!["p. 4: Título: O Jogo da Vida".into()],
            precisa_revisao: conf < 0.9,
            motivos: if conf < 0.9 {
                vec!["só uma fonte".into()]
            } else {
                vec![]
            },
            homologado: Some("Jogo da Vida".into()),
            estado: "pendente".into(),
            sugestao_id: Some(1),
        }
    }

    #[test]
    fn revisao_mostra_lado_a_lado_evidencias_e_parecido() {
        let item = ItemRevisaoInfo {
            membro: None,
            proposta_homologada: None,
            paginas: vec![],
            paginas_ocr: 0,
            file_id: 5,
            caminho: Some("/mnt/e/_classifica/jogo.pdf".into()),
            tamanho: Some(2048),
            hash: None,
            formato: Some("PDF".into()),
            pendentes: 2,
            campos: vec![
                campo("titulo", "jogo vida", "O Jogo da Vida", 0.95, "alta"),
                campo("estante", "-", "Matemática", 0.6, "media"),
            ],
            homologados_parecidos: vec![ParecidoHomologado {
                livro: livro(),
                motivo: "{\"titulo\":0.9}".into(),
                relacao: Some("mesmaobra".into()),
            }],
            obra_id: Some(3),
            edicao_id: Some(4),
            avisos: vec!["metadados internos vazios".into()],
        };
        let t = texto(&DadosBiblioteca::Revisao {
            itens: vec![item],
            total_pendentes: 12,
            proximo_cursor: Some("9".into()),
        });
        assert!(
            t.contains("EXTRAÍDO") && t.contains("SUGESTÃO") && t.contains("HOMOLOGADO PARECIDO"),
            "{t}"
        );
        assert!(
            t.contains("jogo vida") && t.contains("O Jogo da Vida [ALTA 0,95]"),
            "{t}"
        );
        assert!(
            t.contains("estante *") && t.contains("Matemática [MEDIA 0,60]"),
            "{t}"
        );
        assert!(t.contains("evidência: p. 4: Título"), "{t}");
        assert!(t.contains("revisar porque: só uma fonte"), "{t}");
        assert!(
            t.contains("Já homologado parecido: \"O Jogo da Vida\"") && t.contains("mesmaobra"),
            "{t}"
        );
        assert!(
            t.contains("Obra 3 > Edição 4") && t.contains("aviso: metadados internos vazios"),
            "{t}"
        );
        assert!(
            t.contains("12 sugestão(ões) pendente(s)") && t.contains("--cursor 9"),
            "{t}"
        );
        assert_eq!(
            texto(&DadosBiblioteca::Revisao {
                itens: vec![],
                total_pendentes: 0,
                proximo_cursor: None
            }),
            "Nada para revisar.\n"
        );
    }

    #[test]
    fn sincronizada_prova_e_alerta_quando_mudou() {
        let mut r = ResumoSincronizacao {
            biblioteca: 1,
            livros: 1200,
            removidos: 0,
            formatos: 1500,
            formatos_ligados_ao_catalogo: 1490,
            hashes_importados: 1400,
            hashes_calculados: 90,
            formatos_sem_arquivo_no_catalogo: 10,
            formatos_sem_hash: 3,
            comparacao_metadata_db: "igual".into(),
            biblioteca_intacta: true,
            diferencas: vec![],
            duracao_ms: 1234,
        };
        let t = texto(&DadosBiblioteca::Sincronizada { resumo: r.clone() });
        assert!(t.contains("PROVA DE NÃO ALTERAÇÃO: OK"), "{t}");
        assert!(
            t.contains("1.200 livro(s)") && t.contains("1.490 de 1.500"),
            "{t}"
        );
        assert!(
            t.contains("10 formato(s) ainda sem arquivo") && t.contains("3 formato(s) sem hash"),
            "{t}"
        );
        r.biblioteca_intacta = false;
        r.diferencas = vec!["metadata.db mudou".into()];
        let t = texto(&DadosBiblioteca::Sincronizada { resumo: r });
        assert!(
            t.contains("ATENÇÃO") && t.contains("metadata.db mudou") && !t.contains("OK."),
            "{t}"
        );
    }

    #[test]
    fn cadastro_bloqueado_explica_o_calibre_ocupado() {
        let t = texto(&DadosBiblioteca::Cadastrada {
            resultados: vec![
                ResultadoCadastro {
                    cadastro_id: None,
                    file_id: 1,
                    estado: "cadastrado".into(),
                    calibre_id: Some(30),
                    erro: None,
                },
                ResultadoCadastro {
                    cadastro_id: None,
                    file_id: 2,
                    estado: "pendente".into(),
                    calibre_id: None,
                    erro: None,
                },
            ],
            bloqueado: Some("Calibre em execução".into()),
        });
        assert!(t.contains("BLOQUEADO: Calibre em execução"), "{t}");
        assert!(
            t.contains("Feche o Calibre") && t.contains("nada é duplicado"),
            "{t}"
        );
        assert!(t.contains("homologar CADASTRO"), "{t}");
        let d = dica_de_erro("calibre_ocupado").unwrap();
        assert!(d.contains("Feche o Calibre"));
        for c in [
            "calibredb_ausente",
            "aprovacao_humana_exigida",
            "aprovacao_invalida",
            "biblioteca_nao_registrada",
            "sem_homologada",
            "sem_a_classificar",
            "cadastro_inexistente",
            "cadastro_nao_pronto",
            "ja_existe_na_homologada",
            "config_invalida",
            "papel_invalido",
        ] {
            assert!(dica_de_erro(c).is_some(), "{c}");
        }
        assert!(dica_de_erro("outro").is_none());
    }

    #[test]
    fn homologacao_diz_o_que_aconteceu() {
        let mut h = HomologacaoInfo {
            resumo: None,
            id: 1,
            cadastro_id: 12,
            estado: "proposta".into(),
            aprovado_por: None,
            aprovado_em: None,
            calibre_id: None,
            erro: None,
        };
        assert!(
            texto(&DadosBiblioteca::Homologacao {
                homologacao: h.clone()
            })
            .contains("Nada foi escrito")
        );
        h.estado = "aplicada".into();
        h.aprovado_por = Some("Chico".into());
        h.aprovado_em = Some(0);
        h.calibre_id = Some(99);
        let t = texto(&DadosBiblioteca::Homologacao { homologacao: h });
        assert!(
            t.contains("aprovada por Chico") && t.contains("Escrito na Biblioteca Homologada"),
            "{t}"
        );
    }

    #[test]
    fn alertas_destacam_ja_tem_este_livro() {
        let a = |id, tipo: &str| AlertaInfo {
            id,
            chave: None,
            tipo: tipo.into(),
            file_id: Some(id * 10),
            dados: serde_json::json!({"titulo": "O Jogo da Vida"}),
            criado_em: 0,
            visto_em: None,
            silenciado: false,
        };
        let t = texto(&DadosBiblioteca::Alertas {
            itens: vec![a(1, "ja_tem_este_livro"), a(2, "extracao_falhou")],
            nao_vistos_total: 2,
            proximo_cursor: None,
        });
        assert!(
            t.contains("VOCÊ JÁ TEM ESTE LIVRO") && t.contains("extracao_falhou"),
            "{t}"
        );
        assert!(
            t.contains("2 alerta(s) não visto(s)") && t.contains("O Jogo da Vida"),
            "{t}"
        );
    }

    #[test]
    fn fila_processamento_e_enfileiramento() {
        let c = ContagemFila {
            pendentes: 1200,
            em_andamento: 1,
            concluidos: 30,
            falhos: 2,
            excecoes: 0,
        };
        let t = texto(&DadosBiblioteca::Fila {
            contagem: c,
            itens: vec![ItemFilaInfo {
                id: 1,
                file_id: 9,
                caminho: Some("/x/y.pdf".into()),
                motivo: dag_core::MotivoFila::Novo,
                prioridade: 5,
                estado: dag_core::EstadoFilaInfo::Falhou,
                etapa: Some("extracao".into()),
                tentativas: 3,
                erro: Some("pdftotext ausente".into()),
                enfileirado_em: 0,
            }],
            proximo_cursor: None,
        });
        assert!(
            t.contains("pendentes 1.200")
                && t.contains("falhou")
                && t.contains("pdftotext ausente"),
            "{t}"
        );
        let t = texto(&DadosBiblioteca::Processada {
            resumo: ResumoProcessamento {
                processados: 10,
                duplicatas_exatas: 3,
                extraidos: 6,
                reaproveitados: 1,
                revisao_pendente: 5,
                interrompido: true,
                pendentes_restantes: 90,
                consultas_ia: 4,
                ..Default::default()
            },
        });
        assert!(
            t.contains("PAUSADO")
                && t.contains("restam 90 na fila")
                && t.contains("`dag biblioteca revisar`"),
            "{t}"
        );
        let t = texto(&DadosBiblioteca::Enfileirada {
            resumo: ResumoEnfileiramento {
                examinados: 100,
                enfileirados: 80,
                ja_na_fila: 15,
                ignorados: 5,
                interrompido: false,
            },
        });
        assert!(
            t.contains("80 enfileirado(s)") && t.contains("processar"),
            "{t}"
        );
    }

    #[test]
    fn perfil_espaco_config_resumo_e_registro() {
        let b = BibliotecaInfo {
            id: 1,
            caminho: "/mnt/e/Livros".into(),
            papel: PapelRegistro::Homologada,
            nome: "Livros".into(),
            estado: "ativa".into(),
            versao_calibre: Some("7.6".into()),
            library_uuid: None,
            livros: 1200,
            formatos: 1500,
            ultima_sincronizacao: None,
            ultimo_resultado: None,
        };
        assert!(
            texto(&DadosBiblioteca::Bibliotecas {
                itens: vec![b.clone()]
            })
            .contains("Homologada")
        );
        assert!(
            texto(&DadosBiblioteca::Bibliotecas { itens: vec![] }).contains("Nenhuma biblioteca")
        );
        let t = texto(&DadosBiblioteca::Registrada {
            biblioteca: b.clone(),
            criada: false,
            avisos: vec!["sem coluna #pasta".into()],
        });
        assert!(
            t.contains("registrada: #1")
                && t.contains("aviso: sem coluna #pasta")
                && t.contains("sincronizar 1"),
            "{t}"
        );
        let p = PerfilResumo {
            versao_esquema: Some(25),
            total_livros: 1200,
            total_autores: 800,
            total_tags: 90,
            total_series: 10,
            total_editoras: 50,
            total_arquivos_formato: 1500,
            formatos: vec![("PDF".into(), 900), ("EPUB".into(), 600)],
            tags_principais: vec![("xadrez".into(), 40)],
            candidatos_estante: vec![(
                "#pasta".into(),
                90,
                vec!["texto, todos preenchidos".into()],
            )],
            nao_clonaveis: vec!["coluna #calc (fórmula)".into()],
            perfil_bruto: serde_json::json!({"tags_controle": ["_revisar"]}),
        };
        let campos = vec![
            CampoInfo {
                rotulo: "pasta".into(),
                nome: "Pasta".into(),
                datatype: "text".into(),
                multiplo: false,
                clonavel: true,
                motivo: None,
            },
            CampoInfo {
                rotulo: "calc".into(),
                nome: "Calc".into(),
                datatype: "composite".into(),
                multiplo: false,
                clonavel: false,
                motivo: Some("fórmula".into()),
            },
        ];
        let t = texto(&DadosBiblioteca::Perfil {
            biblioteca: b,
            perfil: p,
            campos,
        });
        assert!(t.contains("#pasta") && t.contains("(90 ponto(s))"), "{t}");
        assert!(
            t.contains("não: fórmula") && t.contains("1 de 2 coluna(s)"),
            "{t}"
        );
        assert!(
            t.contains("PDF 900") && t.contains("Tags de controle: _revisar"),
            "{t}"
        );
        assert!(t.contains("coluna #calc (fórmula)"), "{t}");
        let t = texto(&DadosBiblioteca::Espaco {
            relatorio: RelatorioEspacoInfo {
                volumes: vec![EspacoVolumeInfo {
                    volume_id: 2,
                    caminhos: 10,
                    arquivos_fisicos: 9,
                    bytes_logicos: 10 << 20,
                    bytes_alocados: 10 << 20,
                    estimativa_recuperavel: 4 << 20,
                    economia_incerta: 1 << 20,
                    efetivamente_recuperado: 0,
                    caminhos_hardlink_ignorados: 1,
                    membros_ignorados: 0,
                }],
                grupos: vec![GrupoEspacoInfo {
                    hash: "ab".repeat(32),
                    tamanho_logico: 4 << 20,
                    mantida: (1, 1),
                    redundantes: vec![(2, 2, 4 << 20, true, None)],
                    bytes_estimados: 4 << 20,
                    bytes_incertos: 0,
                }],
                observacao: "Estimativa, não promessa.".into(),
            },
        });
        assert!(
            t.contains("ESTIMADO")
                && t.contains("4,0 MiB")
                && t.contains("hard links")
                && t.contains("Estimativa, não promessa."),
            "{t}"
        );
        let t = texto(&DadosBiblioteca::Config {
            config: ConfigBiblioteca::default(),
        });
        assert!(
            t.contains("ia.ativa = false")
                && t.contains("ocr.dpi = 300")
                && t.contains("ocr.idiomas = [\"por\",\"eng\"]"),
            "{t}"
        );
        let t = texto(&DadosBiblioteca::Resumo {
            resumo: ResumoBiblioteca {
                alertas_por_tipo: vec![("ja_tem_este_livro".into(), 5, 2)],
                revisoes_pendentes: 7,
                ..Default::default()
            },
        });
        assert!(
            t.contains("revisões pendentes: 7") && t.contains("ja_tem_este_livro: 5 / 2"),
            "{t}"
        );
        assert!(
            texto(&DadosBiblioteca::Decidida {
                lote: "L1".into(),
                aplicadas: 3,
                ignoradas: vec!["tags: nenhuma sugestão pendente".into()]
            })
            .contains("desfazer L1")
        );
        assert!(texto(&DadosBiblioteca::Vistos { marcados: 2 }).contains("2 alerta(s)"));
        assert!(
            texto(&DadosBiblioteca::Desfeita {
                lote: "L1".into(),
                desfeitas: 3
            })
            .contains("desfeita")
        );
    }
}
