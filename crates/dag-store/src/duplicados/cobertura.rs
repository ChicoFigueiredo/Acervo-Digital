//! Cobertura (A-28): um arquivo é coberto se uma cópia sua tem Marcação de Backup ativa em
//! outro Dispositivo Físico. Usa as regras puras de `dag_core::regras`.

use dag_core::regras::Marcacao;
use dag_core::regras::{Copia, tem_cobertura};
use dag_core::{
    ArquivoId, CoberturaArquivo, DispositivoId, EstadoMarcacao, HashCompleto, OrigemMarcacao,
    TipoMarcacao,
};
use rusqlite::{Connection, OptionalExtension};

use super::resolver::representante;
use super::{CopiaCarregada, GrupoCarregado, MarcaEfetiva, Resultado, carregar_grupo};

const NAO: CoberturaArquivo = CoberturaArquivo {
    coberto: false,
    nao_verificada_desde: None,
};

fn marcacao_core(c: &CopiaCarregada, m: MarcaEfetiva, hash: HashCompleto) -> Marcacao {
    Marcacao {
        arquivo: ArquivoId(c.arquivo),
        tipo: m.tipo,
        estado: m.estado,
        origem: if m.escopo.eh_regra() {
            OrigemMarcacao::Regra
        } else {
            OrigemMarcacao::Manual
        },
        hash_na_marcacao: hash,
    }
}

/// Cobertura de cada cópia do grupo, na ordem de `g.copias`.
///
/// Volume sem Dispositivo Físico conhecido nunca dá nem recebe Cobertura. Cobertura apoiada
/// só em cópias de Volumes desconectados é mantida com `nao_verificada_desde` (último contato
/// com o Volume).
pub(crate) fn cobertura_do_grupo(g: &GrupoCarregado) -> Vec<CoberturaArquivo> {
    let copias: Vec<Copia> = g
        .copias
        .iter()
        .filter_map(|c| {
            c.dispositivo.map(|d| Copia {
                arquivo: ArquivoId(c.arquivo),
                volume: dag_core::VolumeId(c.volume),
                dispositivo: DispositivoId(d),
            })
        })
        .collect();
    let backups: Vec<(Marcacao, bool, i64)> = g
        .copias
        .iter()
        .filter(|c| c.dispositivo.is_some())
        .filter_map(|c| {
            c.marca
                .filter(|m| m.tipo == TipoMarcacao::Backup && m.estado == EstadoMarcacao::Ativa)
                .map(|m| {
                    (
                        marcacao_core(c, m, g.hash),
                        c.conectado,
                        c.visto_em.unwrap_or(0),
                    )
                })
        })
        .collect();
    let todas: Vec<Marcacao> = backups.iter().map(|b| b.0.clone()).collect();
    let online: Vec<Marcacao> = backups
        .iter()
        .filter(|b| b.1)
        .map(|b| b.0.clone())
        .collect();
    g.copias
        .iter()
        .map(|c| {
            let Some(d) = c.dispositivo else { return NAO };
            let dev = DispositivoId(d);
            if !tem_cobertura(dev, &copias, &todas) {
                return NAO;
            }
            if tem_cobertura(dev, &copias, &online) {
                return CoberturaArquivo {
                    coberto: true,
                    nao_verificada_desde: None,
                };
            }
            // Só há cobertura em Volume desconectado: o selo é a data mais recente dessas cópias.
            let desde = backups
                .iter()
                .filter(|b| !b.1 && tem_cobertura(dev, &copias, std::slice::from_ref(&b.0)))
                .map(|b| b.2)
                .max();
            CoberturaArquivo {
                coberto: true,
                nao_verificada_desde: Some(desde.unwrap_or(0)),
            }
        })
        .collect()
}

/// Cobertura do Arquivo Físico de `file_id`. Arquivo que não é cópia de nenhum grupo não tem
/// Cobertura.
///
/// # Errors
/// Falha do SQLite.
pub fn cobertura_do_arquivo(conn: &Connection, file_id: i64) -> Resultado<CoberturaArquivo> {
    let rep = representante(conn, file_id)?;
    let grupo: Option<i64> = conn
        .query_row(
            "SELECT grupo_id FROM grupo_membros WHERE file_id = ?1",
            [rep],
            |l| l.get(0),
        )
        .optional()?;
    let Some(grupo) = grupo else { return Ok(NAO) };
    let Some(g) = carregar_grupo(conn, grupo)? else {
        return Ok(NAO);
    };
    Ok(g.copias
        .iter()
        .position(|c| c.arquivo == rep)
        .map_or(NAO, |i| cobertura_do_grupo(&g)[i]))
}
