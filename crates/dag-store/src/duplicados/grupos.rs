//! Grupos de Duplicados: reconstrução incremental, Cópia Principal, listagem e detalhe.

use dag_core::regras::{Copia, escolher_copia_principal};
use dag_core::{
    ArquivoId, Autor, ClasseRecuperavel, CopiaGrupo, DetalheGrupo, DispositivoId, GrupoDuplicados,
    GrupoId, MarcacaoDaCopia, ModoPrincipal, OrdemDuplicados, ResumoDuplicados, VolumeId,
};
use rusqlite::{Connection, OptionalExtension, params};

use super::agregado;
use super::cobertura::cobertura_do_grupo;
use super::espaco::dividir;
use super::marcacoes::converter_marcas_de_grupo;
use super::resolver::{representante, resolver_arquivo, resolver_volume};
use super::{
    ErroDuplicados, GrupoCarregado, HASH_VALIDO, Resultado, carregar_grupo, percorrer_grupos,
    representante_sql, u,
};
use crate::Catalogo;

/// O que reconstruir.
#[derive(Debug, Clone, Copy)]
pub enum Escopo<'a> {
    /// Todos os Grupos (só as diferenças são gravadas).
    Tudo,
    /// Só os grupos destes `(tamanho, hash)`. Quem chama passa as chaves antigas **e** novas
    /// dos arquivos que mudaram: um arquivo que saiu de `K1` e entrou em `K2` exige as duas.
    Hashes(&'a [(u64, [u8; 32])]),
}

/// O que a reconstrução mudou.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResumoReconstrucao {
    pub grupos_criados: u64,
    pub grupos_removidos: u64,
    pub membros_adicionados: u64,
    pub membros_removidos: u64,
    pub principais_alteradas: u64,
    /// Marcações de grupo que viraram Marcação de arquivo porque a cópia saiu do grupo.
    pub marcacoes_convertidas: u64,
}

