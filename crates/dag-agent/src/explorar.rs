//! `Comando::Explorar`: filhos diretos de uma pasta por tamanho, paginados por cursor opaco.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use dag_core::{EntradaExploracao, Finalidade, Resposta, TipoEntradaExploracao};
use dag_store::inventario::consulta::{
    self, CursorFilhos, Filho, TipoFilho, buscar_pasta, estatisticas_da_pasta, filhos_por_tamanho,
};
use rusqlite::{Connection, OptionalExtension};

use crate::erro::ErroAgente;
use crate::finalidade::de_nome;

const LIMITE_PADRAO: usize = 50;
const LIMITE_MAXIMO: usize = 1000;

fn u(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

fn codificar_cursor(c: CursorFilhos) -> String {
    format!("{}.{}.{}", c.bytes, c.ordem, c.id)
}

fn decodificar_cursor(s: &str) -> Result<CursorFilhos, ErroAgente> {
    let invalido = || ErroAgente::Invalido(format!("cursor inválido: `{s}`"));
    let mut p = s.split('.');
    let bytes = p.next().and_then(|x| x.parse().ok()).ok_or_else(invalido)?;
    let ordem: u8 = p.next().and_then(|x| x.parse().ok()).ok_or_else(invalido)?;
    let id = p.next().and_then(|x| x.parse().ok()).ok_or_else(invalido)?;
    if p.next().is_some() || ordem > 1 {
        return Err(invalido());
    }
    Ok(CursorFilhos { bytes, ordem, id })
}

/// Caminho absoluto sem barra final; relativo vira absoluto pelo `canonicalize` (a pasta
/// precisa existir) ou, se sumiu, pelo diretório atual.
fn normalizar(pasta: &str) -> Result<Vec<u8>, ErroAgente> {
    let p = Path::new(pasta);
    let abs: PathBuf = if let Ok(c) = std::fs::canonicalize(p) {
        c
    } else if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| ErroAgente::es("diretório atual", e))?
            .join(p)
    };
    let mut b = abs.as_os_str().as_bytes().to_vec();
    while b.len() > 1 && b.last() == Some(&b'/') {
        b.pop();
    }
    Ok(b)
}

