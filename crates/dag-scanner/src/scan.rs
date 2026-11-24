//! O Full Scan: raízes, coordenação, lotes, pausa/cancelamento/retomada e agregação.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dag_core::VolumeDescoberto;
use dag_store::Catalogo;
use dag_store::inventario::agregacao::{ResumoAgregacao, agregar_execucao};
use dag_store::inventario::{
    self, ContadoresDb, DiretorioPendente, EstadoScan, FaseScan, FimDiretorio, NovaPasta,
    NovoArquivo, NovoProblema, Severidade, TipoEntrada,
};
use dag_store::reconciliacao::{self, Efeito, Escopo, NovoEvento, TipoEvento};
use rusqlite::Connection;

use crate::classificador::{Classificador, SemClassificacao};
use crate::config::{ConfigScan, FaseProgresso, Progresso};
use crate::controle::{ControleScan, EstadoControle};
use crate::elevado::LeitorElevado;
use crate::erro::ErroScan;
use crate::fast::{self, Contagem, ResumoFast};
use crate::visita::{self, Contexto, Entrada, Fim, Job, Msg, Natureza};

type PreparoExecucao = (i64, HashMap<u64, InfoVolume>, Vec<Escopo>);
type CallbackProgresso = Arc<dyn Fn(&Progresso) + Send + Sync>;

/// Como o scan terminou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesfechoScan {
    Concluida,
    /// Terminou, mas houve problemas de acesso (agregados afetados ficam `parcial`).
    ConcluidaComErros,
    /// Parou a pedido; a fila persiste e `retomar` continua de onde parou.
    Cancelada,
}

/// Resultado de `executar`/`retomar`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultadoScan {
    pub execucao_id: i64,
    pub desfecho: DesfechoScan,
    /// Contadores exatos, recalculados do Catálogo.
    pub contadores: ContadoresDb,
    /// `None` se cancelado antes da agregação.
    pub agregacao: Option<ResumoAgregacao>,
    pub duracao: Duration,
    /// Só no Fast Scan: o que foi reaproveitado, atualizado, renomeado e removido.
    pub reconciliacao: Option<ResumoFast>,
}

/// Full Scan (fase de inventário). Construa, ajuste e chame [`executar`](Self::executar).
pub struct FullScan {
    raizes: Vec<PathBuf>,
    volumes: Vec<VolumeDescoberto>,
    config: ConfigScan,
    controle: ControleScan,
    progresso: Option<CallbackProgresso>,
    classificador: Arc<dyn Classificador>,
    leitor_elevado: Option<Arc<dyn LeitorElevado>>,
    rapido: bool,
    max_eventos: u64,
}

impl fmt::Debug for FullScan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FullScan")
            .field("raizes", &self.raizes)
            .field("volumes", &self.volumes.len())
            .field("config", &self.config)
            .field("controle", &self.controle)
            .finish_non_exhaustive()
    }
}

fn agora() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

#[derive(Debug)]
struct InfoVolume {
    /// `None` em pseudo-filesystems (não são registrados no Catálogo).
    id: Option<i64>,
    pseudo: bool,
}

#[derive(Debug, Default, Clone, Copy)]
struct Vivo {
    fast: Contagem,
    pastas: u64,
    arquivos: u64,
    especiais: u64,
    bytes: u64,
    ignorados: u64,
    erros: u64,
}

#[derive(Debug)]
struct Raiz {
    caminho: Vec<u8>,
    dev: u64,
    inode: u64,
    attrs: String,
    mtime_ns: Option<i64>,
}

struct ProblemaRaiz {
    caminho: Vec<u8>,
    codigo: &'static str,
    motivo: String,
    severidade: Severidade,
}

impl FullScan {
    /// `raizes` são pastas (ou pontos de montagem) escolhidos; `volumes` vêm da descoberta de
    /// plataforma. Uma raiz é resolvida com `canonicalize` (a escolha do usuário é explícita);
    /// dali em diante nenhum symlink é seguido.
    #[must_use]
    pub fn novo(raizes: Vec<PathBuf>, volumes: Vec<VolumeDescoberto>) -> Self {
        Self {
            raizes,
            volumes,
            config: ConfigScan::default(),
            controle: ControleScan::novo(),
            progresso: None,
            classificador: Arc::new(SemClassificacao),
            leitor_elevado: None,
            rapido: false,
            max_eventos: fast::MAX_EVENTOS_PADRAO,
        }
    }

    /// Troca o modo para Fast Scan (ver [`crate::FastScan`]).
    pub(crate) fn rapido(mut self, max_eventos: u64) -> Self {
        self.rapido = true;
        self.max_eventos = max_eventos;
        self
    }

    #[must_use]
    pub fn com_config(mut self, config: ConfigScan) -> Self {
        self.config = config;
        self
    }

    #[must_use]
    pub fn com_controle(mut self, controle: ControleScan) -> Self {
        self.controle = controle;
        self
    }

    /// Callback chamado pelo coordenador após lotes (no máximo a cada `intervalo_de_progresso`).
    #[must_use]
    pub fn com_progresso(mut self, f: impl Fn(&Progresso) + Send + Sync + 'static) -> Self {
        self.progresso = Some(Arc::new(f));
        self
    }

    #[must_use]
    pub fn com_classificador(mut self, c: Arc<dyn Classificador>) -> Self {
        self.classificador = c;
        self
    }