/// (Re)constrói os Grupos de Duplicados a partir de `hashes`: chave `(tamanho > 0, hash)`,
/// entre todos os Volumes (inclusive desconectados), um membro por Arquivo Físico. É
/// idempotente e só grava diferenças, então o id do grupo e a escolha manual de Cópia Principal
/// sobrevivem (CA-A21). Em seguida reescolhe a Cópia Principal pela Prioridade de Volume.
///
/// Marcações de grupo cuja cópia deixou de pertencer ao grupo viram Marcações de arquivo
/// (evento `alterada` no histórico), para não serem apagadas junto com o grupo.
///
/// # Errors
/// Falha do SQLite.
pub fn reconstruir_grupos(
    cat: &mut Catalogo,
    escopo: Escopo<'_>,
    autor: Autor,
    agora: i64,
) -> Resultado<ResumoReconstrucao> {
    cat.armazem_mut()
        .transacao_imediata(|tx| -> Resultado<ResumoReconstrucao> {
            let mut r = ResumoReconstrucao::default();
            tx.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS _alvo (
                     file_id INTEGER PRIMARY KEY, logical_bytes INTEGER NOT NULL, hash BLOB NOT NULL);
                 CREATE TEMP TABLE IF NOT EXISTS _escopo (
                     logical_bytes INTEGER NOT NULL, hash BLOB NOT NULL,
                     PRIMARY KEY (logical_bytes, hash));
                 CREATE INDEX IF NOT EXISTS temp._alvo_chave ON _alvo(logical_bytes, hash);
                 DELETE FROM _alvo;
                 DELETE FROM _escopo;",
            )?;
            let (filtro_f, filtro_g) = match escopo {
                Escopo::Tudo => (String::new(), String::new()),
                Escopo::Hashes(chaves) => {
                    let mut ins = tx.prepare("INSERT OR IGNORE INTO _escopo VALUES (?1, ?2)")?;
                    for (t, h) in chaves {
                        ins.execute(params![*t as i64, h.as_slice()])?;
                    }
                    (
                        " AND EXISTS (SELECT 1 FROM _escopo e WHERE e.logical_bytes = f.logical_bytes AND e.hash = h.hash)"
                            .to_string(),
                        " AND EXISTS (SELECT 1 FROM _escopo e WHERE e.logical_bytes = g.logical_bytes AND e.hash = g.hash)"
                            .to_string(),
                    )
                }
            };
            // 1. Alvo: um Arquivo Físico por linha, só chaves com 2+ cópias. O pré-filtro pelo
            // índice `idx_hashes_dup` mantém a tabela temporária só com candidatos a duplicado
            // (e não uma linha por arquivo hasheado: dezenas de milhões).
            tx.execute(
                &format!(
                    "INSERT INTO _alvo (file_id, logical_bytes, hash)
                     SELECT f.id, f.logical_bytes, h.hash FROM files f JOIN hashes h ON h.file_id = f.id
                     WHERE f.state = 'ativo' AND f.kind = 'arquivo' AND f.logical_bytes > 0
                       AND f.na_lixeira = 0
                       AND {HASH_VALIDO}
                       AND (f.hardlink_group IS NULL OR f.id = {rep}){filtro_f}
                       AND (h.size_bytes, h.hash) IN (
                           SELECT size_bytes, hash FROM hashes
                           GROUP BY size_bytes, hash HAVING COUNT(*) >= 2)",
                    rep = representante_sql("f")
                ),
                [],
            )?;
            tx.execute(
                "DELETE FROM _alvo WHERE (logical_bytes, hash) IN (
                     SELECT logical_bytes, hash FROM _alvo GROUP BY logical_bytes, hash HAVING COUNT(*) < 2)",
                [],
            )?;
            // 2. Membros que saem: não estão no alvo, ou o alvo os põe em outra chave.
            let saindo = format!(
                "SELECT gm.file_id FROM grupo_membros gm JOIN grupos_duplicados g ON g.id = gm.grupo_id
                 LEFT JOIN _alvo a ON a.file_id = gm.file_id
                 WHERE (a.file_id IS NULL OR a.logical_bytes <> g.logical_bytes OR a.hash <> g.hash){filtro_g}"
            );
            // Marcações de grupo das cópias que saem (ou de grupos que vão sumir) viram de arquivo.
            r.marcacoes_convertidas += converter_marcas_de_grupo(
                tx,
                &format!(
                    "m.file_id IN ({saindo}) OR m.grupo_id IN (
                        SELECT g.id FROM grupos_duplicados g
                        WHERE NOT EXISTS (SELECT 1 FROM _alvo a WHERE a.logical_bytes = g.logical_bytes AND a.hash = g.hash){filtro_g})"
                ),
                autor,
                agora,
            )?;
            r.membros_removidos = tx.execute(
                &format!("DELETE FROM grupo_membros WHERE file_id IN ({saindo})"),
                [],
            )? as u64;
            // 3. Grupos novos e membros novos.
            r.grupos_criados = tx.execute(
                "INSERT INTO grupos_duplicados (logical_bytes, hash, atualizado_em)
                 SELECT DISTINCT logical_bytes, hash, ?1 FROM _alvo WHERE true
                 ON CONFLICT (logical_bytes, hash) DO NOTHING",
                [agora],
            )? as u64;
            r.membros_adicionados = tx.execute(
                "INSERT INTO grupo_membros (file_id, grupo_id)
                 SELECT a.file_id, g.id FROM _alvo a
                 JOIN grupos_duplicados g ON g.logical_bytes = a.logical_bytes AND g.hash = a.hash
                 WHERE true
                 ON CONFLICT (file_id) DO UPDATE SET grupo_id = excluded.grupo_id
                 WHERE grupo_membros.grupo_id <> excluded.grupo_id",
                [],
            )? as u64;
            // 4. Grupos vazios saem (as Marcações de grupo já foram convertidas acima).
            r.grupos_removidos = tx.execute(
                "DELETE FROM grupos_duplicados WHERE id NOT IN (SELECT DISTINCT grupo_id FROM grupo_membros)",
                [],
            )? as u64;
            // 5. Cópia Principal.
            r.principais_alteradas = recalcular_principais(tx, None, agora)?;
            tx.execute_batch("DELETE FROM _alvo; DELETE FROM _escopo;")?;
            if matches!(escopo, Escopo::Tudo) {
                // Os grupos refletem os hashes de agora (a transação é a escritora).
                tx.execute(
                    "UPDATE contadores SET valor = (SELECT valor FROM contadores WHERE chave = 'hashes_mudancas')
                     WHERE chave = 'grupos_em'",
                    [],
                )?;
            }
            Ok(r)
        })
}

