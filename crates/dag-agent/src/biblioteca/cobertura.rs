//! Verificação de Cobertura da Biblioteca Homologada (B-26/CA-B24).
//!
//! Decisão do dono: "backup verificado" é a Cobertura da Fase A (cópia com Marcação de Backup em
//! outro Dispositivo Físico). O manifesto é um instantâneo completo (metadata.db e todos os
//! arquivos, com BLAKE3, reaproveitando o hash do Catálogo quando tamanho e mtime batem). A
//! "restauração amostral" é uma verificação de leitura: relê por inteiro uma amostra aleatória das
//! cópias de backup e confere o BLAKE3, sem copiar nada. Tudo é somente leitura sobre o acervo;
//! só o resultado vai para o Catálogo (`backups_biblioteca`).

use std::collections::HashMap;
use std::path::Path;

use dag_biblio::PapelBiblioteca;
use dag_biblio::descarte::VerificacaoCobertura;
use dag_calibre::instantaneo::{self, Escopo};
use dag_core::{
    ConfigBiblioteca, DadosBiblioteca, FormatoSemCoberturaInfo, Resposta, ResumoCobertura,
};
use dag_store::biblioteca as store;
use rusqlite::Connection;

use super::{blake3_arquivo, dados, hex, resolver_biblioteca};
use crate::agente::Agente;
use crate::erro::ErroAgente;

const FATIA: i64 = 500;
const AMOSTRA_PADRAO: u32 = 10;
const EXEMPLOS: usize = 50;
const EXEMPLOS_GRAVADOS: usize = 200;

struct FormatoLinha {
    id: i64,
    caminho: String,
    tamanho: u64,
    mtime_ns: Option<i64>,
    hash: Option<[u8; 32]>,
    file_id: Option<i64>,
}

