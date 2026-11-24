//! Saída legível (pt-BR) das respostas: tabelas com tamanhos em unidades humanas.

use dag_core::formato::{bytes_humanos, data_hora_utc, inteiro_pt_br};
use dag_core::{
    ArquivoSemCobertura, ClasseRecuperavel, CoberturaArquivo, CoberturaPasta, CoberturaVolume,
    DadosRelatorio, DetalheGrupo, EntradaExploracao, EscopoMarcacao, EspacoItem, EstadoExecucao,
    EstadoMarcacao, Finalidade, FormatoExportacao, GrupoDuplicados, MarcacaoInfo, ModoPrincipal,
    NivelEspaco, Resposta, ResultadoMarcacao, ResumoDuplicados, ResumoHash, ResumoVerificacao,
    StatusExecucao, TipoEntradaExploracao, TipoMarcacao, TipoRelatorio, VolumeInfo,
};

/// Tabela de texto: colunas alinhadas à esquerda (`false`) ou à direita (`true`).
fn tabela(cabecalho: &[&str], direita: &[bool], linhas: &[Vec<String>]) -> String {
    let n = cabecalho.len();
    let mut larg: Vec<usize> = cabecalho.iter().map(|c| c.chars().count()).collect();
    for l in linhas {
        for (i, c) in l.iter().enumerate().take(n) {
            larg[i] = larg[i].max(c.chars().count());
        }
    }
    let fmt = |cols: Vec<&str>| -> String {
        let mut s = String::new();
        for (i, c) in cols.iter().enumerate() {
            let pad = larg[i].saturating_sub(c.chars().count());
            if i > 0 {
                s.push_str("  ");
            }
            if direita[i] {
                s.push_str(&" ".repeat(pad));
                s.push_str(c);
            } else if i + 1 < n {
                s.push_str(c);
                s.push_str(&" ".repeat(pad));
            } else {
                s.push_str(c);
            }
        }
        s
    };
    let mut saida = fmt(cabecalho.to_vec());
    saida.push('\n');
    for l in linhas {
        saida.push_str(&fmt(l.iter().map(String::as_str).collect()));
        saida.push('\n');
    }
    saida
}

fn finalidade(f: Option<Finalidade>) -> String {
    match f {
        Some(Finalidade::Sistema) => "sistema",
        Some(Finalidade::Aplicativo) => "aplicativo",
        Some(Finalidade::Usuario) => "usuário",
        Some(Finalidade::Compartilhado) => "compartilhado",
        Some(Finalidade::Misto) => "misto",
        Some(Finalidade::Desconhecido) | None => "-",
    }
    .to_string()
}

fn opt(s: Option<&str>) -> String {
    s.unwrap_or("-").to_string()
}

/// Lista de volumes. Sem `todos`, só os reais, com rodapé sobre os ocultos; com `todos`, junta os
/// ocultos e acrescenta a coluna CLASSE.
pub fn volumes(reais: &[VolumeInfo], ocultos: &[VolumeInfo], todos: bool) -> String {
    let mostrar: Vec<&VolumeInfo> = if todos {
        reais.iter().chain(ocultos).collect()
    } else {
        reais.iter().collect()
    };
    let mut s = if mostrar.is_empty() {
        "Nenhum volume.\n".to_string()
    } else {
        tabela_volumes(&mostrar, todos)
    };
    if !todos && !ocultos.is_empty() {
        s.push_str(&format!(
            "\n{} volume(s) de imagem/virtuais ocultos (snap, loop...): use --todos\n",
            ocultos.len()
        ));
    }
    s
}

fn tabela_volumes(v: &[&VolumeInfo], com_classe: bool) -> String {
    let linhas: Vec<Vec<String>> = v
        .iter()
        .map(|v| {
            let uso = match (v.capacidade_bytes, v.livre_bytes) {
                (Some(c), Some(l)) if c > 0 => {
                    format!("{:.0}%", (c.saturating_sub(l)) as f64 * 100.0 / c as f64)
                }
                _ => "-".into(),
            };
            let mut linha = vec![
                if v.id.0 == 0 {
                    "-".into()
                } else {
                    v.id.to_string()
                },
                opt(v.ponto_montagem.as_deref()),
                opt(v.fs_type.as_deref()),
                opt(v.rotulo.as_deref()),
                v.capacidade_bytes.map_or("-".into(), bytes_humanos),
                v.livre_bytes.map_or("-".into(), bytes_humanos),
                uso,
                if v.conectado { "conectado" } else { "offline" }.into(),
            ];
            if com_classe {
                let c = v.classe.rotulo();
                linha.push(if v.sistema {
                    format!("{c} (sistema)")
                } else {
                    c.into()
                });
            }
            linha
        })
        .collect();
    let mut titulos = vec![
        "ID",
        "MONTAGEM",
        "FS",
        "RÓTULO",
        "CAPACIDADE",
        "LIVRE",
        "USO",
        "ESTADO",
    ];
    let mut alinh = vec![true, false, false, false, true, true, true, false];
    if com_classe {
        titulos.push("CLASSE");
        alinh.push(false);
    }
    tabela(&titulos, &alinh, &linhas)
}

fn estado(e: &StatusExecucao) -> String {
    match (e.estado_detalhe.as_str(), e.retomando) {
        ("hash_em_andamento", true) => return "hash em andamento (retomada)".into(),
        ("hash_parado", true) => return "interrompida no hash, retomando".into(),
        ("hash_em_andamento", _) => return "hash em andamento".into(),
        ("hash_parado", _) => return "hash parado (retomável)".into(),
        ("hash_finalizando", _) => return "hash finalizando".into(),
        ("interrompida", true) => return "interrompida, retomando".into(),
        _ => {}
    }
    if e.retomando && e.estado == EstadoExecucao::EmAndamento {
        return "em andamento (retomada)".into();
    }
    match e.estado {
        EstadoExecucao::Concluida if e.estado_detalhe == "concluida_com_erros" => {
            "concluída c/ erros".into()
        }
        EstadoExecucao::Concluida => "concluída".into(),
        EstadoExecucao::EmAndamento => "em andamento".into(),
        EstadoExecucao::Pausada if e.estado_detalhe == "interrompida" => "interrompida".into(),
        EstadoExecucao::Pausada => "pausada".into(),
        EstadoExecucao::Cancelada => "cancelada".into(),
        EstadoExecucao::Falhou => "falhou".into(),
        EstadoExecucao::Pendente => "pendente".into(),
    }
}

