//! Configurações do usuário persistidas no Catálogo (tabela `configuracoes`, chave/valor).

use dag_core::OpcoesHash;
use rusqlite::{Connection, OptionalExtension, params};

use crate::Resultado;

const CHAVE_THREADS: &str = "hash.threads_por_disco";
const CHAVE_DISCOS: &str = "hash.max_discos";

/// Valor de uma chave, se existir.
///
/// # Errors
/// Falha do SQLite.
pub fn ler(conn: &Connection, chave: &str) -> Resultado<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT valor FROM configuracoes WHERE chave = ?1",
            [chave],
            |l| l.get(0),
        )
        .optional()?)
}

/// Grava (ou troca) o valor de uma chave.
///
/// # Errors
/// Falha do SQLite.
pub fn gravar(conn: &Connection, chave: &str, valor: &str) -> Resultado<()> {
    conn.execute(
        "INSERT INTO configuracoes (chave, valor) VALUES (?1, ?2)
         ON CONFLICT(chave) DO UPDATE SET valor = ?2",
        params![chave, valor],
    )?;
    Ok(())
}

fn ler_u32(conn: &Connection, chave: &str) -> Resultado<Option<u32>> {
    Ok(ler(conn, chave)?.and_then(|v| v.trim().parse().ok()))
}

/// Paralelismo do Hash Completo persistido (campos ausentes ou ilegíveis = `None`).
///
/// # Errors
/// Falha do SQLite.
pub fn ler_hash(conn: &Connection) -> Resultado<OpcoesHash> {
    Ok(OpcoesHash {
        threads_por_disco: ler_u32(conn, CHAVE_THREADS)?,
        max_discos: ler_u32(conn, CHAVE_DISCOS)?,
    })
}

/// Grava os campos definidos de `o` (os `None` ficam como estavam). O chamador valida antes.
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_hash(conn: &Connection, o: &OpcoesHash) -> Resultado<()> {
    if let Some(t) = o.threads_por_disco {
        gravar(conn, CHAVE_THREADS, &t.to_string())?;
    }
    if let Some(d) = o.max_discos {
        gravar(conn, CHAVE_DISCOS, &d.to_string())?;
    }
    Ok(())
}
