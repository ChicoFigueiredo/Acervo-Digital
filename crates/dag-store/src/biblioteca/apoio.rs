//! Consultas de apoio da integração da Curadoria no agente (somente leitura do Catálogo, mais
//! duas limpezas de sugestões pendentes). Tudo aditivo: nada aqui altera `files`, `hashes` nem
//! as Marcações da Fase A.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::{Alerta, Pagina, TipoAlerta, hash_de_blob, limite_de, texto_enum};
use crate::Resultado;
use crate::duplicados::{HASH_VALIDO, carregar_grupo, cobertura_do_arquivo};

/// Um arquivo do Catálogo com o que a curadoria precisa saber dele.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InfoArquivo {
    pub file_id: i64,
    pub volume_id: i64,
    pub rotulo_volume: Option<String>,
    pub dispositivo_id: Option<i64>,
    pub volume_conectado: bool,
    pub path_raw: Vec<u8>,
    pub caminho: String,
    pub nome: String,
    pub tamanho: u64,
    pub mtime_ns: Option<i64>,
    pub ativo: bool,
    pub na_lixeira: bool,
    /// Hash BLAKE3 do Catálogo, só se a evidência (tamanho, mtime, ctime, inode) ainda vale.
    pub hash: Option<[u8; 32]>,
    /// `ok` ou `divergente` (Verificação de Integridade), se houve.
    pub verificacao: Option<String>,
    pub native_dev: Option<i64>,
    pub native_file_id: Option<i64>,
    pub nlink: Option<i64>,
    pub alocado: Option<u64>,
}

impl InfoArquivo {
    /// Caminho de verdade no disco, a partir dos bytes (`path_raw`). `caminho` é só para exibir e
    /// perde bytes inválidos em UTF-8 (nomes antigos em Latin-1): abrir por ele falha.
    #[must_use]
    pub fn caminho_no_disco(&self) -> std::path::PathBuf {
        use std::os::unix::ffi::OsStrExt;
        std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&self.path_raw))
    }
}

const SELECT_INFO: &str =
    "SELECT f.id, f.volume_id, v.rotulo, v.device_id, v.mounted, f.path_raw, f.path_display,
        f.name_display, f.logical_bytes, f.mtime_ns, f.state, f.na_lixeira,
        CASE WHEN h.file_id IS NOT NULL AND (HASH_VALIDO) THEN h.hash END,
        h.verification_state, f.native_dev, f.native_file_id, f.nlink, f.allocated_bytes
 FROM files f JOIN volumes v ON v.id = f.volume_id LEFT JOIN hashes h ON h.file_id = f.id";

fn de_linha_info(l: &rusqlite::Row<'_>) -> rusqlite::Result<InfoArquivo> {
    Ok(InfoArquivo {
        file_id: l.get(0)?,
        volume_id: l.get(1)?,
        rotulo_volume: l.get(2)?,
        dispositivo_id: l.get(3)?,
        volume_conectado: l.get::<_, i64>(4)? != 0,
        path_raw: l.get(5)?,
        caminho: l.get(6)?,
        nome: l.get(7)?,
        tamanho: l.get::<_, i64>(8)?.max(0) as u64,
        mtime_ns: l.get(9)?,
        ativo: l.get::<_, String>(10)? == "ativo",
        na_lixeira: l.get::<_, i64>(11)? != 0,
        hash: hash_de_blob(l.get(12)?),
        verificacao: l.get(13)?,
        native_dev: l.get(14)?,
        native_file_id: l.get(15)?,
        nlink: l.get(16)?,
        alocado: l.get::<_, Option<i64>>(17)?.map(|x| x.max(0) as u64),
    })
}

fn sql_info(onde: &str) -> String {
    format!(
        "{} WHERE {onde}",
        SELECT_INFO.replace("HASH_VALIDO", HASH_VALIDO)
    )
}

/// Informações de um arquivo (ou `None` se o id não existe).
///
/// # Errors
/// Falha do SQLite.
pub fn info_arquivo(conn: &Connection, file_id: i64) -> Resultado<Option<InfoArquivo>> {
    Ok(conn
        .query_row(&sql_info("f.id = ?1"), [file_id], de_linha_info)
        .optional()?)
}

/// Arquivo ativo por Volume e caminho bruto (usa o índice `(volume_id, path_raw)`).
///
/// # Errors
/// Falha do SQLite.
pub fn info_arquivo_por_caminho(
    conn: &Connection,
    volume_id: i64,
    path_raw: &[u8],
) -> Resultado<Option<InfoArquivo>> {
    Ok(conn
        .query_row(
            &sql_info("f.volume_id = ?1 AND f.path_raw = ?2"),
            params![volume_id, path_raw],
            de_linha_info,
        )
        .optional()?)
}

