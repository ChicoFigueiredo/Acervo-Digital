//! Enumerações do domínio. Serializadas em `snake_case`.

use serde::{Deserialize, Serialize};

/// Tipo de varredura.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModoVarredura {
    /// Visita tudo e calcula o Hash Completo de todos os arquivos.
    FullScan,
    /// Reconcilia reaproveitando metadados e hash do que não mudou.
    FastScan,
}

/// Etapas do pipeline de varredura, em ordem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EtapaVarredura {
    DescobertaVolumes,
    Estrutura,
    Inventario,
    Classificacao,
    Hash,
    Agregacao,
}

/// Estado de uma execução de varredura.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoExecucao {
    Pendente,
    EmAndamento,
    Pausada,
    Cancelada,
    Concluida,
    Falhou,
}

/// Tipo de Marcação: Marcação de Backup ou Marcação para Deleção.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoMarcacao {
    Backup,
    Delecao,
}

/// Estado de uma Marcação. `Pendente` = Marcação Pendente (fora de todas as contas).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoMarcacao {
    Ativa,
    Pendente,
}

/// Como a Marcação foi criada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrigemMarcacao {
    Regra,
    Manual,
}

/// Cliente que originou um pedido ao worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Autor {
    Gui,
    Cli,
    Mcp,
}

/// Finalidade de arquivos e pastas (A-08).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Finalidade {
    Sistema,
    Aplicativo,
    Usuario,
    Compartilhado,
    Misto,
    Desconhecido,
}
