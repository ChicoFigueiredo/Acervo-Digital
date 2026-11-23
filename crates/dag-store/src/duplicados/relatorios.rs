//! Relatórios de Cobertura (A-28): resumo por Volume e pasta, e arquivos sem Cobertura.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};

use dag_core::{
    ArquivoId, ArquivoSemCobertura, CoberturaArquivo, CoberturaPasta, CoberturaVolume,
    DadosRelatorio, HashCompleto, PastaId, VolumeId,
};
use rusqlite::Connection;

use super::cobertura::cobertura_do_grupo;
use super::{
    ErroDuplicados, HASH_VALIDO, Resultado, carregar_grupo, eh_unica_copia, hash_de,
    representante_sql, u,
};

/// Arquivo físico (representante) com sua Cobertura.
struct ArquivoCob {
    id: i64,
    volume: i64,
    rotulo: Option<String>,
    pasta: i64,
    caminho: String,
    tamanho: u64,
    hash: Option<HashCompleto>,
    em_grupo: bool,
    cob: CoberturaArquivo,
}

/// Percorre os arquivos regulares ativos não vazios (um por Arquivo Físico) com `id > depois`,
/// em ordem de id. `f` devolve `false` para parar.
fn percorrer_cobertura(
    conn: &Connection,
    volume: Option<i64>,
    depois: i64,
    mut f: impl FnMut(ArquivoCob) -> Resultado<bool>,
) -> Resultado<()> {
    let mut st = conn.prepare(&format!(
        "SELECT f.id, f.volume_id, v.rotulo, f.folder_id, f.path_display, f.logical_bytes,
                h.hash, gm.grupo_id
         FROM files f JOIN volumes v ON v.id = f.volume_id
         LEFT JOIN hashes h ON h.file_id = f.id AND {HASH_VALIDO}
         LEFT JOIN grupo_membros gm ON gm.file_id = f.id
         WHERE f.id > ?1 AND f.state = 'ativo' AND f.kind = 'arquivo' AND f.logical_bytes > 0
           AND (?2 IS NULL OR f.volume_id = ?2)
           AND (f.hardlink_group IS NULL OR f.id = {rep})
         ORDER BY f.id",
        rep = representante_sql("f")
    ))?;
    let mut cache: HashMap<i64, HashMap<i64, CoberturaArquivo>> = HashMap::new();
    let mut linhas = st.query(rusqlite::params![depois, volume])?;
    while let Some(l) = linhas.next()? {
        let id: i64 = l.get(0)?;
        let hash: Option<Vec<u8>> = l.get(6)?;
        let grupo: Option<i64> = l.get(7)?;
        let cob = match grupo {
            Some(g) => {
                if cache.len() > 4000 {
                    cache.clear();
                }
                let mapa = match cache.entry(g) {
                    Entry::Occupied(o) => o.into_mut(),
                    Entry::Vacant(v) => v.insert(
                        carregar_grupo(conn, g)?
                            .map(|gr| {
                                let cobs = cobertura_do_grupo(&gr);
                                gr.copias.iter().map(|c| c.arquivo).zip(cobs).collect()
                            })
                            .unwrap_or_default(),
                    ),
                };
                mapa.get(&id).copied().unwrap_or(CoberturaArquivo {
                    coberto: false,
                    nao_verificada_desde: None,
                })
            }
            None => CoberturaArquivo {
                coberto: false,
                nao_verificada_desde: None,
            },
        };
        let continuar = f(ArquivoCob {
            id,
            volume: l.get(1)?,
            rotulo: l.get(2)?,
            pasta: l.get(3)?,
            caminho: l.get(4)?,
            tamanho: u(l.get(5)?),
            hash: hash.as_deref().and_then(hash_de),
            em_grupo: grupo.is_some(),
            cob,
        })?;
        if !continuar {
            break;
        }
    }
    Ok(())
}

fn ler_cursor_id(c: Option<&str>) -> Resultado<i64> {
    match c {
        None => Ok(0),
        Some(c) => c
            .parse()
            .map_err(|_| ErroDuplicados::Invalido(format!("cursor inválido: `{c}`"))),
    }
}

