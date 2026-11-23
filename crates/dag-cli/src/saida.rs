//! Saída legível (pt-BR) das respostas: tabelas com tamanhos em unidades humanas.

use dag_core::formato::{bytes_humanos, data_hora_utc, inteiro_pt_br};
use dag_core::{
    EntradaExploracao, EstadoExecucao, Finalidade, Resposta, StatusExecucao, TipoEntradaExploracao,
    VolumeInfo,
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

pub fn volumes(v: &[VolumeInfo]) -> String {
    if v.is_empty() {
        return "Nenhum volume.\n".into();
    }
    let linhas: Vec<Vec<String>> = v
        .iter()
        .map(|v| {
            let uso = match (v.capacidade_bytes, v.livre_bytes) {
                (Some(c), Some(l)) if c > 0 => {
                    format!("{:.0}%", (c.saturating_sub(l)) as f64 * 100.0 / c as f64)
                }
                _ => "-".into(),
            };
            vec![
                v.id.to_string(),
                opt(v.ponto_montagem.as_deref()),
                opt(v.fs_type.as_deref()),
                opt(v.rotulo.as_deref()),
                v.capacidade_bytes.map_or("-".into(), bytes_humanos),
                v.livre_bytes.map_or("-".into(), bytes_humanos),
                uso,
                if v.conectado { "conectado" } else { "offline" }.into(),
            ]
        })
        .collect();
    tabela(
        &[
            "ID",
            "MONTAGEM",
            "FS",
            "RÓTULO",
            "CAPACIDADE",
            "LIVRE",
            "USO",
            "ESTADO",
        ],
        &[true, false, false, false, true, true, true, false],
        &linhas,
    )
}

fn estado(e: &StatusExecucao) -> String {
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
        "Catálogo: {} volume(s), {} pastas, {} arquivos, {} (tamanho lógico)\n",
        inteiro_pt_br(cat.volumes),
        inteiro_pt_br(cat.pastas),
        inteiro_pt_br(cat.arquivos),
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
    s
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
    if desfecho == "cancelada" {
        s.push_str(&format!("Retome com: dag scan --retomar {execucao}\n"));
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

/// Imprime a resposta em texto: dados em stdout, erros em stderr.
pub fn imprimir(r: &Resposta) {
    match r {
        Resposta::Ok { mensagem } => {
            if let Some(m) = mensagem {
                println!("{m}");
            }
        }
        Resposta::Volumes { volumes: v } => print!("{}", volumes(v)),
        Resposta::Status {
            execucoes,
            catalogo,
        } => print!("{}", status(execucoes, catalogo)),
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
                *duracao_ms
            )
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
