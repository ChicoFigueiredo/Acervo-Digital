//! Etapa de Hash Completo (BLAKE3) e Verificação de Integridade, somente-leitura.
//!
//! # Pipeline
//!
//! ```text
//!  fila_hash (SQLite) ──reserva──▶ coordenador ──ItemFila──▶ N leitores (open O_RDONLY, read, BLAKE3)
//!        ▲                            │  ▲                           │ (item, Saida)
//!        └── hash gravado em tx curta ┘  └────────── canal ◀─────────┘
//! ```
//!
//! * Leitores só leem e calculam; o coordenador (a thread que chama `executar`) é o único
//!   escritor do Catálogo e grava em transações curtas.
//! * O hash só é gravado depois da leitura inteira **e** de uma segunda conferência (`fstat` e
//!   `stat` do caminho): se tamanho, mtime, ctime ou inode mudaram durante a leitura, nada é
//!   gravado e o caso vai para `scan_errors` ("alterado durante leitura"). Hash parcial nunca é
//!   persistido (CA-A18).
//! * Reuso (CA-A25): arquivo cuja evidência guardada com o hash (tamanho, mtime, ctime, inode)
//!   ainda bate com o inventário não é lido.
//! * Hard links: uma leitura por arquivo físico; o hash vale para todos os caminhos (CA-A19).
//! * Pausa/cancelamento via [`ControleScan`]. Cancelar interrompe as leituras em andamento
//!   (descartadas, itens voltam a `pendente`); retomar é chamar a etapa de novo: arquivos já
//!   concluídos têm hash válido e não são relidos.
//! * Abre com `O_RDONLY | O_NOATIME` (sem `NOATIME` se o kernel recusar com `EPERM`), nunca
//!   escreve, não segue symlinks (`O_NOFOLLOW`).

use std::ffi::OsStr;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dag_core::{ArquivoId, ExecucaoId};
use dag_store::Catalogo;
use dag_store::hashes::{self, FiltroVerificacao, ItemFila, TipoFila};
use dag_store::inventario::{self, EstadoScan, NovoProblema, Severidade};
use rustix::fs::{Advice, OFlags};

use crate::controle::{ControleScan, EstadoControle};
use crate::erro::ErroScan;
use crate::scan::DesfechoScan;

/// Como a etapa terminou (mesmos desfechos do scan).
pub type Desfecho = DesfechoScan;

/// Parâmetros da etapa. Os padrões são conservadores: disco é o gargalo (A-21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigHash {
    /// Leitores simultâneos (padrão 2).
    pub concorrencia: usize,
    /// Limite global de leitura em bytes por segundo (`None` = sem limite).
    pub limite_bytes_por_seg: Option<u64>,
    /// Tamanho do buffer de leitura de cada leitor (padrão 1 MiB).
    pub tamanho_buffer: usize,
    /// Intervalo mínimo entre chamadas do callback de progresso.
    pub intervalo_de_progresso: Duration,
}

impl Default for ConfigHash {
    fn default() -> Self {
        Self {
            concorrencia: 2,
            limite_bytes_por_seg: None,
            tamanho_buffer: 1 << 20,
            intervalo_de_progresso: Duration::from_millis(250),
        }
    }
}

/// Instantâneo de progresso da etapa.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressoHash {
    /// Linhas de arquivo resolvidas (hasheadas, reaproveitadas ou com erro).
    pub arquivos_feitos: u64,
    pub arquivos_total: u64,
    /// Bytes efetivamente lidos do disco até agora.
    pub bytes_lidos: u64,
    /// Bytes a ler (soma dos arquivos físicos enfileirados).
    pub bytes_total: u64,
    pub bytes_por_seg: f64,
    pub reaproveitados: u64,
}

