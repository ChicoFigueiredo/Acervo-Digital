//! Fast Scan (A-12, spec §5): revisita o escopo das raízes e reconcilia o Catálogo.
//!
//! O Fast Scan usa a mesma visita do Full Scan (workers só leem; um coordenador escreve), mas a
//! gravação compara cada entrada com a linha existente:
//!
//! * **inalterada** (mesmo `(dev, inode)`, tipo, tamanho, mtime e ctime): só `seen_run_id` é
//!   atualizado, dentro do lote; metadados, classificação e `hashes` ficam intactos, então a
//!   etapa de hash reaproveita o Hash Completo e não lê um byte do arquivo (CA-A25);
//! * **nova ou alterada**: upsert normal; o hash antigo deixa de valer (a evidência não bate) e
//!   a etapa de hash relê só esses arquivos; Marcações viram Pendentes na reavaliação (CA-A24).
//!
//! Terminado o inventário **sem cancelamento**, [`finalizar`] decide o que foi renomeado
//! (a linha antiga assume o novo caminho: mesmo `files.id`, hash e Marcações) e o que sumiu
//! (`state = 'ausente'`). A remoção é recusada sob qualquer caminho que a varredura não
//! conseguiu visitar por inteiro ([`Bloqueios`]) e quando a fila de diretórios não terminou;
//! raiz inacessível ou volume desconectado nem chegam a ter escopo (CA-A09, spec §12.1–2).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dag_core::VolumeDescoberto;
use dag_store::Catalogo;
use dag_store::inventario::{self, ContadoresDb};
use dag_store::reconciliacao::{
    self, Bloqueios, Escopo, NovoEvento, Tabela, TipoEvento, coletar_novos_com_chave,
    coletar_sumidos, emparelhar, emparelhar_pastas,
};
use rusqlite::Connection;
use serde::Serialize;

use crate::classificador::Classificador;
use crate::config::{ConfigScan, Progresso};
use crate::controle::ControleScan;
use crate::erro::ErroScan;
use crate::scan::{FullScan, ResultadoScan};

/// Eventos individuais gravados por execução (criado/alterado/removido); renomeações sempre.
pub const MAX_EVENTOS_PADRAO: u64 = 5000;
/// Candidatos a renomeação guardados em memória (acima disso cai em remoção + criação).
const CAP_CANDIDATOS: usize = 500_000;
const LOTE: usize = 2000;

fn agora() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// Contadores vivos do Fast Scan (persistidos junto com os da execução para sobreviver à
/// retomada).
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Contagem {
    pub novos: u64,
    pub alterados: u64,
    pub inalterados: u64,
    pub pastas_novas: u64,
    pub eventos: u64,
    pub omitidos: u64,
}

pub(crate) fn acrescentar_contagem(json: &mut serde_json::Value, c: &Contagem) {
    if let Some(o) = json.as_object_mut() {
        o.insert("fast_novos".into(), c.novos.into());
        o.insert("fast_alterados".into(), c.alterados.into());
        o.insert("fast_inalterados".into(), c.inalterados.into());
        o.insert("fast_pastas_novas".into(), c.pastas_novas.into());
        o.insert("fast_eventos".into(), c.eventos.into());
        o.insert("fast_omitidos".into(), c.omitidos.into());
    }
}

pub(crate) fn contagem_salva(conn: &Connection, run_id: i64) -> Result<Contagem, ErroScan> {
    let json: String = conn.query_row(
        "SELECT counters FROM scan_runs WHERE id = ?1",
        [run_id],
        |l| l.get(0),
    )?;
    let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
    let n = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
    Ok(Contagem {
        novos: n("fast_novos"),
        alterados: n("fast_alterados"),
        inalterados: n("fast_inalterados"),
        pastas_novas: n("fast_pastas_novas"),
        eventos: n("fast_eventos"),
        omitidos: n("fast_omitidos"),
    })
}