fn formatos_da_biblioteca(
    conn: &Connection,
    biblioteca: i64,
) -> Result<Vec<FormatoLinha>, ErroAgente> {
    let mut v = Vec::new();
    let mut apos = 0_i64;
    loop {
        let mut st = conn.prepare_cached(
            "SELECT id, caminho, tamanho, mtime_ns, hash, file_id FROM formatos_homologados
             WHERE biblioteca_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3",
        )?;
        let lote = st
            .query_map(rusqlite::params![biblioteca, apos, FATIA], |l| {
                Ok(FormatoLinha {
                    id: l.get(0)?,
                    caminho: l.get(1)?,
                    tamanho: l.get::<_, i64>(2)? as u64,
                    mtime_ns: l.get(3)?,
                    hash: l
                        .get::<_, Option<Vec<u8>>>(4)?
                        .and_then(|b| <[u8; 32]>::try_from(b).ok()),
                    file_id: l.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let Some(u) = lote.last().map(|f| f.id) else {
            break;
        };
        apos = u;
        v.extend(lote);
    }
    Ok(v)
}

/// Situação da verificação de Cobertura para o fluxo de descarte, segundo a configuração.
pub(super) fn situacao_da_cobertura(
    conn: &Connection,
    cfg: &ConfigBiblioteca,
    biblioteca: i64,
    agora: i64,
) -> VerificacaoCobertura {
    if cfg.cobertura_validade_dias == 0 {
        return VerificacaoCobertura::NaoExigida;
    }
    let Ok(Some(b)) = store::ultimo_backup(conn, biblioteca) else {
        return VerificacaoCobertura::Ausente;
    };
    if b.verificacao != store::VerificacaoBackup::Ok {
        return VerificacaoCobertura::Reprovada;
    }
    let quando = b.verificado_em.unwrap_or(b.criado_em);
    let dias = u32::try_from(((agora - quando).max(0)) / 86_400).unwrap_or(u32::MAX);
    if dias > cfg.cobertura_validade_dias {
        VerificacaoCobertura::Vencida { dias }
    } else {
        VerificacaoCobertura::Aprovada
    }
}

/// Índices distintos sorteados em `0..total` (`n` já limitado a `total`).
fn sortear(total: usize, n: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..total).collect();
    if n >= total {
        return idx;
    }
    // Fisher-Yates parcial com bytes do sistema; se o sorteio falhar, usa o início (determinístico).
    let mut raw = vec![0u8; n * 8];
    if getrandom::fill(&mut raw).is_err() {
        idx.truncate(n);
        return idx;
    }
    for i in 0..n {
        let r = u64::from_le_bytes(raw[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]));
        let j = i + (r % ((total - i) as u64)) as usize;
        idx.swap(i, j);
    }
    idx.truncate(n);
    idx
}

impl Agente {
    pub(super) fn verificar_cobertura(
        &mut self,
        ref_: &str,
        amostra: Option<u32>,
    ) -> Result<Resposta, ErroAgente> {
        let cfg = self.config_biblioteca();
        let agora = self.agora_biblio();
        let b = resolver_biblioteca(self.conexao(), ref_)?;
        if b.papel != PapelBiblioteca::Homologada {
            return Err(ErroAgente::biblio(
                "papel_invalido",
                "só a Biblioteca Homologada tem Cobertura verificada",
            ));
        }
        let raiz = Path::new(&b.caminho).to_path_buf();
        let pedida = amostra.unwrap_or(AMOSTRA_PADRAO);
        self.mensagem("cobertura: montando o manifesto da Homologada (somente leitura)...");

        // 1. Manifesto: instantâneo sem hash, depois hash só do que o Catálogo não conhece.
        let formatos = formatos_da_biblioteca(self.conexao(), b.id)?;
        let por_caminho: HashMap<&str, &FormatoLinha> =
            formatos.iter().map(|f| (f.caminho.as_str(), f)).collect();
        let snap = instantaneo::tirar(&raiz, Escopo::Listagem, None)?;
        let mut entradas: Vec<(String, u64, Option<String>)> =
            Vec::with_capacity(snap.arquivos.len());
        let mut hash_db = String::new();
        for (rel, reg) in &snap.arquivos {
            let abs = raiz.join(rel);
            let reuso = por_caminho
                .get(abs.to_string_lossy().as_ref())
                .filter(|f| {
                    f.tamanho == reg.tamanho
                        && f.mtime_ns.is_some_and(|m| i128::from(m) == reg.mtime_ns)
                })
                .and_then(|f| f.hash)
                .map(|h| hex(&h));
            let h = match (&reg.blake3, reuso) {
                (Some(h), _) => Some(h.clone()),
                (None, Some(h)) => Some(h),
                (None, None) => {
                    if !self.seguir_biblioteca() {
                        return Err(ErroAgente::biblio("cancelado", "verificação cancelada"));
                    }
                    blake3_arquivo(&abs).ok().map(|h| hex(&h))
                }
            };
            if rel == "metadata.db" {
                hash_db = h.clone().unwrap_or_default();
            }
            entradas.push((rel.clone(), reg.tamanho, h));
        }
        let manifesto = serde_json::json!({
            "biblioteca": b.id,
            "gerado_em": agora,
            "metadata_db": {"hash": hash_db},
            "arquivos_total": entradas.len(),
            "arquivos": entradas.iter().map(|(r, t, h)| serde_json::json!([r, t, h])).collect::<Vec<_>>(),
        });

        // 2. Cobertura por Formato e reunião das cópias de backup relíveis.
        let conn = self.conexao();
        let mut cobertos = 0_u64;
        let mut sem: Vec<FormatoSemCoberturaInfo> = Vec::new();
        let mut sem_total = 0_u64;
        let mut reliveis: Vec<(i64, String, [u8; 32], u64)> = Vec::new();
        for f in &formatos {
            let motivo = match (f.file_id, f.hash) {
                (None, _) => Some("o Formato não está no Catálogo (varra a Homologada)".to_owned()),
                (_, None) => Some("o Formato ainda não tem Hash Completo".to_owned()),
                (Some(fid), Some(h)) => {
                    let grupo = store::copias_do_grupo(conn, fid)?;
                    let propria = grupo.iter().find(|c| c.file_id == fid);
                    if propria.is_some_and(|c| c.coberta) {
                        cobertos += 1;
                        let disp = propria.and_then(|c| c.dispositivo_id);
                        if let Some(c) = grupo.iter().find(|c| {
                            c.file_id != fid
                                && c.marcacao_backup
                                && !c.na_lixeira
                                && c.conectado
                                && c.dispositivo_id != disp
                        }) {
                            reliveis.push((f.id, c.caminho.clone(), h, f.tamanho));
                        }
                        None
                    } else {
                        Some(
                            "sem cópia com Marcação de Backup em outro Dispositivo Físico"
                                .to_owned(),
                        )
                    }
                }
            };
            if let Some(m) = motivo {
                sem_total += 1;
                if sem.len() < EXEMPLOS_GRAVADOS {
                    sem.push(FormatoSemCoberturaInfo {
                        formato_id: f.id,
                        caminho: f.caminho.clone(),
                        motivo: m,
                    });
                }
            }
        }

        // 3. Restauração amostral: releitura integral e BLAKE3 das cópias de backup sorteadas.
        let n = (pedida as usize).min(reliveis.len());
        let mut falhas: Vec<String> = Vec::new();
        let (mut ok, mut verificadas) = (0_u32, 0_u32);
        for i in sortear(reliveis.len(), n) {
            if !self.seguir_biblioteca() {
                break;
            }
            let (formato_id, caminho, esperado, tamanho) = &reliveis[i];
            verificadas += 1;
            match blake3_arquivo(Path::new(caminho)) {
                Ok(h) if &h == esperado => {
                    let tam = std::fs::metadata(caminho).map(|m| m.len()).unwrap_or(0);
                    if tam == *tamanho {
                        ok += 1;
                    } else {
                        falhas.push(format!(
                            "formato {formato_id}: {caminho}: tamanho {tam} difere de {tamanho}"
                        ));
                    }
                }
                Ok(_) => falhas.push(format!(
                    "formato {formato_id}: {caminho}: BLAKE3 diferente do da Homologada"
                )),
                Err(e) => falhas.push(format!("formato {formato_id}: {caminho}: {e}")),
            }
        }
        let falha = verificadas - ok;
        let total = formatos.len() as u64;
        let pct_ok = total == 0 || cobertos * 100 >= u64::from(cfg.cobertura_minima_pct) * total;
        let aprovada = pct_ok && falha == 0 && (verificadas > 0 || cobertos == 0);

        // 4. Registro no Catálogo (único lugar onde se escreve).
        let conn = self.conexao();
        let id = store::registrar_backup(conn, b.id, &manifesto.to_string(), agora)?;
        store::registrar_verificacao_backup(
            conn,
            id,
            if aprovada {
                store::VerificacaoBackup::Ok
            } else {
                store::VerificacaoBackup::Falhou
            },
            agora,
        )?;
        let restauracao = serde_json::json!({
            "modo": "releitura_blake3_sem_copia", "pedida": pedida, "verificadas": verificadas,
            "ok": ok, "falha": falha, "falhas": falhas,
        });
        let cobertura = serde_json::json!({
            "formatos_total": total, "cobertos": cobertos, "sem_cobertura": sem_total,
            "minima_pct": cfg.cobertura_minima_pct, "aprovada": aprovada,
            "sem_cobertura_exemplos": sem,
        });
        store::registrar_restauracao_e_cobertura(
            conn,
            id,
            Some(&restauracao.to_string()),
            Some(&cobertura.to_string()),
            agora,
        )?;
        sem.truncate(EXEMPLOS);
        Ok(dados(DadosBiblioteca::Cobertura {
            resumo: ResumoCobertura {
                biblioteca: b.id,
                backup_id: id,
                verificado_em: agora,
                formatos_total: total,
                formatos_cobertos: cobertos,
                formatos_sem_cobertura: sem_total,
                sem_cobertura: sem,
                arquivos_no_manifesto: entradas.len() as u64,
                hash_metadata_db: hash_db,
                amostra_pedida: pedida,
                amostra_verificada: verificadas,
                amostra_ok: ok,
                amostra_falha: falha,
                falhas,
                aprovada,
                validade_dias: cfg.cobertura_validade_dias,
            },
        }))
    }
}