/// Resultado da etapa de hash.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultadoHash {
    pub desfecho: Desfecho,
    /// Linhas de arquivo que receberam hash nesta etapa (um hard link com vários caminhos conta
    /// cada caminho; a leitura física foi uma só, veja `bytes_lidos`).
    pub hasheados: u64,
    /// Linhas de arquivo que já tinham hash válido e não foram lidas.
    pub reaproveitados: u64,
    /// Bytes lidos do disco (inclui leituras descartadas por alteração/cancelamento).
    pub bytes_lidos: u64,
    /// Arquivos que não puderam ser hasheados (acesso, sumiu, mudou durante a leitura).
    pub erros: u64,
    pub duracao: Duration,
}

/// Resultado da Verificação de Integridade.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultadoVerificacao {
    pub desfecho: Desfecho,
    /// Execução (`scan_runs`, modo `verificacao`) que registra a verificação.
    pub execucao_id: i64,
    /// Linhas de arquivo conferidas (ok + divergentes).
    pub verificados: u64,
    /// Arquivos cujo conteúdo não bate com o hash guardado (o hash guardado não é alterado).
    pub divergentes: Vec<ArquivoId>,
    /// Arquivos que mudaram legitimamente (tamanho, mtime ou inode) e por isso foram pulados.
    pub alterados: u64,
    pub erros: u64,
    pub bytes_lidos: u64,
    pub duracao: Duration,
}

type CallbackProgresso = Box<dyn FnMut(&ProgressoHash) + Send>;
type Gancho = Arc<dyn Fn(&Path) + Send + Sync>;

fn agora() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

// ------------------------------------------------------------------ API pública

/// Etapa de Hash Completo das execuções do Full Scan.
pub struct EtapaHash {
    execucao: ExecucaoId,
    config: ConfigHash,
    controle: ControleScan,
    progresso: Option<CallbackProgresso>,
    gancho: Option<Gancho>,
}

impl std::fmt::Debug for EtapaHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EtapaHash")
            .field("execucao", &self.execucao)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl EtapaHash {
    /// Hasheia os arquivos vistos na execução `execucao`.
    #[must_use]
    pub fn nova(execucao: ExecucaoId) -> Self {
        Self {
            execucao,
            config: ConfigHash::default(),
            controle: ControleScan::novo(),
            progresso: None,
            gancho: None,
        }
    }

    #[must_use]
    pub fn com_config(mut self, cfg: ConfigHash) -> Self {
        self.config = cfg;
        self
    }

    #[must_use]
    pub fn com_controle(mut self, c: ControleScan) -> Self {
        self.controle = c;
        self
    }

    /// Callback de progresso, chamado na thread que executa a etapa.
    #[must_use]
    pub fn com_progresso(mut self, f: impl FnMut(&ProgressoHash) + Send + 'static) -> Self {
        self.progresso = Some(Box::new(f));
        self
    }

    /// Ponto de teste: chamado com o caminho depois que o arquivo foi lido por inteiro e antes
    /// da conferência final. Não faz parte da API estável.
    #[doc(hidden)]
    #[must_use]
    pub fn com_gancho_apos_leitura(mut self, f: impl Fn(&Path) + Send + Sync + 'static) -> Self {
        self.gancho = Some(Arc::new(f));
        self
    }

    /// Executa a etapa.
    ///
    /// # Errors
    /// Falha do Catálogo ou execução inexistente. Problemas com arquivos individuais não são
    /// erros: ficam em `scan_errors` e na contagem `erros`.
    pub fn executar(self, cat: &mut Catalogo) -> Result<ResultadoHash, ErroScan> {
        let inicio = Instant::now();
        let run_id = self.execucao.0;
        let t = agora();
        let (fase_anterior, resumo) = cat.armazem_mut().transacao_imediata(
            |tx| -> Result<(String, hashes::ResumoFila), ErroScan> {
                let fase = hashes::fase_da_execucao(tx, run_id)?
                    .ok_or(ErroScan::ExecucaoInexistente(run_id))?;
                hashes::definir_fase(tx, run_id, "hash")?;
                hashes::liberar_reservas(tx, TipoFila::Hash, t)?;
                let resumo = hashes::preparar_fila_hash(tx, run_id, t)?;
                Ok((fase, resumo))
            },
        )?;
        let sem_hash_linhas = resumo.candidatos - resumo.reaproveitados;
        let motor = Motor {
            tipo: TipoFila::Hash,
            fase: "hash",
            run_id,
            config: self.config,
            controle: self.controle,
            progresso: self.progresso,
            gancho: self.gancho,
            arquivos_total: resumo.candidatos,
            bytes_total: resumo.bytes_total,
            reaproveitados: resumo.reaproveitados,
        };
        let r = motor.rodar(cat, inicio)?;
        let desfecho = if r.cancelado {
            Desfecho::Cancelada
        } else if r.erros > 0 {
            Desfecho::ConcluidaComErros
        } else {
            Desfecho::Concluida
        };
        let json = serde_json::json!({
            "candidatos": resumo.candidatos, "reaproveitados": resumo.reaproveitados,
            "sem_hash_no_inicio": sem_hash_linhas, "hasheados": r.feitos_ok,
            "bytes_lidos": r.bytes_lidos, "erros": r.erros, "cancelado": r.cancelado,
        })
        .to_string();
        let fase_final = if fase_anterior == "hash" || fase_anterior == "verificacao" {
            "concluida".to_owned()
        } else {
            fase_anterior
        };
        cat.armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroScan> {
                hashes::salvar_contadores_etapa(tx, run_id, "hash", &json)?;
                if !r.cancelado {
                    hashes::definir_fase(tx, run_id, &fase_final)?;
                }
                Ok(())
            })?;
        Ok(ResultadoHash {
            desfecho,
            hasheados: r.feitos_ok,
            reaproveitados: resumo.reaproveitados,
            bytes_lidos: r.bytes_lidos,
            erros: r.erros,
            duracao: inicio.elapsed(),
        })
    }
}

