//! Leitores dos compactados suportados.

mod rar;
mod sete_z;
mod tar;
mod zip;

use std::io::{self, Read};
use std::sync::Arc;

use crate::FabricaLeitor;

pub(crate) fn fabricas_padrao() -> Vec<Arc<dyn FabricaLeitor>> {
    vec![
        Arc::new(zip::FabricaZip),
        Arc::new(sete_z::FabricaSeteZ),
        Arc::new(rar::FabricaRar),
        Arc::new(tar::FabricaTar::simples()),
        Arc::new(tar::FabricaTar::comprimida(tar::Compressao::Gzip)),
        Arc::new(tar::FabricaTar::comprimida(tar::Compressao::Bzip2)),
        Arc::new(tar::FabricaTar::comprimida(tar::Compressao::Xz)),
        Arc::new(tar::FabricaTar::comprimida(tar::Compressao::Zstd)),
    ]
}

/// Lê até `buf.len()` bytes (ou o fim).
pub(crate) fn ler_ate_encher(r: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Leitor que falha sempre com o erro dado: representa um Membro ilegível sem derrubar o resto.
#[derive(Debug)]
pub(crate) struct LeitorComErro(pub io::ErrorKind, pub String);

impl Read for LeitorComErro {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(self.0, self.1.clone()))
    }
}
