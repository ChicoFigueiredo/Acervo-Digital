//! Gancho opcional para ler **só metadados** de pastas que negaram acesso (A-16).
//!
//! O scanner nunca eleva privilégio por conta própria: quem monta a varredura entrega um
//! [`LeitorElevado`] (ex.: o cliente do `dag-helper` via `pkexec`) somente quando o usuário pediu
//! (`--elevar`). Sem leitor, o comportamento é o de sempre: registrar `permissao_negada`.

use std::io;

/// Tipo do objeto, como `lstat` o vê.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipoElevado {
    Arquivo,
    Pasta,
    Symlink,
    Fifo,
    Socket,
    DispositivoBloco,
    DispositivoChar,
    Outro,
}

/// Metadados de um filho de pasta (os mesmos campos que o scanner coleta com `lstat`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntradaElevada {
    /// Nome do filho, em bytes brutos.
    pub nome: Vec<u8>,
    pub tipo: TipoElevado,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub tamanho: u64,
    /// Blocos de 512 bytes.
    pub blocos: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
    pub alvo_symlink: Option<Vec<u8>>,
}

/// Lê os filhos diretos de uma pasta com privilégio elevado (somente metadados).
pub trait LeitorElevado: Send + Sync {
    /// `pasta` é um caminho absoluto (bytes brutos) cuja listagem deu `EACCES`.
    ///
    /// # Errors
    /// Qualquer falha (sem autorização, sem helper, recusa): o scanner volta a registrar a
    /// pasta como inacessível.
    fn listar(&self, pasta: &[u8]) -> io::Result<Vec<EntradaElevada>>;
}
