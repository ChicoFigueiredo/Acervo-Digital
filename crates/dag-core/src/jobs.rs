//! Trabalhos (jobs) do agente e o fluxo de eventos que os clientes assinam (ADR 0004).
//!
//! O agente é o único escritor do Catálogo; GUI, CLI e MCP são clientes. Cada trabalho longo
//! (varredura, retomada, verificação, a rotina de abertura, o monitor) é um [`JobInfo`] com id
//! próprio e quem o iniciou ([`OrigemJob`]). Quem assina recebe primeiro um
//! [`EventoAgente::Instantaneo`] com tudo o que está em andamento e depois cada mudança.

use serde::{Deserialize, Serialize};

use crate::painel::{ContagemInventario, EntradaPainel, Onde, PainelJob};
use crate::{
    PassoAbertura, Progresso, ProgressoHash, ProgressoInventario, Resposta, SaudeMonitorInfo,
    StatusConfianca,
};

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
    /// Reconstrução dos Grupos de Duplicados (`dag duplicados reconstruir`).
    Duplicados,
    /// Conteúdo de Contêineres (BLAKE3 dos Membros; ADR 0006).
    Conteineres,
    /// Curadoria bibliográfica (fila de livros, sincronização, duplicatas).
    Biblioteca,
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
            Self::Duplicados => "Agrupando duplicados",
            Self::Conteineres => "Conteúdo de contêineres",
            Self::Biblioteca => "Curadoria bibliográfica",
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
    /// Passo estruturado da rotina de abertura (retomada, passada de verificação).
    #[serde(default)]
    pub passo: Option<PassoAbertura>,
    /// Pid do processo do agente que executa o trabalho.
    #[serde(default)]
    pub agente_pid: Option<u32>,
}

impl JobInfo {
    /// Guarda `p` como o último evento da sua etapa (a mais recente fica por último). Um passo
    /// novo da abertura zera o andamento dos passos anteriores.
    pub fn registrar_progresso(&mut self, p: &Progresso) {
        match p {
            Progresso::Mensagem { texto } => {
                self.etapa = Some(texto.clone());
                return;
            }
            Progresso::Passo(x) => {
                self.passo = Some(x.clone());
                self.progresso.clear();
                self.encerrar_agrupamento();
                return;
            }
            _ => {}
        }
        // Chegou andamento de inventário, hash ou contêineres: a etapa de agrupamento (da passada
        // anterior) acabou.
        self.encerrar_agrupamento();
        let mesma = |q: &Progresso| std::mem::discriminant(q) == std::mem::discriminant(p);
        self.progresso.retain(|q| !mesma(q));
        self.progresso.push(p.clone());
    }

    /// Esquece a mensagem de etapa de agrupamento de duplicados (nova etapa começou).
    fn encerrar_agrupamento(&mut self) {
        if self
            .etapa
            .as_deref()
            .is_some_and(|t| crate::painel::etapa_de_duplicados(t).is_some())
        {
            self.etapa = None;
        }
    }

    /// Último progresso de hash (ou de verificação) e se é a verificação.
    #[must_use]
    pub fn hash_atual(&self) -> Option<(&ProgressoHash, bool)> {
        self.progresso.iter().rev().find_map(|p| match p {
            Progresso::Hash(h) => Some((h, false)),
            Progresso::Verificacao(h) => Some((h, true)),
            _ => None,
        })
    }

    /// Último progresso da etapa "Conteúdo de contêineres".
    #[must_use]
    pub fn conteineres_atual(&self) -> Option<&crate::ProgressoConteineres> {
        self.progresso.iter().rev().find_map(|p| match p {
            Progresso::Conteineres(c) => Some(c),
            _ => None,
        })
    }

    /// Último progresso de inventário (ou reconciliação) e se é a reconciliação.
    #[must_use]
    pub fn inventario_atual(&self) -> Option<(&ProgressoInventario, bool)> {
        self.progresso.iter().rev().find_map(|p| match p {
            Progresso::Inventario(i) => Some((i, false)),
            Progresso::Reconciliacao(i) => Some((i, true)),
            _ => None,
        })
    }

    /// Painel do trabalho. `agora` são segundos Unix (só para o tempo decorrido; `None` omite).
    #[must_use]
    pub fn painel(
        &self,
        onde: Onde,
        agora: Option<i64>,
        catalogo: Option<crate::painel::TotaisCatalogoPainel>,
    ) -> PainelJob {
        let hash = self.hash_atual();
        let inv = self.inventario_atual();
        let mut e = EntradaPainel::vazia(onde);
        e.tipo = Some(self.tipo);
        e.origem = Some(self.origem);
        e.estado = Some(self.estado);
        e.execucao = self.execucao;
        e.passo = self.passo.as_ref();
        e.etapa_texto = self.etapa.as_deref();
        e.hash = hash.map(|(h, _)| h);
        e.conteineres = self.conteineres_atual();
        e.verificacao = hash.is_some_and(|(_, v)| v);
        e.inventario = inv.map(|(i, _)| ContagemInventario::from(i));
        e.reconciliacao = inv.is_some_and(|(_, r)| r);
        e.decorrido_s = agora.and_then(|a| u64::try_from(a - self.criado_em).ok());
        e.catalogo = catalogo;
        PainelJob::montar(&e)
    }

    /// Onde o trabalho roda, do ponto de vista de um cliente conectado ao agente.
    #[must_use]
    pub fn onde_no_agente(&self) -> Onde {
        Onde::Agente {
            pid: self.agente_pid,
        }
    }