/// Escopos (volume + caminho da raiz) gravados no checkpoint da execução.
pub(crate) fn escopos_do_checkpoint(
    conn: &Connection,
    checkpoint: Option<&str>,
) -> Result<Vec<Escopo>, ErroScan> {
    let v: serde_json::Value = checkpoint
        .and_then(|c| serde_json::from_str(c).ok())
        .unwrap_or_default();
    let mut out = Vec::new();
    for e in v
        .get("escopos")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(vol), Some(pasta)) = (
            e.get("volume_id").and_then(serde_json::Value::as_i64),
            e.get("pasta_id").and_then(serde_json::Value::as_i64),
        ) else {
            continue;
        };
        let caminho: Option<Vec<u8>> = conn
            .query_row("SELECT path_raw FROM folders WHERE id = ?1", [pasta], |l| {
                l.get(0)
            })
            .ok();
        if let Some(path_raw) = caminho {
            out.push(Escopo {
                volume_id: vol,
                path_raw,
            });
        }
    }
    Ok(out)
}

/// O que o Fast Scan fez.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ResumoFast {
    pub arquivos_novos: u64,
    pub arquivos_alterados: u64,
    /// Reaproveitados sem reescrever nada (nem ler conteúdo).
    pub arquivos_inalterados: u64,
    pub pastas_novas: u64,
    pub arquivos_renomeados: u64,
    pub pastas_renomeadas: u64,
    /// Marcados `ausente`.
    pub arquivos_ausentes: u64,
    pub pastas_ausentes: u64,
    /// Caminhos que a varredura não visitou por inteiro: nada abaixo deles foi dado como removido.
    pub bloqueios: u64,
    /// `false` quando a remoção foi recusada por inteiro (ver `motivo_sem_remocao`).
    pub remocao_aplicada: bool,
    pub motivo_sem_remocao: Option<String>,
    /// Eventos individuais que passaram do limite e só aparecem nos contadores.
    pub eventos_omitidos: u64,
}

pub(crate) fn json_final(c: &ContadoresDb, r: &ResumoFast) -> String {
    let mut v: serde_json::Value = serde_json::from_str(&c.json()).unwrap_or_default();
    if let (Some(o), Ok(r)) = (v.as_object_mut(), serde_json::to_value(r)) {
        o.insert("reconciliacao".into(), r);
    }
    v.to_string()
}