pub fn status(execs: &[StatusExecucao], cat: &dag_core::ResumoCatalogo) -> String {
    let mut s = format!(
        "Catálogo: {} volume(s), {} pastas, {} arquivos ({} com hash, {}), {} (tamanho lógico)\n",
        inteiro_pt_br(cat.volumes),
        inteiro_pt_br(cat.pastas),
        inteiro_pt_br(cat.arquivos),
        inteiro_pt_br(cat.arquivos_com_hash),
        dag_core::painel::pct_texto(dag_core::painel::percentual(
            cat.arquivos_com_hash,
            cat.arquivos
        )),
        bytes_humanos(cat.bytes_logicos),
    );
    if execs.is_empty() {
        s.push_str("Nenhuma varredura registrada.\n");
        return s;
    }
    s.push('\n');
    let linhas: Vec<Vec<String>> = execs
        .iter()
        .map(|e| {
            vec![
                e.id.to_string(),
                format!("{:?}", e.modo).replace("Scan", " Scan"),
                estado(e),
                data_hora_utc(e.iniciada_em),
                inteiro_pt_br(e.pastas),
                inteiro_pt_br(e.arquivos),
                bytes_humanos(e.bytes_logicos),
                inteiro_pt_br(e.erros),
                e.raizes.join(", "),
            ]
        })
        .collect();
    s.push_str(&tabela(
        &[
            "ID",
            "MODO",
            "ESTADO",
            "INÍCIO (UTC)",
            "PASTAS",
            "ARQUIVOS",
            "TAMANHO",
            "ERROS",
            "RAÍZES",
        ],
        &[true, false, false, false, true, true, true, true, false],
        &linhas,
    ));
    for e in execs {
        if let Some(h) = &e.hash {
            s.push_str(&status_hash(e.id.0, h));
        }
    }
    s
}

/// Ponto de Confiança por Raiz.
pub fn status_confianca(c: &[dag_core::StatusConfianca]) -> String {
    let mut s = String::from("Ponto de Confiança (só o monitoramento mantém a raiz em dia):\n");
    let linhas: Vec<Vec<String>> = c
        .iter()
        .map(|r| {
            vec![
                r.raiz.clone(),
                match r.confiavel_em {
                    Some(t) => format!("confiável desde {}", data_hora_utc(t)),
                    None => "não confiável".into(),
                },
                match (r.perdida_em, &r.motivo_perda) {
                    (Some(t), Some(m)) => format!("perdido em {}: {m}", data_hora_utc(t)),
                    _ => "-".into(),
                },
            ]
        })
        .collect();
    s.push_str(&tabela(
        &["RAIZ", "SITUAÇÃO", "ÚLTIMA PERDA"],
        &[false, false, false],
        &linhas,
    ));
    s
}

/// Etapa de hash de uma execução: totais, vazão e uma linha por disco.
fn status_hash(id: i64, h: &dag_core::StatusHash) -> String {
    let feitos = h.arquivos_hasheados;
    let total = feitos + h.arquivos_pendentes + h.arquivos_com_erro;
    let bytes_total = h.bytes_hasheados + h.bytes_pendentes;
    let mut s = format!(
        "\nExecução {id}: etapa de hash ({}) — {} de {} arquivos ({}), {} de {}; pendentes: {} arquivos, {}",
        if h.em_andamento {
            "em andamento"
        } else {
            "parada"
        },
        inteiro_pt_br(feitos),
        inteiro_pt_br(total),
        percentual(feitos, total),
        bytes_humanos(h.bytes_hasheados),
        bytes_humanos(bytes_total),
        inteiro_pt_br(h.arquivos_pendentes),
        bytes_humanos(h.bytes_pendentes),
    );
    if h.arquivos_com_erro > 0 {
        s.push_str(&format!(
            ", {} com erro",
            inteiro_pt_br(h.arquivos_com_erro)
        ));
    }
    if let Some(v) = h.bytes_por_seg {
        s.push_str(&format!(" | {}/s", bytes_humanos(v)));
    }
    s.push('\n');
    let linhas: Vec<Vec<String>> = h
        .discos
        .iter()
        .map(|d| {
            let tot = d.arquivos_hasheados + d.arquivos_pendentes + d.arquivos_com_erro;
            vec![
                d.disco.clone(),
                d.volumes
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                format!(
                    "{}/{}",
                    inteiro_pt_br(d.arquivos_hasheados),
                    inteiro_pt_br(tot)
                ),
                percentual(d.arquivos_hasheados, tot),
                bytes_humanos(d.bytes_hasheados),
                bytes_humanos(d.bytes_pendentes),
            ]
        })
        .collect();
    s.push_str(&tabela(
        &["DISCO", "VOLUMES", "ARQUIVOS", "%", "HASHEADO", "PENDENTE"],
        &[false, false, true, true, true, true],
        &linhas,
    ));
    s
}

fn percentual(feito: u64, total: u64) -> String {
    if total == 0 {
        return "100%".into();
    }
    format!("{}%", feito.saturating_mul(100) / total)
}

