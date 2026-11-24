//! Trabalhos (jobs) do agente e o fluxo de eventos que os clientes assinam (ADR 0004).
//!
//! O agente é o único escritor do Catálogo; GUI, CLI e MCP são clientes. Cada trabalho longo
//! (varredura, retomada, verificação, a rotina de abertura, o monitor) é um [`JobInfo`] com id
//! próprio e quem o iniciou ([`OrigemJob`]). Quem assina recebe primeiro um
//! [`EventoAgente::Instantaneo`] com tudo o que está em andamento e depois cada mudança.

use serde::{Deserialize, Serialize};

use crate::formato::{bytes_humanos, inteiro_pt_br};
use crate::{FaseInventario, Progresso, Resposta, SaudeMonitorInfo, StatusConfianca};

/// O que o trabalho faz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoJob {
    FullScan,
    FastScan,
    /// Retomada de execução gravada no Catálogo (`dag scan --retomar`).
    Retomada,
    VerificacaoIntegridade,
    /// Rotina de abertura do agente: retomada automática e passadas de verificação (ADR 0003).
    Abertura,
    /// Monitoramento contínuo das Raízes (cede a qualquer pedido).
    Monitor,
}

impl TipoJob {
    #[must_use]
    pub fn rotulo(self) -> &'static str {
        match self {
            Self::FullScan => "Full Scan",
            Self::FastScan => "Fast Scan",
            Self::Retomada => "Retomada",
            Self::VerificacaoIntegridade => "Verificação de Integridade",
            Self::Abertura => "Abertura",
            Self::Monitor => "Monitor",
        }
    }
}

/// Quem iniciou o trabalho.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrigemJob {
    Gui,
    Cli,
    Mcp,
    /// A rotina de abertura do próprio agente.
    Abertura,
    /// O monitor do próprio agente.
    Monitor,
}

impl OrigemJob {
    #[must_use]
    pub fn rotulo(self) -> &'static str {
        match self {
            Self::Gui => "gui",
            Self::Cli => "cli",
            Self::Mcp => "mcp",
            Self::Abertura => "abertura",
            Self::Monitor => "monitor",
        }
    }
}

impl From<crate::Autor> for OrigemJob {
    fn from(a: crate::Autor) -> Self {
        match a {
            crate::Autor::Gui => Self::Gui,
            crate::Autor::Cli => Self::Cli,
            crate::Autor::Mcp => Self::Mcp,
        }
    }
}

/// Situação do trabalho.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoJob {
    /// Na fila do agente, esperando o trabalho em curso.
    Enfileirado,
    Executando,
    Pausado,
    /// Cancelamento pedido; grava o que já foi lido e termina.
    Cancelando,
}

impl EstadoJob {
    #[must_use]
    pub fn rotulo(self) -> &'static str {
        match self {
            Self::Enfileirado => "na fila",
            Self::Executando => "executando",
            Self::Pausado => "pausado",
            Self::Cancelando => "cancelando",
        }
    }
}

/// Um trabalho do agente.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobInfo {
    /// Id do trabalho, único por execução do agente (não é o id da execução de varredura).
    pub id: u64,
    pub tipo: TipoJob,
    pub origem: OrigemJob,
    pub raizes: Vec<String>,
    pub estado: EstadoJob,
    /// Execução de varredura em curso, quando já conhecida (é o id que `dag pausar` aceita).
    pub execucao: Option<i64>,
    /// Última mudança de etapa em texto (classificação, duplicados, passo da abertura...).
    #[serde(default)]
    pub etapa: Option<String>,
    /// Último progresso de cada etapa (inventário, reconciliação, hash, verificação).
    #[serde(default)]
    pub progresso: Vec<Progresso>,
    /// Segundos Unix em que o trabalho entrou no agente.
    pub criado_em: i64,
}

impl JobInfo {
    /// Guarda `p` no lugar do último evento da mesma etapa.
    pub fn registrar_progresso(&mut self, p: &Progresso) {
        if let Progresso::Mensagem { texto } = p {
            self.etapa = Some(texto.clone());
            return;
        }
        let mesma = |q: &Progresso| std::mem::discriminant(q) == std::mem::discriminant(p);
        match self.progresso.iter_mut().find(|q| mesma(q)) {
            Some(q) => *q = p.clone(),
            None => self.progresso.push(p.clone()),
        }
    }

