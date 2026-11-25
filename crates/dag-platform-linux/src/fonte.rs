use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// Capacidade e espaço livre de um filesystem, em bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstatisticasFs {
    pub capacidade_bytes: u64,
    /// Espaço disponível a usuários não privilegiados (`f_bavail`), como `df`.
    pub livre_bytes: u64,
}

/// Único canal de acesso ao sistema operacional da descoberta. Todas as operações
/// são de leitura.
pub trait FonteSistema {
    fn ler_arquivo(&self, caminho: &Path) -> io::Result<Vec<u8>>;
    /// Alvo literal do link simbólico (não resolvido).
    fn ler_link(&self, caminho: &Path) -> io::Result<PathBuf>;
    /// Nomes das entradas do diretório (sem ordenação garantida).
    fn listar_dir(&self, caminho: &Path) -> io::Result<Vec<OsString>>;
    fn statvfs(&self, caminho: &Path) -> io::Result<EstatisticasFs>;
}

/// Implementação sobre o sistema real.
#[derive(Debug, Clone, Copy, Default)]
pub struct FonteSistemaReal;

impl FonteSistema for FonteSistemaReal {
    fn ler_arquivo(&self, caminho: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(caminho)
    }

    fn ler_link(&self, caminho: &Path) -> io::Result<PathBuf> {
        std::fs::read_link(caminho)
    }

    fn listar_dir(&self, caminho: &Path) -> io::Result<Vec<OsString>> {
        std::fs::read_dir(caminho)?
            .map(|e| e.map(|e| e.file_name()))
            .collect()
    }

    fn statvfs(&self, caminho: &Path) -> io::Result<EstatisticasFs> {
        let s = rustix::fs::statvfs(caminho)?;
        let tamanho = s.f_frsize;
        Ok(EstatisticasFs {
            capacidade_bytes: s.f_blocks.saturating_mul(tamanho),
            livre_bytes: s.f_bavail.saturating_mul(tamanho),
        })
    }
}

#[cfg(test)]
pub(crate) mod falsa {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    /// Árvore de fixtures em memória.
    #[derive(Debug, Default)]
    pub struct FonteFalsa {
        arquivos: HashMap<PathBuf, Vec<u8>>,
        links: HashMap<PathBuf, PathBuf>,
        diretorios: BTreeSet<PathBuf>,
        stats: HashMap<PathBuf, EstatisticasFs>,
    }

    impl FonteFalsa {
        pub fn arquivo(&mut self, caminho: &str, conteudo: impl AsRef<[u8]>) -> &mut Self {
            self.arquivos
                .insert(PathBuf::from(caminho), conteudo.as_ref().to_vec());
            self
        }

        pub fn link(&mut self, caminho: &str, alvo: &str) -> &mut Self {
            self.links
                .insert(PathBuf::from(caminho), PathBuf::from(alvo));
            self
        }

        pub fn stat(&mut self, caminho: &str, capacidade: u64, livre: u64) -> &mut Self {
            self.stats.insert(
                PathBuf::from(caminho),
                EstatisticasFs {
                    capacidade_bytes: capacidade,
                    livre_bytes: livre,
                },
            );
            self
        }
    }

    fn nao_encontrado() -> io::Error {
        io::Error::from(io::ErrorKind::NotFound)
    }

    impl FonteSistema for FonteFalsa {
        fn ler_arquivo(&self, caminho: &Path) -> io::Result<Vec<u8>> {
            self.arquivos
                .get(caminho)
                .cloned()
                .ok_or_else(nao_encontrado)
        }

        fn ler_link(&self, caminho: &Path) -> io::Result<PathBuf> {
            self.links.get(caminho).cloned().ok_or_else(nao_encontrado)
        }

        fn listar_dir(&self, caminho: &Path) -> io::Result<Vec<OsString>> {
            let filhos: BTreeSet<OsString> = self
                .arquivos
                .keys()
                .chain(self.links.keys())
                .chain(self.diretorios.iter())
                .filter(|p| p.parent() == Some(caminho))
                .filter_map(|p| p.file_name().map(|n| n.to_owned()))
                .collect();
            if filhos.is_empty() && !self.diretorios.contains(caminho) {
                return Err(nao_encontrado());
            }
            Ok(filhos.into_iter().collect())
        }

        fn statvfs(&self, caminho: &Path) -> io::Result<EstatisticasFs> {
            self.stats.get(caminho).copied().ok_or_else(nao_encontrado)
        }
    }
}
