//! Duplicados, Marcações, Cobertura e espaço recuperável (A-24..A-31).
//!
//! Tudo aqui é **somente Catálogo**: nenhuma função toca arquivos do acervo. Escritas (`&mut
//! Catalogo`) rodam numa única transação, inclusive a linha de `marcacoes_historico` que as
//! acompanha. Leituras (`&Connection`) são paginadas por cursor (keyset).
//!
//! # Conceitos
//!
//! * **Cópia** = Arquivo Físico. Os caminhos do Mesmo Arquivo Físico (hard links:
//!   `files.hardlink_group`) são uma cópia só, representada pelo menor `files.id` com Hash
//!   Completo válido; os demais caminhos não são membros de nada nem geram espaço recuperável.
//! * **Hash válido** = linha de `hashes` cuja evidência (tamanho, mtime, ctime, inode) ainda
//!   confere com `files`. Hash defasado é ignorado: o arquivo some do grupo até ser relido.
//! * **Marcação que vale para uma cópia**: a manual (arquivo/grupo) ou, na falta dela, a regra
//!   da pasta mais específica ou a regra do Volume. Criar uma Marcação que daria duas
//!   Marcações de tipos diferentes à mesma cópia é recusado ([`ErroDuplicados::Conflito`]).
//! * **Dispositivo desconhecido** (`volumes.device_id` nulo) nunca dá nem recebe Cobertura:
//!   não há como provar que é outro disco.
//!
//! # Limitações do espaço recuperável
//!
//! Veja [`dag_core::LIMITACOES_ESPACO`]: a estimativa usa tamanho lógico e não sabe de reflink,
//! COW, compressão ou deduplicação do sistema de arquivos.

mod agregado;
mod cobertura;
mod espaco;
mod exportar;
mod grupos;
mod marcacoes;
mod prioridade;
mod relatorios;
mod resolver;

use dag_core::{Autor, EscopoMarcacao, EstadoMarcacao, HashCompleto, TipoMarcacao};
use rusqlite::{Connection, OptionalExtension};

pub use cobertura::cobertura_do_arquivo;
pub use espaco::{
    Acumulado, EspacoCatalogo, espaco_de_grupos, espaco_de_grupos_referencia, relatorio_espaco,
};
pub use exportar::{exportar_relatorio, formato_do_caminho};
pub use grupos::{
    Escopo, FiltroGrupos, PaginaGrupos, ResumoReconstrucao, definir_copia_principal,
    detalhar_grupo, filtro_por_volume, grupo_do_arquivo, listar_grupos, listar_grupos_referencia,
    reconstruir_grupos, resumo_duplicados, resumo_duplicados_referencia,
};
pub use marcacoes::{
    HistoricoMarcacao, confirmar, desmarcar, historico_da_marcacao, listar_marcacoes, marcar,
    reavaliar_marcacoes,
};
pub use prioridade::{ItemPrioridade, definir_prioridade, listar_prioridades};
pub use relatorios::{relatorio_cobertura, relatorio_sem_cobertura};
pub use resolver::{resolver_arquivo, resolver_pasta, resolver_volume};

/// Falhas das operações de duplicados e Marcações.
#[derive(Debug, thiserror::Error)]
pub enum ErroDuplicados {
    #[error("catálogo: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    NaoEncontrado(String),
    #[error("{0}")]
    Invalido(String),
    /// Marcação que daria duas Marcações à mesma cópia.
    #[error("{0}")]
    Conflito(String),
    /// Manual exige Hash Completo válido do arquivo.
    #[error("{0}")]
    SemHash(String),
    #[error("{0}")]
    Ambiguo(String),
    #[error("{contexto}: {fonte}")]
    Es {
        contexto: String,
        #[source]
        fonte: std::io::Error,
    },
}

impl ErroDuplicados {
    /// Código estável para máquinas.
    #[must_use]
    pub fn codigo(&self) -> &'static str {
        match self {
            Self::Sqlite(_) => "catalogo",
            Self::NaoEncontrado(_) => "nao_encontrado",
            Self::Invalido(_) => "invalido",
            Self::Conflito(_) => "conflito_marcacao",
            Self::SemHash(_) => "sem_hash",
            Self::Ambiguo(_) => "ambiguo",
            Self::Es { .. } => "es",
        }
    }
}

