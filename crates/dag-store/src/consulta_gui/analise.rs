//! Dados da página Análise (Tree View e Treemap): só o nível atual mais um nível de filhos,
//! limitado (topo `limite` + "outros"). A base "computador" soma cada Volume uma única vez, então
//! um Volume com vários pontos de montagem (bind mounts) não conta em dobro.

use rusqlite::{Connection, OptionalExtension, params};

use super::navegacao::{info_da_pasta, trilha_da_pasta};
use super::{n, totais_do_catalogo};
use crate::Resultado;

/// Onde a análise está.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlvoAnalise {
    /// Todos os Volumes (cada um uma vez).
    Computador,
    Volume(i64),
    Pasta(i64),
}

/// Identidade de um bloco do nível.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChaveNo {
    Volume(i64),
    Pasta(i64),
    Arquivo(i64),
}

/// Um filho do nível.
#[derive(Debug, Clone, PartialEq)]
pub struct NoAnalise {
    pub chave: ChaveNo,
    pub nome: String,
    pub bytes: u64,
    pub alocado: Option<u64>,
    /// Arquivo: categoria do formato. Pasta: categoria predominante entre os arquivos diretos
    /// (por bytes, sobre os 2.000 maiores).
    pub categoria: Option<String>,
    pub finalidade: Option<String>,
    pub mtime_ns: Option<i64>,
    /// Pasta: arquivos regulares na subárvore.
    pub arquivos: Option<u64>,
    /// Caminho catalogado de pasta ou arquivo (`None` para Volume).
    pub caminho: Option<String>,
    /// Arquivo Na Lixeira.
    pub na_lixeira: bool,
    /// Marcação direta (arquivo) ou regra (pasta), para o menu de contexto.
    pub marca: Option<super::navegacao::MarcaVista>,
}

impl NoAnalise {
    /// Pasta ou Volume (dá para descer).
    #[must_use]
    pub fn navegavel(&self) -> bool {
        !matches!(self.chave, ChaveNo::Arquivo(_))
    }
}

/// Um elo do breadcrumb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elo {
    pub nome: String,
    pub alvo: AlvoAnalise,
}

/// Resultado da consulta de um nível.
#[derive(Debug, Clone, PartialEq)]
pub struct Analise {
    pub alvo: AlvoAnalise,
    pub trilha: Vec<Elo>,
    /// Total do nível (a "pasta-mãe" para as barras dos filhos).
    pub bytes_nivel: u64,
    /// Total do Volume do nível; `None` no nível Computador.
    pub bytes_volume: Option<u64>,
    /// Soma dos Volumes (uma vez cada).
    pub bytes_computador: u64,
    /// Número de pastas acima do nível dentro do Volume (0 = raiz do Volume/Computador).
    pub profundidade: u32,
    pub filhos: Vec<NoAnalise>,
    /// Bytes do nível que não aparecem em `filhos` (cauda cortada pelo limite).
    pub outros_bytes: u64,
    /// Havia mais filhos do que o limite.
    pub truncado: bool,
}

fn rotulo_volume(rotulo: Option<String>, montagem: Option<String>, id: i64) -> String {
    rotulo
        .filter(|r| !r.is_empty())
        .or(montagem)
        .unwrap_or_else(|| format!("Volume {id}"))
}

fn bytes_do_volume(conn: &Connection, v: i64) -> Resultado<Option<u64>> {
    Ok(conn
        .query_row(
            "SELECT logical_bytes FROM volume_stats WHERE volume_id = ?1",
            [v],
            |l| l.get::<_, i64>(0),
        )
        .optional()?
        .map(n))
}

fn nome_do_volume(conn: &Connection, v: i64) -> Resultado<String> {
    let (r, m): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT v.rotulo, (SELECT path_display FROM mount_points m WHERE m.volume_id = v.id
                              ORDER BY length(m.path_raw), m.id LIMIT 1)
             FROM volumes v WHERE v.id = ?1",
            [v],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )
        .optional()?
        .unwrap_or((None, None));
    Ok(rotulo_volume(r, m, v))
}

