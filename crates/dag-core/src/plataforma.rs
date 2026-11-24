//! Fronteira de plataforma (A-22): o que o DAG pede ao sistema operacional.
//!
//! Só o Linux está implementado (`dag-platform-linux::PlataformaLinux`). Windows e macOS entram
//! como **contrato**: um novo crate `dag-platform-<so>` implementa [`Plataforma`] e o resto do
//! DAG (agente, scanner, store, CLI, GUI, MCP) não muda. Nada aqui finge suporte: um sistema
//! sem implementação simplesmente não tem valor de [`Plataforma`].
//!
//! O contrato cobre, por ora, o que já variava por SO: descoberta de Volumes e Dispositivos
//! Físicos e a descrição dos mecanismos nativos de monitoramento, serviço em segundo plano e
//! elevação sob demanda. O acesso a arquivos usa `std` e é portável.

use crate::VolumeDescoberto;

/// Sistema operacional de uma [`Plataforma`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SistemaOperacional {
    Linux,
    Windows,
    MacOs,
}

impl SistemaOperacional {
    #[must_use]
    pub const fn nome(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::MacOs => "macos",
        }
    }
}

/// Mecanismos nativos que a plataforma usa (para exibir na GUI/CLI e nos relatórios).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mecanismos {
    /// Observação de mudanças (Linux: inotify; Windows: ReadDirectoryChangesW; macOS: FSEvents).
    pub monitor: &'static str,
    /// Serviço em segundo plano opt-in (Linux: systemd de usuário).
    pub servico: &'static str,
    /// Elevação sob demanda (Linux: Polkit + helper restrito; Windows: UAC; macOS: helper).
    pub elevacao: &'static str,
}

/// Falha da plataforma, com texto já pronto para o usuário.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ErroPlataforma(pub String);

/// Contrato entre o DAG e um sistema operacional.
pub trait Plataforma: Send + Sync {
    fn sistema(&self) -> SistemaOperacional;

    fn mecanismos(&self) -> Mecanismos;

    /// Volumes e Dispositivos Físicos visíveis agora, somente leitura. Falhas parciais viram
    /// campos ausentes, não erro.
    ///
    /// # Errors
    /// Quando a fonte indispensável do sistema não pôde ser lida.
    fn descobrir_volumes(&self) -> Result<Vec<VolumeDescoberto>, ErroPlataforma>;
}
