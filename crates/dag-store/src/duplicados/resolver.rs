//! Resolução de referências do usuário (rótulo, caminho, id) para ids do Catálogo.

use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use dag_core::Alvo;
use rusqlite::Connection;

use super::{ErroDuplicados, Resultado, representante_sql};

/// Resolve um Volume por id numérico, rótulo, `platform_id` ou ponto de montagem.
///
/// # Errors
/// [`ErroDuplicados::NaoEncontrado`] ou [`ErroDuplicados::Ambiguo`].
pub fn resolver_volume(conn: &Connection, ref_: &str) -> Resultado<i64> {
    let ref_ = ref_.trim();
    if let Ok(id) = ref_.parse::<i64>()
        && let Ok(id) = conn.query_row("SELECT id FROM volumes WHERE id = ?1", [id], |l| l.get(0))
    {
        return Ok(id);
    }
    let candidatos = |sql: &str, p: &dyn rusqlite::ToSql| -> Resultado<Vec<i64>> {
        let mut st = conn.prepare(sql)?;
        Ok(st
            .query_map([p], |l| l.get(0))?
            .collect::<Result<Vec<_>, _>>()?)
    };
    for sql in [
        "SELECT id FROM volumes WHERE rotulo = ?1 ORDER BY id",
        "SELECT id FROM volumes WHERE lower(rotulo) = lower(?1) ORDER BY id",
        "SELECT id FROM volumes WHERE platform_id = ?1",
    ] {
        let v = candidatos(sql, &ref_)?;
        match v.len() {
            0 => {}
            1 => return Ok(v[0]),
            _ => {
                return Err(ErroDuplicados::Ambiguo(format!(
                    "mais de um Volume casa com `{ref_}`; use o id numérico (`dag volumes`)"
                )));
            }
        }
    }
    let mp = caminho_bytes(ref_);
    let v = candidatos(
        "SELECT DISTINCT volume_id FROM mount_points WHERE path_raw = ?1",
        &mp,
    )?;
    match v.len() {
        1 => Ok(v[0]),
        0 => Err(ErroDuplicados::NaoEncontrado(format!(
            "Volume não encontrado: `{ref_}` (use id, rótulo ou ponto de montagem; veja `dag volumes`)"
        ))),
        _ => Err(ErroDuplicados::Ambiguo(format!(
            "mais de um Volume com o ponto de montagem `{ref_}`; use o id numérico"
        ))),
    }
}

fn caminho_bytes(c: &str) -> Vec<u8> {
    let b = c.as_bytes();
    if b.len() > 1 && b.ends_with(b"/") {
        b[..b.len() - 1].to_vec()
    } else {
        b.to_vec()
    }
}

/// Resolve o caminho de uma pasta catalogada. Devolve `(folder_id, volume_id)`. Prefere Volume
/// conectado; se ainda houver empate, é ambíguo.
///
/// # Errors
/// [`ErroDuplicados::NaoEncontrado`] ou [`ErroDuplicados::Ambiguo`].
pub fn resolver_pasta(conn: &Connection, caminho: &str) -> Resultado<(i64, i64)> {
    let bruto = caminho_bytes(caminho);
    let achar = |b: &[u8]| -> Resultado<Vec<(i64, i64, bool)>> {
        let mut st = conn.prepare(
            "SELECT f.id, f.volume_id, v.mounted FROM folders f JOIN volumes v ON v.id = f.volume_id
             WHERE f.path_raw = ?1 AND f.state <> 'removido' ORDER BY f.id",
        )?;
        Ok(st
            .query_map([b], |l| {
                Ok((l.get(0)?, l.get(1)?, l.get::<_, i64>(2)? != 0))
            })?
            .collect::<Result<Vec<_>, _>>()?)
    };
    let mut v = achar(&bruto)?;
    if v.is_empty()
        && let Ok(c) = std::fs::canonicalize(Path::new(caminho))
    {
        v = achar(c.as_os_str().as_bytes())?;
    }
    escolher(v, || format!("pasta não catalogada: {caminho}"), caminho)
}

fn escolher(
    mut v: Vec<(i64, i64, bool)>,
    msg: impl FnOnce() -> String,
    caminho: &str,
) -> Resultado<(i64, i64)> {
    if v.is_empty() {
        return Err(ErroDuplicados::NaoEncontrado(msg()));
    }
    if v.len() > 1 {
        let conectados: Vec<_> = v.iter().copied().filter(|x| x.2).collect();
        if !conectados.is_empty() {
            v = conectados;
        }
    }
    if v.len() > 1 {
        return Err(ErroDuplicados::Ambiguo(format!(
            "o caminho `{caminho}` existe em mais de um Volume do Catálogo; use o id numérico"
        )));
    }
    Ok((v[0].0, v[0].1))
}

/// Resolve um [`Alvo`] de arquivo (`Caminho` ou `Arquivo`) para o `files.id` do **Arquivo
/// Físico** (representante do hard link).
///
/// # Errors
/// [`ErroDuplicados::NaoEncontrado`], [`ErroDuplicados::Ambiguo`] ou
/// [`ErroDuplicados::Invalido`] se o alvo não é um arquivo.
pub fn resolver_arquivo(conn: &Connection, alvo: &Alvo) -> Resultado<i64> {
    let id = match alvo {
        Alvo::Arquivo(a) => {
            let ok: bool = conn
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM files WHERE id = ?1 AND kind = 'arquivo')",
                    [a.0],
                    |l| l.get::<_, i64>(0),
                )
                .map(|n| n != 0)?;
            if !ok {
                return Err(ErroDuplicados::NaoEncontrado(format!(
                    "arquivo {a} não existe no Catálogo"
                )));
            }
            a.0
        }
        Alvo::Caminho(c) => {
            let bruto = caminho_bytes(c);
            let achar = |b: &[u8]| -> Resultado<Vec<(i64, i64, bool)>> {
                let mut st = conn.prepare(
                    "SELECT f.id, f.volume_id, v.mounted FROM files f JOIN volumes v ON v.id = f.volume_id
                     WHERE f.path_raw = ?1 AND f.kind = 'arquivo' AND f.state <> 'removido' ORDER BY f.id",
                )?;
                Ok(st
                    .query_map([b], |l| {
                        Ok((l.get(0)?, l.get(1)?, l.get::<_, i64>(2)? != 0))
                    })?
                    .collect::<Result<Vec<_>, _>>()?)
            };
            let mut v = achar(&bruto)?;
            if v.is_empty()
                && let Ok(p) = std::fs::canonicalize(Path::new(c))
            {
                v = achar(p.as_os_str().as_bytes())?;
            }
            escolher(v, || format!("arquivo não catalogado: {c}"), c)?.0
        }
        _ => {
            return Err(ErroDuplicados::Invalido(
                "o alvo deve ser um arquivo (caminho ou id)".into(),
            ));
        }
    };
    representante(conn, id)
}

/// `files.id` que representa o Arquivo Físico de `file_id`.
pub(crate) fn representante(conn: &Connection, file_id: i64) -> Resultado<i64> {
    let sql = format!(
        "SELECT {} FROM files f WHERE f.id = ?1",
        representante_sql("f")
    );
    Ok(conn.query_row(&sql, [file_id], |l| l.get(0))?)
}
