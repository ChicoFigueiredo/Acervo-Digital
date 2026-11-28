//! Arquivos que o próprio processo nunca abre: o Catálogo aberto e seus acompanhantes
//! (`-wal`, `-shm`, `-journal`, `.lock`).
//!
//! No Linux, fechar **qualquer** descritor de um arquivo solta todas as travas POSIX (`fcntl`)
//! que o processo tinha nele, inclusive as que o SQLite guarda no `-shm` do Catálogo. Sem elas,
//! a próxima conexão de outro processo acha que é a primeira, reinicia o índice do WAL e o
//! agente leva SIGBUS ao escrever na região mapeada (`walIndexAppend`). Como o Catálogo costuma
//! morar num volume varrido (`~/.local/share/dag`), o hash e a leitura de contêineres o abririam
//! e fechariam. Este registro barra essas aberturas antes do `open`.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

static PROTEGIDOS: RwLock<Vec<PathBuf>> = RwLock::new(Vec::new());

/// Protege `catalogo` e todo arquivo cujo caminho começa por ele (`catalogo.sqlite-wal`,
/// `catalogo.sqlite-shm`, `catalogo.sqlite.lock`…). Idempotente.
pub fn proteger_catalogo(catalogo: &Path) {
    let mut v = PROTEGIDOS.write().unwrap_or_else(PoisonError::into_inner);
    if !v.iter().any(|p| p == catalogo) {
        v.push(catalogo.to_path_buf());
    }
}

/// `caminho` é o Catálogo protegido ou um de seus acompanhantes.
#[must_use]
pub fn protegido(caminho: &Path) -> bool {
    let alvo = caminho.as_os_str().as_bytes();
    PROTEGIDOS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|p| alvo.starts_with(p.as_os_str().as_bytes()))
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn catalogo_e_acompanhantes_ficam_protegidos() {
        let cat = Path::new("/tmp/dag-protegidos-teste/catalogo.sqlite");
        proteger_catalogo(cat);
        proteger_catalogo(cat);
        for nome in ["", "-wal", "-shm", "-journal", ".lock"] {
            let mut s = cat.as_os_str().to_owned();
            s.push(nome);
            assert!(protegido(Path::new(&s)), "{nome}");
        }
        assert!(!protegido(Path::new(
            "/tmp/dag-protegidos-teste/outro.sqlite"
        )));
        assert!(!protegido(Path::new("/tmp/dag-protegidos-teste")));
    }

    #[test]
    fn hash_e_conteineres_nao_abrem_o_catalogo_protegido() {
        let dir = tempfile::tempdir().unwrap();
        let cat = dir.path().join("catalogo.sqlite");
        let shm = dir.path().join("catalogo.sqlite-shm");
        let livre = dir.path().join("livro.pdf");
        for p in [&cat, &shm, &livre] {
            std::fs::write(p, b"x").unwrap();
        }
        proteger_catalogo(&cat);
        assert!(crate::hash::abrir_somente_leitura(&cat).is_err());
        assert!(crate::hash::abrir_somente_leitura(&shm).is_err());
        assert!(crate::hash::abrir_somente_leitura(&livre).is_ok());
    }
}
