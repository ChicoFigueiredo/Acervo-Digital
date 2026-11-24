//! Eventos de progresso que o worker emite enquanto trabalha (varredura, reconciliação, hash,
//! verificação). Serializável: o mesmo formato vai pelo socket do agente (spec §11) e alimenta
//! a barra de progresso da GUI, a linha de stderr da CLI e as notificações do MCP.

use serde::{Deserialize, Serialize};

use crate::ExecucaoId;

/// Fase do inventário de uma varredura.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaseInventario {
    Inventario,
    Pausado,
    Agregacao,
    /// Fim do inventário: seguem classificação, hash e duplicados.
    Concluido,
}

/// Contagem do inventário (Full Scan) ou da reconciliação (Fast Scan) num instante.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressoInventario {
    pub execucao: ExecucaoId,
    pub fase: FaseInventario,
    pub pastas: u64,
    pub arquivos: u64,
    pub especiais: u64,
    pub bytes_logicos: u64,
    pub ignorados: u64,
    pub erros: u64,
    /// Diretórios descobertos e ainda não visitados (aproximado).
    pub diretorios_pendentes: u64,
    pub decorrido_ms: u64,
    pub entradas_por_segundo: f64,
}

/// Andamento de um Dispositivo Físico na etapa de hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressoDiscoHash {
    pub disco: String,
    pub arquivos_feitos: u64,
    pub arquivos_total: u64,
    pub bytes_lidos: u64,
    pub bytes_total: u64,
    pub bytes_por_seg: f64,
    pub ativo: bool,
    pub concluido: bool,
}

/// Andamento da etapa de Hash Completo (ou da Verificação de Integridade), com bytes reais.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressoHash {
    /// Linhas de arquivo resolvidas (hasheadas, reaproveitadas ou com erro).
    pub arquivos_feitos: u64,
    pub arquivos_total: u64,
    /// Bytes efetivamente lidos do disco até agora.
    pub bytes_lidos: u64,
    /// Bytes a ler (soma dos arquivos físicos enfileirados).
    pub bytes_total: u64,
    pub bytes_por_seg: f64,
    pub reaproveitados: u64,
    /// Um item por disco físico (a etapa lê um disco por thread, vários discos ao mesmo tempo).
    #[serde(default)]
    pub discos: Vec<ProgressoDiscoHash>,
}

/// Um evento de progresso.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "etapa", rename_all = "snake_case")]
pub enum Progresso {
    /// Inventário do Full Scan.
    Inventario(ProgressoInventario),
    /// Reconciliação do Fast Scan.
    Reconciliacao(ProgressoInventario),
    /// Hash Completo.
    Hash(ProgressoHash),
    /// Verificação de Integridade.
    Verificacao(ProgressoHash),
    /// Mudança de etapa sem contadores (classificação, duplicados, monitor...).
    Mensagem { texto: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializa_com_etapa_e_volta() {
        let p = Progresso::Hash(ProgressoHash {
            arquivos_feitos: 1,
            arquivos_total: 4,
            bytes_lidos: 10,
            bytes_total: 40,
            bytes_por_seg: 2.5,
            reaproveitados: 0,
            discos: Vec::new(),
        });
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["etapa"], "hash");
        assert_eq!(v["bytes_lidos"], 10);
        assert_eq!(serde_json::from_value::<Progresso>(v).unwrap(), p);
        let m = Progresso::Mensagem { texto: "x".into() };
        let j = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Progresso>(&j).unwrap(), m);
        let i = Progresso::Reconciliacao(ProgressoInventario {
            execucao: ExecucaoId(3),
            fase: FaseInventario::Agregacao,
            pastas: 1,
            arquivos: 2,
            especiais: 0,
            bytes_logicos: 5,
            ignorados: 0,
            erros: 0,
            diretorios_pendentes: 0,
            decorrido_ms: 7,
            entradas_por_segundo: 1.0,
        });
        let j = serde_json::to_string(&i).unwrap();
        assert_eq!(serde_json::from_str::<Progresso>(&j).unwrap(), i);
    }
}