#[allow(clippy::too_many_arguments)]
fn scan(
    execucao: i64,
    desfecho: &str,
    pastas: u64,
    arquivos: u64,
    especiais: u64,
    bytes: u64,
    ignorados: u64,
    erros: u64,
    ms: u64,
    hash: Option<&ResumoHash>,
    dup: Option<&ResumoDuplicados>,
    rec: Option<&dag_core::ResumoReconciliacao>,
) -> String {
    let seg = ms as f64 / 1000.0;
    let rotulo = match desfecho {
        "concluida" => "concluída",
        "concluida_com_erros" => "concluída com erros",
        _ => "cancelada",
    };
    let mut s = format!(
        "Varredura {execucao} {rotulo} em {} s: {} pastas, {} arquivos, {} especiais, {} (tamanho lógico); {} ignorados, {} erros.\n",
        format!("{seg:.1}").replace('.', ","),
        inteiro_pt_br(pastas),
        inteiro_pt_br(arquivos),
        inteiro_pt_br(especiais),
        bytes_humanos(bytes),
        inteiro_pt_br(ignorados),
        inteiro_pt_br(erros),
    );
    if let Some(r) = rec {
        s.push_str(&reconciliacao(r));
    }
    if let Some(h) = hash {
        s.push_str(&format!(
            "Hash Completo: {} arquivo(s) lidos ({}), {} reaproveitados, {} erro(s).\n",
            inteiro_pt_br(h.hasheados),
            bytes_humanos(h.bytes_lidos),
            inteiro_pt_br(h.reaproveitados),
            inteiro_pt_br(h.erros),
        ));
    }
    if let Some(d) = dup {
        s.push_str(&format!(
            "Duplicados: {} grupo(s), {} cópias; espaço recuperável (estimativa): {}.\n",
            inteiro_pt_br(d.grupos),
            inteiro_pt_br(d.copias),
            bytes_humanos(d.espaco_recuperavel_bytes),
        ));
        if d.marcacoes_pendentes > 0 {
            s.push_str(&format!(
                "Atenção: {} Marcação(ões) pendente(s) (o conteúdo mudou); confirme com `dag confirmar <id>`.\n",
                inteiro_pt_br(d.marcacoes_pendentes)
            ));
        }
    }
    if desfecho == "cancelada" {
        s.push_str(&format!("Retome com: dag scan --retomar {execucao}\n"));
    }
    s
}

fn reconciliacao(r: &dag_core::ResumoReconciliacao) -> String {
    let mut s = format!(
        "Reconciliação: {} novo(s), {} alterado(s), {} inalterado(s), {} arquivo(s) e {} pasta(s) renomeado(s), {} arquivo(s) e {} pasta(s) ausente(s).\n",
        inteiro_pt_br(r.arquivos_novos),
        inteiro_pt_br(r.arquivos_alterados),
        inteiro_pt_br(r.arquivos_inalterados),
        inteiro_pt_br(r.arquivos_renomeados),
        inteiro_pt_br(r.pastas_renomeadas),
        inteiro_pt_br(r.arquivos_ausentes),
        inteiro_pt_br(r.pastas_ausentes),
    );
    if r.bloqueios > 0 {
        s.push_str(&format!(
            "{} subárvore(s) protegida(s) por erro de leitura: nada nelas foi marcado como ausente.\n",
            inteiro_pt_br(r.bloqueios)
        ));
    }
    if !r.remocao_aplicada {
        s.push_str(&format!(
            "Remoções não aplicadas: {}.\n",
            r.motivo_sem_remocao
                .as_deref()
                .unwrap_or("varredura incompleta")
        ));
    }
    s
}

fn saude_monitor(m: &dag_core::SaudeMonitorInfo) -> String {
    let estado = if m.em_execucao {
        format!("ativo ({})", m.estado)
    } else if m.estado == "parado" || m.heartbeat.is_none() {
        "parado".to_string()
    } else {
        format!("sem sinal (último estado: {})", m.estado)
    };
    let opt = |t: Option<i64>| t.map_or_else(|| "nunca".to_string(), data_hora_utc);
    let mut s = format!(
        "Monitor: {estado}\n  watches: {}/{}; backlog: {}; eventos processados: {}; overflows: {}\n  último evento: {}; última reconciliação: {}\n",
        inteiro_pt_br(m.watches),
        inteiro_pt_br(m.watches_limite),
        inteiro_pt_br(m.backlog),
        inteiro_pt_br(m.eventos_processados),
        inteiro_pt_br(m.overflows),
        opt(m.ultimo_evento_em),
        opt(m.ultima_reconciliacao),
    );
    if m.raizes_parciais > 0 {
        s.push_str(&format!(
            "  ATENÇÃO: {} raiz(es) com cobertura parcial (limite de watches)\n",
            m.raizes_parciais
        ));
    }
    if m.lacunas_abertas > 0 || m.eventos_nao_processados > 0 {
        s.push_str(&format!(
            "  Reconciliação pendente: {} lacuna(s) aberta(s), {} evento(s) não processado(s); rode `dag scan --fast`\n",
            inteiro_pt_br(m.lacunas_abertas),
            inteiro_pt_br(m.eventos_nao_processados)
        ));
    }
    if let Some(e) = &m.erro {
        s.push_str(&format!("  erro: {e}\n"));
    }
    s
}

fn eventos(e: &[dag_core::EventoInfo]) -> String {
    if e.is_empty() {
        return "Nenhum evento registrado.\n".to_string();
    }
    let linhas: Vec<Vec<String>> = e
        .iter()
        .map(|x| {
            let caminho = match (&x.caminho_antigo, &x.caminho) {
                (Some(a), Some(n)) => format!("{a} -> {n}"),
                (_, Some(n)) => n.clone(),
                _ => x.detalhe.clone().unwrap_or_default(),
            };
            vec![
                x.id.to_string(),
                data_hora_utc(x.observado_em),
                x.tipo.clone(),
                x.fonte.clone(),
                if x.reconciliar && x.processado_em.is_none() {
                    "pendente".into()
                } else {
                    String::new()
                },
                caminho,
            ]
        })
        .collect();
    tabela(
        &[
            "ID",
            "QUANDO (UTC)",
            "TIPO",
            "FONTE",
            "RECONCILIAÇÃO",
            "CAMINHO",
        ],
        &[true, false, false, false, false, false],
        &linhas,
    )
}

fn monitor_encerrado(r: &dag_core::ResumoMonitorInfo) -> String {
    format!(
        "Monitor encerrado após {} s: {} lote(s), {} criado(s), {} alterado(s), {} removido(s), {} renomeado(s), {} lacuna(s) ({} overflow), {} reconciliação(ões) Fast Scan, {} watches.\n",
        format!("{:.1}", r.duracao_ms as f64 / 1000.0).replace('.', ","),
        inteiro_pt_br(r.lotes),
        inteiro_pt_br(r.criados),
        inteiro_pt_br(r.alterados),
        inteiro_pt_br(r.removidos),
        inteiro_pt_br(r.renomeados),
        inteiro_pt_br(r.lacunas),
        inteiro_pt_br(r.overflows),
        inteiro_pt_br(r.fast_scans),
        inteiro_pt_br(r.watches),
    )
}