fn achar_pasta(conn: &Connection, caminho: &[u8]) -> Result<Option<i64>, ErroAgente> {
    let mut st = conn.prepare("SELECT id FROM volumes ORDER BY id")?;
    let volumes: Vec<i64> = st.query_map([], |l| l.get(0))?.collect::<Result<_, _>>()?;
    for v in volumes {
        if let Some(id) = buscar_pasta(conn, v, caminho)? {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn finalidade_de(
    conn: &Connection,
    tabela: &str,
    id: i64,
) -> Result<Option<Finalidade>, ErroAgente> {
    let sql = format!(
        "SELECT c.category FROM {tabela} t JOIN classifications c ON c.id = t.class_id WHERE t.id = ?1"
    );
    let cat: Option<String> = conn.query_row(&sql, [id], |l| l.get(0)).optional()?;
    Ok(cat.as_deref().and_then(de_nome))
}

fn entrada(conn: &Connection, f: &Filho) -> Result<EntradaExploracao, ErroAgente> {
    let mut e = EntradaExploracao {
        tipo: match f.tipo {
            TipoFilho::Pasta => TipoEntradaExploracao::Pasta,
            TipoFilho::Arquivo => TipoEntradaExploracao::Arquivo,
        },
        nome: f.nome_display.clone(),
        bytes_logicos: u(f.bytes_logicos),
        bytes_alocados: f.bytes_alocados.map(u),
        arquivos: None,
        pastas: None,
        kind: f.kind.clone(),
        mime: None,
        categoria: None,
        finalidade: None,
        estado_agregado: f.estado_agregado.clone(),
        mtime_ns: f.mtime_ns,
    };
    match f.tipo {
        TipoFilho::Pasta => {
            if let Some(s) = estatisticas_da_pasta(conn, f.id)? {
                e.arquivos = Some(u(s.arquivos));
                e.pastas = Some(u(s.pastas));
            }
            e.finalidade = finalidade_de(conn, "folders", f.id)?;
        }
        TipoFilho::Arquivo => {
            let t: Option<(Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT t.mime, t.broad_category FROM files a
                     JOIN file_types t ON t.id = a.mime_id WHERE a.id = ?1",
                    [f.id],
                    |l| Ok((l.get(0)?, l.get(1)?)),
                )
                .optional()?;
            if let Some((mime, cat)) = t {
                e.mime = mime;
                e.categoria = cat;
            }
            e.finalidade = finalidade_de(conn, "files", f.id)?;
        }
    }
    Ok(e)
}

/// Raízes catalogadas (pastas sem pai), de todos os volumes, por tamanho.
fn raizes(conn: &Connection, limite: usize) -> Result<Vec<EntradaExploracao>, ErroAgente> {
    let mut st = conn.prepare("SELECT id FROM volumes ORDER BY id")?;
    let volumes: Vec<i64> = st.query_map([], |l| l.get(0))?.collect::<Result<_, _>>()?;
    let mut saida = Vec::new();
    for v in volumes {
        for (id, path_raw) in consulta::pastas_raiz(conn, v)? {
            let s = estatisticas_da_pasta(conn, id)?;
            let nome = dag_store::inventario::exibir(&path_raw);
            let (mtime, estado): (Option<i64>, Option<String>) = (
                conn.query_row("SELECT mtime_ns FROM folders WHERE id = ?1", [id], |l| {
                    l.get(0)
                })?,
                s.as_ref().map(|s| s.estado.clone()),
            );
            saida.push(Filho {
                tipo: TipoFilho::Pasta,
                id,
                nome_raw: path_raw,
                nome_display: nome,
                bytes_logicos: s.as_ref().map_or(0, |s| s.bytes_logicos),
                bytes_alocados: s.as_ref().and_then(|s| s.bytes_alocados_por_entrada),
                mtime_ns: mtime,
                kind: None,
                estado_agregado: estado,
            });
        }
    }
    saida.sort_by(|a, b| b.bytes_logicos.cmp(&a.bytes_logicos).then(a.id.cmp(&b.id)));
    saida.truncate(limite);
    saida.iter().map(|f| entrada(conn, f)).collect()
}

pub(crate) fn explorar(
    conn: &Connection,
    pasta: Option<&str>,
    limite: Option<u32>,
    cursor: Option<&str>,
) -> Result<Resposta, ErroAgente> {
    let limite = limite.map_or(LIMITE_PADRAO, |l| (l as usize).clamp(1, LIMITE_MAXIMO));
    let Some(pasta) = pasta else {
        return Ok(Resposta::Explorar {
            pasta: None,
            entradas: raizes(conn, limite)?,
            proximo_cursor: None,
        });
    };
    let caminho = normalizar(pasta)?;
    let id = achar_pasta(conn, &caminho)?.ok_or_else(|| {
        ErroAgente::NaoEncontrado(format!(
            "pasta não catalogada: {} (rode `dag scan --full` sobre ela ou sobre um ancestral)",
            dag_store::inventario::exibir(&caminho)
        ))
    })?;
    let cursor = cursor.map(decodificar_cursor).transpose()?;
    let pagina = filhos_por_tamanho(conn, id, limite, cursor)?;
    let entradas = pagina
        .itens
        .iter()
        .map(|f| entrada(conn, f))
        .collect::<Result<_, _>>()?;
    Ok(Resposta::Explorar {
        pasta: Some(dag_store::inventario::exibir(&caminho)),
        entradas,
        proximo_cursor: pagina.proximo.map(codificar_cursor),
    })
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn cursor_ida_e_volta() {
        let c = CursorFilhos {
            bytes: 10,
            ordem: 1,
            id: 7,
        };
        let s = codificar_cursor(c);
        assert_eq!(decodificar_cursor(&s).unwrap(), c);
        assert!(decodificar_cursor("x").is_err());
        assert!(decodificar_cursor("1.2.3").is_err());
        assert!(decodificar_cursor("1.1.3.4").is_err());
    }
}
