//! Tipos do conteúdo de Contêineres, da Fila de Serviço e do Cofre de Senhas (ADR 0006).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Senha de Contêiner em trânsito. Nunca aparece em `Debug`/logs; só vai do cliente ao agente
/// (o agente a cifra antes de guardar) e nunca volta numa [`Resposta`](crate::Resposta).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Senha(pub String);

impl fmt::Debug for Senha {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Senha(***)")
    }
}

impl Senha {
    /// Texto da senha, só para entregar ao leitor ou ao Cofre.
    #[must_use]
    pub fn expor(&self) -> &str {
        &self.0
    }
}

/// Tipos de item da Fila de Serviço (v1).
pub const PEDIDO_SENHA: &str = "pedido_senha";
pub const FALHA_LEITURA: &str = "falha_leitura";

/// Item da Fila de Serviço (Pedido de Serviço).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PedidoInfo {
    pub id: i64,
    /// `pedido_senha` ou `falha_leitura`.
    pub tipo: String,
    pub arquivo: Option<i64>,
    /// Caminho do objeto (Contêiner).
    pub objeto: Option<String>,
    pub motivo: String,
    /// `aberto`, `resolvido` ou `dispensado`.
    pub estado: String,
    pub criado_em: i64,
    pub resolvido_em: Option<i64>,
}

/// Senha guardada no Cofre de Senhas: só o rótulo, nunca o valor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemCofre {
    pub id: i64,
    pub rotulo: String,
}

/// Situação do Cofre de Senhas (chave no keyring do sistema).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SituacaoCofre {
    /// Chave no keyring do sistema.
    Chaveiro,
    /// Keyring indisponível; arquivo local trancado por frase.
    Trancado,
    /// Cofre em arquivo, aberto nesta sessão.
    Aberto,
    /// Sem keyring nem arquivo: senhas não podem ser guardadas nem usadas.
    Indisponivel,
}

/// Filho direto de uma pasta virtual dentro de um Contêiner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntradaMembroInfo {
    pub nome: String,
    /// Pasta virtual ou Contêiner aninhado: dá para abrir.
    pub navegavel: bool,
    pub bytes: u64,
    pub arquivos: u64,
    pub mtime_ns: Option<i64>,
    /// `ordem` do Membro aninhado que é Contêiner (passar como `pai_ordem` para entrar nele).
    pub aninhado_ordem: Option<i64>,
    /// BLAKE3 do Membro em hexadecimal.
    pub hash: Option<String>,
    /// `lido`, `cifrado`, `nao_suportado`, `aninhado_grande` ou `erro`.
    pub estado: Option<String>,
}

/// Andamento da etapa "Conteúdo de contêineres" no Catálogo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusConteineres {
    pub pendentes: u64,
    pub lendo: u64,
    pub lidos: u64,
    pub aguardando_senha: u64,
    pub aguardando_chaveiro: u64,
    pub falhas: u64,
    pub nao_conteineres: u64,
    pub membros: u64,
    pub bytes_lidos: u64,
    pub bytes_pendentes: u64,
    /// Itens abertos na Fila de Serviço.
    pub pedidos_abertos: u64,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn senha_nao_aparece_no_debug() {
        let s = Senha("segredo".into());
        assert!(!format!("{s:?}").contains("segredo"));
        assert_eq!(serde_json::to_string(&s).unwrap(), "\"segredo\"");
    }
}
