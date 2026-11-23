//! Invariante de escritor único (spec §6): trava consultiva exclusiva em `<catálogo>.lock`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;

use crate::caminho::{caminho_do_lock, preparar_diretorio};
use crate::erro::ErroAgente;

/// Quem a detém é o único escritor do Catálogo. A trava é solta ao descartar a guarda (ou quando
/// o processo morre, inclusive por `kill -9`: o kernel libera o `flock`).
#[derive(Debug)]
pub struct GuardaEscritor {
    _arquivo: File,
    caminho: PathBuf,
}

impl GuardaEscritor {
    /// Tenta adquirir a trava sem esperar.
    ///
    /// # Errors
    /// [`ErroAgente::EscritorOcupado`] se outro detentor existe; E/S ao criar o arquivo.
    pub fn adquirir(catalogo: &Path) -> Result<Self, ErroAgente> {
        let lock = caminho_do_lock(catalogo);
        preparar_diretorio(&lock)?;
        let mut arquivo = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock)
            .map_err(|e| ErroAgente::es(format!("abrir {}", lock.display()), e))?;
        match flock(&arquivo, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(Errno::WOULDBLOCK) => {
                return Err(ErroAgente::EscritorOcupado(catalogo.to_path_buf()));
            }
            Err(e) => {
                return Err(ErroAgente::es(
                    format!("travar {}", lock.display()),
                    e.into(),
                ));
            }
        }
        // Informativo (quem detém); a trava é o flock, não o conteúdo.
        let _ = arquivo.set_len(0);
        let _ = writeln!(arquivo, "{}", std::process::id());
        Ok(Self {
            _arquivo: arquivo,
            caminho: lock,
        })
    }

    /// Caminho do arquivo de trava.
    #[must_use]
    pub fn caminho(&self) -> &Path {
        &self.caminho
    }
}