/// Verificação de Integridade (A-33): relê arquivos que já têm hash e compara com o guardado.
/// Nunca modifica os arquivos nem sobrescreve o hash guardado; só grava `verified_at` e
/// `verification_state` (`ok` ou `divergente`).
pub struct VerificacaoIntegridade {
    volumes: Vec<i64>,
    raizes: Vec<PathBuf>,
    config: ConfigHash,
    controle: ControleScan,
    progresso: Option<CallbackProgresso>,
}

impl std::fmt::Debug for VerificacaoIntegridade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerificacaoIntegridade")
            .field("volumes", &self.volumes)
            .field("raizes", &self.raizes)
            .finish_non_exhaustive()
    }
}

impl VerificacaoIntegridade {
    /// Verifica todo o Catálogo (use [`volumes`](Self::volumes) ou [`raizes`](Self::raizes)
    /// para restringir).
    #[must_use]
    pub fn nova() -> Self {
        Self {
            volumes: Vec::new(),
            raizes: Vec::new(),
            config: ConfigHash::default(),
            controle: ControleScan::novo(),
            progresso: None,
        }
    }

    /// Restringe a estes volumes (ids do Catálogo).
    #[must_use]
    pub fn volumes(mut self, ids: impl IntoIterator<Item = dag_core::VolumeId>) -> Self {
        self.volumes.extend(ids.into_iter().map(|v| v.0));
        self
    }

    /// Restringe a estas pastas/arquivos (caminhos absolutos).
    #[must_use]
    pub fn raizes(mut self, caminhos: impl IntoIterator<Item = PathBuf>) -> Self {
        self.raizes.extend(caminhos);
        self
    }

    #[must_use]
    pub fn com_config(mut self, cfg: ConfigHash) -> Self {
        self.config = cfg;
        self
    }

    #[must_use]
    pub fn com_controle(mut self, c: ControleScan) -> Self {
        self.controle = c;
        self
    }

    #[must_use]
    pub fn com_progresso(mut self, f: impl FnMut(&ProgressoHash) + Send + 'static) -> Self {
        self.progresso = Some(Box::new(f));
        self
    }