fn tipo_marcacao(t: TipoMarcacao) -> &'static str {
    match t {
        TipoMarcacao::Backup => "backup",
        TipoMarcacao::Delecao => "deleção",
    }
}

fn nome_marcacao(t: TipoMarcacao) -> &'static str {
    match t {
        TipoMarcacao::Backup => "Marcação de Backup",
        TipoMarcacao::Delecao => "Marcação para Deleção",
    }
}

fn escopo(e: EscopoMarcacao) -> &'static str {
    match e {
        EscopoMarcacao::RegraVolume => "regra de Volume",
        EscopoMarcacao::RegraPasta => "regra de pasta",
        EscopoMarcacao::ManualArquivo => "manual",
        EscopoMarcacao::ManualGrupo => "manual (grupo)",
    }
}

fn estado_marcacao(e: EstadoMarcacao) -> &'static str {
    match e {
        EstadoMarcacao::Ativa => "ativa",
        EstadoMarcacao::Pendente => "PENDENTE",
    }
}

fn hash_curto(h: &dag_core::HashCompleto) -> String {
    h.to_string().chars().take(12).collect()
}

pub fn duplicados(grupos: &[GrupoDuplicados], total: u64, proximo: Option<&str>) -> String {
    if grupos.is_empty() {
        return "Nenhum Grupo de Duplicados.\n".into();
    }
    let linhas: Vec<Vec<String>> = grupos
        .iter()
        .map(|g| {
            let off = if g.copias_offline > 0 {
                format!(" ({} offline)", g.copias_offline)
            } else {
                String::new()
            };
            vec![
                g.id.to_string(),
                bytes_humanos(g.tamanho),
                format!("{}{off}", g.copias),
                bytes_humanos(g.bytes_marcados_delecao),
                bytes_humanos(g.bytes_redundantes_estimados),
                bytes_humanos(g.espaco_recuperavel_bytes),
                match (g.copia_principal, g.modo_principal) {
                    (Some(a), ModoPrincipal::Manual) => format!("arq {a} (manual)"),
                    (Some(a), ModoPrincipal::Auto) => format!("arq {a} (auto)"),
                    (None, _) => "-".into(),
                },
                hash_curto(&g.hash),
            ]
        })
        .collect();
    let mut s = tabela(
        &[
            "GRUPO",
            "TAMANHO",
            "CÓPIAS",
            "MARCADO P/ DELEÇÃO",
            "REDUNDANTE (EST.)",
            "RECUPERÁVEL",
            "CÓPIA PRINCIPAL",
            "HASH",
        ],
        &[true, true, false, true, true, true, false, false],
        &linhas,
    );
    s.push_str(&format!(
        "\n{} grupo(s) no total; espaço recuperável é estimativa (veja `dag relatorio espaco` para as limitações).\n",
        inteiro_pt_br(total)
    ));
    if let Some(c) = proximo {
        s.push_str(&format!("Há mais: repita com --cursor {c}\n"));
    }
    s
}

fn cobertura_txt(c: &CoberturaArquivo) -> String {
    match (c.coberto, c.nao_verificada_desde) {
        (true, Some(t)) => format!("sim (não verificada desde {})", data_hora_utc(t)),
        (true, None) => "sim".into(),
        (false, _) => "não".into(),
    }
}

pub fn grupo(d: &DetalheGrupo) -> String {
    let g = &d.grupo;
    let mut s = format!(
        "Grupo {} — {} × {} cópia(s) (hash {}…), Cópia Principal {}\n",
        g.id,
        bytes_humanos(g.tamanho),
        g.copias,
        hash_curto(&g.hash),
        match g.modo_principal {
            ModoPrincipal::Auto => "automática",
            ModoPrincipal::Manual => "manual",
        }
    );
    s.push_str(&format!(
        "Espaço recuperável: {} marcado para Deleção + {} estimado = {}\n\n",
        bytes_humanos(g.bytes_marcados_delecao),
        bytes_humanos(g.bytes_redundantes_estimados),
        bytes_humanos(g.espaco_recuperavel_bytes),
    ));
    let linhas: Vec<Vec<String>> = d
        .copias
        .iter()
        .map(|c| {
            let vol = c
                .volume_rotulo
                .clone()
                .unwrap_or_else(|| c.volume.to_string());
            let vol = if c.conectado {
                vol
            } else {
                format!(
                    "{vol} (offline{})",
                    c.visto_em
                        .map_or(String::new(), |t| format!(" desde {}", data_hora_utc(t)))
                )
            };
            let mut caminho = c.caminho.clone();
            if c.outros_caminhos > 0 {
                caminho.push_str(&format!(" (+{} hard link)", c.outros_caminhos));
            }
            vec![
                c.arquivo.to_string(),
                vol,
                caminho,
                if c.principal { "sim" } else { "-" }.into(),
                c.marcacao.as_ref().map_or("-".into(), |m| {
                    format!(
                        "{} #{} {} ({})",
                        tipo_marcacao(m.tipo),
                        m.id,
                        estado_marcacao(m.estado),
                        escopo(m.escopo)
                    )
                }),
                cobertura_txt(&c.cobertura),
                match c.recuperavel {
                    ClasseRecuperavel::Nao => "-",
                    ClasseRecuperavel::Marcada => "marcada",
                    ClasseRecuperavel::Estimada => "estimada",
                }
                .into(),
            ]
        })
        .collect();
    s.push_str(&tabela(
        &[
            "ARQ",
            "VOLUME",
            "CAMINHO",
            "PRINCIPAL",
            "MARCAÇÃO",
            "COBERTURA",
            "RECUPERÁVEL",
        ],
        &[true, false, false, false, false, false, false],
        &linhas,
    ));
    s
}

