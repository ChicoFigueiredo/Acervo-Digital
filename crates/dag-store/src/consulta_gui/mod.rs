//! Consultas somente-leitura para a GUI (spec §6, "exibição escalável"): cada função devolve no
//! máximo uma página ou um nível da árvore, nunca o Catálogo inteiro (CA-A08, A-10).
//!
//! Tudo aqui recebe uma [`Connection`] aberta por [`abrir_leitura`] (`query_only`): a GUI nunca
//! escreve no SQLite; quem escreve é o worker (único escritor).

mod analise;
mod navegacao;

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

pub use analise::{AlvoAnalise, Analise, ChaveNo, Elo as EloAnalise, NoAnalise, analisar};
pub use navegacao::{
    CursorArquivo, CursorPasta, InfoPasta, LIMITE_MAXIMO, LinhaArquivo, MarcaVista, NoPasta,
    PaginaArquivos, PaginaPastas, arquivos_da_pasta, info_da_pasta, raizes, subpastas,
    trilha_da_pasta,
};

use crate::Resultado;

fn n(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

/// Abre uma conexão somente-leitura (WAL permite ler com o escritor ativo).
///
/// # Errors
/// Falha do SQLite (arquivo ausente, sem permissão...).
pub fn abrir_leitura(caminho: &Path) -> Resultado<Connection> {
    let c = Connection::open_with_flags(
        caminho,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    c.busy_timeout(Duration::from_secs(5))?;
    c.execute_batch("PRAGMA query_only = ON;")?;
    Ok(c)
}

/// Uma linha da tabela de Volumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinhaVolume {
    pub id: i64,
    pub rotulo: Option<String>,
    /// Ponto de montagem mais curto (o "principal").
    pub montagem: Option<String>,
    /// Quantos pontos de montagem o Volume tem (bind mounts contam no mesmo Volume).
    pub montagens: u32,
    pub fs_type: Option<String>,
    pub capacidade: Option<u64>,
    pub livre: Option<u64>,
    /// Texto do Dispositivo Físico (modelo, rótulo ou identificador).
    pub dispositivo: Option<String>,
    pub tipo_dispositivo: Option<String>,
    pub conectado: bool,
    /// Bytes lógicos catalogados deste Volume (`volume_stats`), se já varrido.
    pub bytes_catalogados: Option<u64>,
    pub visto_em: Option<i64>,
}

impl LinhaVolume {
    /// Uso do Volume em % (0..=100), se capacidade e espaço livre são conhecidos.
    #[must_use]
    pub fn uso_percentual(&self) -> Option<f64> {
        let (c, l) = (self.capacidade?, self.livre?);
        if c == 0 {
            return None;
        }
        Some((c.saturating_sub(l) as f64 / c as f64 * 100.0).clamp(0.0, 100.0))
    }
}

/// Volumes conhecidos (poucos): conectados primeiro.
///
/// # Errors
/// Falha do SQLite.
pub fn listar_volumes(conn: &Connection) -> Resultado<Vec<LinhaVolume>> {
    let mut st = conn.prepare(
        "SELECT v.id, v.rotulo, v.fs_type, v.capacity_bytes, v.free_bytes, v.mounted,
                v.last_seen_at, COALESCE(d.modelo, d.rotulo, d.identificador), d.tipo,
                (SELECT path_display FROM mount_points m WHERE m.volume_id = v.id
                 ORDER BY length(m.path_raw), m.id LIMIT 1),
                (SELECT COUNT(*) FROM mount_points m WHERE m.volume_id = v.id),
                (SELECT logical_bytes FROM volume_stats s WHERE s.volume_id = v.id)
         FROM volumes v LEFT JOIN dispositivos d ON d.id = v.device_id
         ORDER BY v.mounted DESC, v.id",
    )?;
    let linhas = st
        .query_map([], |l| {
            Ok(LinhaVolume {
                id: l.get(0)?,
                rotulo: l.get(1)?,
                fs_type: l.get(2)?,
                capacidade: l.get::<_, Option<i64>>(3)?.map(n),
                livre: l.get::<_, Option<i64>>(4)?.map(n),
                conectado: l.get::<_, i64>(5)? != 0,
                visto_em: l.get(6)?,
                dispositivo: l.get(7)?,
                tipo_dispositivo: l.get(8)?,
                montagem: l.get(9)?,
                montagens: u32::try_from(l.get::<_, i64>(10)?).unwrap_or(0),
                bytes_catalogados: l.get::<_, Option<i64>>(11)?.map(n),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(linhas)
}

/// Totais do Catálogo: cada Volume entra **uma vez** (por `volume_stats`), qualquer que seja o
/// número de pontos de montagem.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TotaisCatalogo {
    pub volumes: u64,
    pub pastas: u64,
    pub arquivos: u64,
    pub bytes_logicos: u64,
    /// Arquivos do Catálogo com hash gravado (linhas de `hashes`).
    pub arquivos_com_hash: u64,
}

/// Soma dos agregados por Volume.
///
/// # Errors
/// Falha do SQLite.
pub fn totais_do_catalogo(conn: &Connection) -> Resultado<TotaisCatalogo> {
    let volumes: i64 = conn.query_row("SELECT COUNT(*) FROM volumes", [], |l| l.get(0))?;
    let (pastas, arquivos, bytes): (i64, i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(folder_count),0), COALESCE(SUM(file_count),0),
                COALESCE(SUM(logical_bytes),0) FROM volume_stats",
        [],
        |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)),
    )?;
    let com_hash = crate::hashes::total_com_hash(conn)?;
    Ok(TotaisCatalogo {
        volumes: n(volumes),
        pastas: n(pastas),
        arquivos: n(arquivos),
        bytes_logicos: n(bytes),
        arquivos_com_hash: com_hash,
    })
}

/// Uma linha da tabela de últimas varreduras.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinhaExecucao {
    pub id: i64,
    pub modo: String,
    pub estado: String,
    pub fase: String,
    pub raizes: Vec<String>,
    pub iniciada_em: i64,
    pub encerrada_em: Option<i64>,
    pub pastas: u64,
    pub arquivos: u64,
    pub bytes_logicos: u64,
    pub erros: u64,
}