    /// Executa a verificação.
    ///
    /// # Errors
    /// Falha do Catálogo.
    pub fn executar(self, cat: &mut Catalogo) -> Result<ResultadoVerificacao, ErroScan> {
        let inicio = Instant::now();
        let t = agora();
        let raizes_json = serde_json::to_string(
            &self
                .raizes
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        )
        .map_err(|e| ErroScan::Interno(e.to_string()))?;
        let (run_id, total) =
            cat.armazem_mut()
                .transacao_imediata(|tx| -> Result<(i64, (u64, u64)), ErroScan> {
                    let run_id =
                        inventario::criar_execucao(tx, "verificacao", &raizes_json, None, t)?;
                    hashes::definir_fase(tx, run_id, "verificacao")?;
                    hashes::limpar_fila(tx, TipoFila::Verificacao)?;
                    if self.volumes.is_empty() && self.raizes.is_empty() {
                        hashes::enfileirar_verificacao(tx, FiltroVerificacao::Tudo, t)?;
                    }
                    for v in &self.volumes {
                        hashes::enfileirar_verificacao(tx, FiltroVerificacao::Volume(*v), t)?;
                    }
                    for r in &self.raizes {
                        hashes::enfileirar_verificacao(
                            tx,
                            FiltroVerificacao::Prefixo(r.as_os_str().as_bytes()),
                            t,
                        )?;
                    }
                    Ok((run_id, hashes::totais_pendentes(tx, TipoFila::Verificacao)?))
                })?;
        let motor = Motor {
            tipo: TipoFila::Verificacao,
            fase: "verificacao",
            run_id,
            config: self.config,
            controle: self.controle,
            progresso: self.progresso,
            gancho: None,
            arquivos_total: total.0,
            bytes_total: total.1,
            reaproveitados: 0,
        };
        let r = motor.rodar(cat, inicio)?;
        let desfecho = if r.cancelado {
            Desfecho::Cancelada
        } else if r.erros > 0 || !r.divergentes.is_empty() {
            Desfecho::ConcluidaComErros
        } else {
            Desfecho::Concluida
        };
        let json = serde_json::json!({
            "verificados": r.feitos_ok, "divergentes": r.divergentes.len(),
            "alterados": r.alterados, "bytes_lidos": r.bytes_lidos, "erros": r.erros,
        })
        .to_string();
        let estado = match desfecho {
            Desfecho::Concluida => EstadoScan::Concluida,
            Desfecho::ConcluidaComErros => EstadoScan::ConcluidaComErros,
            Desfecho::Cancelada => EstadoScan::Cancelada,
        };
        cat.armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroScan> {
                inventario::salvar_contadores(tx, run_id, &json)?;
                inventario::definir_estado(tx, run_id, estado, None, agora())?;
                if !r.cancelado {
                    hashes::definir_fase(tx, run_id, "concluida")?;
                }
                Ok(())
            })?;
        Ok(ResultadoVerificacao {
            desfecho,
            execucao_id: run_id,
            verificados: r.feitos_ok,
            divergentes: r.divergentes.into_iter().map(ArquivoId).collect(),
            alterados: r.alterados,
            erros: r.erros,
            bytes_lidos: r.bytes_lidos,
            duracao: inicio.elapsed(),
        })
    }
}

// ------------------------------------------------------------------ leitor

/// O que um leitor devolve ao coordenador.
enum Saida {
    Lido {
        hash: [u8; 32],
        bytes: u64,
    },
    /// O arquivo mudou (desde o inventário ou durante a leitura): nada a gravar.
    Alterado {
        codigo: &'static str,
        motivo: String,
    },
    Falha {
        codigo: &'static str,
        motivo: String,
    },
    Abortado,
}

/// Limite global de bytes por segundo, compartilhado entre os leitores.
struct Limitador {
    bps: u64,
    estado: Mutex<(Instant, u64)>,
}

