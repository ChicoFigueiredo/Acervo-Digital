//! Registro de trabalhos e assinantes do agente (ADR 0004).
//!
//! Todo trabalho longo do agente (varredura, retomada, verificação, a abertura, o monitor) passa
//! por aqui, e cada mudança vira um [`EventoAgente`] entregue a quem assinou. O instantâneo e o
//! cadastro do assinante acontecem sob a mesma trava que a emissão: ninguém perde um evento entre
//! o instantâneo e o primeiro evento seguinte.

use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Mutex, MutexGuard};

use dag_core::{
    EstadoJob, EventoAgente, JobInfo, OrigemJob, Progresso, Resposta, SaudeMonitorInfo,
    StatusConfianca, TipoJob,
};

use crate::agente::agora;

/// Eventos que um assinante pode ter na fila antes de ser considerado lento e desligado (ele se
/// reconecta e recebe um instantâneo novo).
const FILA_POR_ASSINANTE: usize = 4096;

#[derive(Debug, Default)]
struct Interno {
    proximo: u64,
    jobs: Vec<JobInfo>,
    assinantes: Vec<SyncSender<EventoAgente>>,
    monitor: Option<SaudeMonitorInfo>,
    confianca: Vec<StatusConfianca>,
}

#[derive(Debug, Default)]
pub(crate) struct Registro {
    interno: Mutex<Interno>,
}

fn travar(m: &Mutex<Interno>) -> MutexGuard<'_, Interno> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Interno {
    fn emitir(&mut self, e: &EventoAgente) {
        self.assinantes.retain(|tx| match tx.try_send(e.clone()) {
            Ok(()) => true,
            // Lento demais ou desconectado: sai da lista (a conexão dele fecha e ele refaz a
            // assinatura com um instantâneo).
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        });
    }

    fn achar(&mut self, id: u64) -> Option<&mut JobInfo> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }
}

impl Registro {
    /// Cadastra um trabalho (na fila ou já executando) e avisa os assinantes.
    pub(crate) fn novo_job(
        &self,
        tipo: TipoJob,
        origem: OrigemJob,
        raizes: Vec<String>,
        execucao: Option<i64>,
        estado: EstadoJob,
    ) -> u64 {
        let mut g = travar(&self.interno);
        g.proximo += 1;
        let job = JobInfo {
            id: g.proximo,
            tipo,
            origem,
            raizes,
            estado,
            execucao,
            etapa: None,
            progresso: Vec::new(),
            criado_em: agora(),
            passo: None,
            agente_pid: Some(std::process::id()),
        };
        let id = job.id;
        g.jobs.push(job.clone());
        g.emitir(&if estado == EstadoJob::Enfileirado {
            EventoAgente::JobEnfileirado { job }
        } else {
            EventoAgente::JobIniciado { job }
        });
        id
    }

    /// Sai da fila e começa a executar.
    pub(crate) fn iniciar(&self, id: u64) {
        let mut g = travar(&self.interno);
        if let Some(j) = g.achar(id)
            && j.estado == EstadoJob::Enfileirado
        {
            j.estado = EstadoJob::Executando;
            let job = j.clone();
            g.emitir(&EventoAgente::JobIniciado { job });
        }
    }

    pub(crate) fn definir_estado(&self, id: u64, estado: EstadoJob) {
        let mut g = travar(&self.interno);
        if let Some(j) = g.achar(id)
            && j.estado != estado
        {
            j.estado = estado;
            let job = j.clone();
            g.emitir(&EventoAgente::JobAtualizado { job });
        }
    }

    pub(crate) fn definir_execucao(&self, id: u64, execucao: i64) {
        let mut g = travar(&self.interno);
        if let Some(j) = g.achar(id)
            && j.execucao != Some(execucao)
        {
            j.execucao = Some(execucao);
            let job = j.clone();
            g.emitir(&EventoAgente::JobAtualizado { job });
        }
    }