/// Depois do inventário completo: renomeações e remoções do escopo.
pub(crate) fn finalizar(
    cat: &mut Catalogo,
    run_id: i64,
    escopos: &[Escopo],
    max_eventos: u64,
    c: &Contagem,
) -> Result<ResumoFast, ErroScan> {
    let mut r = ResumoFast {
        arquivos_novos: c.novos,
        arquivos_alterados: c.alterados,
        arquivos_inalterados: c.inalterados,
        pastas_novas: c.pastas_novas,
        ..ResumoFast::default()
    };
    let mut eventos = c.eventos;
    let mut omitidos = c.omitidos;
    let t = agora();
    let (fila, bloq, escopos) = {
        let conn = cat.armazem().conexao();
        let fila = inventario::situacao_da_fila(conn, run_id)?;
        let bloq = Bloqueios::da_execucao(conn, run_id)?;
        // Só escopos de volume conectado (defesa em profundidade: a raiz já foi validada).
        let mut ativos = Vec::new();
        for e in escopos {
            let montado: i64 = conn
                .query_row(
                    "SELECT mounted FROM volumes WHERE id = ?1",
                    [e.volume_id],
                    |l| l.get(0),
                )
                .unwrap_or(0);
            if montado == 1 {
                ativos.push(e.clone());
            }
        }
        (fila, bloq, ativos)
    };
    r.bloqueios = bloq.len() as u64;
    if fila.livres > 0 || fila.reservadas > 0 {
        r.motivo_sem_remocao = Some("a fila de diretórios não foi esgotada".into());
        return registrar_criados(
            cat,
            run_id,
            &mut r,
            max_eventos,
            &mut eventos,
            &mut omitidos,
        );
    }
    if escopos.is_empty() {
        r.motivo_sem_remocao = Some("nenhuma raiz com volume conectado".into());
        return registrar_criados(
            cat,
            run_id,
            &mut r,
            max_eventos,
            &mut eventos,
            &mut omitidos,
        );
    }

    // 1. Pastas renomeadas (ids preservados; os filhos novos passam para a pasta antiga).
    let sumidas = coletar_sumidos(
        cat.armazem().conexao(),
        Tabela::Pastas,
        run_id,
        &escopos,
        &bloq,
        CAP_CANDIDATOS,
    )?;
    if !sumidas.cands.is_empty() {
        let novas = coletar_novos_com_chave(
            cat.armazem().conexao(),
            Tabela::Pastas,
            run_id,
            &sumidas.cands,
        )?;
        let pares = emparelhar_pastas(&sumidas.cands, &novas);
        for lote in pares.chunks(LOTE) {
            cat.armazem_mut()
                .transacao_imediata(|tx| -> Result<(), ErroScan> {
                    for (velha, nova) in lote {
                        reconciliacao::aplicar_renomeacao_pasta(
                            tx,
                            *velha,
                            *nova,
                            run_id,
                            "fast_scan",
                            t,
                        )?;
                    }
                    Ok(())
                })?;
            r.pastas_renomeadas += lote.len() as u64;
        }
    }

    // 2. Arquivos renomeados.
    let sumidos = coletar_sumidos(
        cat.armazem().conexao(),
        Tabela::Arquivos,
        run_id,
        &escopos,
        &bloq,
        CAP_CANDIDATOS,
    )?;
    if !sumidos.cands.is_empty() {
        let novos = coletar_novos_com_chave(
            cat.armazem().conexao(),
            Tabela::Arquivos,
            run_id,
            &sumidos.cands,
        )?;
        let pares = emparelhar(&sumidos.cands, &novos);
        for lote in pares.chunks(LOTE) {
            cat.armazem_mut()
                .transacao_imediata(|tx| -> Result<(), ErroScan> {
                    for (velho, novo) in lote {
                        reconciliacao::aplicar_renomeacao_arquivo(
                            tx,
                            *velho,
                            *novo,
                            run_id,
                            "fast_scan",
                            t,
                            true,
                        )?;
                    }
                    Ok(())
                })?;
            r.arquivos_renomeados += lote.len() as u64;
        }
    }

    // 3. Remoções: o que ainda sobrou sem ser visto, fora dos bloqueios.
    for (tabela, pasta) in [(Tabela::Pastas, true), (Tabela::Arquivos, false)] {
        let sumidos = coletar_sumidos(cat.armazem().conexao(), tabela, run_id, &escopos, &bloq, 0)?;
        for lote in sumidos.ids.chunks(LOTE) {
            let mut feitos = 0u64;
            let (mut ev, mut om) = (eventos, omitidos);
            cat.armazem_mut()
                .transacao_imediata(|tx| -> Result<(), ErroScan> {
                    for id in lote {
                        let Some(caminho) = reconciliacao::marcar_ausente(tx, tabela, *id)? else {
                            continue;
                        };
                        feitos += 1;
                        if ev < max_eventos {
                            ev += 1;
                            reconciliacao::registrar_evento(
                                tx,
                                &NovoEvento {
                                    caminho: Some(&caminho),
                                    arquivo_id: (!pasta).then_some(*id),
                                    execucao_id: Some(run_id),
                                    detalhe: pasta.then_some("pasta"),
                                    ..NovoEvento::processado(TipoEvento::Removido, "fast_scan", t)
                                },
                            )?;
                        } else {
                            om += 1;
                        }
                    }
                    Ok(())
                })?;
            (eventos, omitidos) = (ev, om);
            if pasta {
                r.pastas_ausentes += feitos;
            } else {
                r.arquivos_ausentes += feitos;
            }
        }
    }
    r.remocao_aplicada = true;
    registrar_criados(
        cat,
        run_id,
        &mut r,
        max_eventos,
        &mut eventos,
        &mut omitidos,
    )
}