impl Limitador {
    fn consumir(&self, n: u64, abortar: &dyn Fn() -> bool) {
        let espera = {
            let mut g = self.estado.lock().unwrap_or_else(PoisonError::into_inner);
            g.1 += n;
            let alvo = Duration::from_secs_f64(g.1 as f64 / self.bps as f64);
            let decorrido = g.0.elapsed();
            if decorrido > alvo + Duration::from_secs(1) {
                // Ficou ocioso (pausa): não acumula crédito para uma rajada.
                *g = (Instant::now(), n);
                Duration::ZERO
            } else {
                alvo.saturating_sub(decorrido)
            }
        };
        let fim = Instant::now() + espera;
        while !abortar() {
            let falta = fim.saturating_duration_since(Instant::now());
            if falta.is_zero() {
                break;
            }
            std::thread::sleep(falta.min(Duration::from_millis(50)));
        }
    }
}

fn abrir_somente_leitura(caminho: &Path) -> std::io::Result<File> {
    let base = (OFlags::NOFOLLOW | OFlags::NONBLOCK).bits() as i32;
    let tentativa = |extra: i32| {
        OpenOptions::new()
            .read(true)
            .custom_flags(base | extra)
            .open(caminho)
    };
    match tentativa(OFlags::NOATIME.bits() as i32) {
        Err(e) if e.kind() == ErrorKind::PermissionDenied => tentativa(0),
        outro => outro,
    }
}

fn ns(s: i64, n: i64) -> i64 {
    s * 1_000_000_000 + n
}

fn mtime_ns(m: &Metadata) -> i64 {
    ns(m.mtime(), m.mtime_nsec())
}

fn ctime_ns(m: &Metadata) -> i64 {
    ns(m.ctime(), m.ctime_nsec())
}

/// A metadata de disco bate com a evidência esperada? Na verificação o ctime não conta: um byte
/// corrompido com mtime restaurado muda o ctime, e é exatamente o que se quer detectar.
fn bate(m: &Metadata, item: &ItemFila, usar_ctime: bool) -> bool {
    i64::try_from(m.len()).is_ok_and(|t| t == item.tamanho)
        && item.mtime_ns.is_none_or(|v| v == mtime_ns(m))
        && (!usar_ctime || item.ctime_ns.is_none_or(|v| v == ctime_ns(m)))
        && item
            .native_file_id
            .is_none_or(|v| i64::try_from(m.ino()).is_ok_and(|i| i == v))
}

fn estavel(a: &Metadata, b: &Metadata) -> bool {
    a.len() == b.len()
        && mtime_ns(a) == mtime_ns(b)
        && ctime_ns(a) == ctime_ns(b)
        && a.ino() == b.ino()
        && a.dev() == b.dev()
}

fn falha_io(e: &std::io::Error) -> Saida {
    let codigo = match e.kind() {
        ErrorKind::NotFound => "hash_ausente",
        ErrorKind::PermissionDenied => "hash_permissao",
        _ => "hash_io",
    };
    Saida::Falha {
        codigo,
        motivo: e.to_string(),
    }
}

struct Leitor<'a> {
    verificacao: bool,
    abortar: &'a (dyn Fn() -> bool + Sync),
    limitador: Option<&'a Limitador>,
    gancho: Option<&'a Gancho>,
    bytes_lidos: &'a AtomicU64,
}

