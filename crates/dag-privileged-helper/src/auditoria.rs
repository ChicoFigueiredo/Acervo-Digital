//! Auditoria append-only: uma linha por pedido para o syslog (`/dev/log`) e, se existir e for
//! gravável, `/var/log/dag-helper.log` (aberto só com `O_APPEND`, nunca criado aqui).
//! É a única escrita que o helper faz. Falhas de auditoria nunca derrubam o pedido.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::net::UnixDatagram;
use std::path::Path;

pub const ARQUIVO_LOG: &str = "/var/log/dag-helper.log";
const SOCKET_SYSLOG: &str = "/dev/log";

/// Texto seguro para uma linha de log (sem quebras de linha nem controles).
#[must_use]
pub fn escapar(s: &str) -> String {
    s.escape_debug().to_string()
}

/// Monta a linha de auditoria.
#[must_use]
pub fn linha(uid: u32, caminho: &str, resultado: &str) -> String {
    format!(
        "uid={uid} caminho=\"{}\" resultado={}",
        escapar(caminho),
        escapar(resultado)
    )
}

/// Grava no syslog (facility authpriv, nível info) e no arquivo de log, se houver.
pub fn registrar(texto: &str) {
    let pid = std::process::id();
    if let Ok(s) = UnixDatagram::unbound() {
        let msg = format!("<86>dag-helper[{pid}]: {texto}");
        let _ = s.send_to(msg.as_bytes(), SOCKET_SYSLOG);
    }
    registrar_em(Path::new(ARQUIVO_LOG), texto);
}

/// Acrescenta ao arquivo, só se já existir. Visível para testes.
pub fn registrar_em(arquivo: &Path, texto: &str) {
    if let Ok(mut f) = OpenOptions::new().append(true).open(arquivo) {
        let _ = writeln!(f, "dag-helper[{}]: {texto}", std::process::id());
    }
}