/// Eventos `criado` dos arquivos novos que sobraram (as renomeações já viraram `renomeado`).
fn registrar_criados(
    cat: &mut Catalogo,
    run_id: i64,
    r: &mut ResumoFast,
    max_eventos: u64,
    eventos: &mut u64,
    omitidos: &mut u64,
) -> Result<ResumoFast, ErroScan> {
    let restante = max_eventos.saturating_sub(*eventos);
    let novos: Vec<(i64, i64, Vec<u8>)> = {
        let conn = cat.armazem().conexao();
        let mut st = conn.prepare(
            "SELECT id, volume_id, path_raw FROM files WHERE first_run_id = ?1 AND state = 'ativo'
             ORDER BY id LIMIT ?2",
        )?;
        st.query_map(
            rusqlite::params![run_id, i64::try_from(restante).unwrap_or(i64::MAX)],
            |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)),
        )?
        .collect::<Result<_, _>>()?
    };
    let total_novos: i64 = cat.armazem().conexao().query_row(
        "SELECT COUNT(*) FROM files WHERE first_run_id = ?1 AND state = 'ativo'",
        [run_id],
        |l| l.get(0),
    )?;
    let t = agora();
    for lote in novos.chunks(LOTE) {
        cat.armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroScan> {
                for (id, vol, caminho) in lote {
                    reconciliacao::registrar_evento(
                        tx,
                        &NovoEvento {
                            volume_id: Some(*vol),
                            caminho: Some(caminho),
                            arquivo_id: Some(*id),
                            execucao_id: Some(run_id),
                            ..NovoEvento::processado(TipoEvento::Criado, "fast_scan", t)
                        },
                    )?;
                }
                Ok(())
            })?;
    }
    *eventos += novos.len() as u64;
    *omitidos += u64::try_from(total_novos)
        .unwrap_or(0)
        .saturating_sub(novos.len() as u64);
    r.eventos_omitidos = *omitidos;
    // Os contadores de novos refletem o que de fato sobrou como novo (renomeados saem).
    r.arquivos_novos = u64::try_from(total_novos).unwrap_or(0);
    Ok(r.clone())
}

/// Fast Scan sobre as raízes. Mesma interface do [`FullScan`]; o resultado traz
/// [`ResultadoScan::reconciliacao`].
pub struct FastScan {
    inner: FullScan,
}

impl std::fmt::Debug for FastScan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FastScan")
            .field("inner", &self.inner)
            .finish()
    }
}

impl FastScan {
    /// `raizes` já catalogadas (ou novas); `volumes` vêm da descoberta de plataforma.
    #[must_use]
    pub fn novo(raizes: Vec<PathBuf>, volumes: Vec<VolumeDescoberto>) -> Self {
        Self {
            inner: FullScan::novo(raizes, volumes).rapido(MAX_EVENTOS_PADRAO),
        }
    }

    #[must_use]
    pub fn com_config(mut self, config: ConfigScan) -> Self {
        self.inner = self.inner.com_config(config);
        self
    }

    #[must_use]
    pub fn com_controle(mut self, controle: ControleScan) -> Self {
        self.inner = self.inner.com_controle(controle);
        self
    }

    #[must_use]
    pub fn com_progresso(mut self, f: impl Fn(&Progresso) + Send + Sync + 'static) -> Self {
        self.inner = self.inner.com_progresso(f);
        self
    }

    #[must_use]
    pub fn com_classificador(mut self, c: Arc<dyn Classificador>) -> Self {
        self.inner = self.inner.com_classificador(c);
        self
    }

    /// Limite de eventos individuais (criado/alterado/removido) gravados por execução.
    #[must_use]
    pub fn com_max_eventos(mut self, n: u64) -> Self {
        self.inner = self.inner.rapido(n);
        self
    }

    #[must_use]
    pub fn controle(&self) -> ControleScan {
        self.inner.controle()
    }

    /// Inicia um Fast Scan.
    ///
    /// # Errors
    /// [`ErroScan::SemRaizes`] se nenhuma raiz serve; falhas do Catálogo.
    pub fn executar(&self, catalogo: &mut Catalogo) -> Result<ResultadoScan, ErroScan> {
        self.inner.executar(catalogo)
    }

    /// Retoma um Fast Scan cancelado, pausado ou interrompido (a remoção só acontece no fim).
    ///
    /// # Errors
    /// Como [`FullScan::retomar`].
    pub fn retomar(
        &self,
        catalogo: &mut Catalogo,
        execucao_id: i64,
    ) -> Result<ResultadoScan, ErroScan> {
        self.inner.retomar(catalogo, execucao_id)
    }
}