    /// Resumo curto, em uma linha, do que o trabalho está fazendo (o mais avançado: hash, depois
    /// verificação, depois inventário).
    #[must_use]
    pub fn resumo_progresso(&self) -> String {
        let h = |p: &crate::ProgressoHash, nome: &str| {
            format!(
                "{nome} {}/{} arquivos, {} de {}",
                inteiro_pt_br(p.arquivos_feitos),
                inteiro_pt_br(p.arquivos_total),
                bytes_humanos(p.bytes_lidos),
                bytes_humanos(p.bytes_total)
            )
        };
        let mut hash = None;
        let mut verif = None;
        let mut inv = None;
        for p in &self.progresso {
            match p {
                Progresso::Hash(x) => hash = Some(h(x, "hash")),
                Progresso::Verificacao(x) => verif = Some(h(x, "verificação")),
                Progresso::Inventario(i) | Progresso::Reconciliacao(i) => {
                    let pausa = if i.fase == FaseInventario::Pausado {
                        " (pausado)"
                    } else {
                        ""
                    };
                    inv = Some(format!(
                        "{} pastas, {} arquivos{pausa}",
                        inteiro_pt_br(i.pastas),
                        inteiro_pt_br(i.arquivos)
                    ));
                }
                Progresso::Mensagem { .. } => {}
            }
        }
        hash.or(verif)
            .or(inv)
            .or_else(|| self.etapa.clone())
            .unwrap_or_default()
    }

    /// Execuções que o usuário vê (varreduras e retomadas); fora o monitor e a abertura.
    #[must_use]
    pub fn eh_varredura(&self) -> bool {
        !matches!(self.tipo, TipoJob::Monitor | TipoJob::Abertura)
    }
}

/// Evento do fluxo assinado (`Assinar`). Os clientes mantêm o mapa de trabalhos aplicando-os na
/// ordem em que chegam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "evento", rename_all = "snake_case")]
pub enum EventoAgente {
    /// Primeiro quadro de toda assinatura: tudo o que existe agora. Substitui o estado local.
    Instantaneo {
        jobs: Vec<JobInfo>,
        #[serde(default)]
        monitor: Option<SaudeMonitorInfo>,
        #[serde(default)]
        confianca: Vec<StatusConfianca>,
    },
    /// Um trabalho entrou na fila do agente.
    JobEnfileirado { job: JobInfo },
    /// O trabalho começou a executar.
    JobIniciado { job: JobInfo },
    /// Mudou o estado (pausa, retomada, cancelamento), a execução ou a etapa.
    JobAtualizado { job: JobInfo },
    /// Progresso (inventário, hash por disco, verificação, passo da abertura).
    JobProgresso { job: u64, progresso: Progresso },
    /// O trabalho terminou (ou foi cancelado). `resposta` é a resposta que o pedido recebeu.
    JobFinalizado {
        job: JobInfo,
        desfecho: String,
        resposta: Box<Resposta>,
    },
    /// Saúde do monitor mudou (lacunas, overflow, backlog, estado).
    Monitor { saude: SaudeMonitorInfo },
    /// Mudou o Ponto de Confiança de alguma Raiz.
    PontoConfianca { confianca: Vec<StatusConfianca> },
    /// Sinal de vida, para o cliente detectar um agente travado.
    Batida,
}

/// Mapa de trabalhos mantido por um cliente a partir dos eventos.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VisaoJobs {
    pub jobs: Vec<JobInfo>,
    pub monitor: Option<SaudeMonitorInfo>,
    pub confianca: Vec<StatusConfianca>,
}

