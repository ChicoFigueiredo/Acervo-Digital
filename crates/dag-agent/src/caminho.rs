//! Onde mora o Catálogo e como o diretório é preparado (dado privado: 0700/0600).

use std::ffi::OsString;
use std::fs::{DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::erro::ErroAgente;

/// Variável de ambiente que escolhe o Catálogo (abaixo da flag `--catalogo`).
pub const VAR_CATALOGO: &str = "DAG_CATALOGO";

/// Resolve o caminho: flag, depois `DAG_CATALOGO`, depois `$XDG_DATA_HOME/dag/catalogo.sqlite`
/// (padrão `~/.local/share`). Valores vazios são ignorados; `XDG_DATA_HOME` relativo também
/// (a especificação XDG manda ignorar).
///
/// # Errors
/// [`ErroAgente::Invalido`] se nem `XDG_DATA_HOME` nem `HOME` servem.
pub fn resolver_caminho_catalogo(
    flag: Option<&Path>,
    env_dag: Option<OsString>,
    xdg_data_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, ErroAgente> {
    if let Some(f) = flag.filter(|f| !f.as_os_str().is_empty()) {
        return Ok(f.to_path_buf());
    }
    if let Some(e) = env_dag.filter(|e| !e.is_empty()) {
        return Ok(PathBuf::from(e));
    }
    let base = match xdg_data_home.map(PathBuf::from).filter(|p| p.is_absolute()) {
        Some(x) => x,
        None => {
            let home = home
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .ok_or_else(|| {
                    ErroAgente::Invalido(
                        "não foi possível achar o diretório de dados: defina --catalogo, DAG_CATALOGO ou HOME"
                            .into(),
                    )
                })?;
            home.join(".local/share")
        }
    };
    Ok(base.join("dag").join("catalogo.sqlite"))
}

/// [`resolver_caminho_catalogo`] lendo o ambiente do processo.
///
/// # Errors
/// Como [`resolver_caminho_catalogo`].
pub fn caminho_catalogo_do_ambiente(flag: Option<&Path>) -> Result<PathBuf, ErroAgente> {
    resolver_caminho_catalogo(
        flag,
        std::env::var_os(VAR_CATALOGO),
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
    )
}

/// Caminho do arquivo de trava do escritor: `<catálogo>.lock`.
#[must_use]
pub fn caminho_do_lock(catalogo: &Path) -> PathBuf {
    let mut s = catalogo.as_os_str().to_owned();
    s.push(".lock");
    PathBuf::from(s)
}

/// Cria o diretório do Catálogo (0700, se faltar) e o arquivo do banco vazio (0600, se faltar).
pub(crate) fn preparar(catalogo: &Path) -> Result<(), ErroAgente> {
    preparar_diretorio(catalogo)?;
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(catalogo)
        .map_err(|e| ErroAgente::es(format!("criar {}", catalogo.display()), e))?;
    Ok(())
}

/// Só o diretório pai (usado também para o arquivo de trava).
pub(crate) fn preparar_diretorio(arquivo: &Path) -> Result<(), ErroAgente> {
    if let Some(pai) = arquivo.parent().filter(|p| !p.as_os_str().is_empty()) {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(pai)
            .map_err(|e| ErroAgente::es(format!("criar diretório {}", pai.display()), e))?;
    }
    Ok(())
}

#[cfg(test)]
mod testes {
    use super::*;

    fn o(s: &str) -> Option<OsString> {
        Some(s.into())
    }

    #[test]
    fn precedencia_flag_env_xdg_home() {
        let f = Path::new("/f/c.sqlite");
        assert_eq!(
            resolver_caminho_catalogo(Some(f), o("/e/c"), o("/x"), o("/h")).unwrap(),
            PathBuf::from("/f/c.sqlite")
        );
        assert_eq!(
            resolver_caminho_catalogo(None, o("/e/c"), o("/x"), o("/h")).unwrap(),
            PathBuf::from("/e/c")
        );
        assert_eq!(
            resolver_caminho_catalogo(None, o(""), o("/x"), o("/h")).unwrap(),
            PathBuf::from("/x/dag/catalogo.sqlite")
        );
        assert_eq!(
            resolver_caminho_catalogo(None, None, o("relativo"), o("/h")).unwrap(),
            PathBuf::from("/h/.local/share/dag/catalogo.sqlite")
        );
        assert!(resolver_caminho_catalogo(None, None, None, None).is_err());
    }

    #[test]
    fn lock_ao_lado_do_catalogo() {
        assert_eq!(
            caminho_do_lock(Path::new("/a/b.sqlite")),
            PathBuf::from("/a/b.sqlite.lock")
        );
    }
}