    /// Leitor de metadados com privilégio para pastas que negam acesso (opt-in, `--elevar`).
    #[must_use]
    pub fn com_leitor_elevado(mut self, l: Arc<dyn LeitorElevado>) -> Self {
        self.leitor_elevado = Some(l);
        self
    }

    /// Alça para pausar/cancelar de outra thread.
    #[must_use]
    pub fn controle(&self) -> ControleScan {
        self.controle.clone()
    }

    /// Inicia uma nova execução.
    ///
    /// # Errors
    /// [`ErroScan::SemRaizes`] se nenhuma raiz serve; falhas do Catálogo.
    pub fn executar(&self, catalogo: &mut Catalogo) -> Result<ResultadoScan, ErroScan> {
        let inicio = Instant::now();
        let (validas, problemas) = self.resolver_raizes();
        let raizes_json = serde_json::to_string(
            &self
                .raizes
                .iter()
                .map(|r| inventario::exibir(r.as_os_str().as_bytes()))
                .collect::<Vec<_>>(),
        )
        .map_err(|e| ErroScan::Interno(e.to_string()))?;
        let checkpoint = serde_json::json!({
            "config": serde_json::to_value(&self.config).map_err(|e| ErroScan::Interno(e.to_string()))?,
        })
        .to_string();
        let t = agora();
        let modo = if self.rapido { "fast" } else { "full" };
        let (run_id, mapa, escopos) = catalogo.armazem_mut().transacao_imediata(
            |tx| -> Result<PreparoExecucao, ErroScan> {
                let mapa = self.registrar_volumes(tx, t)?;
                let run_id =
                    inventario::criar_execucao(tx, modo, &raizes_json, Some(&checkpoint), t)?;
                let mut escopos = Vec::new();
                let mut escopos_json = Vec::new();
                for p in &problemas {
                    inventario::registrar_problema(
                        tx,
                        &NovoProblema {
                            run_id,
                            pasta_id: None,
                            path_raw: Some(&p.caminho),
                            fase: "inventario",
                            codigo: p.codigo,
                            motivo: Some(&p.motivo),
                            severidade: p.severidade,
                            observado_em: t,
                        },
                    )?;
                }
                for r in &validas {
                    let nome = nome_da_raiz(&r.caminho);
                    let volume_id = mapa.get(&r.dev).and_then(|v| v.id).ok_or_else(|| {
                        ErroScan::Interno("raiz validada sem volume registrado".into())
                    })?;
                    let nova = NovaPasta {
                        volume_id,
                        pai: None,
                        path_raw: &r.caminho,
                        name_raw: nome,
                        dev: r.dev,
                        inode: r.inode,
                        attrs: &r.attrs,
                        mtime_ns: r.mtime_ns,
                        run_id,
                    };
                    // No Fast Scan a raiz já catalogada mantém o pai (raiz aninhada em outra).
                    let id = if self.rapido {
                        reconciliacao::registrar_raiz(tx, &nova)?
                    } else {
                        inventario::upsert_pasta(tx, &nova)?
                    };
                    inventario::enfileirar_diretorio(tx, run_id, id, &r.caminho, t)?;
                    escopos.push(Escopo {
                        volume_id,
                        path_raw: r.caminho.clone(),
                    });
                    escopos_json.push(serde_json::json!({"volume_id": volume_id, "pasta_id": id}));
                }
                if self.rapido {
                    let cp = serde_json::json!({
                        "config": serde_json::to_value(&self.config)
                            .map_err(|e| ErroScan::Interno(e.to_string()))?,
                        "escopos": escopos_json,
                    })
                    .to_string();
                    tx.execute(
                        "UPDATE scan_runs SET checkpoint = ?2 WHERE id = ?1",
                        rusqlite::params![run_id, cp],
                    )?;
                }
                Ok((run_id, mapa, escopos))
            },
        )?;
        if validas.is_empty() {
            catalogo
                .armazem_mut()
                .transacao_imediata(|tx| -> Result<(), ErroScan> {
                    inventario::definir_estado(tx, run_id, EstadoScan::Falhou, None, agora())?;
                    Ok(())
                })?;
            return Err(ErroScan::SemRaizes);
        }
        let devs_raiz = validas.iter().map(|r| r.dev).collect();
        self.rodar(
            catalogo,
            run_id,
            mapa,
            devs_raiz,
            escopos,
            self.rapido,
            inicio,
        )
    }

