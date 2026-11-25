//! Única porta de escrita do `dag-cli`: arquivo NOVO pedido pelo usuário (relatório do
//! benchmark, casos do conjunto ouro). Cria com exclusividade e nunca sobrescreve nem apaga,
//! como `dag-store/src/duplicados/exportar.rs`. Nada catalogado é tocado.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Cria `caminho` (que não pode existir) e grava `conteudo`.
///
/// # Errors
/// Texto em pt-BR: o arquivo já existe (não sobrescrevo), a pasta não existe ou falha de E/S.
pub fn criar_novo(caminho: &Path, conteudo: &[u8]) -> Result<(), String> {
    let mut arq = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(caminho)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => format!(
                "{} já existe; não sobrescrevo (escolha outro nome ou apague você mesmo)",
                caminho.display()
            ),
            std::io::ErrorKind::NotFound => format!(
                "a pasta de {} não existe; crie-a antes (mkdir)",
                caminho.display()
            ),
            _ => format!("não consegui criar {}: {e}", caminho.display()),
        })?;
    arq.write_all(conteudo)
        .and_then(|()| arq.sync_all())
        .map_err(|e| format!("falha ao gravar {}: {e}", caminho.display()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod testes {
    use super::*;

    #[test]
    fn cria_e_nunca_sobrescreve() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.json");
        criar_novo(&p, b"um").unwrap();
        let e = criar_novo(&p, b"dois").unwrap_err();
        assert!(e.contains("já existe"), "{e}");
        assert_eq!(std::fs::read(&p).unwrap(), b"um");
        let e = criar_novo(&t.path().join("x/y.json"), b"z").unwrap_err();
        assert!(e.contains("não existe"), "{e}");
    }
}