impl Leitor<'_> {
    fn ler(&self, item: &ItemFila, buf: &mut [u8]) -> Saida {
        if (self.abortar)() {
            return Saida::Abortado;
        }
        let caminho = Path::new(OsStr::from_bytes(&item.path_raw));
        let mut f = match abrir_somente_leitura(caminho) {
            Ok(f) => f,
            Err(e) => return falha_io(&e),
        };
        let antes = match f.metadata() {
            Ok(m) => m,
            Err(e) => return falha_io(&e),
        };
        if !antes.is_file() {
            return Saida::Falha {
                codigo: "hash_nao_regular",
                motivo: "deixou de ser arquivo regular".into(),
            };
        }
        if !bate(&antes, item, !self.verificacao) {
            return Saida::Alterado {
                codigo: "hash_alterado_desde_inventario",
                motivo: "tamanho, data ou inode diferem do inventário; refaça a varredura".into(),
            };
        }
        let _ = rustix::fs::fadvise(&f, 0, None, Advice::Sequential);
        let mut hasher = blake3::Hasher::new();
        let mut lidos: u64 = 0;
        loop {
            if (self.abortar)() {
                return Saida::Abortado;
            }
            let n = match f.read(buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return falha_io(&e),
            };
            hasher.update(&buf[..n]);
            lidos += n as u64;
            self.bytes_lidos.fetch_add(n as u64, Ordering::Relaxed);
            if let Some(l) = self.limitador {
                l.consumir(n as u64, self.abortar);
            }
        }
        if let Some(g) = self.gancho {
            g(caminho);
        }
        let depois_fd = match f.metadata() {
            Ok(m) => m,
            Err(e) => return falha_io(&e),
        };
        let depois_caminho = std::fs::symlink_metadata(caminho);
        let mudou = !estavel(&antes, &depois_fd)
            || lidos != antes.len()
            || depois_caminho.is_err()
            || depois_caminho.is_ok_and(|m| !estavel(&antes, &m));
        if mudou {
            return Saida::Alterado {
                codigo: "hash_alterado_durante_leitura",
                motivo: "alterado durante leitura; hash não gravado".into(),
            };
        }
        Saida::Lido {
            hash: *hasher.finalize().as_bytes(),
            bytes: lidos,
        }
    }
}

// ------------------------------------------------------------------ coordenador

struct Motor {
    tipo: TipoFila,
    fase: &'static str,
    run_id: i64,
    config: ConfigHash,
    controle: ControleScan,
    progresso: Option<CallbackProgresso>,
    gancho: Option<Gancho>,
    arquivos_total: u64,
    bytes_total: u64,
    reaproveitados: u64,
}

#[derive(Default)]
struct Contagem {
    feitos_ok: u64,
    erros: u64,
    alterados: u64,
    bytes_lidos: u64,
    cancelado: bool,
    divergentes: Vec<i64>,
}