pub type Resultado<T> = Result<T, ErroDuplicados>;

// ---------------------------------------------------------------------- SQL compartilhado

/// Hash válido: a evidência guardada ainda confere com a linha de `files` (aliases `f` e `h`).
pub(crate) const HASH_VALIDO: &str = "h.size_bytes = f.logical_bytes AND h.mtime_ns IS f.mtime_ns \
     AND h.ctime_ns IS f.ctime_ns AND h.native_file_id IS f.native_file_id";

/// `files.id` que representa o arquivo físico de `{a}` (o menor id com hash válido entre os
/// caminhos ativos do mesmo hard link; se nenhum tem hash, o menor id ativo).
pub(crate) fn representante_sql(a: &str) -> String {
    "COALESCE(
        (SELECT MIN(g.id) FROM files g JOIN hashes gh ON gh.file_id = g.id
         WHERE g.volume_id = {a}.volume_id AND g.hardlink_group = {a}.hardlink_group
           AND g.state = 'ativo' AND g.kind = 'arquivo'
           AND gh.size_bytes = g.logical_bytes AND gh.mtime_ns IS g.mtime_ns
           AND gh.ctime_ns IS g.ctime_ns AND gh.native_file_id IS g.native_file_id),
        (SELECT MIN(g.id) FROM files g
         WHERE g.volume_id = {a}.volume_id AND g.hardlink_group = {a}.hardlink_group
           AND g.state = 'ativo' AND g.kind = 'arquivo'),
        {a}.id)"
        .replace("{a}", a)
}

/// `a` está em `b` ou abaixo dele (caminhos brutos). Funciona em BLOB sem concatenar.
pub(crate) fn sob_sql(a: &str, b: &str) -> String {
    "({a} = {b} OR {b} = x'2f'
       OR (substr({a}, 1, length({b})) = {b} AND substr({a}, length({b}) + 1, 1) = x'2f'))"
        .replace("{a}", a)
        .replace("{b}", b)
}

pub(crate) fn tipo_str(t: TipoMarcacao) -> &'static str {
    match t {
        TipoMarcacao::Backup => "backup",
        TipoMarcacao::Delecao => "delecao",
    }
}

pub(crate) fn tipo_de(s: &str) -> TipoMarcacao {
    if s == "delecao" {
        TipoMarcacao::Delecao
    } else {
        TipoMarcacao::Backup
    }
}

pub(crate) fn estado_str(e: EstadoMarcacao) -> &'static str {
    match e {
        EstadoMarcacao::Ativa => "ativa",
        EstadoMarcacao::Pendente => "pendente",
    }
}

pub(crate) fn estado_de(s: &str) -> EstadoMarcacao {
    if s == "pendente" {
        EstadoMarcacao::Pendente
    } else {
        EstadoMarcacao::Ativa
    }
}

pub(crate) fn autor_str(a: Autor) -> &'static str {
    match a {
        Autor::Gui => "gui",
        Autor::Cli => "cli",
        Autor::Mcp => "mcp",
    }
}

pub(crate) fn autor_de(s: &str) -> Autor {
    match s {
        "gui" => Autor::Gui,
        "mcp" => Autor::Mcp,
        _ => Autor::Cli,
    }
}

pub(crate) fn hash_de(v: &[u8]) -> Option<HashCompleto> {
    <[u8; 32]>::try_from(v).ok().map(HashCompleto)
}

pub(crate) fn u(x: i64) -> u64 {
    u64::try_from(x).unwrap_or(0)
}

// ---------------------------------------------------------------------- cópias carregadas

/// Marcação que vale para uma cópia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MarcaEfetiva {
    pub id: i64,
    pub tipo: TipoMarcacao,
    pub estado: EstadoMarcacao,
    pub escopo: EscopoMarcacao,
}