    /// Retoma uma execução cancelada, pausada, interrompida (processo morto) ou que falhou, a
    /// partir de `scan_queue`, sem duplicar linhas. Use um [`ControleScan`] em `Executando`.
    ///
    /// # Errors
    /// [`ErroScan::ExecucaoInexistente`], [`ErroScan::NaoRetomavel`] ou falhas do Catálogo.
    pub fn retomar(
        &self,
        catalogo: &mut Catalogo,
        execucao_id: i64,
    ) -> Result<ResultadoScan, ErroScan> {
        let inicio = Instant::now();
        let t = agora();
        let exec = inventario::obter_execucao(catalogo.armazem().conexao(), execucao_id)?
            .ok_or(ErroScan::ExecucaoInexistente(execucao_id))?;
        // Fase `hash`: o inventário já terminou (estado `concluida`); o que falta é a etapa de
        // hash, que o chamador reexecuta (a fila persistente é reconstruída a partir dos hashes
        // já gravados, então nada é relido).
        // A etapa de hash deixa a execução `em_andamento` (e, depois de uma queda ou de um
        // encerramento, `interrompida`) na fase `hash`: o inventário também já terminou.
        let inventario_pronto = matches!(exec.fase, FaseScan::Concluida | FaseScan::Hash)
            && matches!(
                exec.estado,
                EstadoScan::Concluida
                    | EstadoScan::ConcluidaComErros
                    | EstadoScan::EmAndamento
                    | EstadoScan::Pausada
                    | EstadoScan::Interrompida
            );
        if exec.fase == FaseScan::Concluida || inventario_pronto || !exec.estado.retomavel() {
            if exec.fase == FaseScan::Concluida || inventario_pronto {
                let c =
                    inventario::contadores_da_execucao(catalogo.armazem().conexao(), execucao_id)?;
                let desfecho = if exec.estado == EstadoScan::ConcluidaComErros || c.erros > 0 {
                    DesfechoScan::ConcluidaComErros
                } else {
                    DesfechoScan::Concluida
                };
                return Ok(ResultadoScan {
                    execucao_id,
                    desfecho,
                    contadores: c,
                    agregacao: None,
                    duracao: inicio.elapsed(),
                    reconciliacao: None,
                });
            }
            return Err(ErroScan::NaoRetomavel {
                id: execucao_id,
                estado: exec.estado.como_str().to_string(),
            });
        }
        let (mapa, devs_raiz) = catalogo.armazem_mut().transacao_imediata(
            |tx| -> Result<(HashMap<u64, InfoVolume>, HashSet<u64>), ErroScan> {
                let mapa = self.registrar_volumes(tx, t)?;
                inventario::liberar_reservas(tx, execucao_id)?;
                inventario::definir_estado(tx, execucao_id, EstadoScan::EmAndamento, None, t)?;
                let mut st = tx.prepare(
                    "SELECT DISTINCT native_dev FROM folders WHERE seen_run_id = ?1 AND parent_folder_id IS NULL",
                )?;
                let devs = st
                    .query_map([execucao_id], |l| Ok(l.get::<_, i64>(0)? as u64))?
                    .collect::<Result<HashSet<_>, _>>()?;
                Ok((mapa, devs))
            },
        )?;
        let rapido = exec.modo == "fast";
        let escopos = if rapido {
            fast::escopos_do_checkpoint(catalogo.armazem().conexao(), exec.checkpoint.as_deref())?
        } else {
            Vec::new()
        };
        self.rodar(
            catalogo,
            execucao_id,
            mapa,
            devs_raiz,
            escopos,
            rapido,
            inicio,
        )
    }

    fn registrar_volumes(
        &self,
        tx: &Connection,
        t: i64,
    ) -> Result<HashMap<u64, InfoVolume>, ErroScan> {
        // Registra os conectados e marca como desconectados (sem tocar nas linhas) os que sumiram.
        let sincronia = reconciliacao::sincronizar_volumes(tx, &self.volumes, t)?;
        let mut mapa = HashMap::new();
        for v in &self.volumes {
            let id = if v.pseudo {
                None
            } else {
                sincronia.conectados.get(&v.st_dev).copied()
            };
            mapa.insert(
                v.st_dev,
                InfoVolume {
                    id,
                    pseudo: v.pseudo,
                },
            );
        }
        Ok(mapa)
    }