/// Categoria predominante (por bytes) entre os arquivos diretos da pasta.
fn categoria_predominante(conn: &Connection, pasta: i64) -> Resultado<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT t.broad_category FROM (
                 SELECT mime_id, logical_bytes FROM files WHERE folder_id = ?1
                 ORDER BY logical_bytes DESC LIMIT 2000) a
             JOIN file_types t ON t.id = a.mime_id
             WHERE t.broad_category IS NOT NULL
             GROUP BY t.broad_category ORDER BY SUM(a.logical_bytes) DESC LIMIT 1",
            [pasta],
            |l| l.get(0),
        )
        .optional()?)
}

fn pastas_filhas(
    conn: &Connection,
    sql_where: &str,
    parametro: i64,
    limite: usize,
) -> Resultado<Vec<NoAnalise>> {
    let sql = format!(
        "SELECT f.id, f.name_display, COALESCE(s.logical_bytes, 0), s.allocated_entry_bytes,
                s.recursive_file_count, f.mtime_ns, c.category, f.path_display,
                (SELECT m.tipo || '|' || m.autor || '|' || m.criada_em FROM marcacoes m
                  WHERE m.folder_id = f.id ORDER BY m.tipo DESC LIMIT 1)
         FROM folders f LEFT JOIN folder_stats s ON s.folder_id = f.id
         LEFT JOIN classifications c ON c.id = f.class_id
         WHERE {sql_where}
         ORDER BY COALESCE(s.logical_bytes, 0) DESC, f.id LIMIT ?2"
    );
    let mut st = conn.prepare_cached(&sql)?;
    let mut v: Vec<NoAnalise> = st
        .query_map(params![parametro, limite as i64], |l| {
            Ok(NoAnalise {
                chave: ChaveNo::Pasta(l.get(0)?),
                nome: l.get(1)?,
                bytes: n(l.get(2)?),
                alocado: l.get::<_, Option<i64>>(3)?.map(n),
                arquivos: l.get::<_, Option<i64>>(4)?.map(n),
                mtime_ns: l.get(5)?,
                finalidade: l.get(6)?,
                categoria: None,
                caminho: Some(l.get(7)?),
                na_lixeira: false,
                marca: super::navegacao::MarcaVista::de_texto(l.get(8)?),
            })
        })?
        .collect::<Result<_, _>>()?;
    for p in &mut v {
        if let ChaveNo::Pasta(id) = p.chave {
            p.categoria = categoria_predominante(conn, id)?;
        }
    }
    Ok(v)
}

fn arquivos_filhos(conn: &Connection, pasta: i64, limite: usize) -> Resultado<Vec<NoAnalise>> {
    let mut st = conn.prepare_cached(
        "SELECT a.id, a.name_display, a.logical_bytes, a.allocated_bytes, a.mtime_ns,
                t.broad_category, c.category, a.path_display, a.na_lixeira,
                (SELECT m.tipo || '|' || m.autor || '|' || m.criada_em FROM marcacoes m
                  WHERE m.file_id = a.id ORDER BY m.tipo DESC LIMIT 1)
         FROM files a LEFT JOIN file_types t ON t.id = a.mime_id
         LEFT JOIN classifications c ON c.id = a.class_id
         WHERE a.folder_id = ?1 ORDER BY a.logical_bytes DESC, a.id LIMIT ?2",
    )?;
    Ok(st
        .query_map(params![pasta, limite as i64], |l| {
            Ok(NoAnalise {
                chave: ChaveNo::Arquivo(l.get(0)?),
                nome: l.get(1)?,
                bytes: n(l.get(2)?),
                alocado: l.get::<_, Option<i64>>(3)?.map(n),
                mtime_ns: l.get(4)?,
                categoria: l.get(5)?,
                finalidade: l.get(6)?,
                arquivos: None,
                caminho: Some(l.get(7)?),
                na_lixeira: l.get::<_, i64>(8)? != 0,
                marca: super::navegacao::MarcaVista::de_texto(l.get(9)?),
            })
        })?
        .collect::<Result<_, _>>()?)
}