/// Uma cópia carregada do Catálogo, com a Marcação que vale para ela.
#[derive(Debug, Clone)]
pub(crate) struct CopiaCarregada {
    pub arquivo: i64,
    pub volume: i64,
    pub rotulo: Option<String>,
    pub dispositivo: Option<i64>,
    pub conectado: bool,
    /// Último contato com o Volume, senão a data do hash.
    pub visto_em: Option<i64>,
    pub pasta: i64,
    pub caminho: String,
    pub outros_caminhos: u32,
    pub marca: Option<MarcaEfetiva>,
}

/// Um Membro de Contêiner que é cópia do grupo, com a Marcação que vale para o Contêiner.
#[derive(Debug, Clone)]
pub(crate) struct MembroCarregado {
    pub membro: i64,
    pub conteiner: i64,
    pub conteiner_caminho: String,
    pub caminho_interno: String,
    pub volume: i64,
    pub rotulo: Option<String>,
    pub dispositivo: Option<i64>,
    pub conectado: bool,
    pub visto_em: Option<i64>,
    pub marca: Option<MarcaEfetiva>,
}

/// Grupo com todas as cópias.
#[derive(Debug, Clone)]
pub(crate) struct GrupoCarregado {
    pub id: i64,
    pub tamanho: u64,
    pub hash: HashCompleto,
    pub principal: Option<i64>,
    pub manual: bool,
    pub copias: Vec<CopiaCarregada>,
    /// Membros de Contêineres do grupo (contam como cópia; nunca como espaço recuperável).
    pub membros: Vec<MembroCarregado>,
}

const SELECT_COPIAS: &str =
    "SELECT gm.grupo_id, f.id, f.volume_id, v.rotulo, v.device_id, v.mounted,
        COALESCE(v.last_seen_at, h.verified_at, h.hashed_at), f.folder_id, f.path_display,
        CASE WHEN f.hardlink_group IS NULL THEN 0 ELSE
            (SELECT COUNT(*) FROM files g WHERE g.volume_id = f.volume_id
               AND g.hardlink_group = f.hardlink_group AND g.state = 'ativo' AND g.id <> f.id)
        END
 FROM grupo_membros gm
 JOIN files f ON f.id = gm.file_id
 JOIN volumes v ON v.id = f.volume_id
 JOIN hashes h ON h.file_id = f.id";

/// Marcação que vale para o arquivo: manual, depois regra da pasta mais específica, depois
/// regra do Volume.
pub(crate) fn marca_efetiva(
    conn: &Connection,
    file_id: i64,
) -> rusqlite::Result<Option<MarcaEfetiva>> {
    let ler = |l: &rusqlite::Row<'_>| -> rusqlite::Result<MarcaEfetiva> {
        let tipo: String = l.get(1)?;
        let estado: String = l.get(2)?;
        let escopo: String = l.get(3)?;
        Ok(MarcaEfetiva {
            id: l.get(0)?,
            tipo: tipo_de(&tipo),
            estado: estado_de(&estado),
            escopo: EscopoMarcacao::de_str(&escopo).unwrap_or(EscopoMarcacao::ManualArquivo),
        })
    };
    if let Some(m) = conn
        .prepare_cached("SELECT id, tipo, estado, escopo FROM marcacoes WHERE file_id = ?1")?
        .query_row([file_id], ler)
        .optional()?
    {
        return Ok(Some(m));
    }
    let regra_pasta = format!(
        "SELECT m.id, m.tipo, m.estado, m.escopo FROM marcacoes m
         JOIN folders pf ON pf.id = m.folder_id
         JOIN files f ON f.id = ?1
         WHERE m.escopo = 'regra_pasta' AND pf.volume_id = f.volume_id AND {}
         ORDER BY length(pf.path_raw) DESC LIMIT 1",
        sob_sql("f.path_raw", "pf.path_raw")
    );
    if let Some(m) = conn
        .prepare_cached(&regra_pasta)?
        .query_row([file_id], ler)
        .optional()?
    {
        return Ok(Some(m));
    }
    conn.prepare_cached(
        "SELECT m.id, m.tipo, m.estado, m.escopo FROM marcacoes m
         JOIN files f ON f.id = ?1
         WHERE m.escopo = 'regra_volume' AND m.volume_id = f.volume_id",
    )?
    .query_row([file_id], ler)
    .optional()
}

