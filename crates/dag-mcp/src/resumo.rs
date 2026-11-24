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
        Resposta::Volumes { volumes } => format!("{} Volume(s) conhecido(s).", volumes.len()),
        Resposta::ExecucaoIniciada { execucao } => {
            format!("Execução {} iniciada.", execucao.0)
        }
        Resposta::Status {
            execucoes,
            catalogo,
            ..
        } => format!(
            "{} execução(ões); Catálogo: {} volume(s), {} pasta(s), {} arquivo(s), {}.",
            execucoes.len(),
            catalogo.volumes,
            catalogo.pastas,
            catalogo.arquivos,
            bytes(catalogo.bytes_logicos)
        ),
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
        Progresso::Hash(h) | Progresso::Verificacao(h) => format!(
            "hash: {}/{} arquivo(s), {}/{}",
            h.arquivos_feitos,
            h.arquivos_total,
            bytes(h.bytes_lidos),
            bytes(h.bytes_total)
        ),
        Progresso::Mensagem { texto } => texto.clone(),
    }
}
