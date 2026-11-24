//! Resumo em português do Brasil de uma [`Resposta`].

use dag_core::{Progresso, Resposta};

fn bytes(n: u64) -> String {
    let un = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < un.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", un[i])
    }
}

fn mais(c: &Option<String>) -> &'static str {
    if c.is_some() {
        "; há mais páginas (proximo_cursor)"
    } else {
        ""
    }
}

/// Texto curto para humanos; o dado completo vai em `structuredContent`.
pub fn resumir(r: &Resposta) -> String {
    match r {
        Resposta::Ok { mensagem } => mensagem.clone().unwrap_or_else(|| "Concluído.".into()),
        Resposta::Volumes { volumes, ocultos } => format!(
            "{} Volume(s) real(is); {} de imagem/virtuais ocultos.",
            volumes.len(),
            ocultos.len()
        ),
        Resposta::ExecucaoIniciada { execucao } => {
            format!("Execução {} iniciada.", execucao.0)
        }
        Resposta::Status {
            execucoes,
            catalogo,
            confianca,
            ..
        } => format!(
            "{} execução(ões){}{}{}; Catálogo: {} volume(s), {} pasta(s), {} arquivo(s) ({} com hash), {}.",
            execucoes.len(),
            execucoes
                .iter()
                .filter(|e| e.retomando)
                .map(|e| format!(" [execução {}: interrompida, retomando]", e.id.0))
                .collect::<String>(),
            if confianca.is_empty() {
                String::new()
            } else {
                format!(
                    " [Ponto de Confiança: {} de {} raiz(es)]",
                    confianca
                        .iter()
                        .filter(|c| c.confiavel_em.is_some())
                        .count(),
                    confianca.len()
                )
            },
            execucoes
                .iter()
                .filter_map(|e| e.hash.as_ref().map(|h| (e.id.0, h)))
                .map(|(id, h)| format!(
                    " [execução {id}: hash {} de {} arquivos, {} disco(s)]",
                    h.arquivos_hasheados,
                    h.arquivos_hasheados + h.arquivos_pendentes + h.arquivos_com_erro,
                    h.discos.len()
                ))
                .collect::<String>(),
            catalogo.volumes,
            catalogo.pastas,
            catalogo.arquivos,
            catalogo.arquivos_com_hash,
            bytes(catalogo.bytes_logicos)
        ),
        Resposta::Jobs { jobs } => {
            if jobs.is_empty() {
                "Nenhum trabalho no agente.".into()
            } else {
                let itens: Vec<String> = jobs
                    .iter()
                    .map(|j| {
                        let p = j.painel(j.onde_no_agente(), None, None);
                        let andamento = p.resumo();
                        format!(
                            "#{} {} (iniciado por {}, {}{}) {}{}",
                            j.id,
                            p.titulo,
                            j.origem.rotulo(),
                            j.estado.rotulo(),
                            j.execucao
                                .map_or(String::new(), |e| format!(", execução {e}")),
                            p.onde.texto(),
                            if andamento.is_empty() {
                                String::new()
                            } else {
                                format!(": {andamento}")
                            }
                        )
                    })
                    .collect();
                format!("{} trabalho(s): {}.", jobs.len(), itens.join("; "))
            }
        }
        Resposta::Eventos { eventos } => format!("{} evento(s).", eventos.len()),
        Resposta::SaudeMonitor { .. } => "Saúde do monitor consultada.".into(),
        Resposta::MonitorEncerrado { .. } => "Monitoramento encerrado.".into(),
        Resposta::ScanConcluido {
            execucao,
            desfecho,
            pastas,
            arquivos,
            bytes_logicos,
            erros,
            duracao_ms,
            ..
        } => format!(
            "Varredura {} {desfecho}: {pastas} pasta(s), {arquivos} arquivo(s), {} em {duracao_ms} ms, {erros} erro(s).",
            execucao.0,
            bytes(*bytes_logicos),
        ),
        Resposta::Explorar {
            entradas,
            proximo_cursor,
            ..
        } => format!("{} entrada(s){}.", entradas.len(), mais(proximo_cursor)),
        Resposta::Duplicados {
            grupos,
            total,
            proximo_cursor,
        } => format!(
            "{} grupo(s) de duplicados nesta página, {total} no total{}.",
            grupos.len(),
            mais(proximo_cursor)
        ),
        Resposta::Grupo { .. } => "Detalhe do grupo de duplicados.".into(),
        Resposta::Marcacao { .. } => {
            "Marcação registrada no Catálogo (nada foi apagado nem movido no disco).".into()
        }
        Resposta::Desmarcado { removidas } => {
            format!("{} marcação(ões) removida(s) do Catálogo.", removidas.len())
        }
        Resposta::Relatorio { .. } => "Página de relatório.".into(),
        Resposta::RelatorioExportado {
            caminho, linhas, ..
        } => format!("Relatório gravado em {caminho} ({linhas} linha(s))."),
        Resposta::ConfigHash {
            threads_por_disco,
            max_discos,
        } => format!(
            "Hash Completo: {threads_por_disco} thread(s) por disco, até {max_discos} disco(s) em paralelo."
        ),
        Resposta::Verificacao { .. } => "Verificação de Integridade concluída.".into(),
        Resposta::Erro { codigo, mensagem } => format!("Erro ({codigo}): {mensagem}"),
    }
}

/// Linha curta para `notifications/progress`.
pub fn texto_progresso(p: &Progresso) -> String {
    match p {
        Progresso::Inventario(i) | Progresso::Reconciliacao(i) => format!(
            "inventário: {} pasta(s), {} arquivo(s), {}, {} erro(s)",
            i.pastas,
            i.arquivos,
            bytes(i.bytes_logicos),
            i.erros
        ),
        Progresso::Hash(h) | Progresso::Verificacao(h) if h.preparando => {
            "hash: montando a fila de hash…".into()
        }
        Progresso::Hash(h) | Progresso::Verificacao(h) => {
            let eta = dag_core::painel::eta_s(
                h.bytes_total.saturating_sub(h.bytes_lidos),
                h.bytes_por_seg,
                h.janela_ms,
            );
            format!(
                "hash: {}/{} arquivo(s) ({}), {}/{}, {}/s, {} thread(s) por disco em até {} disco(s), {} disco(s) listados, tempo restante {}",
                h.arquivos_feitos,
                h.arquivos_total,
                dag_core::painel::pct_texto(dag_core::painel::percentual(
                    h.arquivos_feitos,
                    h.arquivos_total
                )),
                bytes(h.bytes_lidos),
                bytes(h.bytes_total),
                bytes(h.bytes_por_seg as u64),
                h.threads_por_disco,
                h.max_discos,
                h.discos.len(),
                dag_core::painel::eta_texto(eta)
            )
        }
        Progresso::Mensagem { texto } => texto.clone(),
        Progresso::Passo(p) => match p.tipo {
            dag_core::TipoPasso::Retomada => format!(
                "abertura: retomando a execução #{} ({})",
                p.execucao.unwrap_or_default(),
                p.modo
            ),
            dag_core::TipoPasso::Verificacao => format!(
                "abertura: Fast Scan de verificação, passada {} de {}",
                p.passada, p.de
            ),
        },
    }
}