fn alerta_unica(r: &ResultadoMarcacao) -> String {
    if r.unicas_copias_total == 0 {
        return String::new();
    }
    let mut s = format!(
        "\n⚠ ÚNICA CÓPIA: {} arquivo(s) marcado(s) para Deleção não têm nenhuma outra cópia no Catálogo.\n",
        inteiro_pt_br(r.unicas_copias_total)
    );
    for a in &r.unicas_copias {
        s.push_str(&format!("  ⚠ {}\n", a.caminho));
    }
    let resto = r
        .unicas_copias_total
        .saturating_sub(r.unicas_copias.len() as u64);
    if resto > 0 {
        s.push_str(&format!("  ... e mais {}\n", inteiro_pt_br(resto)));
    }
    s.push_str(
        "  Se esta for a decisão errada, remova com `dag desmarcar`. O DAG nunca apaga nem move arquivos.\n",
    );
    s
}

pub fn marcacao(r: &ResultadoMarcacao) -> String {
    let mut s = String::new();
    for m in &r.marcacoes {
        s.push_str(&format!(
            "{} #{} {} ({}): {}\n",
            nome_marcacao(m.tipo),
            m.id,
            if m.estado == EstadoMarcacao::Pendente {
                "pendente"
            } else {
                "registrada"
            },
            escopo(m.escopo),
            m.alvo
        ));
    }
    if r.marcacoes.is_empty() {
        s.push_str("Nada a marcar: as cópias já tinham esta Marcação.\n");
    }
    if r.ja_marcadas > 0 {
        s.push_str(&format!(
            "{} cópia(s) já tinham a Marcação.\n",
            r.ja_marcadas
        ));
    }
    for a in &r.avisos {
        s.push_str(&format!("Aviso: {a}\n"));
    }
    s.push_str(&alerta_unica(r));
    s.push_str("Somente o Catálogo foi alterado.\n");
    s
}

pub fn desmarcado(v: &[MarcacaoInfo]) -> String {
    let mut s = String::new();
    for m in v {
        s.push_str(&format!(
            "{} #{} removida: {}\n",
            nome_marcacao(m.tipo),
            m.id,
            m.alvo
        ));
    }
    s
}

fn cobertura_volumes(v: &[CoberturaVolume]) -> String {
    let linhas: Vec<Vec<String>> = v
        .iter()
        .map(|x| {
            vec![
                x.volume.to_string(),
                x.rotulo.clone().unwrap_or_else(|| "-".into()),
                if x.conectado { "conectado" } else { "offline" }.into(),
                inteiro_pt_br(x.arquivos),
                inteiro_pt_br(x.com_cobertura),
                inteiro_pt_br(x.cobertura_nao_verificada),
                inteiro_pt_br(x.sem_cobertura),
                bytes_humanos(x.bytes_sem_cobertura),
            ]
        })
        .collect();
    tabela(
        &[
            "VOLUME",
            "RÓTULO",
            "ESTADO",
            "ARQUIVOS",
            "COM COBERTURA",
            "NÃO VERIFICADA",
            "SEM COBERTURA",
            "TAMANHO SEM COB.",
        ],
        &[true, false, false, true, true, true, true, true],
        &linhas,
    )
}

fn cobertura_pastas(v: &[CoberturaPasta]) -> String {
    let linhas: Vec<Vec<String>> = v
        .iter()
        .map(|x| {
            vec![
                x.caminho.clone(),
                inteiro_pt_br(x.arquivos),
                inteiro_pt_br(x.com_cobertura),
                inteiro_pt_br(x.sem_cobertura),
                bytes_humanos(x.bytes_sem_cobertura),
            ]
        })
        .collect();
    tabela(
        &[
            "PASTA",
            "ARQUIVOS",
            "COM COBERTURA",
            "SEM COBERTURA",
            "TAMANHO SEM COB.",
        ],
        &[false, true, true, true, true],
        &linhas,
    )
}

fn sem_cobertura(v: &[ArquivoSemCobertura]) -> String {
    if v.is_empty() {
        return "Todos os arquivos têm Cobertura.\n".into();
    }
    let linhas: Vec<Vec<String>> = v
        .iter()
        .map(|a| {
            vec![
                a.arquivo.to_string(),
                a.volume_rotulo
                    .clone()
                    .unwrap_or_else(|| a.volume.to_string()),
                bytes_humanos(a.tamanho),
                if a.unica_copia {
                    "⚠ ÚNICA CÓPIA"
                } else {
                    "-"
                }
                .into(),
                a.caminho.clone(),
            ]
        })
        .collect();
    tabela(
        &["ARQ", "VOLUME", "TAMANHO", "ALERTA", "CAMINHO"],
        &[true, false, true, false, false],
        &linhas,
    )
}

fn espaco_item(i: &EspacoItem) -> Vec<String> {
    vec![
        i.id.map_or("-".into(), |x| x.to_string()),
        match i.conectado {
            Some(false) => format!("{} (offline)", i.nome),
            _ => i.nome.clone(),
        },
        bytes_humanos(i.bytes_marcados_delecao),
        bytes_humanos(i.bytes_redundantes_estimados),
        bytes_humanos(i.bytes_total),
        inteiro_pt_br(i.copias),
    ]
}

fn espaco(total: &EspacoItem, itens: &[EspacoItem], limitacoes: &[String]) -> String {
    let mut s = format!(
        "Espaço recuperável deste computador: {} ({} marcado para Deleção + {} estimado), {} cópia(s).\n",
        bytes_humanos(total.bytes_total),
        bytes_humanos(total.bytes_marcados_delecao),
        bytes_humanos(total.bytes_redundantes_estimados),
        inteiro_pt_br(total.copias),
    );
    if !itens.is_empty() {
        let nivel = match itens[0].nivel {
            NivelEspaco::Computador => "COMPUTADOR",
            NivelEspaco::Volume => "VOLUME",
            NivelEspaco::Pasta => "PASTA",
            NivelEspaco::Grupo => "GRUPO",
        };
        let linhas: Vec<Vec<String>> = itens.iter().map(espaco_item).collect();
        s.push('\n');
        s.push_str(&tabela(
            &[
                "ID",
                nivel,
                "MARCADO P/ DELEÇÃO",
                "REDUNDANTE (EST.)",
                "TOTAL",
                "CÓPIAS",
            ],
            &[true, false, true, true, true, true],
            &linhas,
        ));
    }
    s.push_str("\nLimitações:\n");
    for l in limitacoes {
        s.push_str(&format!("  - {l}\n"));
    }
    s
}