/// Consulta o nível `alvo`: no máximo `limite` filhos (≥ 1), mais `outros_bytes`.
///
/// # Errors
/// Falha do SQLite.
pub fn analisar(conn: &Connection, alvo: AlvoAnalise, limite: usize) -> Resultado<Analise> {
    let limite = limite.max(1);
    let bytes_computador = totais_do_catalogo(conn)?.bytes_logicos;
    let raiz = Elo {
        nome: "Computador".into(),
        alvo: AlvoAnalise::Computador,
    };
    let (trilha, bytes_nivel, bytes_volume, profundidade, mut filhos, mut truncado) = match alvo {
        AlvoAnalise::Computador => {
            let mut st = conn.prepare(
                "SELECT v.id, v.rotulo, (SELECT path_display FROM mount_points m
                                         WHERE m.volume_id = v.id
                                         ORDER BY length(m.path_raw), m.id LIMIT 1),
                        COALESCE(s.logical_bytes, 0), s.allocated_entry_bytes, s.file_count
                 FROM volumes v LEFT JOIN volume_stats s ON s.volume_id = v.id
                 ORDER BY COALESCE(s.logical_bytes, 0) DESC, v.id LIMIT ?1",
            )?;
            let filhos: Vec<NoAnalise> = st
                .query_map([limite as i64 + 1], |l| {
                    let id: i64 = l.get(0)?;
                    Ok(NoAnalise {
                        caminho: None,
                        na_lixeira: false,
                        marca: None,
                        chave: ChaveNo::Volume(id),
                        nome: rotulo_volume(l.get(1)?, l.get(2)?, id),
                        bytes: n(l.get(3)?),
                        alocado: l.get::<_, Option<i64>>(4)?.map(n),
                        arquivos: l.get::<_, Option<i64>>(5)?.map(n),
                        categoria: None,
                        finalidade: None,
                        mtime_ns: None,
                    })
                })?
                .collect::<Result<_, _>>()?;
            (vec![raiz], bytes_computador, None, 0, filhos, false)
        }
        AlvoAnalise::Volume(v) => {
            let nome = nome_do_volume(conn, v)?;
            let total = bytes_do_volume(conn, v)?.unwrap_or(0);
            let filhos = pastas_filhas(
                conn,
                "f.volume_id = ?1 AND f.parent_folder_id IS NULL",
                v,
                limite + 1,
            )?;
            let trilha = vec![
                raiz,
                Elo {
                    nome,
                    alvo: AlvoAnalise::Volume(v),
                },
            ];
            (trilha, total, Some(total), 0, filhos, false)
        }
        AlvoAnalise::Pasta(id) => {
            let info = info_da_pasta(conn, id)?;
            let volume = info.as_ref().map_or(0, |i| i.volume_id);
            let mut trilha = vec![
                raiz,
                Elo {
                    nome: nome_do_volume(conn, volume)?,
                    alvo: AlvoAnalise::Volume(volume),
                },
            ];
            let ancestrais = trilha_da_pasta(conn, id)?;
            let profundidade = u32::try_from(ancestrais.len().saturating_sub(1)).unwrap_or(0);
            trilha.extend(ancestrais.iter().map(|a| Elo {
                nome: a.nome.clone(),
                alvo: AlvoAnalise::Pasta(a.id),
            }));
            let total: u64 = conn
                .query_row(
                    "SELECT logical_bytes FROM folder_stats WHERE folder_id = ?1",
                    [id],
                    |l| l.get::<_, i64>(0),
                )
                .optional()?
                .map_or(0, n);
            let mut filhos = pastas_filhas(conn, "f.parent_folder_id = ?1", id, limite + 1)?;
            filhos.extend(arquivos_filhos(conn, id, limite + 1)?);
            (
                trilha,
                total,
                bytes_do_volume(conn, volume)?,
                profundidade,
                filhos,
                false,
            )
        }
    };
    // Pastas antes de arquivos no desempate; ordem estável por tamanho.
    filhos.sort_by(|a, b| {
        let ordem = |x: &NoAnalise| match x.chave {
            ChaveNo::Volume(i) | ChaveNo::Pasta(i) => (0u8, i),
            ChaveNo::Arquivo(i) => (1u8, i),
        };
        b.bytes.cmp(&a.bytes).then(ordem(a).cmp(&ordem(b)))
    });
    if filhos.len() > limite {
        filhos.truncate(limite);
        truncado = true;
    }
    let mostrado: u64 = filhos.iter().map(|f| f.bytes).sum();
    Ok(Analise {
        alvo,
        trilha,
        bytes_nivel,
        bytes_volume,
        bytes_computador,
        profundidade,
        filhos,
        outros_bytes: bytes_nivel.saturating_sub(mostrado),
        truncado,
    })
}