    pub(crate) fn progresso(&self, id: u64, p: &Progresso) {
        let mut g = travar(&self.interno);
        let Some(j) = g.achar(id) else { return };
        j.registrar_progresso(p);
        g.emitir(&EventoAgente::JobProgresso {
            job: id,
            progresso: p.clone(),
        });
    }

    /// O trabalho terminou: sai da lista e os assinantes recebem o desfecho.
    pub(crate) fn finalizar(&self, id: u64, resposta: &Resposta) {
        let mut g = travar(&self.interno);
        let Some(pos) = g.jobs.iter().position(|j| j.id == id) else {
            return;
        };
        let job = g.jobs.remove(pos);
        g.emitir(&EventoAgente::JobFinalizado {
            job,
            desfecho: desfecho_de(resposta),
            resposta: Box::new(resposta.clone()),
        });
    }

    #[must_use]
    pub(crate) fn jobs(&self) -> Vec<JobInfo> {
        travar(&self.interno).jobs.clone()
    }

    #[must_use]
    pub(crate) fn job(&self, id: u64) -> Option<JobInfo> {
        travar(&self.interno)
            .jobs
            .iter()
            .find(|j| j.id == id)
            .cloned()
    }

    #[must_use]
    pub(crate) fn tem_assinantes(&self) -> bool {
        !travar(&self.interno).assinantes.is_empty()
    }

    /// Cadastra um assinante e devolve o instantâneo (o primeiro evento dele) e o canal dos
    /// seguintes. `monitor` e `confianca` são a leitura mais fresca do Catálogo.
    pub(crate) fn assinar(
        &self,
        monitor: Option<SaudeMonitorInfo>,
        confianca: Vec<StatusConfianca>,
    ) -> (EventoAgente, Receiver<EventoAgente>) {
        let (tx, rx) = sync_channel(FILA_POR_ASSINANTE);
        let mut g = travar(&self.interno);
        if monitor.is_some() {
            g.monitor.clone_from(&monitor);
        }
        g.confianca.clone_from(&confianca);
        g.assinantes.push(tx);
        let instantaneo = EventoAgente::Instantaneo {
            jobs: g.jobs.clone(),
            monitor: g.monitor.clone(),
            confianca: g.confianca.clone(),
        };
        (instantaneo, rx)
    }

    /// Compara a leitura do monitor e do Ponto de Confiança com a última e avisa só o que mudou.
    pub(crate) fn observar_monitor(
        &self,
        saude: SaudeMonitorInfo,
        confianca: Vec<StatusConfianca>,
    ) {
        let mut g = travar(&self.interno);
        let mudou_saude = g.monitor.as_ref().is_none_or(|m| !mesma_saude(m, &saude));
        if mudou_saude {
            g.monitor = Some(saude.clone());
            g.emitir(&EventoAgente::Monitor { saude });
        }
        if g.confianca != confianca {
            g.confianca.clone_from(&confianca);
            g.emitir(&EventoAgente::PontoConfianca { confianca });
        }
    }

    /// Avisa os assinantes que o agente vai se reexecutar.
    pub(crate) fn reiniciando(&self, de: String, para: String) {
        travar(&self.interno).emitir(&EventoAgente::AgenteReiniciando { de, para });
    }

    pub(crate) fn batida(&self) {
        travar(&self.interno).emitir(&EventoAgente::Batida);
    }
}

/// O heartbeat muda a cada poucos segundos sem significar nada para quem acompanha.
fn mesma_saude(a: &SaudeMonitorInfo, b: &SaudeMonitorInfo) -> bool {
    let mut a = a.clone();
    a.heartbeat = b.heartbeat;
    a == *b
}

/// Resumo do desfecho de um trabalho para quem só olha o evento final.
#[must_use]
pub(crate) fn desfecho_de(r: &Resposta) -> String {
    match r {
        Resposta::ScanConcluido { desfecho, .. } => desfecho.clone(),
        Resposta::Erro { codigo, .. } => format!("erro:{codigo}"),
        _ => "concluida".into(),
    }
}
