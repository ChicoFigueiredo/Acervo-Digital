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

/// Arquivo grande em leitura agora: `bytes_lidos` de `bytes_total` (sobe durante a leitura, não só
/// quando o arquivo termina).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeituraEmCurso {
    /// Caminho do arquivo (exibição).
    pub caminho: String,
    pub bytes_lidos: u64,
    pub bytes_total: u64,
}

/// Andamento de um Dispositivo Físico na etapa de hash.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProgressoDiscoHash {
    pub disco: String,
    /// Volumes do Catálogo (ids) hospedados neste disco que têm trabalho.
    #[serde(default)]
    pub volumes: Vec<i64>,
    /// Threads de leitura deste disco (a configuração em vigor), 0 se o disco ainda não começou.
    #[serde(default)]
    pub threads: u32,
    /// Leituras em curso agora (no máximo `threads`).
    #[serde(default)]
    pub leitores_ativos: u32,
    pub arquivos_feitos: u64,
    pub arquivos_total: u64,
    pub bytes_lidos: u64,
    pub bytes_total: u64,
    /// Vazão recente deste disco (janela móvel), em bytes por segundo.
    pub bytes_por_seg: f64,
    pub ativo: bool,
    pub concluido: bool,
    /// Arquivos grandes em leitura neste disco agora (os pequenos não aparecem).
    #[serde(default)]
    pub lendo: Vec<LeituraEmCurso>,
}

/// Andamento da etapa de Hash Completo (ou da Verificação de Integridade), com bytes reais.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProgressoHash {
    /// Linhas de arquivo resolvidas (hasheadas, reaproveitadas ou com erro).
    pub arquivos_feitos: u64,
    pub arquivos_total: u64,
    /// Bytes efetivamente lidos do disco até agora.
    pub bytes_lidos: u64,
    /// Bytes a ler (soma dos arquivos físicos enfileirados).
    pub bytes_total: u64,
    /// Vazão recente (janela móvel de alguns segundos), em bytes por segundo.
    pub bytes_por_seg: f64,
    pub reaproveitados: u64,
    /// Execução cujo hash está sendo feito (a fila persistente pertence a ela).
    #[serde(default)]
    pub execucao: Option<i64>,
    /// Threads por disco e discos em paralelo em vigor nesta etapa.
    #[serde(default)]
    pub threads_por_disco: u32,
    #[serde(default)]
    pub max_discos: u32,
    /// Arquivos que não puderam ser hasheados até agora.
    #[serde(default)]
    pub erros: u64,
    /// Arquivos resolvidos por segundo na mesma janela móvel de `bytes_por_seg`.
    #[serde(default)]
    pub arquivos_por_seg: f64,
    /// Largura (ms) da janela em que as vazões foram medidas; pequena demais, a estimativa de
    /// tempo restante ainda não é confiável.
    #[serde(default)]
    pub janela_ms: u64,
    /// Tempo desde que a etapa começou (inclui a preparação da fila).
    #[serde(default)]
    pub decorrido_ms: u64,
    /// A fila de hash ainda está sendo montada: os totais ainda não são os definitivos.
    #[serde(default)]
    pub preparando: bool,
    /// Um item por disco físico (a etapa lê um disco por thread, vários discos ao mesmo tempo).
    #[serde(default)]
    pub discos: Vec<ProgressoDiscoHash>,
}

/// Andamento da etapa "Conteúdo de contêineres" (ADR 0006): a leitura em fluxo dos compactados
/// depois do Hash Completo. `andamento` tem o mesmo formato do hash (Contêineres no lugar de
/// arquivos, bytes de Contêiner lidos, um item por disco físico).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProgressoConteineres {
    pub andamento: ProgressoHash,
    /// Membros gravados no Catálogo até agora, nesta etapa.
    #[serde(default)]
    pub membros: u64,
    /// Contêineres cifrados que esperam senha (Fila de Serviço).
    #[serde(default)]
    pub aguardando_senha: u64,
    /// Contêineres com Falha de Leitura.
    #[serde(default)]
    pub falhas: u64,
    /// Contêineres que reaproveitaram os Membros de uma cópia idêntica.
    #[serde(default)]
    pub copiados: u64,
}

/// O que a rotina de abertura do agente está fazendo agora (dado estruturado: a interface não
/// precisa interpretar o texto das mensagens).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PassoAbertura {
    pub tipo: TipoPasso,
    /// Retomada: execução retomada.
    #[serde(default)]
    pub execucao: Option<i64>,
    /// Retomada: "Full Scan" ou "Fast Scan".
    #[serde(default)]
    pub modo: String,
    /// Retomada: a execução parou no meio do inventário (senão, no hash).
    #[serde(default)]
    pub no_inventario: bool,
    /// Verificação: passada atual e teto de passadas.
    #[serde(default)]
    pub passada: u32,
    #[serde(default)]
    pub de: u32,
}

/// Tipo de passo da abertura.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoPasso {
    /// Retomada de uma Execução Interrompida.
    Retomada,
    /// Passada de verificação (Fast Scan).
    Verificacao,
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
    /// Conteúdo de contêineres (compactados lidos em fluxo, depois do Hash Completo).
    Conteineres(ProgressoConteineres),
    /// Mudança de etapa sem contadores (classificação, duplicados, monitor...).
    Mensagem { texto: String },
    /// Passo estruturado da rotina de abertura (retomada, passada de verificação). Um passo novo
    /// zera o andamento dos passos anteriores.
    Passo(PassoAbertura),
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
            ..ProgressoHash::default()
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
