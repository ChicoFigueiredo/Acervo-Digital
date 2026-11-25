//! Tipos do Fast Scan e do monitor (incremento 5): resumo da reconciliação, eventos persistidos
//! e saúde do monitor, no formato trocado entre clientes e o worker.

use serde::{Deserialize, Serialize};

use crate::{ArquivoId, ExecucaoId, VolumeId};

/// O que o Fast Scan fez (A-12).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumoReconciliacao {
    pub arquivos_novos: u64,
    pub arquivos_alterados: u64,
    /// Reaproveitados sem reescrever metadados nem reler conteúdo.
    pub arquivos_inalterados: u64,
    pub pastas_novas: u64,
    pub arquivos_renomeados: u64,
    pub pastas_renomeadas: u64,
    /// Marcados `ausente`.
    pub arquivos_ausentes: u64,
    pub pastas_ausentes: u64,
    /// Caminhos que a varredura não visitou por inteiro (nada abaixo deles foi dado como removido).
    pub bloqueios: u64,
    /// `false`: a remoção foi recusada por inteiro (veja `motivo_sem_remocao`).
    pub remocao_aplicada: bool,
    pub motivo_sem_remocao: Option<String>,
    pub eventos_omitidos: u64,
}

impl ResumoReconciliacao {
    /// Entradas que mudaram desde o Catálogo: novas, alteradas, renomeadas e ausentes.
    #[must_use]
    pub fn mudancas(&self) -> u64 {
        self.arquivos_novos
            + self.arquivos_alterados
            + self.pastas_novas
            + self.arquivos_renomeados
            + self.pastas_renomeadas
            + self.arquivos_ausentes
            + self.pastas_ausentes
    }

    /// Passada sem mudança nenhuma e sem caminho que a varredura não conseguiu visitar: o
    /// critério do Ponto de Confiança.
    #[must_use]
    pub fn sem_mudancas(&self) -> bool {
        self.mudancas() == 0 && self.bloqueios == 0
    }
}

/// Evento persistido em `scan_events`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventoInfo {
    pub id: i64,
    /// `criado`, `alterado`, `removido`, `renomeado`, `overflow`, `montado`, `desmontado`.
    pub tipo: String,
    /// `inotify`, `fast_scan`, `volumes`.
    pub fonte: String,
    pub volume: Option<VolumeId>,
    pub arquivo: Option<ArquivoId>,
    pub execucao: Option<ExecucaoId>,
    pub caminho: Option<String>,
    /// Só em renomeações.
    pub caminho_antigo: Option<String>,
    /// Segundos Unix.
    pub observado_em: i64,
    /// `None` = ainda não processado (lacuna aberta).
    pub processado_em: Option<i64>,
    /// Lacuna: exige Fast Scan antes de ser declarada processada.
    pub reconciliar: bool,
    pub detalhe: Option<String>,
}

/// Saúde do monitor e pendências de reconciliação (A-15, A-19).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaudeMonitorInfo {
    /// Situação do monitor, única fonte para todas as telas: `ativo`, `iniciando`,
    /// `aguardando_abertura` (o agente ainda retoma/verifica; o monitor sobe depois),
    /// `pausado_por_trabalho` (cedeu o escritor a um pedido), `parado` (com `situacao_motivo`) ou
    /// `desligado` (agente iniciado sem monitor). Vazio = agente antigo (use `estado`).
    #[serde(default)]
    pub situacao: String,
    #[serde(default)]
    pub situacao_motivo: Option<String>,
    /// `parado`, `iniciando`, `ativo`, `reconciliando`, `encerrando`.
    pub estado: String,
    /// Há um monitor vivo agora (heartbeat recente e estado diferente de `parado`).
    pub em_execucao: bool,
    pub iniciado_em: Option<i64>,
    pub heartbeat: Option<i64>,
    /// Diretórios sujos + lacunas + Fast Scans pendentes.
    pub backlog: u64,
    pub ultimo_evento_em: Option<i64>,
    pub ultima_reconciliacao: Option<i64>,
    pub watches: u64,
    pub watches_limite: u64,
    pub overflows: u64,
    pub eventos_processados: u64,
    /// Raízes sem cobertura completa de watches (a reconciliação periódica cobre o resto).
    pub raizes_parciais: u64,
    pub raizes: Vec<String>,
    pub erro: Option<String>,
    /// Lacunas (eventos perdidos) ainda sem correção: reconciliação pendente.
    pub lacunas_abertas: u64,
    pub eventos_nao_processados: u64,
}

impl SaudeMonitorInfo {
    /// A situação do monitor em palavras (CLI, GUI e MCP mostram o mesmo texto).
    #[must_use]
    pub fn situacao_texto(&self) -> String {
        let motivo = self
            .situacao_motivo
            .as_ref()
            .map_or_else(String::new, |x| format!(": {x}"));
        match self.situacao.as_str() {
            "ativo" => format!("ativo ({})", self.estado),
            "iniciando" => "iniciando".to_string(),
            "aguardando_abertura" => {
                "aguardando a abertura terminar (retomada/verificação em curso; sobe depois)".into()
            }
            "pausado_por_trabalho" => "pausado para atender um trabalho (volta ao terminar)".into(),
            "desligado" => format!("desligado{motivo}"),
            "parado" => format!("parado{motivo}"),
            // Agente antigo, sem `situacao`.
            _ if self.em_execucao => format!("ativo ({})", self.estado),
            _ if self.estado == "parado" || self.heartbeat.is_none() => "parado".to_string(),
            _ => format!("sem sinal (último estado: {})", self.estado),
        }
    }
}

/// Resumo de uma sessão de monitoramento encerrada.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumoMonitorInfo {
    pub execucao: Option<ExecucaoId>,
    pub lotes: u64,
    pub diretorios: u64,
    pub criados: u64,
    pub alterados: u64,
    pub removidos: u64,
    pub renomeados: u64,
    pub lacunas: u64,
    pub overflows: u64,
    pub fast_scans: u64,
    pub watches: u64,
    pub duracao_ms: u64,
}
