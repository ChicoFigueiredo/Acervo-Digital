//! `dag status --seguir` / `dag acompanhar`: acompanha o fluxo de eventos do agente até Ctrl-C.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dag_agent::{
    Cliente, EventoObservado, ExibidorStderr, Observador, OpcoesObservador, caminho_socket_padrao,
};
use dag_core::formato::data_hora_utc;
use dag_core::{EventoAgente, JobInfo, SaudeMonitorInfo, StatusConfianca};

static PARAR: AtomicBool = AtomicBool::new(false);

fn rotulo_job(j: &JobInfo) -> String {
    let exec = j
        .execucao
        .map_or(String::new(), |e| format!(", execução {e}"));
    format!(
        "job {} ({}, {}{exec})",
        j.id,
        j.tipo.rotulo(),
        j.origem.rotulo()
    )
}

fn hora() -> String {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    // `data_hora_utc` devolve "AAAA-MM-DD HH:MM:SS"; só a hora interessa na linha.
    data_hora_utc(s)
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string()
}

/// O que mudou no monitor e vale uma linha (backlog e contadores mudam o tempo todo).
fn chave_monitor(m: &SaudeMonitorInfo) -> (String, bool, u64, u64, Option<String>) {
    (
        m.situacao_texto(),
        m.em_execucao,
        m.lacunas_abertas,
        m.overflows,
        m.erro.clone(),
    )
}

fn linha_monitor(m: &SaudeMonitorInfo) -> String {
    format!(
        "monitor: {}{}, lacunas abertas {}, overflows {}, backlog {}{}",
        m.situacao_texto(),
        if m.em_execucao { " (vivo)" } else { "" },
        m.lacunas_abertas,
        m.overflows,
        m.backlog,
        m.erro
            .as_ref()
            .map_or(String::new(), |e| format!(", erro: {e}"))
    )
}

fn linha_confianca(c: &StatusConfianca) -> String {
    match (c.confiavel_em, &c.motivo_perda) {
        (Some(_), _) => format!("ponto de confiança: {} confiável", c.raiz),
        (None, Some(m)) => format!("ponto de confiança: {} perdido ({m})", c.raiz),
        (None, None) => format!("ponto de confiança: {} ainda não confiável", c.raiz),
    }
}

/// Acompanha o agente. Sai com 1 se não há agente; Ctrl-C encerra com 0.
pub fn seguir(catalogo: &Path, socket: Option<&Path>, json: bool) -> u8 {
    let socket = socket.map_or_else(caminho_socket_padrao, Path::to_path_buf);
    if Cliente::conectar_se_serve(&socket, catalogo).is_none() {
        eprintln!(
            "erro: nenhum agente do DAG está em execução para este catálogo (suba com `dag agente` ou inicie uma varredura)"
        );
        return 1;
    }
    if let Err(e) = ctrlc::set_handler(|| PARAR.store(true, Ordering::SeqCst)) {
        eprintln!("aviso: Ctrl-C não tratado ({e})");
    }
    let obs = Observador::iniciar(OpcoesObservador::novo(catalogo.to_path_buf(), socket), None);
    let mut exibidor = ExibidorStderr::novo();
    let mut visao = dag_core::VisaoJobs::default();
    let mut ultimo_monitor = None;
    while !PARAR.load(Ordering::SeqCst) {
        let Some(ev) = obs.proximo(Duration::from_millis(200)) else {
            continue;
        };
        if json {
            let v = match &ev {
                EventoObservado::Conectado(i) => {
                    serde_json::json!({"evento": "conectado", "agente": i})
                }
                EventoObservado::Desconectado { motivo } => {
                    serde_json::json!({"evento": "desconectado", "motivo": motivo})
                }
                EventoObservado::Evento(e) => serde_json::to_value(e).unwrap_or_default(),
            };
            println!("{v}");
            if let EventoObservado::Evento(e) = &ev {
                visao.aplicar(e);
            }
            continue;
        }
        match ev {
            EventoObservado::Conectado(i) => {
                println!("[{}] conectado ao agente (pid {})", hora(), i.pid);
            }
            EventoObservado::Desconectado { motivo } => {
                println!("[{}] agente desconectado, reconectando ({motivo})", hora());
            }
            EventoObservado::Evento(e) => {
                let fim = visao.aplicar(&e);
                mostrar(&e, fim.as_ref(), &visao, &mut exibidor, &mut ultimo_monitor);
            }
        }
    }
    obs.parar();
    0
}

fn mostrar(
    e: &EventoAgente,
    fim: Option<&JobInfo>,
    visao: &dag_core::VisaoJobs,
    exibidor: &mut ExibidorStderr,
    ultimo_monitor: &mut Option<(String, bool, u64, u64, Option<String>)>,
) {
    let h = hora();
    match e {
        EventoAgente::Instantaneo {
            jobs,
            monitor,
            confianca,
        } => {
            print!("[{h}] ");
            print!("{}", crate::saida::jobs(jobs));
            if let Some(m) = monitor {
                println!("[{h}] {}", linha_monitor(m));
                *ultimo_monitor = Some(chave_monitor(m));
            }
            for c in confianca {
                println!("[{h}] {}", linha_confianca(c));
            }
        }
        EventoAgente::JobEnfileirado { job } => {
            println!("[{h}] {} entrou na fila", rotulo_job(job));
        }
        EventoAgente::JobIniciado { job } => println!("[{h}] {} iniciado", rotulo_job(job)),
        EventoAgente::JobAtualizado { job } => {
            println!("[{h}] {}: {}", rotulo_job(job), job.estado.rotulo());
        }
        EventoAgente::JobProgresso {
            job,
            progresso: dag_core::Progresso::Passo(_),
        } => {
            // Mudou o passo da abertura: diz em palavras o que roda agora e onde.
            if let Some(j) = visao.job(*job) {
                let p = j.painel(j.onde_no_agente(), None, None);
                println!("[{h}] {} — {}", p.titulo, p.onde.texto());
            }
        }
        EventoAgente::JobProgresso { progresso, .. } => exibidor.mostrar(progresso),
        EventoAgente::JobFinalizado { desfecho, .. } => {
            if let Some(j) = fim {
                println!("[{h}] {} terminou: {desfecho}", rotulo_job(j));
            }
        }
        EventoAgente::Monitor { saude } => {
            let k = chave_monitor(saude);
            if ultimo_monitor.as_ref() != Some(&k) {
                println!("[{h}] {}", linha_monitor(saude));
                *ultimo_monitor = Some(k);
            }
        }
        EventoAgente::PontoConfianca { confianca } => {
            for c in confianca {
                println!("[{h}] {}", linha_confianca(c));
            }
        }
        EventoAgente::Batida => {}
        EventoAgente::AgenteReiniciando { de, para } => {
            println!("[{h}] agente reiniciando no lugar ({de} -> {para}); reconectando...");
        }
    }
}