fn marcacoes(v: &[MarcacaoInfo]) -> String {
    if v.is_empty() {
        return "Nenhuma Marcação.\n".into();
    }
    let linhas: Vec<Vec<String>> = v
        .iter()
        .map(|m| {
            vec![
                m.id.to_string(),
                tipo_marcacao(m.tipo).into(),
                estado_marcacao(m.estado).into(),
                escopo(m.escopo).into(),
                m.alvo.clone(),
                m.hash_na_marcacao.as_ref().map_or("-".into(), hash_curto),
                match m.autor {
                    dag_core::Autor::Gui => "gui",
                    dag_core::Autor::Cli => "cli",
                    dag_core::Autor::Mcp => "mcp",
                }
                .into(),
                data_hora_utc(m.criada_em),
            ]
        })
        .collect();
    tabela(
        &[
            "ID",
            "TIPO",
            "ESTADO",
            "ESCOPO",
            "ALVO",
            "HASH",
            "AUTOR",
            "CRIADA (UTC)",
        ],
        &[true, false, false, false, false, false, false, false],
        &linhas,
    )
}

pub fn relatorio(d: &DadosRelatorio, proximo: Option<&str>) -> String {
    let mut s = match d {
        DadosRelatorio::Cobertura { volumes, pastas } => {
            let mut s = cobertura_volumes(volumes);
            if !pastas.is_empty() {
                s.push_str("\nPastas com arquivos sem Cobertura:\n");
                s.push_str(&cobertura_pastas(pastas));
            }
            s
        }
        DadosRelatorio::SemCobertura { arquivos } => sem_cobertura(arquivos),
        DadosRelatorio::Espaco {
            total,
            itens,
            limitacoes,
        } => espaco(total, itens, limitacoes),
        DadosRelatorio::Marcacoes { marcacoes: m } => marcacoes(m),
    };
    if let Some(c) = proximo {
        s.push_str(&format!("\nHá mais: repita com --cursor {c}\n"));
    }
    s
}

pub fn exportado(
    tipo: TipoRelatorio,
    caminho: &str,
    formato: FormatoExportacao,
    linhas: u64,
) -> String {
    let nome = match tipo {
        TipoRelatorio::Cobertura => "cobertura",
        TipoRelatorio::SemCobertura => "sem-cobertura",
        TipoRelatorio::Espaco => "espaço",
        TipoRelatorio::Marcacoes => "marcações",
    };
    format!(
        "Relatório de {nome} gravado em {caminho} ({} linha(s), {}).\n",
        inteiro_pt_br(linhas),
        match formato {
            FormatoExportacao::Csv => "CSV",
            FormatoExportacao::Json => "JSON",
        }
    )
}

pub fn verificacao(r: &ResumoVerificacao) -> String {
    let mut s = format!(
        "Verificação de Integridade {}: {} arquivo(s) conferido(s) ({}), {} divergente(s), {} alterado(s) desde o hash (pulados), {} erro(s).\n",
        match r.desfecho.as_str() {
            "concluida" => "concluída",
            "concluida_com_erros" => "concluída com erros",
            _ => "cancelada",
        },
        inteiro_pt_br(r.verificados),
        bytes_humanos(r.bytes_lidos),
        inteiro_pt_br(r.divergentes),
        inteiro_pt_br(r.alterados),
        inteiro_pt_br(r.erros),
    );
    if r.divergentes > 0 {
        s.push_str(&format!(
            "⚠ {} arquivo(s) com conteúdo diferente do hash guardado (possível corrupção silenciosa); nada foi alterado: ids {:?}\n",
            r.divergentes,
            r.arquivos_divergentes.iter().map(|a| a.0).collect::<Vec<_>>()
        ));
    }
    s
}

pub fn explorar(
    pasta: Option<&str>,
    entradas: &[EntradaExploracao],
    proximo: Option<&str>,
) -> String {
    let mut s = match pasta {
        Some(p) => format!("{p}\n"),
        None => "Raízes catalogadas\n".to_string(),
    };
    if entradas.is_empty() {
        s.push_str("(vazia)\n");
        return s;
    }
    let linhas: Vec<Vec<String>> = entradas
        .iter()
        .map(|e| {
            let eh_pasta = e.tipo == TipoEntradaExploracao::Pasta;
            let nome = if eh_pasta {
                format!("{}/", e.nome.trim_end_matches('/'))
            } else {
                e.nome.clone()
            };
            let tipo = if eh_pasta {
                "pasta".to_string()
            } else {
                e.kind.clone().unwrap_or_else(|| "arquivo".into())
            };
            let itens = e.arquivos.map_or("-".into(), inteiro_pt_br);
            let detalhe = match (&e.categoria, &e.mime) {
                (Some(c), Some(m)) => format!("{c} ({m})"),
                (None, Some(m)) => m.clone(),
                _ => "-".into(),
            };
            vec![
                tipo,
                nome,
                bytes_humanos(e.bytes_logicos),
                e.bytes_alocados.map_or("-".into(), bytes_humanos),
                itens,
                finalidade(e.finalidade),
                detalhe,
            ]
        })
        .collect();
    s.push_str(&tabela(
        &[
            "TIPO",
            "NOME",
            "LÓGICO",
            "ALOCADO",
            "ARQUIVOS",
            "FINALIDADE",
            "FORMATO",
        ],
        &[false, false, true, true, true, false, false],
        &linhas,
    ));
    if let Some(c) = proximo {
        s.push_str(&format!("\nHá mais: repita com --cursor {c}\n"));
    }
    s
}

/// Tabela dos trabalhos do agente (qualquer cliente que os tenha iniciado), seguida do painel de
/// cada trabalho em execução: o que faz, onde roda, totais, threads e andamento por disco.
pub fn jobs(jobs: &[dag_core::JobInfo]) -> String {
    jobs_com_catalogo(jobs, None, Some(agora_s()))
}