impl Motor {
    fn rodar(mut self, cat: &mut Catalogo, inicio: Instant) -> Result<Contagem, ErroScan> {
        let verificacao = self.tipo == TipoFila::Verificacao;
        let leitores = self.config.concorrencia.max(1);
        let alvo = leitores * 4;
        let buffer = self.config.tamanho_buffer.max(4096);
        let parar = AtomicBool::new(false);
        let bytes_lidos = AtomicU64::new(0);
        let limitador = self
            .config
            .limite_bytes_por_seg
            .filter(|b| *b > 0)
            .map(|bps| Limitador {
                bps,
                estado: Mutex::new((Instant::now(), 0)),
            });
        let controle = self.controle.clone();
        let parar_ref = &parar;
        let abortar = move || controle.cancelado() || parar_ref.load(Ordering::Relaxed);
        let (tx_item, rx_item) = mpsc::sync_channel::<ItemFila>(alvo);
        let rx_item = Mutex::new(rx_item);
        let (tx_saida, rx_saida) = mpsc::channel::<(ItemFila, Saida)>();
        let gancho = self.gancho.take();
        let mut conta = Contagem::default();
        let mut feitos_linhas: u64 = self.reaproveitados;
        let mut ultimo = Instant::now()
            .checked_sub(self.config.intervalo_de_progresso)
            .unwrap_or_else(Instant::now);

        let resultado: Result<(), ErroScan> = std::thread::scope(|s| {
            for _ in 0..leitores {
                let tx = tx_saida.clone();
                let rx = &rx_item;
                let leitor = Leitor {
                    verificacao,
                    abortar: &abortar,
                    limitador: limitador.as_ref(),
                    gancho: gancho.as_ref(),
                    bytes_lidos: &bytes_lidos,
                };
                s.spawn(move || {
                    let mut buf = vec![0u8; buffer];
                    loop {
                        let item = rx.lock().unwrap_or_else(PoisonError::into_inner).recv();
                        let Ok(item) = item else { break };
                        let saida = leitor.ler(&item, &mut buf);
                        if tx.send((item, saida)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(tx_saida);

            let mut em_voo: usize = 0;
            let mut fila_vazia = false;
            let r: Result<(), ErroScan> = loop {
                let cancelado = self.controle.cancelado();
                let pausado = self.controle.estado() == EstadoControle::Pausado;
                if !cancelado && !pausado && !fila_vazia && em_voo < alvo / 2 {
                    let t = agora();
                    let falta = alvo - em_voo;
                    let lote = cat.armazem_mut().transacao_imediata(
                        |tx| -> Result<Vec<ItemFila>, ErroScan> {
                            Ok(hashes::reservar(tx, self.tipo, falta, t)?)
                        },
                    );
                    match lote {
                        Ok(lote) => {
                            if lote.len() < falta {
                                fila_vazia = true;
                            }
                            for item in lote {
                                em_voo += 1;
                                if tx_item.send(item).is_err() {
                                    break;
                                }
                            }
                        }
                        Err(e) => break Err(e),
                    }
                }
                if em_voo == 0 {
                    if cancelado || fila_vazia {
                        break Ok(());
                    }
                    if pausado {
                        self.emitir(
                            &conta,
                            feitos_linhas,
                            &bytes_lidos,
                            inicio,
                            &mut ultimo,
                            true,
                        );
                        std::thread::sleep(Duration::from_millis(25));
                        continue;
                    }
                }
                match rx_saida.recv_timeout(Duration::from_millis(100)) {
                    Ok(primeiro) => {
                        let mut lote = vec![primeiro];
                        while let Ok(m) = rx_saida.try_recv() {
                            lote.push(m);
                        }
                        em_voo -= lote.len();
                        match self.gravar_lote(cat, lote) {
                            Ok(d) => {
                                feitos_linhas += d.linhas;
                                conta.feitos_ok += d.ok;
                                conta.erros += d.erros;
                                conta.alterados += d.alterados;
                                conta.divergentes.extend(d.divergentes);
                            }
                            Err(e) => break Err(e),
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break Ok(()),
                }
                self.emitir(
                    &conta,
                    feitos_linhas,
                    &bytes_lidos,
                    inicio,
                    &mut ultimo,
                    false,
                );
            };
            if r.is_err() {
                parar.store(true, Ordering::Relaxed);
            }
            drop(tx_item);
            // Recolhe o que ainda estava em voo: abortados voltam à fila; concluídos a tempo
            // (leitura inteira já feita) são gravados.
            let mut resto = Vec::new();
            while em_voo > 0 {
                match rx_saida.recv() {
                    Ok(m) => {
                        resto.push(m);
                        em_voo -= 1;
                    }
                    Err(_) => break,
                }
            }
            if !resto.is_empty()
                && let Ok(d) = self.gravar_lote(cat, resto)
            {
                feitos_linhas += d.linhas;
                conta.feitos_ok += d.ok;
                conta.erros += d.erros;
                conta.alterados += d.alterados;
                conta.divergentes.extend(d.divergentes);
            }
            r
        });
        resultado?;
        conta.bytes_lidos = bytes_lidos.load(Ordering::Relaxed);
        let t = agora();
        let restantes =
            cat.armazem_mut()
                .transacao_imediata(|tx| -> Result<(u64, u64), ErroScan> {
                    hashes::liberar_reservas(tx, self.tipo, t)?;
                    Ok(hashes::totais_pendentes(tx, self.tipo)?)
                })?;
        conta.cancelado = self.controle.cancelado() && restantes.0 > 0;
        self.emitir(
            &conta,
            feitos_linhas,
            &bytes_lidos,
            inicio,
            &mut ultimo,
            true,
        );
        Ok(conta)
    }

    fn emitir(
        &mut self,
        conta: &Contagem,
        feitos: u64,
        bytes: &AtomicU64,
        inicio: Instant,
        ultimo: &mut Instant,
        forcar: bool,
    ) {
        let Some(cb) = self.progresso.as_mut() else {
            return;
        };
        if !forcar && ultimo.elapsed() < self.config.intervalo_de_progresso {
            return;
        }
        *ultimo = Instant::now();
        let lidos = bytes.load(Ordering::Relaxed);
        let seg = inicio.elapsed().as_secs_f64();
        cb(&ProgressoHash {
            arquivos_feitos: feitos,
            arquivos_total: self.arquivos_total,
            bytes_lidos: lidos,
            bytes_total: self.bytes_total,
            bytes_por_seg: if seg > 0.0 { lidos as f64 / seg } else { 0.0 },
            reaproveitados: self.reaproveitados,
        });
        let _ = conta;
    }

    /// Grava um lote de resultados numa transação curta.
    fn gravar_lote(
        &self,
        cat: &mut Catalogo,
        lote: Vec<(ItemFila, Saida)>,
    ) -> Result<Delta, ErroScan> {
        let t = agora();
        let run_id = self.run_id;
        let tipo = self.tipo;
        let fase = self.fase;
        cat.armazem_mut()
            .transacao_imediata(|tx| -> Result<Delta, ErroScan> {
                let mut d = Delta::default();
                for (item, saida) in &lote {
                    match saida {
                        Saida::Lido { hash, bytes } if tipo == TipoFila::Hash => {
                            let n = hashes::gravar_hash(tx, item, hash, *bytes, t)?;
                            d.ok += n;
                            d.linhas += n;
                        }
                        Saida::Lido { hash, bytes } => {
                            let guardado = item.hash_guardado.as_deref().unwrap_or_default();
                            let divergente = guardado != hash.as_slice();
                            let marcados =
                                hashes::registrar_verificacao(tx, item, divergente, *bytes, t)?;
                            d.ok += marcados.len() as u64;
                            d.linhas += marcados.len() as u64;
                            if divergente {
                                d.divergentes.extend(&marcados);
                                registrar(
                                    tx,
                                    run_id,
                                    item,
                                    fase,
                                    "verificacao_divergente",
                                    "conteúdo difere do hash guardado",
                                    Severidade::Erro,
                                    t,
                                )?;
                            }
                        }
                        Saida::Alterado { codigo, motivo } => {
                            hashes::marcar_erro(tx, item.fila_id, motivo, t)?;
                            if tipo == TipoFila::Hash {
                                registrar(
                                    tx,
                                    run_id,
                                    item,
                                    fase,
                                    codigo,
                                    motivo,
                                    Severidade::Aviso,
                                    t,
                                )?;
                                d.erros += 1;
                            } else {
                                registrar(
                                    tx,
                                    run_id,
                                    item,
                                    fase,
                                    codigo,
                                    motivo,
                                    Severidade::Info,
                                    t,
                                )?;
                                d.alterados += 1;
                            }
                            d.linhas += 1;
                        }
                        Saida::Falha { codigo, motivo } => {
                            hashes::marcar_erro(tx, item.fila_id, motivo, t)?;
                            registrar(
                                tx,
                                run_id,
                                item,
                                fase,
                                codigo,
                                motivo,
                                Severidade::Aviso,
                                t,
                            )?;
                            d.erros += 1;
                            d.linhas += 1;
                        }
                        Saida::Abortado => hashes::devolver_item(tx, item.fila_id, t)?,
                    }
                }
                Ok(d)
            })
    }
}

#[derive(Default)]
struct Delta {
    linhas: u64,
    ok: u64,
    erros: u64,
    alterados: u64,
    divergentes: Vec<i64>,
}

#[allow(clippy::too_many_arguments)]
fn registrar(
    conn: &rusqlite::Connection,
    run_id: i64,
    item: &ItemFila,
    fase: &str,
    codigo: &str,
    motivo: &str,
    severidade: Severidade,
    t: i64,
) -> Result<(), ErroScan> {
    inventario::registrar_problema(
        conn,
        &NovoProblema {
            run_id,
            pasta_id: item.pasta_id,
            path_raw: Some(&item.path_raw),
            fase,
            codigo,
            motivo: Some(motivo),
            severidade,
            observado_em: t,
        },
    )?;
    Ok(())
}