    /// Valida as raízes (fora de transação: só lê o filesystem).
    fn resolver_raizes(&self) -> (Vec<Raiz>, Vec<ProblemaRaiz>) {
        let mut validas: Vec<Raiz> = Vec::new();
        let mut problemas = Vec::new();
        let mut candidatas: Vec<(Vec<u8>, fs_meta::Meta)> = Vec::new();
        for r in &self.raizes {
            let bruto = r.as_os_str().as_bytes().to_vec();
            let canon = match std::fs::canonicalize(r) {
                Ok(c) => c.as_os_str().as_bytes().to_vec(),
                Err(e) => {
                    let (codigo, _) = visita::classificar_io(&e);
                    problemas.push(ProblemaRaiz {
                        caminho: bruto,
                        codigo: if codigo == "erro_es" {
                            "raiz_inacessivel"
                        } else {
                            codigo
                        },
                        motivo: format!("raiz inacessível: {e}"),
                        severidade: Severidade::Impeditivo,
                    });
                    continue;
                }
            };
            match visita::lstat(&canon) {
                Ok(m) if m.is_dir() => candidatas.push((canon, fs_meta::Meta::de(&m))),
                Ok(_) => problemas.push(ProblemaRaiz {
                    caminho: canon,
                    codigo: "raiz_nao_e_diretorio",
                    motivo: "a raiz escolhida não é um diretório".into(),
                    severidade: Severidade::Impeditivo,
                }),
                Err(e) => problemas.push(ProblemaRaiz {
                    caminho: canon,
                    codigo: "raiz_inacessivel",
                    motivo: format!("raiz inacessível: {e}"),
                    severidade: Severidade::Impeditivo,
                }),
            }
        }
        candidatas.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));
        for (caminho, meta) in candidatas {
            let Some(info) = self.volumes.iter().find(|v| v.st_dev == meta.dev) else {
                problemas.push(ProblemaRaiz {
                    caminho,
                    codigo: "raiz_sem_volume",
                    motivo: format!("nenhum volume descoberto com st_dev {}", meta.dev),
                    severidade: Severidade::Impeditivo,
                });
                continue;
            };
            if info.pseudo {
                problemas.push(ProblemaRaiz {
                    caminho,
                    codigo: "raiz_pseudo_filesystem",
                    motivo: format!("{} é um pseudo-filesystem e não é varrido", info.fs_type),
                    severidade: Severidade::Impeditivo,
                });
                continue;
            }
            if validas
                .iter()
                .any(|v| v.dev == meta.dev && dentro_de(&caminho, &v.caminho))
            {
                problemas.push(ProblemaRaiz {
                    caminho,
                    codigo: "raiz_redundante",
                    motivo: "já está dentro de outra raiz escolhida".into(),
                    severidade: Severidade::Info,
                });
                continue;
            }
            validas.push(Raiz {
                caminho,
                dev: meta.dev,
                inode: meta.inode,
                attrs: meta.attrs,
                mtime_ns: meta.mtime_ns,
            });
        }
        (validas, problemas)
    }

    #[allow(clippy::too_many_arguments)]
    fn rodar(
        &self,
        catalogo: &mut Catalogo,
        run_id: i64,
        mapa: HashMap<u64, InfoVolume>,
        devs_raiz: HashSet<u64>,
        escopos: Vec<Escopo>,
        rapido: bool,
        inicio: Instant,
    ) -> Result<ResultadoScan, ErroScan> {
        let vivo = {
            let c = inventario::contadores_da_execucao(catalogo.armazem().conexao(), run_id)?;
            let fast = if rapido {
                fast::contagem_salva(catalogo.armazem().conexao(), run_id)?
            } else {
                Contagem::default()
            };
            Vivo {
                pastas: c.pastas,
                arquivos: c.arquivos,
                especiais: c.especiais,
                bytes: c.bytes_logicos,
                ignorados: c.ignorados,
                erros: c.erros,
                fast,
            }
        };
        let mut ctx = Ctx {
            rapido,
            max_eventos: self.max_eventos,
            run_id,
            mapa,
            devs_raiz,
            vivo,
            tipos: HashMap::new(),
            inicio,
            ultimo_progresso: None,
        };
        let fim = match self.laco(catalogo, &mut ctx) {
            Ok(f) => f,
            Err(e) => {
                let _ = catalogo
                    .armazem_mut()
                    .transacao_imediata(|tx| -> Result<(), ErroScan> {
                        inventario::liberar_reservas(tx, run_id)?;
                        inventario::definir_estado(tx, run_id, EstadoScan::Falhou, None, agora())?;
                        Ok(())
                    });
                return Err(e);
            }
        };
        if fim == FimLaco::Cancelado {
            let contadores = catalogo.armazem_mut().transacao_imediata(
                |tx| -> Result<ContadoresDb, ErroScan> {
                    inventario::liberar_reservas(tx, run_id)?;
                    let c = inventario::contadores_da_execucao(tx, run_id)?;
                    let mut json = serde_json::from_str(&c.json()).unwrap_or_default();
                    if rapido {
                        fast::acrescentar_contagem(&mut json, &ctx.vivo.fast);
                    }
                    inventario::salvar_contadores(tx, run_id, &json.to_string())?;
                    if self.controle.encerrando() {
                        // Encerramento do programa, não decisão do usuário: continua sozinha.
                        inventario::definir_estado(
                            tx,
                            run_id,
                            EstadoScan::Interrompida,
                            None,
                            agora(),
                        )?;
                        tx.execute(
                            "UPDATE scan_runs SET retomada_automatica = 1 WHERE id = ?1",
                            [run_id],
                        )?;
                    } else {
                        inventario::definir_estado(
                            tx,
                            run_id,
                            EstadoScan::Cancelada,
                            None,
                            agora(),
                        )?;
                    }
                    Ok(c)
                },
            )?;
            self.emitir(&mut ctx, FaseProgresso::Concluido, 0, true);
            return Ok(ResultadoScan {
                execucao_id: run_id,
                desfecho: DesfechoScan::Cancelada,
                contadores,
                agregacao: None,
                duracao: inicio.elapsed(),
                reconciliacao: None,
            });
        }
        // Fast Scan: com o escopo inteiro revisitado, só agora é possível decidir o que foi
        // renomeado e o que sumiu. Cancelado/interrompido nunca chega aqui (nada vira ausente).
        let reconciliacao = if rapido {
            Some(fast::finalizar(
                catalogo,
                run_id,
                &escopos,
                ctx.max_eventos,
                &ctx.vivo.fast,
            )?)
        } else {
            None
        };
        catalogo
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroScan> {
                inventario::definir_estado(
                    tx,
                    run_id,
                    EstadoScan::EmAndamento,
                    Some(FaseScan::Estatisticas),
                    agora(),
                )?;
                Ok(())
            })?;
        self.emitir(&mut ctx, FaseProgresso::Agregacao, 0, true);
        let agregacao =
            match agregar_execucao(catalogo.armazem_mut().conexao_mut(), run_id, agora()) {
                Ok(a) => a,
                Err(e) => {
                    let _ =
                        catalogo
                            .armazem_mut()
                            .transacao_imediata(|tx| -> Result<(), ErroScan> {
                                inventario::definir_estado(
                                    tx,
                                    run_id,
                                    EstadoScan::Falhou,
                                    None,
                                    agora(),
                                )?;
                                Ok(())
                            });
                    return Err(e.into());
                }
            };
        let (contadores, estado) = catalogo.armazem_mut().transacao_imediata(
            |tx| -> Result<(ContadoresDb, EstadoScan), ErroScan> {
                let c = inventario::contadores_da_execucao(tx, run_id)?;
                let json = match &reconciliacao {
                    Some(r) => fast::json_final(&c, r),
                    None => c.json(),
                };
                inventario::salvar_contadores(tx, run_id, &json)?;
                let estado = if c.erros > 0 {
                    EstadoScan::ConcluidaComErros
                } else {
                    EstadoScan::Concluida
                };
                inventario::definir_estado(tx, run_id, estado, Some(FaseScan::Concluida), agora())?;
                Ok((c, estado))
            },
        )?;
        self.emitir(&mut ctx, FaseProgresso::Concluido, 0, true);
        Ok(ResultadoScan {
            execucao_id: run_id,
            desfecho: if estado == EstadoScan::ConcluidaComErros {
                DesfechoScan::ConcluidaComErros
            } else {
                DesfechoScan::Concluida
            },
            contadores,
            agregacao: Some(agregacao),
            duracao: inicio.elapsed(),
            reconciliacao,
        })
    }

    fn emitir(&self, ctx: &mut Ctx, fase: FaseProgresso, pendentes: u64, forcar: bool) {
        let Some(cb) = &self.progresso else { return };
        if !forcar
            && ctx
                .ultimo_progresso
                .is_some_and(|t| t.elapsed() < self.config.intervalo_de_progresso)
        {
            return;
        }
        ctx.ultimo_progresso = Some(Instant::now());
        let decorrido = ctx.inicio.elapsed();
        let total = ctx.vivo.pastas + ctx.vivo.arquivos + ctx.vivo.especiais;
        cb(&Progresso {
            execucao_id: ctx.run_id,
            fase,
            pastas: ctx.vivo.pastas,
            arquivos: ctx.vivo.arquivos,
            especiais: ctx.vivo.especiais,
            bytes_logicos: ctx.vivo.bytes,
            ignorados: ctx.vivo.ignorados,
            erros: ctx.vivo.erros,
            diretorios_pendentes: pendentes,
            decorrido,
            entradas_por_segundo: total as f64 / decorrido.as_secs_f64().max(1e-3),
        });
    }

    fn laco(&self, catalogo: &mut Catalogo, ctx: &mut Ctx) -> Result<FimLaco, ErroScan> {
        let cfg = &self.config;
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let job_rx = Mutex::new(job_rx);
        let (res_tx, res_rx) = mpsc::sync_channel::<Msg>(cfg.fila_de_resultados.max(1));
        let visita_ctx = Contexto {
            exclusoes: &cfg.exclusoes,
            classificador: self.classificador.as_ref(),
            controle: &self.controle,
            entradas_por_mensagem: cfg.entradas_por_mensagem.max(1),
            elevado: self.leitor_elevado.as_deref(),
        };
        thread::scope(|s| {
            for _ in 0..cfg.concorrencia.max(1) {
                let res_tx = res_tx.clone();
                let job_rx = &job_rx;
                let visita_ctx = &visita_ctx;
                s.spawn(move || {
                    loop {
                        let job = {
                            let Ok(rx) = job_rx.lock() else { return };
                            match rx.recv() {
                                Ok(j) => j,
                                Err(_) => return,
                            }
                        };
                        if !visita::visitar(&job, visita_ctx, &res_tx) {
                            return;
                        }
                    }
                });
            }
            drop(res_tx);
            let mut coord = Coordenador {
                scan: self,
                ctx,
                cat: catalogo,
                job_tx,
                res_rx,
                prontos: VecDeque::new(),
                em_voo: HashMap::new(),
                voando: 0,
                pendentes: Vec::new(),
                linhas_pendentes: 0,
                ultimo_flush: Instant::now(),
                fonte_vazia: false,
            };
            coord.executar()
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FimLaco {
    Concluido,
    Cancelado,
}

mod fs_meta {
    use std::os::unix::fs::MetadataExt;

    pub(super) struct Meta {
        pub dev: u64,
        pub inode: u64,
        pub attrs: String,
        pub mtime_ns: Option<i64>,
    }

    impl Meta {
        pub(super) fn de(m: &std::fs::Metadata) -> Self {
            let e = crate::visita::entrada_de(Vec::new(), 0, m);
            Self {
                dev: m.dev(),
                inode: m.ino(),
                attrs: e.attrs,
                mtime_ns: e.mtime_ns,
            }
        }
    }
}

fn dentro_de(caminho: &[u8], base: &[u8]) -> bool {
    caminho == base
        || (caminho.starts_with(base)
            && (base.last() == Some(&b'/') || caminho.get(base.len()) == Some(&b'/')))
}

fn nome_da_raiz(caminho: &[u8]) -> &[u8] {
    if caminho == b"/" {
        return caminho;
    }
    Path::new(OsStr::from_bytes(caminho))
        .file_name()
        .map_or(caminho, OsStr::as_bytes)
}

struct Ctx {
    rapido: bool,
    max_eventos: u64,
    run_id: i64,
    mapa: HashMap<u64, InfoVolume>,
    devs_raiz: HashSet<u64>,
    vivo: Vivo,
    tipos: HashMap<(Option<String>, Option<String>, &'static str), i64>,
    inicio: Instant,
    ultimo_progresso: Option<Instant>,
}

struct Coordenador<'a> {
    scan: &'a FullScan,
    ctx: &'a mut Ctx,
    cat: &'a mut Catalogo,
    job_tx: Sender<Job>,
    res_rx: Receiver<Msg>,
    prontos: VecDeque<DiretorioPendente>,
    em_voo: HashMap<i64, DiretorioPendente>,
    voando: usize,
    pendentes: Vec<Msg>,
    linhas_pendentes: usize,
    ultimo_flush: Instant,
    fonte_vazia: bool,
}

impl Coordenador<'_> {
    fn executar(&mut self) -> Result<FimLaco, ErroScan> {
        let cfg = &self.scan.config;
        let max_voo = cfg.concorrencia.max(1) * 2;
        loop {
            match self.scan.controle.estado() {
                EstadoControle::Cancelado => {
                    self.drenar()?;
                    self.descarregar()?;
                    return Ok(FimLaco::Cancelado);
                }
                EstadoControle::Pausado => {
                    self.pausar()?;
                    continue;
                }
                EstadoControle::Executando => {}
            }
            // Despacha diretórios da fronteira para os workers.
            while self.voando < max_voo {
                if self.prontos.is_empty() {
                    if self.fonte_vazia {
                        break;
                    }
                    let run_id = self.ctx.run_id;
                    let lote = cfg.reservas_por_vez.max(1);
                    let novos = self.cat.armazem_mut().transacao_imediata(
                        |tx| -> Result<Vec<DiretorioPendente>, ErroScan> {
                            Ok(inventario::reservar_diretorios(tx, run_id, lote)?)
                        },
                    )?;
                    if novos.is_empty() {
                        self.fonte_vazia = true;
                        break;
                    }
                    self.prontos.extend(novos);
                }
                let Some(d) = self.prontos.pop_front() else {
                    break;
                };
                self.job_tx
                    .send(Job {
                        fila_id: d.fila_id,
                        caminho: d.path_raw.clone(),
                    })
                    .map_err(|_| ErroScan::Interno("workers encerraram antes da hora".into()))?;
                self.em_voo.insert(d.fila_id, d);
                self.voando += 1;
            }
            if self.voando == 0 && self.pendentes.is_empty() && self.prontos.is_empty() {
                if self.fonte_vazia {
                    return Ok(FimLaco::Concluido);
                }
                continue;
            }
            if self.voando == 0 {
                self.descarregar()?;
                continue;
            }
            match self.res_rx.recv_timeout(Duration::from_millis(25)) {
                Ok(m) => self.aceitar(m),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(ErroScan::Interno("canal de resultados fechado".into()));
                }
            }
            while self.linhas_pendentes < cfg.linhas_por_transacao * 2 {
                match self.res_rx.try_recv() {
                    Ok(m) => self.aceitar(m),
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
            if self.linhas_pendentes >= cfg.linhas_por_transacao
                || (!self.pendentes.is_empty()
                    && self.ultimo_flush.elapsed() >= Duration::from_millis(150))
            {
                self.descarregar()?;
            }
        }
    }

    fn aceitar(&mut self, m: Msg) {
        self.linhas_pendentes += m.linhas();
        if matches!(m, Msg::Fim { .. }) {
            self.voando -= 1;
        }
        self.pendentes.push(m);
    }

    /// Espera os diretórios em andamento terminarem (ao pausar/cancelar).
    fn drenar(&mut self) -> Result<(), ErroScan> {
        while self.voando > 0 {
            match self.res_rx.recv_timeout(Duration::from_millis(25)) {
                Ok(m) => {
                    self.aceitar(m);
                    if self.linhas_pendentes >= self.scan.config.linhas_por_transacao * 4 {
                        self.descarregar()?;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(ErroScan::Interno("canal de resultados fechado".into()));
                }
            }
        }
        Ok(())
    }

    fn pausar(&mut self) -> Result<(), ErroScan> {
        self.drenar()?;
        self.descarregar()?;
        let run_id = self.ctx.run_id;
        self.cat
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroScan> {
                inventario::liberar_reservas(tx, run_id)?;
                inventario::definir_estado(tx, run_id, EstadoScan::Pausada, None, agora())?;
                Ok(())
            })?;
        self.prontos.clear();
        self.fonte_vazia = false;
        self.scan.emitir(self.ctx, FaseProgresso::Pausado, 0, true);
        while self.scan.controle.estado() == EstadoControle::Pausado {
            thread::sleep(Duration::from_millis(20));
        }
        if self.scan.controle.estado() == EstadoControle::Executando {
            self.cat
                .armazem_mut()
                .transacao_imediata(|tx| -> Result<(), ErroScan> {
                    inventario::definir_estado(tx, run_id, EstadoScan::EmAndamento, None, agora())?;
                    Ok(())
                })?;
        }
        Ok(())
    }

    /// Grava as mensagens pendentes numa transação curta.
    fn descarregar(&mut self) -> Result<(), ErroScan> {
        if self.pendentes.is_empty() {
            return Ok(());
        }
        let msgs = std::mem::take(&mut self.pendentes);
        self.linhas_pendentes = 0;
        let run_id = self.ctx.run_id;
        let ctx = &mut *self.ctx;
        let em_voo = &self.em_voo;
        let terminados =
            self.cat
                .armazem_mut()
                .transacao_imediata(|tx| -> Result<Vec<i64>, ErroScan> {
                    aplicar(tx, ctx, em_voo, &msgs)
                })?;
        for id in terminados {
            self.em_voo.remove(&id);
        }
        self.ultimo_flush = Instant::now();
        self.fonte_vazia = false;
        let pendentes = self.prontos.len() as u64 + self.voando as u64;
        let c = self.ctx.vivo;
        let mut json = serde_json::json!({
            "pastas": c.pastas, "arquivos": c.arquivos, "especiais": c.especiais,
            "bytes_logicos": c.bytes, "ignorados": c.ignorados, "erros": c.erros,
        });
        if self.ctx.rapido {
            fast::acrescentar_contagem(&mut json, &c.fast);
        }
        let json = json.to_string();
        self.cat
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroScan> {
                inventario::salvar_contadores(tx, run_id, &json)?;
                Ok(())
            })?;
        self.scan
            .emitir(self.ctx, FaseProgresso::Inventario, pendentes, false);
        Ok(())
    }
}

fn aplicar(
    tx: &Connection,
    ctx: &mut Ctx,
    em_voo: &HashMap<i64, DiretorioPendente>,
    msgs: &[Msg],
) -> Result<Vec<i64>, ErroScan> {
    let t = agora();
    let mut terminados = Vec::new();
    for m in msgs {
        match m {
            Msg::Lote {
                fila_id,
                entradas,
                ignoradas,
                problemas,
            } => {
                let dir = em_voo.get(fila_id).ok_or_else(|| {
                    ErroScan::Interno(format!("lote de diretório desconhecido {fila_id}"))
                })?;
                for e in entradas {
                    aplicar_entrada(tx, ctx, dir, e, t)?;
                }
                for ig in ignoradas {
                    inventario::registrar_problema(
                        tx,
                        &NovoProblema {
                            run_id: ctx.run_id,
                            pasta_id: Some(dir.pasta_id),
                            path_raw: Some(&ig.caminho),
                            fase: "inventario",
                            codigo: "excluido",
                            motivo: Some(&ig.motivo),
                            severidade: Severidade::Info,
                            observado_em: t,
                        },
                    )?;
                    ctx.vivo.ignorados += 1;
                }
                for p in problemas {
                    registrar(tx, ctx, Some(dir.pasta_id), p, t)?;
                }
            }
            Msg::Fim { fila_id, fim } => {
                let dir = em_voo.get(fila_id).ok_or_else(|| {
                    ErroScan::Interno(format!("fim de diretório desconhecido {fila_id}"))
                })?;
                let resultado = match fim {
                    Fim::Ok => FimDiretorio::Concluido,
                    Fim::Abortado => FimDiretorio::Liberar,
                    Fim::Falha(p) => {
                        registrar(tx, ctx, Some(dir.pasta_id), p, t)?;
                        FimDiretorio::Falhou(format!("{}: {}", p.codigo, p.motivo))
                    }
                };
                inventario::finalizar_diretorio(tx, *fila_id, &resultado)?;
                terminados.push(*fila_id);
            }
        }
    }
    Ok(terminados)
}

fn registrar(
    tx: &Connection,
    ctx: &mut Ctx,
    pasta_id: Option<i64>,
    p: &visita::Problema,
    t: i64,
) -> Result<(), ErroScan> {
    inventario::registrar_problema(
        tx,
        &NovoProblema {
            run_id: ctx.run_id,
            pasta_id,
            path_raw: Some(&p.caminho),
            fase: "inventario",
            codigo: p.codigo,
            motivo: Some(&p.motivo),
            severidade: p.severidade,
            observado_em: t,
        },
    )?;
    if p.severidade == Severidade::Info {
        ctx.vivo.ignorados += 1;
    } else {
        ctx.vivo.erros += 1;
    }
    Ok(())
}

fn aplicar_entrada(
    tx: &Connection,
    ctx: &mut Ctx,
    dir: &DiretorioPendente,
    e: &Entrada,
    t: i64,
) -> Result<(), ErroScan> {
    match e.natureza {
        Natureza::Pasta => aplicar_pasta(tx, ctx, dir, e, t),
        Natureza::Outra(tipo) => {
            let mime_id = match &e.tipo {
                Some(td) => {
                    let chave = (td.extensao.clone(), td.mime.clone(), td.fonte.como_str());
                    if let Some(id) = ctx.tipos.get(&chave) {
                        Some(*id)
                    } else {
                        let id = inventario::obter_tipo_de_arquivo(
                            tx,
                            td.extensao.as_deref(),
                            td.mime.as_deref(),
                            td.descricao.as_deref(),
                            td.categoria.as_deref(),
                            td.fonte.como_str(),
                            td.confianca,
                        )?;
                        if ctx.tipos.len() > 10_000 {
                            ctx.tipos.clear();
                        }
                        ctx.tipos.insert(chave, id);
                        Some(id)
                    }
                }
                None => None,
            };
            let novo = NovoArquivo {
                volume_id: dir.volume_id,
                pasta_id: dir.pasta_id,
                path_raw: &e.caminho,
                name_raw: e.nome(),
                tipo,
                dev: e.dev,
                inode: e.inode,
                nlink: e.nlink,
                bytes_logicos: e.tamanho,
                bytes_alocados: e.alocado,
                mtime_ns: e.mtime_ns,
                ctime_ns: e.ctime_ns,
                birthtime_ns: e.birth_ns,
                attrs: &e.attrs,
                alvo_symlink: e.alvo.as_deref(),
                mime_id,
                run_id: ctx.run_id,
            };
            if ctx.rapido {
                match reconciliacao::gravar_arquivo(tx, &novo)? {
                    Efeito::Inalterado => ctx.vivo.fast.inalterados += 1,
                    Efeito::Novo => ctx.vivo.fast.novos += 1,
                    Efeito::Alterado => {
                        ctx.vivo.fast.alterados += 1;
                        if ctx.vivo.fast.eventos < ctx.max_eventos {
                            ctx.vivo.fast.eventos += 1;
                            reconciliacao::registrar_evento(
                                tx,
                                &NovoEvento {
                                    volume_id: Some(dir.volume_id),
                                    caminho: Some(&e.caminho),
                                    execucao_id: Some(ctx.run_id),
                                    ..NovoEvento::processado(TipoEvento::Alterado, "fast_scan", t)
                                },
                            )?;
                        } else {
                            ctx.vivo.fast.omitidos += 1;
                        }
                    }
                }
            } else {
                inventario::upsert_arquivo(tx, &novo)?;
            }
            if tipo == TipoEntrada::Arquivo {
                ctx.vivo.arquivos += 1;
            } else {
                ctx.vivo.especiais += 1;
            }
            ctx.vivo.bytes += e.tamanho;
            Ok(())
        }
    }
}

fn aplicar_pasta(
    tx: &Connection,
    ctx: &mut Ctx,
    dir: &DiretorioPendente,
    e: &Entrada,
    t: i64,
) -> Result<(), ErroScan> {
    let ignorar = |tx: &Connection, ctx: &mut Ctx, codigo: &'static str, motivo: String| {
        inventario::registrar_problema(
            tx,
            &NovoProblema {
                run_id: ctx.run_id,
                pasta_id: Some(dir.pasta_id),
                path_raw: Some(&e.caminho),
                fase: "inventario",
                codigo,
                motivo: Some(&motivo),
                severidade: Severidade::Info,
                observado_em: t,
            },
        )?;
        ctx.vivo.ignorados += 1;
        Ok::<(), ErroScan>(())
    };
    if e.dev != dir.dev {
        // Fronteira de volume: a pasta de montagem existe no volume pai, mas o conteúdo
        // pertence a outro filesystem e não é varrido por este caminho.
        let (codigo, motivo) = match ctx.mapa.get(&e.dev) {
            Some(v) if v.pseudo => (
                "volume_pseudo",
                "pseudo-filesystem (proc, sysfs, devtmpfs...) não é varrido".to_string(),
            ),
            Some(_) if ctx.devs_raiz.contains(&e.dev) => (
                "limite_de_volume",
                "outro volume, selecionado como raiz: varrido à parte, não por este caminho"
                    .to_string(),
            ),
            Some(_) => (
                "limite_de_volume",
                "outro volume não selecionado como raiz: fronteira de montagem não cruzada"
                    .to_string(),
            ),
            None => (
                "volume_desconhecido",
                format!(
                    "st_dev {} não corresponde a nenhum volume descoberto",
                    e.dev
                ),
            ),
        };
        let base = e.attrs.trim_end_matches('}');
        let attrs = format!("{base},\"limite_de_volume\":true}}");
        gravar_pasta(
            tx,
            ctx,
            &NovaPasta {
                volume_id: dir.volume_id,
                pai: Some(dir.pasta_id),
                path_raw: &e.caminho,
                name_raw: e.nome(),
                dev: e.dev,
                inode: e.inode,
                attrs: &attrs,
                mtime_ns: e.mtime_ns,
                run_id: ctx.run_id,
            },
        )?;
        ctx.vivo.pastas += 1;
        return ignorar(tx, ctx, codigo, motivo);
    }
    if inventario::pasta_ja_vista_por_outro_caminho(
        tx,
        dir.volume_id,
        e.dev,
        e.inode,
        ctx.run_id,
        &e.caminho,
    )? {
        return ignorar(
            tx,
            ctx,
            "diretorio_duplicado",
            "mesmo (dev, inode) já visto em outro caminho (bind mount ou alias): não varrido de novo"
                .to_string(),
        );
    }
    let id = gravar_pasta(
        tx,
        ctx,
        &NovaPasta {
            volume_id: dir.volume_id,
            pai: Some(dir.pasta_id),
            path_raw: &e.caminho,
            name_raw: e.nome(),
            dev: e.dev,
            inode: e.inode,
            attrs: &e.attrs,
            mtime_ns: e.mtime_ns,
            run_id: ctx.run_id,
        },
    )?;
    inventario::enfileirar_diretorio(tx, ctx.run_id, id, &e.caminho, t)?;
    ctx.vivo.pastas += 1;
    Ok(())
}

/// Grava a pasta: upsert simples no Full Scan; comparação com a linha existente no Fast Scan.
fn gravar_pasta(tx: &Connection, ctx: &mut Ctx, p: &NovaPasta<'_>) -> Result<i64, ErroScan> {
    if !ctx.rapido {
        return Ok(inventario::upsert_pasta(tx, p)?);
    }
    let (id, efeito) = reconciliacao::gravar_pasta(tx, p)?;
    if efeito == Efeito::Novo {
        ctx.vivo.fast.pastas_novas += 1;
    }
    Ok(id)
}
