//! `dag-helper`: leitura **somente de metadados** de uma pasta protegida, com privilégio
//! concedido sob demanda pelo Polkit (`pkexec`). Requisito A-16.
//!
//! * Aceita um único formato de pedido ([`Requisicao`]); qualquer outra coisa é recusada.
//! * Não abre conteúdo de arquivo, não calcula hash, não escreve nada além da linha de auditoria,
//!   não executa comandos e não toca no Catálogo SQLite.
//! * Segue o caminho por componente com `openat`/`O_NOFOLLOW`, recusando symlinks.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod auditoria;
mod listagem;
mod protocolo;
mod validacao;

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub use listagem::{ErroListagem, listar};
pub use protocolo::{
    Linha, Metadados, Requisicao, TipoArquivo, VERSAO_PROTOCOLO, codificar_bytes, decodificar_bytes,
};
pub use validacao::{
    ErroValidacao, MAX_RAIZES, MAX_REQUISICAO, Validado, interpretar, ler_politica, validar,
};

/// Caminho padrão do helper instalado.
pub const CAMINHO_HELPER_PADRAO: &str = "/usr/libexec/dag/dag-helper";
/// Política opcional do administrador (raízes permitidas, uma por linha).
pub const ARQUIVO_POLITICA: &str = "/etc/dag/helper.roots";

/// Resultado de um pedido, para o código de saída e a auditoria.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desfecho {
    pub caminho: String,
    pub resultado: String,
    pub ok: bool,
}

fn escrever(saida: &mut dyn Write, l: &Linha) -> io::Result<()> {
    serde_json::to_writer(&mut *saida, l).map_err(io::Error::other)?;
    saida.write_all(b"\n")
}

/// Atende um pedido: lê o JSON de `entrada` e escreve as linhas em `saida`.
pub fn atender(
    entrada: &mut dyn Read,
    saida: &mut dyn Write,
    politica: Option<&[PathBuf]>,
) -> Desfecho {
    let mut buf = Vec::new();
    let lido = entrada.take(MAX_REQUISICAO + 1).read_to_end(&mut buf);
    let falha = |saida: &mut dyn Write, caminho: String, codigo: &str, msg: String| {
        let _ = escrever(
            saida,
            &Linha::Erro {
                codigo: codigo.into(),
                mensagem: msg,
            },
        );
        Desfecho {
            caminho,
            resultado: format!("recusado:{codigo}"),
            ok: false,
        }
    };
    if let Err(e) = lido {
        return falha(saida, String::new(), "requisicao_invalida", e.to_string());
    }
    if buf.len() as u64 > MAX_REQUISICAO {
        return falha(
            saida,
            String::new(),
            "requisicao_invalida",
            "requisição grande demais".into(),
        );
    }
    let req = match interpretar(&buf) {
        Ok(r) => r,
        Err(e) => return falha(saida, String::new(), e.codigo(), e.to_string()),
    };
    let validado = match validar(&req, politica) {
        Ok(v) => v,
        Err(e) => return falha(saida, req.caminho.clone(), e.codigo(), e.to_string()),
    };
    let res = listar(&validado, &mut |l| escrever(saida, &l));
    match res {
        Ok(n) => {
            let _ = escrever(saida, &Linha::Fim { entradas: n });
            Desfecho {
                caminho: req.caminho,
                resultado: format!("ok entradas={n}"),
                ok: true,
            }
        }
        Err(e) => falha(saida, req.caminho, e.codigo(), e.to_string()),
    }
}

/// Uid de quem pediu: `PKEXEC_UID` (definido pelo pkexec) ou o uid real do processo.
#[must_use]
pub fn uid_solicitante() -> u32 {
    std::env::var("PKEXEC_UID")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| rustix::process::getuid().as_raw())
}

/// Ponto de entrada do binário: atende stdin→stdout, audita e devolve o código de saída.
#[must_use]
pub fn principal() -> i32 {
    let politica = ler_politica(Path::new(ARQUIVO_POLITICA));
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut saida = io::BufWriter::new(stdout.lock());
    let d = atender(&mut stdin.lock(), &mut saida, politica.as_deref());
    let _ = saida.flush();
    auditoria::registrar(&auditoria::linha(
        uid_solicitante(),
        &d.caminho,
        &d.resultado,
    ));
    i32::from(!d.ok) * 2
}