/// Volume cujo ponto de montagem é o maior prefixo do caminho.
///
/// # Errors
/// Falha do SQLite.
pub fn volume_do_caminho(conn: &Connection, caminho: &[u8]) -> Resultado<Option<i64>> {
    let mut st = conn.prepare("SELECT volume_id, path_raw FROM mount_points")?;
    let linhas = st
        .query_map([], |l| Ok((l.get::<_, i64>(0)?, l.get::<_, Vec<u8>>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut melhor: Option<(i64, usize)> = None;
    for (v, mp) in linhas {
        let dentro = caminho == mp.as_slice()
            || (caminho.starts_with(&mp)
                && (mp.last() == Some(&b'/') || caminho.get(mp.len()) == Some(&b'/')));
        if dentro && melhor.is_none_or(|(_, n)| mp.len() > n) {
            melhor = Some((v, mp.len()));
        }
    }
    Ok(melhor.map(|(v, _)| v))
}

/// Volumes que têm arquivos sob o prefixo (um por ponto de montagem relacionado ao caminho).
///
/// # Errors
/// Falha do SQLite.
pub fn volumes_sob_prefixo(conn: &Connection, prefixo: &[u8]) -> Resultado<Vec<i64>> {
    let mut st = conn.prepare("SELECT volume_id, path_raw FROM mount_points")?;
    let linhas = st
        .query_map([], |l| Ok((l.get::<_, i64>(0)?, l.get::<_, Vec<u8>>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut v: Vec<i64> = linhas
        .into_iter()
        .filter(|(_, mp)| mp.starts_with(prefixo) || prefixo.starts_with(mp))
        .map(|(v, _)| v)
        .collect();
    v.sort_unstable();
    v.dedup();
    Ok(v)
}

/// Linha de listagem de arquivos de um prefixo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArquivoListado {
    pub file_id: i64,
    pub path_raw: Vec<u8>,
    pub nome: String,
    pub tamanho: u64,
    pub mtime_ns: Option<i64>,
}

/// Arquivos regulares ativos de um Volume sob `[lo, hi)` (faixa de `path_raw`), depois de
/// `apos` (keyset por caminho), em ordem de caminho.
///
/// # Errors
/// Falha do SQLite.
pub fn arquivos_na_faixa(
    conn: &Connection,
    volume_id: i64,
    lo: &[u8],
    hi: &[u8],
    apos: &[u8],
    limite: usize,
) -> Resultado<Vec<ArquivoListado>> {
    let inicio = if apos > lo { apos } else { lo };
    let incl = if apos > lo { ">" } else { ">=" };
    let mut st = conn.prepare(&format!(
        "SELECT id, path_raw, name_display, logical_bytes, mtime_ns FROM files
         WHERE volume_id = ?1 AND path_raw {incl} ?2 AND path_raw < ?3
           AND state = 'ativo' AND kind = 'arquivo' AND na_lixeira = 0
         ORDER BY path_raw LIMIT ?4"
    ))?;
    let v = st
        .query_map(params![volume_id, inicio, hi, limite as i64], |l| {
            Ok(ArquivoListado {
                file_id: l.get(0)?,
                path_raw: l.get(1)?,
                nome: l.get(2)?,
                tamanho: l.get::<_, i64>(3)?.max(0) as u64,
                mtime_ns: l.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// O arquivo é o arquivo de algum Formato homologado (está dentro da Homologada)?
///
/// # Errors
/// Falha do SQLite.
pub fn e_formato_homologado(conn: &Connection, file_id: i64) -> Resultado<bool> {
    Ok(conn
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM formatos_homologados WHERE file_id = ?1)")?
        .query_row([file_id], |l| l.get::<_, i64>(0))?
        != 0)
}

/// Existe arquivo ativo com este caminho bruto neste Volume?
///
/// # Errors
/// Falha do SQLite.
pub fn existe_caminho(conn: &Connection, volume_id: i64, path_raw: &[u8]) -> Resultado<bool> {
    Ok(conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM files WHERE volume_id = ?1 AND path_raw = ?2 AND state = 'ativo')",
        )?
        .query_row(params![volume_id, path_raw], |l| l.get::<_, i64>(0))?
        != 0)
}

/// Uma cópia no Grupo de Duplicados de um arquivo (Fase A), com Cobertura.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CopiaDoGrupo {
    pub file_id: i64,
    pub volume_id: i64,
    pub rotulo_volume: Option<String>,
    pub dispositivo_id: Option<i64>,
    pub conectado: bool,
    pub caminho: String,
    /// Tem Cobertura (Marcação de Backup em outro Dispositivo Físico).
    pub coberta: bool,
    /// Esta cópia carrega Marcação de Backup ativa.
    pub marcacao_backup: bool,
    pub na_lixeira: bool,
    pub hash_conferido: bool,
}

/// Cópias do Grupo de Duplicados do arquivo (vazio se o arquivo não está em nenhum grupo).
///
/// # Errors
/// Falha do SQLite.
pub fn copias_do_grupo(conn: &Connection, file_id: i64) -> Resultado<Vec<CopiaDoGrupo>> {
    let Some(grupo) = crate::duplicados::grupo_do_arquivo(conn, file_id)
        .map_err(|e| super::invalido(e.to_string()))?
    else {
        return Ok(Vec::new());
    };
    let Some(g) = carregar_grupo(conn, grupo).map_err(|e| super::invalido(e.to_string()))? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(g.copias.len());
    for c in &g.copias {
        let (lixeira, verif): (i64, Option<String>) = conn.query_row(
            "SELECT f.na_lixeira, h.verification_state FROM files f
             LEFT JOIN hashes h ON h.file_id = f.id WHERE f.id = ?1",
            [c.arquivo],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )?;
        out.push(CopiaDoGrupo {
            file_id: c.arquivo,
            volume_id: c.volume,
            rotulo_volume: c.rotulo.clone(),
            dispositivo_id: c.dispositivo,
            conectado: c.conectado,
            caminho: c.caminho.clone(),
            coberta: cobertura_do_arquivo(conn, c.arquivo)
                .map_err(|e| super::invalido(e.to_string()))?
                .coberto,
            marcacao_backup: c.marca.is_some_and(|m| {
                m.tipo == dag_core::TipoMarcacao::Backup
                    && m.estado == dag_core::EstadoMarcacao::Ativa
            }),
            na_lixeira: lixeira != 0,
            hash_conferido: verif.as_deref() == Some("ok"),
        });
    }
    Ok(out)
}

/// Evento de novidade do monitor/Fast Scan (arquivo criado, alterado ou renomeado).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventoNovidade {
    pub id: i64,
    pub tipo: String,
    pub volume_id: Option<i64>,
    pub file_id: Option<i64>,
    pub path_raw: Option<Vec<u8>>,
    pub observado_em: i64,
}

/// Eventos `criado`/`alterado`/`renomeado` com id maior que `apos` (em ordem de id).
///
/// # Errors
/// Falha do SQLite.
pub fn eventos_de_novidade(
    conn: &Connection,
    apos: i64,
    limite: usize,
) -> Resultado<Vec<EventoNovidade>> {
    let mut st = conn.prepare(
        "SELECT id, event_type, volume_id, file_id, path_raw, observed_at FROM scan_events
         WHERE id > ?1 AND event_type IN ('criado','alterado','renomeado') ORDER BY id LIMIT ?2",
    )?;
    let v = st
        .query_map(params![apos, limite as i64], |l| {
            Ok(EventoNovidade {
                id: l.get(0)?,
                tipo: l.get(1)?,
                volume_id: l.get(2)?,
                file_id: l.get(3)?,
                path_raw: l.get(4)?,
                observado_em: l.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Maior id de `scan_events` (ponto de partida do watcher numa instalação nova).
///
/// # Errors
/// Falha do SQLite.
pub fn ultimo_evento_id(conn: &Connection) -> Resultado<i64> {
    Ok(
        conn.query_row("SELECT COALESCE(MAX(id), 0) FROM scan_events", [], |l| {
            l.get(0)
        })?,
    )
}

/// Hash do arquivo na última extração gravada (para detectar conteúdo que mudou).
///
/// # Errors
/// Falha do SQLite.
pub fn hash_da_extracao(conn: &Connection, file_id: i64) -> Resultado<Option<[u8; 32]>> {
    Ok(hash_de_blob(
        conn.query_row(
            "SELECT hash_arquivo FROM extracoes WHERE file_id = ?1 ORDER BY criado_em DESC, id DESC LIMIT 1",
            [file_id],
            |l| l.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()?
        .flatten(),
    ))
}

/// O conteúdo mudou: some o que dependia do conteúdo antigo (sugestões pendentes, extração e
/// candidatos de duplicidade do arquivo). Decisões humanas ficam no histórico.
///
/// # Errors
/// Falha do SQLite.
pub fn desassociar_conteudo(conn: &Connection, file_id: i64) -> Resultado<u64> {
    let mut n = conn.execute(
        "DELETE FROM sugestoes WHERE file_id = ?1 AND estado = 'pendente'",
        [file_id],
    )? as u64;
    n += conn.execute("DELETE FROM extracoes WHERE file_id = ?1", [file_id])? as u64;
    n += conn.execute(
        "DELETE FROM candidatos_duplicidade WHERE file_id = ?1 AND estado = 'aberto'",
        [file_id],
    )? as u64;
    Ok(n)
}

/// Apaga sugestões pendentes do arquivo (para a versão de regras) que não estão em `manter`.
///
/// # Errors
/// Falha do SQLite.
pub fn limpar_pendentes_exceto(
    conn: &Connection,
    file_id: i64,
    versao_regras: &str,
    manter: &[i64],
) -> Resultado<u64> {
    let ids = serde_json::to_string(manter)?;
    Ok(conn.execute(
        "DELETE FROM sugestoes WHERE file_id = ?1 AND versao_regras = ?2 AND estado = 'pendente'
           AND id NOT IN (SELECT value FROM json_each(?3))",
        params![file_id, versao_regras, ids],
    )? as u64)
}

/// Livros homologados com o mesmo título normalizado.
///
/// # Errors
/// Falha do SQLite.
pub fn livros_por_titulo_norm(
    conn: &Connection,
    titulo_norm: &str,
    limite: usize,
) -> Resultado<Vec<super::LivroHomologado>> {
    let mut st = conn
        .prepare("SELECT id FROM livros_homologados WHERE titulo_norm = ?1 ORDER BY id LIMIT ?2")?;
    let ids = st
        .query_map(params![titulo_norm, limite as i64], |l| l.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut v = Vec::new();
    for id in ids {
        if let Some(l) = super::obter_livro(conn, id)? {
            v.push(l);
        }
    }
    Ok(v)
}

/// Todos os alertas, do mais novo ao mais antigo (keyset por id).
///
/// # Errors
/// Falha do SQLite.
pub fn listar_alertas(
    conn: &Connection,
    antes_de_id: Option<i64>,
    limite: usize,
) -> Resultado<Pagina<Alerta, i64>> {
    let limite = limite_de(limite);
    let mut st =
        conn.prepare("SELECT id FROM alertas_biblio WHERE id < ?1 ORDER BY id DESC LIMIT ?2")?;
    let ids = st
        .query_map(
            params![antes_de_id.unwrap_or(i64::MAX), (limite + 1) as i64],
            |l| l.get::<_, i64>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut itens = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(a) = super::obter_alerta(conn, id)? {
            itens.push(a);
        }
    }
    Ok(Pagina::de_linhas(itens, limite, |a| a.id))
}

/// Alertas de um tipo desde um instante (contagem).
///
/// # Errors
/// Falha do SQLite.
pub fn contar_alertas_tipo(conn: &Connection, tipo: TipoAlerta, desde: i64) -> Resultado<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM alertas_biblio WHERE tipo = ?1 AND criado_em >= ?2",
        params![texto_enum(&tipo), desde],
        |l| l.get(0),
    )?)
}

/// Quantos jobs entraram na fila desde o instante.
///
/// # Errors
/// Falha do SQLite.
pub fn jobs_enfileirados_desde(conn: &Connection, desde: i64) -> Resultado<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM jobs_bibliograficos WHERE enfileirado_em >= ?1",
        [desde],
        |l| l.get(0),
    )?)
}

/// Obra, edição e formato bibliográfico do arquivo (para a revisão), se já existirem.
///
/// # Errors
/// Falha do SQLite.
pub fn obra_edicao_do_arquivo(conn: &Connection, file_id: i64) -> Resultado<Option<(i64, i64)>> {
    Ok(conn
        .query_row(
            "SELECT e.obra_id, e.id FROM formatos_bibliograficos fb JOIN edicoes e ON e.id = fb.edicao_id
             WHERE fb.file_id = ?1 ORDER BY fb.id DESC LIMIT 1",
            [file_id],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )
        .optional()?)
}

/// Contagens para os testes e o resumo: obras, edições, formatos bibliográficos.
///
/// # Errors
/// Falha do SQLite.
pub fn contagens_obras(conn: &Connection) -> Resultado<(i64, i64, i64)> {
    Ok(conn.query_row(
        "SELECT (SELECT COUNT(*) FROM obras), (SELECT COUNT(*) FROM edicoes),
                (SELECT COUNT(*) FROM formatos_bibliograficos)",
        [],
        |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)),
    )?)
}