/// Arquivos sem Cobertura (inclui os que não têm nenhuma outra cópia), por id, paginados.
///
/// # Errors
/// [`ErroDuplicados::Invalido`] para cursor mal formado; falha do SQLite.
pub fn relatorio_sem_cobertura(
    conn: &Connection,
    volume: Option<i64>,
    limite: u32,
    cursor: Option<&str>,
) -> Resultado<(DadosRelatorio, Option<String>)> {
    let depois = ler_cursor_id(cursor)?;
    let limite = limite.max(1) as usize;
    let mut arquivos = Vec::new();
    let mut proximo = None;
    percorrer_cobertura(conn, volume, depois, |a| {
        if a.cob.coberto {
            return Ok(true);
        }
        if arquivos.len() == limite {
            proximo = arquivos
                .last()
                .map(|x: &ArquivoSemCobertura| x.arquivo.0.to_string());
            return Ok(false);
        }
        let unica = a.hash.is_some() && !a.em_grupo && eh_unica_copia(conn, a.id)?;
        arquivos.push(ArquivoSemCobertura {
            arquivo: ArquivoId(a.id),
            volume: VolumeId(a.volume),
            volume_rotulo: a.rotulo,
            caminho: a.caminho,
            tamanho: a.tamanho,
            hash: a.hash,
            unica_copia: unica,
        });
        Ok(true)
    })?;
    Ok((DadosRelatorio::SemCobertura { arquivos }, proximo))
}

/// Resumo de Cobertura por Volume (todos) e por pasta (as com arquivo sem Cobertura, por id,
/// paginadas; conta os arquivos diretamente na pasta).
///
/// # Errors
/// [`ErroDuplicados::Invalido`] para cursor mal formado; falha do SQLite.
pub fn relatorio_cobertura(
    conn: &Connection,
    volume: Option<i64>,
    limite: u32,
    cursor: Option<&str>,
) -> Resultado<(DadosRelatorio, Option<String>)> {
    let depois = ler_cursor_id(cursor)?;
    let mut volumes: BTreeMap<i64, CoberturaVolume> = BTreeMap::new();
    let mut pastas: BTreeMap<i64, CoberturaPasta> = BTreeMap::new();
    percorrer_cobertura(conn, volume, 0, |a| {
        let v = volumes.entry(a.volume).or_insert_with(|| CoberturaVolume {
            volume: VolumeId(a.volume),
            rotulo: a.rotulo.clone(),
            conectado: true,
            arquivos: 0,
            com_cobertura: 0,
            cobertura_nao_verificada: 0,
            sem_cobertura: 0,
            bytes_sem_cobertura: 0,
        });
        let p = pastas.entry(a.pasta).or_insert_with(|| CoberturaPasta {
            pasta: PastaId(a.pasta),
            volume: VolumeId(a.volume),
            caminho: String::new(),
            arquivos: 0,
            com_cobertura: 0,
            sem_cobertura: 0,
            bytes_sem_cobertura: 0,
        });
        v.arquivos += 1;
        p.arquivos += 1;
        if a.cob.coberto {
            v.com_cobertura += 1;
            p.com_cobertura += 1;
            if a.cob.nao_verificada_desde.is_some() {
                v.cobertura_nao_verificada += 1;
            }
        } else {
            v.sem_cobertura += 1;
            v.bytes_sem_cobertura += a.tamanho;
            p.sem_cobertura += 1;
            p.bytes_sem_cobertura += a.tamanho;
        }
        Ok(true)
    })?;
    let mut st = conn.prepare("SELECT mounted FROM volumes WHERE id = ?1")?;
    for (id, v) in &mut volumes {
        v.conectado = st.query_row([id], |l| l.get::<_, i64>(0))? != 0;
    }
    let limite = limite.max(1) as usize;
    let mut st = conn.prepare("SELECT path_display FROM folders WHERE id = ?1")?;
    let mut lista = Vec::new();
    let mut proximo = None;
    for (id, mut p) in pastas
        .into_iter()
        .filter(|(id, p)| *id > depois && p.sem_cobertura > 0)
    {
        if lista.len() == limite {
            proximo = lista.last().map(|x: &CoberturaPasta| x.pasta.0.to_string());
            break;
        }
        p.caminho = st.query_row([id], |l| l.get(0))?;
        lista.push(p);
    }
    Ok((
        DadosRelatorio::Cobertura {
            volumes: volumes.into_values().collect(),
            pastas: lista,
        },
        proximo,
    ))
}
