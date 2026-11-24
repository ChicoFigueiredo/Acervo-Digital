//! Configuração, exclusões e relatório de progresso.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Como uma exclusão casa com uma entrada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Casamento {
    /// Nome exato do último componente, em qualquer lugar da árvore (bytes brutos).
    Nome(Vec<u8>),
    /// Caminho absoluto exato (a entrada e, por consequência, tudo abaixo dela).
    Caminho(Vec<u8>),
    /// Glob de caminho (`*`, `**`, `?`; `~` já expandido): casa a entrada e tudo abaixo dela.
    Glob(String),
}

/// Exclusão explícita, com a justificativa que fica registrada (A-19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exclusao {
    pub casamento: Casamento,
    pub motivo: String,
}

impl Exclusao {
    #[must_use]
    pub fn nome(nome: impl Into<Vec<u8>>, motivo: impl Into<String>) -> Self {
        Self {
            casamento: Casamento::Nome(nome.into()),
            motivo: motivo.into(),
        }
    }

    #[must_use]
    pub fn caminho(caminho: impl Into<Vec<u8>>, motivo: impl Into<String>) -> Self {
        Self {
            casamento: Casamento::Caminho(caminho.into()),
            motivo: motivo.into(),
        }
    }

    /// Exclusão por glob de caminho. `home` expande o `~` do padrão.
    #[must_use]
    pub fn glob(padrao: &str, motivo: impl Into<String>, home: Option<&str>) -> Self {
        Self {
            casamento: Casamento::Glob(dag_core::erros::expandir_til(padrao, home)),
            motivo: motivo.into(),
        }
    }

    /// A entrada (`caminho` inteiro, `nome` = último componente) está excluída.
    #[must_use]
    pub fn casa(&self, caminho: &[u8], nome: &[u8]) -> bool {
        match &self.casamento {
            Casamento::Nome(n) => n == nome,
            Casamento::Caminho(c) => c == caminho,
            Casamento::Glob(g) => {
                dag_core::erros::casa_caminho(g, &String::from_utf8_lossy(caminho), None)
            }
        }
    }
}

/// Limites e parâmetros. Os padrões são conservadores (A-21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigScan {
    /// Threads que leem diretórios (E/S de metadados em paralelo).
    pub concorrencia: usize,
    /// Linhas por transação do escritor (1–5 mil recomendado).
    pub linhas_por_transacao: usize,
    /// Entradas por mensagem worker → escritor (limita a memória de um diretório gigante).
    pub entradas_por_mensagem: usize,
    /// Capacidade do canal de resultados (mensagens), que dá contrapressão aos workers.
    pub fila_de_resultados: usize,
    /// Diretórios reservados do banco por vez (tamanho da fronteira em RAM).
    pub reservas_por_vez: usize,
    /// Intervalo mínimo entre chamadas do callback de progresso.
    pub intervalo_de_progresso: Duration,
    pub exclusoes: Vec<Exclusao>,
}

impl Default for ConfigScan {
    fn default() -> Self {
        Self {
            concorrencia: 4,
            linhas_por_transacao: 4000,
            entradas_por_mensagem: 2048,
            fila_de_resultados: 32,
            reservas_por_vez: 128,
            intervalo_de_progresso: Duration::from_millis(250),
            exclusoes: Vec::new(),
        }
    }
}

/// Fase informada no progresso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaseProgresso {
    Inventario,
    Pausado,
    Agregacao,
    Concluido,
}

/// Instantâneo de progresso (A-15).
#[derive(Debug, Clone, PartialEq)]
pub struct Progresso {
    pub execucao_id: i64,
    pub fase: FaseProgresso,
    pub pastas: u64,
    pub arquivos: u64,
    pub especiais: u64,
    pub bytes_logicos: u64,
    pub ignorados: u64,
    pub erros: u64,
    /// Diretórios já descobertos e ainda não visitados (aproximado).
    pub diretorios_pendentes: u64,
    pub decorrido: Duration,
    pub entradas_por_segundo: f64,
}