impl VisaoJobs {
    /// Aplica um evento; devolve o trabalho que terminou, se foi o caso.
    pub fn aplicar(&mut self, e: &EventoAgente) -> Option<JobInfo> {
        match e {
            EventoAgente::Instantaneo {
                jobs,
                monitor,
                confianca,
            } => {
                self.jobs.clone_from(jobs);
                self.monitor.clone_from(monitor);
                self.confianca.clone_from(confianca);
            }
            EventoAgente::JobEnfileirado { job }
            | EventoAgente::JobIniciado { job }
            | EventoAgente::JobAtualizado { job } => self.guardar(job),
            EventoAgente::JobProgresso { job, progresso } => {
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == *job) {
                    j.registrar_progresso(progresso);
                }
            }
            EventoAgente::JobFinalizado { job, .. } => {
                self.jobs.retain(|j| j.id != job.id);
                return Some(job.clone());
            }
            EventoAgente::Monitor { saude } => self.monitor = Some(saude.clone()),
            EventoAgente::PontoConfianca { confianca } => self.confianca.clone_from(confianca),
            EventoAgente::Batida => {}
        }
        None
    }

    fn guardar(&mut self, job: &JobInfo) {
        match self.jobs.iter_mut().find(|j| j.id == job.id) {
            Some(j) => *j = job.clone(),
            None => self.jobs.push(job.clone()),
        }
    }

    #[must_use]
    pub fn job(&self, id: u64) -> Option<&JobInfo> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// A varredura que o usuário acompanha: ver [`job_em_foco`].
    #[must_use]
    pub fn em_foco(&self) -> Option<&JobInfo> {
        job_em_foco(&self.jobs)
    }
}

/// O trabalho que a interface acompanha: o primeiro que não é o monitor, com preferência pelo que
/// já saiu da fila (o agente executa um por vez).
#[must_use]
pub fn job_em_foco(jobs: &[JobInfo]) -> Option<&JobInfo> {
    let candidatos = || jobs.iter().filter(|j| j.tipo != TipoJob::Monitor);
    candidatos()
        .find(|j| j.estado != EstadoJob::Enfileirado)
        .or_else(|| candidatos().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecucaoId, FaseInventario, ProgressoInventario};

    fn job(id: u64, estado: EstadoJob) -> JobInfo {
        JobInfo {
            id,
            tipo: TipoJob::FullScan,
            origem: OrigemJob::Cli,
            raizes: vec!["/x".into()],
            estado,
            execucao: None,
            etapa: None,
            progresso: Vec::new(),
            criado_em: 1,
        }
    }

    fn inv(arquivos: u64) -> Progresso {
        Progresso::Inventario(ProgressoInventario {
            execucao: ExecucaoId(1),
            fase: FaseInventario::Inventario,
            pastas: 1,
            arquivos,
            especiais: 0,
            bytes_logicos: 0,
            ignorados: 0,
            erros: 0,
            diretorios_pendentes: 0,
            decorrido_ms: 0,
            entradas_por_segundo: 0.0,
        })
    }

    #[test]
    fn visao_aplica_os_eventos_na_ordem() {
        let mut v = VisaoJobs::default();
        v.aplicar(&EventoAgente::Instantaneo {
            jobs: vec![job(1, EstadoJob::Executando)],
            monitor: None,
            confianca: Vec::new(),
        });
        v.aplicar(&EventoAgente::JobEnfileirado {
            job: job(2, EstadoJob::Enfileirado),
        });
        v.aplicar(&EventoAgente::JobProgresso {
            job: 1,
            progresso: inv(3),
        });
        v.aplicar(&EventoAgente::JobProgresso {
            job: 1,
            progresso: inv(9),
        });
        v.aplicar(&EventoAgente::JobProgresso {
            job: 1,
            progresso: Progresso::Mensagem { texto: "x".into() },
        });
        let j = v.job(1).unwrap();
        assert_eq!(j.progresso.len(), 1, "só o último de cada etapa");
        assert_eq!(j.etapa.as_deref(), Some("x"));
        assert_eq!(v.em_foco().unwrap().id, 1);
        let fim = v.aplicar(&EventoAgente::JobFinalizado {
            job: job(1, EstadoJob::Executando),
            desfecho: "concluida".into(),
            resposta: Box::new(Resposta::Ok { mensagem: None }),
        });
        assert_eq!(fim.unwrap().id, 1);
        assert_eq!(v.jobs.len(), 1);
        assert_eq!(v.em_foco().unwrap().id, 2);
    }

    #[test]
    fn eventos_serializam_com_nome_estavel() {
        let e = EventoAgente::JobIniciado {
            job: job(7, EstadoJob::Executando),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["evento"], "job_iniciado");
        assert_eq!(v["job"]["origem"], "cli");
        assert_eq!(serde_json::from_value::<EventoAgente>(v).unwrap(), e);
    }
}