/// Últimas varreduras (mais recentes primeiro), no máximo `limite` (≤ 200).
///
/// # Errors
/// Falha do SQLite.
pub fn ultimas_execucoes(conn: &Connection, limite: usize) -> Resultado<Vec<LinhaExecucao>> {
    let mut st = conn.prepare(
        "SELECT id, mode, status, phase, roots, started_at, ended_at, counters
         FROM scan_runs WHERE mode IN ('full','fast') ORDER BY id DESC LIMIT ?1",
    )?;
    let linhas = st
        .query_map([limite.min(200) as i64], |l| {
            let roots: String = l.get(4)?;
            let contadores: String = l.get(7)?;
            Ok((
                l.get::<_, i64>(0)?,
                l.get::<_, String>(1)?,
                l.get::<_, String>(2)?,
                l.get::<_, String>(3)?,
                roots,
                l.get::<_, i64>(5)?,
                l.get::<_, Option<i64>>(6)?,
                contadores,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(linhas
        .into_iter()
        .map(|(id, modo, estado, fase, roots, ini, fim, cont)| {
            let c: serde_json::Value = serde_json::from_str(&cont).unwrap_or_default();
            let g = |k: &str| c.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
            LinhaExecucao {
                id,
                modo,
                estado,
                fase,
                raizes: serde_json::from_str(&roots).unwrap_or_default(),
                iniciada_em: ini,
                encerrada_em: fim,
                pastas: g("pastas"),
                arquivos: g("arquivos"),
                bytes_logicos: g("bytes_logicos"),
                erros: g("erros"),
            }
        })
        .collect())
}

/// Andamento da fila de hash, lido do Catálogo (contagens por estado, só índice). O callback de
/// progresso do scanner não expõe a etapa de hash fora do stderr; esta leitura é o que a GUI
/// pode mostrar com denominador real.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AndamentoHash {
    pub total: u64,
    pub concluidos: u64,
    pub com_erro: u64,
}

/// Contagem da `fila_hash` por estado.
///
/// # Errors
/// Falha do SQLite.
pub fn andamento_do_hash(conn: &Connection) -> Resultado<AndamentoHash> {
    let mut st = conn.prepare("SELECT state, COUNT(*) FROM fila_hash GROUP BY state")?;
    let mut a = AndamentoHash::default();
    let linhas = st
        .query_map([], |l| Ok((l.get::<_, String>(0)?, l.get::<_, i64>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (estado, c) in linhas {
        let c = n(c);
        a.total += c;
        match estado.as_str() {
            "concluido" => a.concluidos += c,
            "erro" => {
                a.concluidos += c;
                a.com_erro += c;
            }
            _ => {}
        }
    }
    Ok(a)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn uso_percentual_calcula_e_protege_zero() {
        let mut v = LinhaVolume {
            id: 1,
            rotulo: None,
            montagem: None,
            montagens: 1,
            fs_type: None,
            capacidade: Some(1000),
            livre: Some(250),
            dispositivo: None,
            tipo_dispositivo: None,
            conectado: true,
            bytes_catalogados: None,
            visto_em: None,
        };
        assert_eq!(v.uso_percentual(), Some(75.0));
        v.capacidade = Some(0);
        assert_eq!(v.uso_percentual(), None);
        v.capacidade = None;
        assert_eq!(v.uso_percentual(), None);
    }
}