pub fn agora_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Como [`jobs`], com os totais do Catálogo no painel e o relógio (segundos Unix) injetado.
pub fn jobs_com_catalogo(
    jobs: &[dag_core::JobInfo],
    catalogo: Option<dag_core::painel::TotaisCatalogoPainel>,
    agora: Option<i64>,
) -> String {
    if jobs.is_empty() {
        return "Trabalhos do agente: nenhum\n".into();
    }
    let linhas: Vec<Vec<String>> = jobs
        .iter()
        .map(|j| {
            vec![
                j.id.to_string(),
                j.tipo.rotulo().to_string(),
                j.origem.rotulo().to_string(),
                j.estado.rotulo().to_string(),
                j.execucao.map_or_else(|| "-".into(), |e| e.to_string()),
                j.resumo_progresso(),
            ]
        })
        .collect();
    let mut s = format!(
        "Trabalhos do agente\n{}",
        tabela(
            &["JOB", "TIPO", "ORIGEM", "ESTADO", "EXECUÇÃO", "ANDAMENTO"],
            &[true, false, false, false, true, false],
            &linhas,
        )
    );
    for j in jobs
        .iter()
        .filter(|j| j.tipo != dag_core::TipoJob::Monitor)
        .filter(|j| j.estado != dag_core::EstadoJob::Enfileirado)
    {
        s.push_str(&format!("\n[job {}]\n", j.id));
        for l in j.painel(j.onde_no_agente(), agora, catalogo).linhas() {
            s.push_str(&l);
            s.push('\n');
        }
    }
    s
}

/// Imprime a resposta em texto: dados em stdout, erros em stderr.
pub fn imprimir(r: &Resposta, todos: bool) {
    match r {
        Resposta::Ok { mensagem } => {
            if let Some(m) = mensagem {
                println!("{m}");
            }
        }
        Resposta::Volumes {
            volumes: v,
            ocultos,
        } => print!("{}", volumes(v, ocultos, todos)),
        Resposta::Status {
            execucoes,
            catalogo,
            monitor,
            confianca,
        } => {
            print!("{}", status(execucoes, catalogo));
            if !confianca.is_empty() {
                print!("\n{}", status_confianca(confianca));
            }
            if let Some(m) = monitor {
                print!("\n{}", saude_monitor(m));
            }
        }
        Resposta::SaudeMonitor { saude } => print!("{}", saude_monitor(saude)),
        Resposta::Jobs { jobs: j } => print!("{}", jobs(j)),
        Resposta::Eventos { eventos: e } => print!("{}", eventos(e)),
        Resposta::MonitorEncerrado { resumo } => print!("{}", monitor_encerrado(resumo)),
        Resposta::ScanConcluido {
            execucao,
            desfecho,
            pastas,
            arquivos,
            especiais,
            bytes_logicos,
            ignorados,
            erros,
            duracao_ms,
            hash,
            duplicados: dup,
            reconciliacao,
        } => print!(
            "{}",
            scan(
                execucao.0,
                desfecho,
                *pastas,
                *arquivos,
                *especiais,
                *bytes_logicos,
                *ignorados,
                *erros,
                *duracao_ms,
                hash.as_ref(),
                dup.as_ref(),
                reconciliacao.as_ref()
            )
        ),
        Resposta::Duplicados {
            grupos,
            total,
            proximo_cursor,
        } => print!("{}", duplicados(grupos, *total, proximo_cursor.as_deref())),
        Resposta::Grupo { detalhe } => print!("{}", grupo(detalhe)),
        Resposta::Marcacao { resultado } => print!("{}", marcacao(resultado)),
        Resposta::Desmarcado { removidas } => print!("{}", desmarcado(removidas)),
        Resposta::Relatorio {
            dados,
            proximo_cursor,
        } => print!("{}", relatorio(dados, proximo_cursor.as_deref())),
        Resposta::RelatorioExportado {
            tipo,
            caminho,
            formato,
            linhas,
        } => print!("{}", exportado(*tipo, caminho, *formato, *linhas)),
        Resposta::Verificacao { resumo } => print!("{}", verificacao(resumo)),
        Resposta::ConfigHash {
            threads_por_disco,
            max_discos,
        } => println!(
            "Hash Completo: {threads_por_disco} thread(s) por disco, até {max_discos} disco(s) em paralelo (vale na próxima execução ou retomada)"
        ),
        Resposta::Explorar {
            pasta,
            entradas,
            proximo_cursor,
        } => print!(
            "{}",
            explorar(pasta.as_deref(), entradas, proximo_cursor.as_deref())
        ),
        Resposta::Erro { mensagem, .. } => eprintln!("erro: {mensagem}"),
        outra => {
            if let Ok(j) = serde_json::to_string_pretty(outra) {
                println!("{j}");
            }
        }
    }
}

#[cfg(test)]
mod testes_volumes {
    use super::*;
    use dag_core::{ClasseVolume, VolumeId};

    fn v(id: i64, mont: &str, classe: ClasseVolume, sistema: bool) -> VolumeInfo {
        VolumeInfo {
            id: VolumeId(id),
            dispositivo: None,
            tipo_dispositivo: None,
            fs_type: Some("ext4".into()),
            rotulo: None,
            ponto_montagem: Some(mont.into()),
            conectado: true,
            capacidade_bytes: Some(100),
            livre_bytes: Some(50),
            classe,
            sistema,
        }
    }

    #[test]
    fn padrao_oculta_com_rodape_e_todos_mostra_classe() {
        let reais = [
            v(1, "/", ClasseVolume::Fisico, false),
            v(2, "/boot/efi", ClasseVolume::Fisico, true),
        ];
        let ocultos = [v(0, "/snap/bare/5", ClasseVolume::Imagem, false)];
        let p = volumes(&reais, &ocultos, false);
        assert!(!p.contains("/snap/bare/5") && !p.contains("CLASSE"), "{p}");
        assert!(
            p.contains("1 volume(s) de imagem/virtuais ocultos (snap, loop...): use --todos"),
            "{p}"
        );
        let t = volumes(&reais, &ocultos, true);
        assert!(t.contains("CLASSE") && t.contains("/snap/bare/5"), "{t}");
        assert!(
            t.contains("físico (sistema)") && t.contains("imagem"),
            "{t}"
        );
        assert!(!t.contains("use --todos"), "{t}");
    }
}