fn copia_da_linha(
    conn: &Connection,
    l: &rusqlite::Row<'_>,
) -> rusqlite::Result<(i64, CopiaCarregada)> {
    let arquivo: i64 = l.get(1)?;
    Ok((
        l.get(0)?,
        CopiaCarregada {
            arquivo,
            volume: l.get(2)?,
            rotulo: l.get(3)?,
            dispositivo: l.get(4)?,
            conectado: l.get::<_, i64>(5)? != 0,
            visto_em: l.get(6)?,
            pasta: l.get(7)?,
            caminho: l.get(8)?,
            outros_caminhos: u32::try_from(l.get::<_, i64>(9)?).unwrap_or(0),
            marca: marca_efetiva(conn, arquivo)?,
        },
    ))
}

type LinhaGrupo = (i64, i64, Vec<u8>, Option<i64>, String);

fn grupo_da_linha(l: &rusqlite::Row<'_>) -> rusqlite::Result<LinhaGrupo> {
    Ok((l.get(0)?, l.get(1)?, l.get(2)?, l.get(3)?, l.get(4)?))
}

/// Membros de Contêineres do grupo, em ordem de id.
fn carregar_membros(conn: &Connection, grupo_id: i64) -> rusqlite::Result<Vec<MembroCarregado>> {
    let mut st = conn.prepare_cached(
        "SELECT m.id, m.conteiner_file_id, f.path_display, m.caminho_display, f.volume_id,
                v.rotulo, v.device_id, v.mounted, v.last_seen_at
         FROM grupo_membros_conteiner g
         JOIN membros m ON m.id = g.membro_id
         JOIN files f ON f.id = m.conteiner_file_id
         JOIN volumes v ON v.id = f.volume_id
         WHERE g.grupo_id = ?1 ORDER BY m.id",
    )?;
    let linhas = st
        .query_map([grupo_id], |l| {
            let conteiner: i64 = l.get(1)?;
            Ok(MembroCarregado {
                membro: l.get(0)?,
                conteiner,
                conteiner_caminho: l.get(2)?,
                caminho_interno: l.get(3)?,
                volume: l.get(4)?,
                rotulo: l.get(5)?,
                dispositivo: l.get(6)?,
                conectado: l.get::<_, i64>(7)? != 0,
                visto_em: l.get(8)?,
                marca: marca_efetiva(conn, conteiner)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(linhas)
}

fn montar_grupo(
    g: LinhaGrupo,
    copias: Vec<CopiaCarregada>,
    membros: Vec<MembroCarregado>,
) -> GrupoCarregado {
    GrupoCarregado {
        membros,
        id: g.0,
        tamanho: u(g.1),
        hash: hash_de(&g.2).unwrap_or(HashCompleto([0; 32])),
        principal: g.3,
        manual: g.4 == "manual",
        copias,
    }
}

/// Um grupo com as cópias (ordenadas por id de arquivo).
pub(crate) fn carregar_grupo(
    conn: &Connection,
    grupo_id: i64,
) -> Resultado<Option<GrupoCarregado>> {
    let Some(g) = conn
        .query_row(
            "SELECT id, logical_bytes, hash, copia_principal_file_id, copia_principal_modo
             FROM grupos_duplicados WHERE id = ?1",
            [grupo_id],
            grupo_da_linha,
        )
        .optional()?
    else {
        return Ok(None);
    };
    let mut st = conn.prepare(&format!(
        "{SELECT_COPIAS} WHERE gm.grupo_id = ?1 ORDER BY f.id"
    ))?;
    let copias = st
        .query_map([grupo_id], |l| copia_da_linha(conn, l).map(|x| x.1))?
        .collect::<Result<Vec<_>, _>>()?;
    let membros = carregar_membros(conn, grupo_id)?;
    Ok(Some(montar_grupo(g, copias, membros)))
}

/// Percorre todos os grupos em ordem de id (um por vez na memória). `f` devolve `false` para
/// parar.
pub(crate) fn percorrer_grupos(
    conn: &Connection,
    mut f: impl FnMut(GrupoCarregado) -> Resultado<bool>,
) -> Resultado<()> {
    let mut grupos = conn.prepare(
        "SELECT id, logical_bytes, hash, copia_principal_file_id, copia_principal_modo
         FROM grupos_duplicados ORDER BY id",
    )?;
    let mut copias = conn.prepare(&format!(
        "{SELECT_COPIAS} WHERE gm.grupo_id = ?1 ORDER BY f.id"
    ))?;
    let mut linhas = grupos.query([])?;
    while let Some(l) = linhas.next()? {
        let g = grupo_da_linha(l)?;
        let id = g.0;
        let cs = copias
            .query_map([id], |l| copia_da_linha(conn, l).map(|x| x.1))?
            .collect::<Result<Vec<_>, _>>()?;
        let ms = carregar_membros(conn, id)?;
        if !f(montar_grupo(g, cs, ms))? {
            break;
        }
    }
    Ok(())
}

/// Há outro Arquivo Físico com o mesmo conteúdo (tamanho + hash válidos) no Catálogo?
pub(crate) fn tem_outra_copia(conn: &Connection, file_id: i64) -> rusqlite::Result<bool> {
    conn.prepare_cached(
        "SELECT EXISTS (
            SELECT 1 FROM files f0 JOIN hashes h0 ON h0.file_id = f0.id
            JOIN hashes h ON h.hash = h0.hash AND h.size_bytes = h0.size_bytes
            JOIN files f ON f.id = h.file_id
            WHERE f0.id = ?1
              AND f.state = 'ativo' AND f.kind = 'arquivo'
              AND h.size_bytes = f.logical_bytes AND h.mtime_ns IS f.mtime_ns
              AND h.ctime_ns IS f.ctime_ns AND h.native_file_id IS f.native_file_id
              AND NOT (f.volume_id = f0.volume_id
                       AND (f.id = f0.id
                            OR (f0.hardlink_group IS NOT NULL AND f.hardlink_group = f0.hardlink_group))))
         OR EXISTS (
            -- Um Membro de Contêiner com o mesmo conteúdo também é cópia (ADR 0006).
            SELECT 1 FROM hashes h0 JOIN membros m ON m.hash = h0.hash AND m.tamanho = h0.size_bytes
            JOIN files c ON c.id = m.conteiner_file_id AND c.state = 'ativo'
            WHERE h0.file_id = ?1 AND m.hash IS NOT NULL AND m.tipo = 'arquivo' AND m.estado = 'lido')",
    )?
    .query_row([file_id], |l| l.get::<_, i64>(0))
    .map(|n| n != 0)
}

/// Única Cópia: tem Hash Completo válido e nenhuma outra cópia física no Catálogo.
pub fn eh_unica_copia(conn: &Connection, file_id: i64) -> rusqlite::Result<bool> {
    let tem_hash: bool = conn
        .prepare_cached(&format!(
            "SELECT EXISTS (SELECT 1 FROM files f JOIN hashes h ON h.file_id = f.id
                            WHERE f.id = ?1 AND {HASH_VALIDO})"
        ))?
        .query_row([file_id], |l| l.get::<_, i64>(0))?
        != 0;
    Ok(tem_hash && !tem_outra_copia(conn, file_id)?)
}

/// Hash Completo válido do arquivo, se houver.
pub(crate) fn hash_valido(
    conn: &Connection,
    file_id: i64,
) -> rusqlite::Result<Option<HashCompleto>> {
    let v: Option<Vec<u8>> = conn
        .prepare_cached(&format!(
            "SELECT h.hash FROM files f JOIN hashes h ON h.file_id = f.id
             WHERE f.id = ?1 AND {HASH_VALIDO}"
        ))?
        .query_row([file_id], |l| l.get(0))
        .optional()?;
    Ok(v.and_then(|b| hash_de(&b)))
}
