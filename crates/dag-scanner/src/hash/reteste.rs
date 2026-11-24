//! Reteste único, no fim da etapa de Hash Completo, dos arquivos que mudaram ou sumiram durante
//! a leitura (`hash_alterado_*`, `hash_ausente`).
//!
//! Arquivos vivos (logs, caches, SQLite `-wal`) costumam estar quietos minutos depois. Cada
//! arquivo é relido **uma vez**:
//!
//! * estável: o hash é gravado e o problema deixa de existir (nenhum erro);
//! * ainda mudando, ou ausente de novo: o problema fica, classificado como **esperado**.
//!
//! Nada aqui altera arquivos catalogados: só lê.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::AtomicU64;

use dag_core::erros::REGRA_RETESTE;
use dag_store::Catalogo;
use dag_store::erros as erros_db;
use dag_store::hashes::{self, ItemFila};

use super::{Gancho, GanchoPedaco, Leitor, Saida, Vaga, agora, ctime_ns, mtime_ns};
use crate::controle::ControleScan;
use crate::erro::ErroScan;

/// Tamanho do lote lido do Catálogo (limita a memória).
const LOTE: i64 = 2000;

struct Candidato {
    erro_id: i64,
    item: ItemFila,
}

/// Metadados lidos agora (tamanho, mtime, ctime, inode), para atualizar o inventário.
type Atual = (i64, i64, i64, i64);

enum Decisao {
    Recuperado {
        hash: [u8; 32],
        bytes: u64,
        atual: Option<Atual>,
    },
    Esperado(String),
}

fn carregar(cat: &Catalogo, run_id: i64, apos: i64) -> Result<Vec<Candidato>, ErroScan> {
    let conn = cat.armazem().conexao();
    let mut st = conn.prepare(
        "SELECT e.id, f.id, f.folder_id, f.volume_id, f.path_raw, f.logical_bytes, f.mtime_ns,
                f.ctime_ns, f.native_file_id, COALESCE(q.id, 0)
         FROM scan_errors e
         JOIN folders d ON d.id = e.folder_id
         JOIN files f ON f.volume_id = d.volume_id AND f.path_raw = e.path_raw
         LEFT JOIN fila_hash q ON q.file_id = f.id
         WHERE e.run_id = ?1 AND e.id > ?2 AND e.phase = 'hash'
           AND e.error_code IN ('hash_alterado_desde_inventario', 'hash_alterado_durante_leitura',
                                'hash_ausente')
           AND f.seen_run_id = e.run_id AND f.kind = 'arquivo' AND f.state = 'ativo'
         ORDER BY e.id LIMIT ?3",
    )?;
    let linhas = st.query_map(rusqlite::params![run_id, apos, LOTE], |l| {
        Ok(Candidato {
            erro_id: l.get(0)?,
            item: ItemFila {
                fila_id: l.get(9)?,
                file_id: l.get(1)?,
                pasta_id: l.get(2)?,
                volume_id: l.get(3)?,
                path_raw: l.get(4)?,
                tamanho: l.get(5)?,
                mtime_ns: l.get(6)?,
                ctime_ns: l.get(7)?,
                native_file_id: l.get(8)?,
                hash_guardado: None,
            },
        })
    })?;
    Ok(linhas.collect::<Result<_, _>>()?)
}

/// Relê uma vez cada arquivo com problema "mudou ou sumiu". Devolve quantos foram recuperados
/// (hash gravado, problema removido).
///
/// # Errors
/// Falha do Catálogo.
pub(super) fn executar(
    cat: &mut Catalogo,
    run_id: i64,
    controle: &ControleScan,
    gancho: Option<&Gancho>,
    gancho_pedaco: Option<&GanchoPedaco>,
) -> Result<u64, ErroScan> {
    let abortar = || controle.cancelado();
    let lidos = AtomicU64::new(0);
    let disco = AtomicU64::new(0);
    let vaga = Vaga::default();
    let leitor = Leitor {
        verificacao: false,
        abortar: &abortar,
        limitador: None,
        gancho,
        gancho_pedaco,
        bytes_lidos: &lidos,
        bytes_disco: &disco,
        vaga: &vaga,
    };
    let mut buf = vec![0u8; 1 << 20];
    let mut recuperados = 0;
    let mut apos = 0;
    loop {
        let lote = carregar(cat, run_id, apos)?;
        let Some(ultimo) = lote.last() else { break };
        apos = ultimo.erro_id;
        let mut decisoes: Vec<(i64, ItemFila, Decisao)> = Vec::with_capacity(lote.len());
        for mut c in lote {
            if abortar() {
                break;
            }
            let mut atual = None;
            // A evidência esperada passa a ser a de agora: o inventário ficou velho, e a
            // estabilidade (antes e depois da leitura) é o que garante o hash.
            if let Ok(m) = std::fs::symlink_metadata(Path::new(OsStr::from_bytes(&c.item.path_raw)))
                && m.is_file()
                && let Ok(tamanho) = i64::try_from(m.len())
                && let Ok(ino) = i64::try_from(m.ino())
            {
                c.item.tamanho = tamanho;
                c.item.mtime_ns = Some(mtime_ns(&m));
                c.item.ctime_ns = Some(ctime_ns(&m));
                c.item.native_file_id = Some(ino);
                atual = Some((tamanho, mtime_ns(&m), ctime_ns(&m), ino));
            }
            match leitor.ler(&c.item, &mut buf) {
                Saida::Lido { hash, bytes } => {
                    decisoes.push((
                        c.erro_id,
                        c.item,
                        Decisao::Recuperado { hash, bytes, atual },
                    ));
                }
                Saida::Alterado { motivo, .. } => decisoes.push((
                    c.erro_id,
                    c.item,
                    Decisao::Esperado(format!("continua mudando no reteste: {motivo}")),
                )),
                Saida::Falha {
                    codigo: "hash_ausente",
                    ..
                } => decisoes.push((
                    c.erro_id,
                    c.item,
                    Decisao::Esperado("continua ausente no reteste (arquivo temporário)".into()),
                )),
                Saida::Falha { .. } | Saida::Abortado => {}
            }
        }
        let t = agora();
        recuperados += cat
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<u64, ErroScan> {
                let mut n = 0;
                for (erro_id, item, d) in &decisoes {
                    match d {
                        Decisao::Recuperado { hash, bytes, atual } => {
                            if let Some((tam, mtime, ctime, ino)) = atual {
                                // Estável, mas o inventário estava velho: atualiza a evidência
                                // desta linha (o Fast Scan acerta o resto).
                                tx.execute(
                                    "UPDATE files SET logical_bytes = ?2, mtime_ns = ?3,
                                         ctime_ns = ?4, native_file_id = ?5 WHERE id = ?1",
                                    rusqlite::params![item.file_id, tam, mtime, ctime, ino],
                                )?;
                            }
                            hashes::gravar_hash(tx, item, hash, *bytes, t)?;
                            erros_db::apagar_problema(tx, *erro_id)?;
                            n += 1;
                        }
                        Decisao::Esperado(motivo) => {
                            erros_db::marcar_esperado(tx, *erro_id, REGRA_RETESTE, motivo)?;
                        }
                    }
                }
                Ok(n)
            })?;
        if abortar() {
            break;
        }
    }
    Ok(recuperados)
}
