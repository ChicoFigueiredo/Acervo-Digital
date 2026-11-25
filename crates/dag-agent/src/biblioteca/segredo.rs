//! Senha do modo servidor do Calibre (B-24): guardada cifrada pelo Cofre do sistema (a mesma
//! guarda das Senhas de Contêiner, ADR 0006), nunca em variável de ambiente nem em claro no
//! Catálogo. Entra por `BibliotecaConfigDefinir` (`homologacao.senha`) e nunca sai: nenhuma
//! resposta, nenhum erro e nenhum log a carrega (o MCP só fornece, jamais recebe).

use dag_scanner::Cofre;
use rusqlite::Connection;

use super::hex;
use crate::agente::Agente;
use crate::erro::ErroAgente;

/// Chave em `configuracoes` do blob cifrado (hexadecimal).
pub(super) const CHAVE_SENHA: &str = "biblioteca.homologacao.senha";

fn de_hex(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect()
}

/// Há senha do servidor guardada? (só olha o Catálogo; não toca o chaveiro).
pub(super) fn tem_senha_servidor(conn: &Connection) -> bool {
    dag_store::configuracao::ler(conn, CHAVE_SENHA)
        .ok()
        .flatten()
        .is_some_and(|v| de_hex(&v).is_some())
}

impl Agente {
    /// Cifra e guarda a senha; vazia apaga. O texto da senha nunca entra em mensagem de erro.
    pub(super) fn guardar_senha_servidor(&self, senha: &str) -> Result<(), ErroAgente> {
        if senha.is_empty() {
            dag_store::configuracao::gravar(self.conexao(), CHAVE_SENHA, "")?;
            return Ok(());
        }
        let blob = self.cofre.cifrar(senha).ok_or_else(|| {
            ErroAgente::biblio(
                "cofre_indisponivel",
                "o Cofre de Senhas está indisponível (chaveiro do sistema trancado ou ausente): \
                 a senha do servidor não foi guardada",
            )
        })?;
        dag_store::configuracao::gravar(self.conexao(), CHAVE_SENHA, &hex(&blob))?;
        Ok(())
    }

    /// A senha em claro, só para entregar ao `calibredb` (processo filho); `None` se não houver
    /// ou o Cofre não decifra.
    pub(super) fn senha_servidor(&self) -> Option<String> {
        let v = dag_store::configuracao::ler(self.conexao(), CHAVE_SENHA)
            .ok()
            .flatten()?;
        self.cofre.decifrar(&de_hex(&v)?)
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn hex_ida_e_volta() {
        let b = vec![0u8, 1, 0xab, 0xff];
        assert_eq!(de_hex(&hex(&b)), Some(b));
        assert!(de_hex("").is_none());
        assert!(de_hex("abc").is_none());
        assert!(de_hex("zz").is_none());
    }
}