    /// Resumo curto, em uma linha, do que o trabalho está fazendo (o mais avançado: hash, depois
    /// verificação, depois inventário).
    #[must_use]
    pub fn resumo_progresso(&self) -> String {
        self.painel(self.onde_no_agente(), None, None).resumo()
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
    /// O agente vai se reexecutar no lugar (binário atualizado ou pedido do usuário): a conexão
    /// cai em instantes e volta sozinha, já na versão nova. Trabalhos em curso ficam
    /// `interrompida` e a abertura do agente novo os retoma.
    AgenteReiniciando { de: String, para: String },
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
            EventoAgente::Batida | EventoAgente::AgenteReiniciando { .. } => {}
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
            passo: None,
            agente_pid: None,
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
    fn passo_novo_zera_o_andamento_do_passo_anterior() {
        use crate::{PassoAbertura, ProgressoHash, TipoPasso};
        let mut j = job(1, EstadoJob::Executando);
        j.tipo = TipoJob::Abertura;
        j.agente_pid = Some(10);
        let passo = |tipo, passada| {
            Progresso::Passo(PassoAbertura {
                tipo,
                execucao: Some(1),
                modo: "Full Scan".into(),
                no_inventario: false,
                passada,
                de: 3,
            })
        };
        j.registrar_progresso(&passo(TipoPasso::Retomada, 0));
        j.registrar_progresso(&Progresso::Hash(ProgressoHash {
            arquivos_total: 10,
            arquivos_feitos: 5,
            ..ProgressoHash::default()
        }));
        let p = j.painel(j.onde_no_agente(), None, None);
        assert_eq!(p.titulo, "Retomada do hash da execução #1 (Full Scan)");
        j.registrar_progresso(&passo(TipoPasso::Verificacao, 1));
        assert!(j.progresso.is_empty(), "o hash do passo anterior some");
        let p = j.painel(j.onde_no_agente(), None, None);
        assert_eq!(p.titulo, "Fast Scan de verificação, passada 1 de 3");
        assert!(p.hash.is_none());
        // Sem a abertura ter dado nenhum passo, o texto nunca fala de inventário no hash.
        let mut k = job(2, EstadoJob::Executando);
        k.registrar_progresso(&Progresso::Hash(ProgressoHash {
            arquivos_total: 4,
            ..ProgressoHash::default()
        }));
        assert_eq!(
            k.painel(Onde::Janela, None, None).titulo,
            "Hash Completo (Full Scan)"
        );
        assert!(k.resumo_progresso().starts_with("hash 0 de 4"));
    }

    /// O hash acabou (com erros) e o agente passa a agrupar duplicados: o painel mostra a etapa
    /// atual, o resumo do hash fica final (nada pendente, erros contados) e a etapa some quando
    /// um novo andamento (próxima passada) chega.
    #[test]
    fn painel_mostra_o_agrupamento_depois_do_hash_com_resumo_final() {
        use crate::painel::EtapaPainel;
        use crate::{ProgressoDiscoHash, ProgressoHash};
        let mut j = job(1, EstadoJob::Executando);
        j.tipo = TipoJob::FastScan;
        j.registrar_progresso(&Progresso::Hash(ProgressoHash {
            arquivos_total: 100,
            arquivos_feitos: 100,
            erros: 7,
            finalizado: true,
            discos: vec![ProgressoDiscoHash {
                disco: "d".into(),
                arquivos_total: 10,
                arquivos_feitos: 10,
                concluido: true,
                ..ProgressoDiscoHash::default()
            }],
            ..ProgressoHash::default()
        }));
        let antes = j.painel(Onde::Janela, None, None);
        assert_eq!(antes.etapa, EtapaPainel::Hash);
        j.registrar_progresso(&Progresso::Mensagem {
            texto: "[duplicados] Agrupando duplicados (pode levar minutos em Catálogos grandes)..."
                .into(),
        });
        let p = j.painel(Onde::Janela, None, None);
        assert_eq!(p.etapa, EtapaPainel::Duplicados);
        assert_eq!(p.titulo, "Agrupando duplicados");
        let h = p.hash.as_ref().unwrap();
        assert_eq!((h.pendentes, h.erros), (0, 7));
        assert!(
            p.resumo().starts_with("Agrupando duplicados…"),
            "{}",
            p.resumo()
        );
        assert!(p.resumo().contains("7 erro(s)"), "{}", p.resumo());
        assert!(p.linha_status().contains("Agrupando duplicados…"));
        let texto = p.linhas().join("\n");
        assert!(
            texto.contains("Etapa atual: agrupando duplicados"),
            "{texto}"
        );
        assert!(texto.contains("pendentes 0"), "{texto}");
        // Marcações: segundo passo da mesma etapa.
        j.registrar_progresso(&Progresso::Mensagem {
            texto: "[marcações] reavaliando Marcações contra o hash atual...".into(),
        });
        assert_eq!(
            j.painel(Onde::Janela, None, None).titulo,
            "Reavaliando Marcações"
        );
        // A próxima passada começa: a etapa de agrupamento da anterior sai do painel.
        j.registrar_progresso(&Progresso::Hash(ProgressoHash {
            arquivos_total: 100,
            preparando: true,
            ..ProgressoHash::default()
        }));
        assert_ne!(
            j.painel(Onde::Janela, None, None).etapa,
            EtapaPainel::Duplicados
        );
        assert!(j.etapa.is_none());
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