#[cfg(test)]
mod testes_status_hash {
    use super::*;
    use dag_core::{StatusDiscoHash, StatusHash};

    #[test]
    fn status_hash_mostra_totais_e_cada_disco() {
        let h = StatusHash {
            em_andamento: true,
            arquivos_hasheados: 25,
            arquivos_pendentes: 75,
            arquivos_com_erro: 0,
            bytes_hasheados: 1 << 30,
            bytes_pendentes: 3 << 30,
            bytes_por_seg: Some(100 << 20),
            discos: vec![
                StatusDiscoHash {
                    disco: "WDC-1".into(),
                    volumes: vec![1, 2],
                    arquivos_hasheados: 20,
                    arquivos_pendentes: 30,
                    arquivos_com_erro: 0,
                    bytes_hasheados: 1 << 30,
                    bytes_pendentes: 1 << 30,
                },
                StatusDiscoHash {
                    disco: "SEA-2".into(),
                    volumes: vec![3],
                    arquivos_hasheados: 5,
                    arquivos_pendentes: 45,
                    arquivos_com_erro: 0,
                    bytes_hasheados: 0,
                    bytes_pendentes: 2 << 30,
                },
            ],
        };
        let t = status_hash(7, &h);
        assert!(t.contains("etapa de hash (em andamento)"), "{t}");
        assert!(t.contains("25 de 100 arquivos (25%)"), "{t}");
        assert!(t.contains("WDC-1") && t.contains("SEA-2"), "{t}");
        assert!(t.contains("1,2"), "{t}");
        assert!(t.contains("/s"), "{t}");
    }
}

#[cfg(test)]
mod testes_painel {
    use super::*;
    use dag_core::painel::TotaisCatalogoPainel;
    use dag_core::{
        EstadoJob, JobInfo, OrigemJob, PassoAbertura, Progresso, ProgressoDiscoHash, ProgressoHash,
        TipoJob, TipoPasso,
    };

    fn job_de_hash() -> JobInfo {
        let mut j = JobInfo {
            id: 3,
            tipo: TipoJob::Abertura,
            origem: OrigemJob::Abertura,
            raizes: vec!["/dados".into()],
            estado: EstadoJob::Executando,
            execucao: Some(1),
            etapa: None,
            progresso: Vec::new(),
            criado_em: 1_000,
            passo: None,
            agente_pid: Some(4242),
        };
        j.registrar_progresso(&Progresso::Passo(PassoAbertura {
            tipo: TipoPasso::Retomada,
            execucao: Some(1),
            modo: "Full Scan".into(),
            no_inventario: false,
            passada: 0,
            de: 0,
        }));
        j.registrar_progresso(&Progresso::Hash(ProgressoHash {
            arquivos_feitos: 8_000_000,
            arquivos_total: 19_700_000,
            bytes_lidos: 5 << 40,
            bytes_total: 12 << 40,
            bytes_por_seg: 210.0 * 1048576.0,
            reaproveitados: 7_900_000,
            execucao: Some(1),
            threads_por_disco: 1,
            max_discos: 4,
            erros: 3,
            arquivos_por_seg: 205.0,
            janela_ms: 30_000,
            discos: vec![
                ProgressoDiscoHash {
                    disco: "WD Red 4TB".into(),
                    volumes: vec![1, 2],
                    threads: 1,
                    leitores_ativos: 1,
                    arquivos_feitos: 3_000_000,
                    arquivos_total: 9_000_000,
                    bytes_lidos: 2 << 40,
                    bytes_total: 6 << 40,
                    bytes_por_seg: 110.0 * 1048576.0,
                    ativo: true,
                    concluido: false,
                },
                ProgressoDiscoHash {
                    disco: "Seagate Barracuda".into(),
                    volumes: vec![3],
                    threads: 1,
                    leitores_ativos: 0,
                    arquivos_feitos: 40,
                    arquivos_total: 40,
                    bytes_lidos: 1 << 30,
                    bytes_total: 1 << 30,
                    bytes_por_seg: 0.0,
                    ativo: false,
                    concluido: true,
                },
            ],
            ..ProgressoHash::default()
        }));
        j
    }

    #[test]
    fn jobs_mostra_o_painel_do_hash_em_palavras_simples() {
        let t = jobs_com_catalogo(
            &[job_de_hash()],
            Some(TotaisCatalogoPainel {
                arquivos: 19_700_000,
                com_hash: Some(8_100_000),
            }),
            Some(1_000 + 4272),
        );
        println!("{t}");
        for esperado in [
            "Em andamento: Retomada do hash da execução #1 (Full Scan) — há 1h 11min",
            "no agente em segundo plano (pid 4242) — pode fechar a janela",
            "iniciado por: abertura do agente",
            "1 thread(s) por disco, até 4 disco(s) em paralelo",
            "7.999.997 com hash de 19.700.000",
            "pendentes 11.700.000",
            "reaproveitados 7.900.000",
            "erros 3",
            "Bytes lidos nesta etapa: 5,0 TiB de 12,0 TiB",
            "arquivos/min",
            "tempo restante: 9h",
            "WD Red 4TB",
            "Seagate Barracuda",
            "lendo",
            "concluído",
            "Catálogo: 19.700.000 arquivos, 8.100.000 com hash",
        ] {
            assert!(t.contains(esperado), "faltou {esperado:?} em:\n{t}");
        }
        assert!(!t.contains("Encontrados até agora"), "{t}");
        assert!(!t.contains("Descobrindo volumes"), "{t}");
    }

    #[test]
    fn status_mostra_o_catalogo_com_hash() {
        let cat = dag_core::ResumoCatalogo {
            volumes: 4,
            pastas: 10,
            arquivos: 200,
            bytes_logicos: 1 << 30,
            arquivos_com_hash: 50,
        };
        let t = status(&[], &cat);
        assert!(t.contains("200 arquivos (50 com hash, 25,0%)"), "{t}");
    }
}