struct EntradaPrioridade {
    id: i64,
    volume: i64,
    pasta: Option<Vec<u8>>,
}

fn sob(caminho: &[u8], base: &[u8]) -> bool {
    caminho == base
        || base == b"/"
        || (caminho.starts_with(base) && caminho.get(base.len()) == Some(&b'/'))
}

/// Reescolhe a Cópia Principal dos grupos em modo `auto` (todos, ou só `so_grupo`) e devolve
/// quantos mudaram. Cópia Principal manual que deixou de ser membro volta ao modo `auto`.
///
/// A regra de escolha é [`escolher_copia_principal`]: cada cópia é mapeada para a entrada de
/// prioridade mais específica que a cobre (pasta mais profunda, senão o Volume); essa entrada
/// faz o papel de "volume" na regra pura, e a ordem da lista é a coluna `ordem`.
pub(crate) fn recalcular_principais(
    conn: &Connection,
    so_grupo: Option<i64>,
    agora: i64,
) -> Resultado<u64> {
    let mut entradas: Vec<EntradaPrioridade> = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT p.id, p.volume_id, f.path_raw FROM prioridades p
             LEFT JOIN folders f ON f.id = p.folder_id ORDER BY p.ordem, p.id",
        )?;
        let linhas = st.query_map([], |l| {
            Ok(EntradaPrioridade {
                id: l.get(0)?,
                volume: l.get(1)?,
                pasta: l.get(2)?,
            })
        })?;
        for e in linhas {
            entradas.push(e?);
        }
    }
    let prioridade: Vec<VolumeId> = entradas.iter().map(|e| VolumeId(e.id)).collect();
    let ids: Vec<(i64, Option<i64>, String)> = {
        let mut st = conn.prepare(
            "SELECT id, copia_principal_file_id, copia_principal_modo FROM grupos_duplicados
             WHERE (?1 IS NULL OR id = ?1) ORDER BY id",
        )?;
        st.query_map([so_grupo], |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)))?
            .collect::<Result<_, _>>()?
    };
    let mut membros = conn.prepare(
        "SELECT f.id, f.volume_id, v.device_id, f.path_raw FROM grupo_membros gm
         JOIN files f ON f.id = gm.file_id JOIN volumes v ON v.id = f.volume_id
         WHERE gm.grupo_id = ?1 ORDER BY f.id",
    )?;
    let mut atualiza = conn.prepare(
        "UPDATE grupos_duplicados SET copia_principal_file_id = ?2, copia_principal_modo = 'auto',
                atualizado_em = ?3 WHERE id = ?1",
    )?;
    let mut mudou = 0;
    for (grupo, atual, modo) in ids {
        let copias: Vec<(Copia, Vec<u8>)> = membros
            .query_map([grupo], |l| {
                Ok((
                    Copia {
                        arquivo: ArquivoId(l.get(0)?),
                        volume: VolumeId(l.get(1)?),
                        dispositivo: DispositivoId(l.get::<_, Option<i64>>(2)?.unwrap_or(0)),
                    },
                    l.get::<_, Vec<u8>>(3)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        let eh_membro =
            |f: Option<i64>| f.is_some_and(|f| copias.iter().any(|c| c.0.arquivo.0 == f));
        if modo == "manual" && eh_membro(atual) {
            continue;
        }
        let com_slot: Vec<Copia> = copias
            .iter()
            .map(|(c, caminho)| {
                // Entrada mais específica: pasta mais profunda que cobre o caminho; senão o Volume.
                let pasta = entradas
                    .iter()
                    .filter(|e| e.volume == c.volume.0)
                    .filter_map(|e| e.pasta.as_deref().map(|p| (e.id, p)))
                    .filter(|(_, p)| sob(caminho, p))
                    .max_by_key(|(_, p)| p.len())
                    .map(|(id, _)| id);
                let slot = pasta.or_else(|| {
                    entradas
                        .iter()
                        .find(|e| e.volume == c.volume.0 && e.pasta.is_none())
                        .map(|e| e.id)
                });
                Copia {
                    volume: VolumeId(slot.unwrap_or(-1)),
                    ..*c
                }
            })
            .collect();
        let novo = escolher_copia_principal(&com_slot, &prioridade, None).map(|a| a.0);
        if novo != atual || modo != "auto" {
            atualiza.execute(params![grupo, novo, agora])?;
            mudou += 1;
        }
    }
    Ok(mudou)
}

/// Troca manual da Cópia Principal (`arquivo: Some`) ou volta à escolha automática (`None`).
///
/// # Errors
/// [`ErroDuplicados::NaoEncontrado`] se o grupo não existe; [`ErroDuplicados::Invalido`] se a
/// cópia não pertence ao grupo.
pub fn definir_copia_principal(
    cat: &mut Catalogo,
    grupo: GrupoId,
    arquivo: Option<&dag_core::Alvo>,
    agora: i64,
) -> Resultado<()> {
    cat.armazem_mut().transacao_imediata(|tx| {
        let existe: bool = tx
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM grupos_duplicados WHERE id = ?1)",
                [grupo.0],
                |l| l.get::<_, i64>(0),
            )
            .map(|n| n != 0)?;
        if !existe {
            return Err(ErroDuplicados::NaoEncontrado(format!(
                "grupo {grupo} não existe (os ids mudam quando o grupo se desfaz)"
            )));
        }
        match arquivo {
            Some(alvo) => {
                let f = resolver_arquivo(tx, alvo)?;
                let rep = representante(tx, f)?;
                let membro: bool = tx
                    .query_row(
                        "SELECT EXISTS (SELECT 1 FROM grupo_membros WHERE file_id = ?1 AND grupo_id = ?2)",
                        [rep, grupo.0],
                        |l| l.get::<_, i64>(0),
                    )
                    .map(|n| n != 0)?;
                if !membro {
                    return Err(ErroDuplicados::Invalido(format!(
                        "o arquivo {rep} não é cópia do grupo {grupo}"
                    )));
                }
                tx.execute(
                    "UPDATE grupos_duplicados SET copia_principal_file_id = ?2,
                            copia_principal_modo = 'manual', atualizado_em = ?3 WHERE id = ?1",
                    params![grupo.0, rep, agora],
                )?;
            }
            None => {
                tx.execute(
                    "UPDATE grupos_duplicados SET copia_principal_modo = 'auto' WHERE id = ?1",
                    [grupo.0],
                )?;
                recalcular_principais(tx, Some(grupo.0), agora)?;
            }
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------- consultas

fn resumo(g: &GrupoCarregado) -> GrupoDuplicados {
    let d = dividir(g);
    let marcados = d.marcadas.len() as u64 * g.tamanho;
    let estimados = d.estimadas.len() as u64 * g.tamanho;
    GrupoDuplicados {
        id: GrupoId(g.id),
        hash: g.hash,
        tamanho: g.tamanho,
        copias: g.copias.len() as u32,
        copias_offline: g.copias.iter().filter(|c| !c.conectado).count() as u32,
        copia_principal: g.principal.map(ArquivoId),
        modo_principal: if g.manual {
            ModoPrincipal::Manual
        } else {
            ModoPrincipal::Auto
        },
        bytes_marcados_delecao: marcados,
        bytes_redundantes_estimados: estimados,
        espaco_recuperavel_bytes: marcados + estimados,
    }
}

/// Filtros da listagem de grupos.
#[derive(Debug, Clone, Copy, Default)]
pub struct FiltroGrupos {
    /// Só grupos com ao menos uma cópia neste Volume.
    pub volume: Option<i64>,
    pub tamanho_minimo: u64,
    pub ordem: OrdemDuplicados,
}

/// Página de grupos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaginaGrupos {
    pub grupos: Vec<GrupoDuplicados>,
    /// Grupos que casam com o filtro, em todas as páginas.
    pub total: u64,
    pub proximo_cursor: Option<String>,
}

fn cursor_de(chave: u64, id: i64) -> String {
    format!("{chave}:{id}")
}

pub(crate) fn ler_cursor(c: Option<&str>) -> Resultado<Option<(u64, i64)>> {
    let Some(c) = c else { return Ok(None) };
    let invalido = || ErroDuplicados::Invalido(format!("cursor inválido: `{c}`"));
    let (a, b) = c.split_once(':').ok_or_else(invalido)?;
    Ok(Some((
        a.parse().map_err(|_| invalido())?,
        b.parse().map_err(|_| invalido())?,
    )))
}

/// Lista Grupos de Duplicados paginados por cursor (keyset sobre `(chave desc, id)`), onde a
/// chave é o espaço recuperável ou o tamanho. O cursor é opaco.
///
/// # Errors
/// [`ErroDuplicados::Invalido`] para cursor mal formado; falha do SQLite.
pub fn listar_grupos(
    conn: &Connection,
    filtro: &FiltroGrupos,
    limite: u32,
    cursor: Option<&str>,
) -> Resultado<PaginaGrupos> {
    let depois = ler_cursor(cursor)?;
    let limite = limite.max(1) as usize;
    let f = agregado::Filtro {
        volume: filtro.volume,
        tamanho_minimo: filtro.tamanho_minimo,
    };
    let total = agregado::total(conn, &f)?;
    let mut pagina = agregado::pagina(conn, &f, filtro.ordem, depois, limite + 1)?;
    let proximo_cursor = if pagina.len() > limite {
        pagina.truncate(limite);
        pagina.last().map(|(k, g)| cursor_de(*k, g.id.0))
    } else {
        None
    };
    Ok(PaginaGrupos {
        grupos: pagina.into_iter().map(|x| x.1).collect(),
        total,
        proximo_cursor,
    })
}

/// Versão de referência de [`listar_grupos`]: carrega todos os grupos na memória. Só para
/// testes de equivalência; não use em produção (memória proporcional ao número de grupos).
///
/// # Errors
/// Como [`listar_grupos`].
#[doc(hidden)]
pub fn listar_grupos_referencia(
    conn: &Connection,
    filtro: &FiltroGrupos,
    limite: u32,
    cursor: Option<&str>,
) -> Resultado<PaginaGrupos> {
    let depois = ler_cursor(cursor)?;
    let mut todos: Vec<(u64, GrupoDuplicados)> = Vec::new();
    percorrer_grupos(conn, |g| {
        if g.tamanho < filtro.tamanho_minimo {
            return Ok(true);
        }
        if filtro
            .volume
            .is_some_and(|v| !g.copias.iter().any(|c| c.volume == v))
        {
            return Ok(true);
        }
        let r = resumo(&g);
        let chave = match filtro.ordem {
            OrdemDuplicados::Recuperavel => r.espaco_recuperavel_bytes,
            OrdemDuplicados::Tamanho => r.tamanho,
        };
        todos.push((chave, r));
        Ok(true)
    })?;
    todos.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
    let total = todos.len() as u64;
    let limite = limite.max(1) as usize;
    let mut pagina: Vec<(u64, GrupoDuplicados)> = todos
        .into_iter()
        .filter(|(k, g)| match depois {
            None => true,
            Some((ck, ci)) => *k < ck || (*k == ck && g.id.0 > ci),
        })
        .take(limite + 1)
        .collect();
    let proximo_cursor = if pagina.len() > limite {
        pagina.truncate(limite);
        pagina.last().map(|(k, g)| cursor_de(*k, g.id.0))
    } else {
        None
    };
    Ok(PaginaGrupos {
        grupos: pagina.into_iter().map(|x| x.1).collect(),
        total,
        proximo_cursor,
    })
}

/// Converte a referência de Volume do usuário (id, rótulo, montagem) para o filtro.
///
/// # Errors
/// Como [`resolver_volume`].
pub fn filtro_por_volume(conn: &Connection, ref_: Option<&str>) -> Resultado<Option<i64>> {
    ref_.map(|r| resolver_volume(conn, r)).transpose()
}

/// Um grupo com todas as cópias, Marcações, Cobertura e a classe de espaço de cada cópia.
///
/// # Errors
/// [`ErroDuplicados::NaoEncontrado`] se o grupo não existe.
pub fn detalhar_grupo(conn: &Connection, grupo: GrupoId) -> Resultado<DetalheGrupo> {
    let g = carregar_grupo(conn, grupo.0)?.ok_or_else(|| {
        ErroDuplicados::NaoEncontrado(format!(
            "grupo {grupo} não existe (os ids mudam quando o grupo se desfaz)"
        ))
    })?;
    let d = dividir(&g);
    let cobs = cobertura_do_grupo(&g);
    let copias = g
        .copias
        .iter()
        .zip(cobs)
        .map(|(c, cob)| CopiaGrupo {
            arquivo: ArquivoId(c.arquivo),
            volume: VolumeId(c.volume),
            volume_rotulo: c.rotulo.clone(),
            dispositivo: c.dispositivo.map(DispositivoId),
            caminho: c.caminho.clone(),
            conectado: c.conectado,
            visto_em: if c.conectado { None } else { c.visto_em },
            principal: g.principal == Some(c.arquivo),
            outros_caminhos: c.outros_caminhos,
            marcacao: c.marca.map(|m| MarcacaoDaCopia {
                id: dag_core::MarcacaoId(m.id),
                tipo: m.tipo,
                estado: m.estado,
                escopo: m.escopo,
            }),
            cobertura: cob,
            recuperavel: if d.marcadas.contains(&ArquivoId(c.arquivo)) {
                ClasseRecuperavel::Marcada
            } else if d.estimadas.contains(&ArquivoId(c.arquivo)) {
                ClasseRecuperavel::Estimada
            } else {
                ClasseRecuperavel::Nao
            },
        })
        .collect();
    Ok(DetalheGrupo {
        grupo: resumo(&g),
        copias,
    })
}

/// Totais atuais de duplicados (para o resumo do fim da varredura).
///
/// # Errors
/// Falha do SQLite.
pub fn resumo_duplicados(conn: &Connection) -> Resultado<ResumoDuplicados> {
    let (grupos, copias, espaco) = agregado::totais(conn)?;
    let mut r = ResumoDuplicados {
        grupos,
        copias,
        espaco_recuperavel_bytes: espaco,
        ..ResumoDuplicados::default()
    };
    r.marcacoes_pendentes = u(conn.query_row(
        "SELECT COUNT(*) FROM marcacoes WHERE estado = 'pendente'",
        [],
        |l| l.get(0),
    )?);
    Ok(r)
}

/// Versão de referência de [`resumo_duplicados`] (carrega cada grupo na memória). Só testes.
///
/// # Errors
/// Falha do SQLite.
#[doc(hidden)]
pub fn resumo_duplicados_referencia(conn: &Connection) -> Resultado<ResumoDuplicados> {
    let mut r = ResumoDuplicados::default();
    percorrer_grupos(conn, |g| {
        let s = resumo(&g);
        r.grupos += 1;
        r.copias += u64::from(s.copias);
        r.espaco_recuperavel_bytes += s.espaco_recuperavel_bytes;
        Ok(true)
    })?;
    r.marcacoes_pendentes = u(conn.query_row(
        "SELECT COUNT(*) FROM marcacoes WHERE estado = 'pendente'",
        [],
        |l| l.get(0),
    )?);
    Ok(r)
}

/// Grupo do arquivo (pelo Arquivo Físico), se ele for cópia de algum.
///
/// # Errors
/// Falha do SQLite.
pub fn grupo_do_arquivo(conn: &Connection, file_id: i64) -> Resultado<Option<i64>> {
    let rep = representante(conn, file_id)?;
    Ok(conn
        .query_row(
            "SELECT grupo_id FROM grupo_membros WHERE file_id = ?1",
            [rep],
            |l| l.get(0),
        )
        .optional()?)
}
