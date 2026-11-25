//! O laço do escritor: drena a fila coalescida, grava lacunas, reconcilia diretórios, dispara
//! Fast Scans e publica a saúde. Roda na thread de quem detém o Catálogo.

use std::collections::{BTreeSet, HashSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use dag_core::VolumeDescoberto;
use dag_scanner::{
    Classificador, ControleScan, DesfechoScan, DirSujo, FastScan, RaizMonitorada, SemClassificacao,
    reconciliar_diretorios,
};
use dag_store::Catalogo;
use dag_store::confianca;
use dag_store::inventario::{self, EstadoScan, FaseScan};
use dag_store::reconciliacao::{self, NovoEvento, Saude, TipoEvento};

use crate::config::ConfigMonitor;
use crate::erro::ErroMonitor;
use crate::fila::{Fila, Lacuna, TipoLacuna, agora};
use crate::observador::{self, limite_do_sistema};

type FonteVolumes = Arc<dyn Fn() -> Vec<VolumeDescoberto> + Send + Sync>;

/// Chamado quando o monitor ficou quieto depois de mudanças: recebe as execuções que carimbaram
/// linhas novas/alteradas (a do monitor e as de Fast Scans) para rodar Hash Completo, duplicados
/// e reavaliação das Marcações. Erro vira `monitor_saude.erro`; não derruba o monitor.
pub type PosReconciliacao = Box<dyn FnMut(&mut Catalogo, &[i64]) -> Result<(), String> + Send>;

/// Totais da sessão de monitoramento.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumoMonitor {
    pub execucao_id: i64,
    pub lotes: u64,
    pub diretorios: u64,
    pub criados: u64,
    pub alterados: u64,
    pub removidos: u64,
    pub renomeados: u64,
    pub lacunas: u64,
    pub overflows: u64,
    pub fast_scans: u64,
    pub watches: u64,
}

/// Alça para injetar situações em teste (ou por operador): overflow sintético e dicas.
#[derive(Debug, Clone)]
pub struct InjetorMonitor {
    fila: Arc<Fila>,
}

impl std::fmt::Debug for Fila {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fila").finish_non_exhaustive()
    }
}

impl InjetorMonitor {
    /// Finge um `IN_Q_OVERFLOW`: o monitor deve gravar a lacuna e reconciliar tudo (CA-A11).
    pub fn injetar_overflow(&self) {
        self.fila.lacuna(Lacuna {
            tipo: TipoLacuna::Overflow,
            raiz: None,
            detalhe: "overflow sintético (injetado)".into(),
            observado_em: agora(),
        });
    }

    /// Finge que o diretório mudou (dica sem evento real do kernel).
    pub fn injetar_dica(&self, diretorio: impl AsRef<std::path::Path>, raiz: usize) {
        self.fila
            .marcar(diretorio.as_ref().as_os_str().as_bytes(), raiz);
    }
}

/// O monitor. Construa, ajuste e chame [`executar`](Self::executar).
pub struct Monitor {
    raizes: Vec<PathBuf>,
    fonte_volumes: FonteVolumes,
    config: ConfigMonitor,
    classificador: Arc<dyn Classificador>,
    pos: Option<PosReconciliacao>,
    fila: Arc<Fila>,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor")
            .field("raizes", &self.raizes)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

struct GapAberta {
    evento_id: i64,
    faltando: HashSet<usize>,
}

impl Monitor {
    /// `raizes`: pastas a vigiar (já catalogadas ou não); `fonte_volumes` devolve os volumes
    /// conectados agora (reconsultada a cada reconciliação periódica).
    #[must_use]
    pub fn novo(
        raizes: Vec<PathBuf>,
        fonte_volumes: impl Fn() -> Vec<VolumeDescoberto> + Send + Sync + 'static,
    ) -> Self {
        let config = ConfigMonitor::default();
        Self {
            raizes,
            fonte_volumes: Arc::new(fonte_volumes),
            fila: Arc::new(Fila::nova(config.max_dirs_pendentes)),
            config,
            classificador: Arc::new(SemClassificacao),
            pos: None,
        }
    }

    #[must_use]
    pub fn com_config(mut self, config: ConfigMonitor) -> Self {
        self.fila = Arc::new(Fila::nova(config.max_dirs_pendentes));
        self.config = config;
        self
    }

    #[must_use]
    pub fn com_classificador(mut self, c: Arc<dyn Classificador>) -> Self {
        self.classificador = c;
        self
    }

    /// Etapa de hash/duplicados "mais tarde": roda quando o monitor fica quieto.
    #[must_use]
    pub fn com_pos_reconciliacao(
        mut self,
        f: impl FnMut(&mut Catalogo, &[i64]) -> Result<(), String> + Send + 'static,
    ) -> Self {
        self.pos = Some(Box::new(f));
        self
    }

    /// Alça de injeção (overflow sintético etc.).
    #[must_use]
    pub fn injetor(&self) -> InjetorMonitor {
        InjetorMonitor {
            fila: Arc::clone(&self.fila),
        }
    }

    /// Roda até `parar` ser cancelado (Ctrl-C). Cancelar também interrompe um Fast Scan em
    /// andamento (sem remover nada). Bloqueia a thread chamadora, que é a única escritora.
    ///
    /// # Errors
    /// [`ErroMonitor::SemRaizes`] se nenhuma raiz serve; falhas do Catálogo ou do inotify.
    pub fn executar(
        &mut self,
        cat: &mut Catalogo,
        parar: &ControleScan,
    ) -> Result<ResumoMonitor, ErroMonitor> {
        let t0 = agora();
        let volumes = (self.fonte_volumes)();
        let raizes = self.resolver_raizes(cat, &volumes, t0)?;
        if raizes.is_empty() {
            return Err(ErroMonitor::SemRaizes);
        }
        let nomes: Vec<String> = raizes
            .iter()
            .map(|r| inventario::exibir(&r.caminho))
            .collect();
        let run_id = cat
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<i64, ErroMonitor> {
                let json = serde_json::to_string(&nomes).unwrap_or_else(|_| "[]".into());
                let id = inventario::criar_execucao(tx, "monitor", &json, None, t0)?;
                inventario::definir_estado(
                    tx,
                    id,
                    EstadoScan::EmAndamento,
                    Some(FaseScan::Concluida),
                    t0,
                )?;
                Ok(id)
            })?;
        let teto = self
            .config
            .max_watches
            .unwrap_or_else(|| (limite_do_sistema() / 2).max(1))
            .max(1);
        let mut laco = Laco {
            cfg: &self.config,
            pos: self.pos.as_mut(),
            fila: Arc::clone(&self.fila),
            fonte_volumes: Arc::clone(&self.fonte_volumes),
            classificador: Arc::clone(&self.classificador),
            raizes,
            run_id,
            gaps: Vec::new(),
            pendente_fast: BTreeSet::new(),
            adiadas: Vec::new(),
            desmontadas: HashSet::new(),
            execucoes_pos: BTreeSet::new(),
            precisa_pos: false,
            ultima_mudanca: Instant::now(),
            ultima_saude: None,
            ultima_reconciliacao: None,
            eventos_processados: 0,
            resumo: ResumoMonitor {
                execucao_id: run_id,
                ..ResumoMonitor::default()
            },
            iniciado_em: t0,
            erro: None,
            estado: "iniciando",
        };
        laco.publicar_saude(cat, true)?;
        // Lacunas abertas por execuções anteriores (processo morto): corrigidas no início.
        let abertas: Vec<i64> = {
            let mut st = cat.armazem().conexao().prepare(
                "SELECT id FROM scan_events WHERE reconcile_required = 1 AND processed_at IS NULL",
            )?;
            st.query_map([], |l| l.get(0))?.collect::<Result<_, _>>()?
        };
        let todas: HashSet<usize> = (0..laco.raizes.len()).collect();
        for id in abertas {
            laco.gaps.push(GapAberta {
                evento_id: id,
                faltando: todas.clone(),
            });
        }
        if self.config.reconciliar_ao_iniciar || !laco.gaps.is_empty() {
            laco.pendente_fast.extend(todas.iter().copied());
        }
        let resultado = thread::scope(|s| {
            let (tx, rx) = mpsc::channel();
            let fila = Arc::clone(&self.fila);
            let rs = laco.raizes.clone();
            let ex = self.config.exclusoes.clone();
            let obs = s.spawn(move || observador::rodar(fila, rs, ex, teto, &tx));
            let r = match rx.recv() {
                Ok(Ok(())) => laco.rodar(cat, parar),
                Ok(Err(e)) => Err(ErroMonitor::Observador(e)),
                Err(_) => Err(ErroMonitor::Observador(
                    "a thread do inotify terminou antes de ficar pronta".into(),
                )),
            };
            self.fila.encerrar();
            let _ = obs.join();
            r
        });
        // Encerramento limpo (ou falha): estado final da execução e da saúde.
        let t = agora();
        laco.estado = "parado";
        if let Err(e) = &resultado {
            laco.erro = Some(e.to_string());
        }
        let _ = laco.publicar_saude(cat, true);
        let estado = if resultado.is_ok() {
            EstadoScan::Concluida
        } else {
            // Monitor que morreu por erro deixou de vigiar: ninguém garante mais as Raízes.
            for r in &laco.raizes {
                let _ = confianca::perder(
                    cat.armazem().conexao(),
                    r.volume_id,
                    &r.caminho,
                    "o monitor terminou com erro",
                    t,
                );
            }
            EstadoScan::Falhou
        };
        cat.armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroMonitor> {
                inventario::definir_estado(tx, run_id, estado, None, t)?;
                Ok(())
            })?;
        resultado?;
        let mut r = laco.resumo.clone();
        r.watches = self.fila.travar().watches as u64;
        Ok(r)
    }

    /// Valida as raízes (diretório, volume conectado) e sincroniza o estado dos volumes.
    fn resolver_raizes(
        &self,
        cat: &mut Catalogo,
        volumes: &[VolumeDescoberto],
        t: i64,
    ) -> Result<Vec<RaizMonitorada>, ErroMonitor> {
        let sincronia = cat
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<_, ErroMonitor> {
                Ok(reconciliacao::sincronizar_volumes(tx, volumes, t)?)
            })?;
        let mut out: Vec<RaizMonitorada> = Vec::new();
        for r in &self.raizes {
            let Ok(canon) = std::fs::canonicalize(r) else {
                continue;
            };
            let Ok(m) = std::fs::symlink_metadata(&canon) else {
                continue;
            };
            if !m.is_dir() {
                continue;
            }
            let Some(volume_id) = sincronia.conectados.get(&m.dev()) else {
                continue;
            };
            let caminho = canon.as_os_str().as_bytes().to_vec();
            if out
                .iter()
                .any(|o| o.dev == m.dev() && reconciliacao::dentro_de(&caminho, &o.caminho))
            {
                continue;
            }
            out.push(RaizMonitorada {
                volume_id: *volume_id,
                caminho,
                dev: m.dev(),
            });
        }
        Ok(out)
    }
}

struct Laco<'a> {
    cfg: &'a ConfigMonitor,
    pos: Option<&'a mut PosReconciliacao>,
    fila: Arc<Fila>,
    fonte_volumes: FonteVolumes,
    classificador: Arc<dyn Classificador>,
    raizes: Vec<RaizMonitorada>,
    run_id: i64,
    gaps: Vec<GapAberta>,
    pendente_fast: BTreeSet<usize>,
    adiadas: Vec<(Instant, usize)>,
    desmontadas: HashSet<usize>,
    execucoes_pos: BTreeSet<i64>,
    precisa_pos: bool,
    ultima_mudanca: Instant,
    ultima_saude: Option<Instant>,
    ultima_reconciliacao: Option<i64>,
    eventos_processados: u64,
    resumo: ResumoMonitor,
    iniciado_em: i64,
    erro: Option<String>,
    estado: &'static str,
}

impl Laco<'_> {
    fn publicar_saude(&mut self, cat: &mut Catalogo, forcar: bool) -> Result<(), ErroMonitor> {
        if !forcar
            && self
                .ultima_saude
                .is_some_and(|t| t.elapsed() < self.cfg.heartbeat)
        {
            return Ok(());
        }
        self.ultima_saude = Some(Instant::now());
        let (backlog_fila, ultimo, watches, limite, overflows, parciais, erro_obs) = {
            let e = self.fila.travar();
            (
                e.sujos.len() + e.lacunas.len(),
                e.ultimo_evento,
                e.watches,
                e.watches_limite,
                e.overflows,
                e.parciais.len(),
                e.erro.clone(),
            )
        };
        let backlog = backlog_fila + self.pendente_fast.len() + self.adiadas.len();
        let s = Saude {
            run_id: Some(self.run_id),
            estado: self.estado.into(),
            iniciado_em: Some(self.iniciado_em),
            heartbeat: Some(agora()),
            backlog: backlog as u64,
            ultimo_evento_em: ultimo,
            ultima_reconciliacao: self.ultima_reconciliacao,
            watches: watches as u64,
            watches_limite: limite as u64,
            overflows,
            eventos_processados: self.eventos_processados,
            raizes_parciais: parciais as u64,
            raizes: self
                .raizes
                .iter()
                .map(|r| inventario::exibir(&r.caminho))
                .collect(),
            erro: self.erro.clone().or(erro_obs),
        };
        reconciliacao::gravar_saude(cat.armazem().conexao(), &s)?;
        Ok(())
    }

    fn rodar(&mut self, cat: &mut Catalogo, parar: &ControleScan) -> Result<(), ErroMonitor> {
        self.estado = "ativo";
        let mut proximo_periodico = self.cfg.reconciliar_a_cada.map(|d| Instant::now() + d);
        loop {
            if parar.cancelado() {
                self.estado = "encerrando";
                return Ok(());
            }
            // 1. Lacunas: gravar (reconcile_required) e agendar Fast Scan.
            self.tratar_lacunas(cat)?;
            // Raízes adiadas que já podem tentar de novo.
            let agora_i = Instant::now();
            let prontas: Vec<usize> = self
                .adiadas
                .iter()
                .filter(|(t, _)| *t <= agora_i)
                .map(|(_, r)| *r)
                .collect();
            self.adiadas.retain(|(t, _)| *t > agora_i);
            self.pendente_fast.extend(prontas);
            // 2. Periódico.
            if proximo_periodico.is_some_and(|t| Instant::now() >= t) {
                self.pendente_fast.extend(0..self.raizes.len());
                proximo_periodico = self.cfg.reconciliar_a_cada.map(|d| Instant::now() + d);
            }
            // 3. Fast Scans pendentes.
            if let Some(raiz) = self.pendente_fast.pop_first() {
                self.fast_scan(cat, parar, raiz)?;
                self.publicar_saude(cat, true)?;
                continue;
            }
            // 4. Diretórios sujos vencidos.
            let lote = self.coletar_vencidos();
            if !lote.is_empty() {
                self.estado = "reconciliando";
                self.reconciliar_lote(cat, &lote)?;
                self.estado = "ativo";
                self.publicar_saude(cat, true)?;
                continue;
            }
            // 5. Quieto: hash e duplicados "mais tarde".
            self.talvez_pos(cat);
            self.publicar_saude(cat, false)?;
            self.esperar(proximo_periodico);
        }
    }

    fn esperar(&self, proximo_periodico: Option<Instant>) {
        let agora_i = Instant::now();
        let mut espera = Duration::from_millis(200);
        {
            let e = self.fila.travar();
            if !e.lacunas.is_empty() {
                return;
            }
            for s in e.sujos.values() {
                let vence =
                    (s.ultimo + self.cfg.debounce).min(s.primeiro + self.cfg.debounce_maximo);
                espera = espera.min(vence.saturating_duration_since(agora_i));
            }
        }
        if let Some(t) = proximo_periodico {
            espera = espera.min(t.saturating_duration_since(agora_i));
        }
        for (t, _) in &self.adiadas {
            espera = espera.min(t.saturating_duration_since(agora_i));
        }
        if espera.is_zero() {
            return;
        }
        let e = self.fila.travar();
        let _ = self
            .fila
            .acordar
            .wait_timeout(e, espera)
            .unwrap_or_else(|p| p.into_inner());
    }

    fn tratar_lacunas(&mut self, cat: &mut Catalogo) -> Result<(), ErroMonitor> {
        let lacunas = std::mem::take(&mut self.fila.travar().lacunas);
        for l in lacunas {
            let alvo: HashSet<usize> = match l.raiz {
                Some(r) => HashSet::from([r]),
                None => (0..self.raizes.len()).collect(),
            };
            let (tipo_evento, volume) = match (l.tipo, l.raiz) {
                (TipoLacuna::RaizDesmontada, Some(r)) => {
                    self.desmontadas.insert(r);
                    (TipoEvento::Desmontado, Some(self.raizes[r].volume_id))
                }
                (_, Some(r)) => (TipoEvento::Overflow, Some(self.raizes[r].volume_id)),
                (_, None) => (TipoEvento::Overflow, None),
            };
            let detalhe = format!("{}: {}", l.tipo.como_str(), l.detalhe);
            let id = reconciliacao::registrar_evento(
                cat.armazem().conexao(),
                &NovoEvento {
                    volume_id: volume,
                    tipo: tipo_evento,
                    fonte: "inotify",
                    caminho: None,
                    caminho_antigo: None,
                    arquivo_id: None,
                    execucao_id: Some(self.run_id),
                    observado_em: l.observado_em,
                    processado_em: None,
                    reconciliar: true,
                    detalhe: Some(&detalhe),
                },
            )?;
            self.resumo.lacunas += 1;
            if l.tipo == TipoLacuna::Overflow {
                self.resumo.overflows += 1;
            }
            // Lacuna = o monitoramento deixou de garantir que viu tudo: sai do Ponto de Confiança.
            let motivo = format!("lacuna do monitor ({})", l.tipo.como_str());
            for r in &alvo {
                confianca::perder(
                    cat.armazem().conexao(),
                    self.raizes[*r].volume_id,
                    &self.raizes[*r].caminho,
                    &motivo,
                    l.observado_em,
                )?;
            }
            self.gaps.push(GapAberta {
                evento_id: id,
                faltando: alvo.clone(),
            });
            self.pendente_fast.extend(alvo);
            // A lacuna invalida o detalhe acumulado das raízes afetadas: o Fast Scan cobre.
        }
        Ok(())
    }

    /// Fast Scan de uma raiz: corrige as lacunas dela e carimba as execuções para o hash.
    fn fast_scan(
        &mut self,
        cat: &mut Catalogo,
        parar: &ControleScan,
        raiz: usize,
    ) -> Result<(), ErroMonitor> {
        self.estado = "reconciliando";
        self.publicar_saude(cat, true)?;
        let inicio = Instant::now();
        let volumes = (self.fonte_volumes)();
        let caminho = PathBuf::from(OsStr::from_bytes(&self.raizes[raiz].caminho));
        let scan = FastScan::novo(vec![caminho], volumes)
            .com_config(self.cfg.scan.clone())
            .com_controle(parar.clone())
            .com_classificador(Arc::clone(&self.classificador));
        let r = scan.executar(cat);
        self.estado = "ativo";
        match r {
            Ok(r) if r.desfecho != DesfechoScan::Cancelada => {
                self.resumo.fast_scans += 1;
                self.ultima_reconciliacao = Some(agora());
                self.execucoes_pos.insert(r.execucao_id);
                self.precisa_pos = true;
                self.ultima_mudanca = Instant::now();
                self.erro = None;
                self.fechar_gaps(cat, raiz)?;
                {
                    let mut e = self.fila.travar();
                    // O que chegou antes do scan começar foi coberto por ele.
                    e.sujos.retain(|d, s| {
                        s.ultimo > inicio
                            || !reconciliacao::dentro_de(d, &self.raizes[raiz].caminho)
                    });
                    e.saturadas.remove(&raiz);
                    if self.desmontadas.remove(&raiz) {
                        e.rearmar.push(raiz);
                    }
                }
                self.fila.acordar.notify_all();
            }
            Ok(_) => {} // cancelado: o laço vai encerrar
            Err(e) => {
                // Raiz indisponível ou falha: a lacuna fica aberta e tentamos de novo adiante.
                self.erro = Some(format!(
                    "reconciliação de {} falhou: {e}",
                    inventario::exibir(&self.raizes[raiz].caminho)
                ));
                self.adiadas
                    .push((Instant::now() + self.cfg.nova_tentativa, raiz));
            }
        }
        Ok(())
    }

    fn fechar_gaps(&mut self, cat: &mut Catalogo, raiz: usize) -> Result<(), ErroMonitor> {
        let mut fechar = Vec::new();
        for g in &mut self.gaps {
            g.faltando.remove(&raiz);
            if g.faltando.is_empty() {
                fechar.push(g.evento_id);
            }
        }
        self.gaps.retain(|g| !g.faltando.is_empty());
        if !fechar.is_empty() {
            let t = agora();
            cat.armazem_mut()
                .transacao_imediata(|tx| -> Result<(), ErroMonitor> {
                    reconciliacao::fechar_eventos(tx, &fechar, t)?;
                    Ok(())
                })?;
            self.eventos_processados += fechar.len() as u64;
        }
        Ok(())
    }

    fn coletar_vencidos(&self) -> Vec<DirSujo> {
        let agora_i = Instant::now();
        let mut e = self.fila.travar();
        let mut vencidos: Vec<(Instant, Vec<u8>, i64)> = e
            .sujos
            .iter()
            .filter(|(_, s)| {
                agora_i.saturating_duration_since(s.ultimo) >= self.cfg.debounce
                    || agora_i.saturating_duration_since(s.primeiro) >= self.cfg.debounce_maximo
            })
            .map(|(d, s)| (s.primeiro, d.clone(), s.observado_em))
            .collect();
        vencidos.sort();
        vencidos.truncate(self.cfg.lote_max.max(1));
        for (_, d, _) in &vencidos {
            e.sujos.remove(d);
        }
        vencidos
            .into_iter()
            .map(|(_, caminho, observado_em)| DirSujo {
                caminho,
                observado_em,
            })
            .collect()
    }

    fn reconciliar_lote(
        &mut self,
        cat: &mut Catalogo,
        lote: &[DirSujo],
    ) -> Result<(), ErroMonitor> {
        let r = reconciliar_diretorios(
            cat,
            self.run_id,
            &self.raizes,
            lote,
            self.classificador.as_ref(),
            &self.cfg.exclusoes,
            self.cfg.estabilidade,
            "inotify",
        )?;
        self.resumo.lotes += 1;
        self.resumo.diretorios += r.diretorios;
        self.resumo.criados += r.criados;
        self.resumo.alterados += r.alterados;
        self.resumo.removidos += r.removidos;
        self.resumo.renomeados += r.renomeados;
        self.eventos_processados += r.criados + r.alterados + r.removidos + r.renomeados;
        if r.mudou() {
            self.precisa_pos = true;
            self.execucoes_pos.insert(self.run_id);
            self.ultima_mudanca = Instant::now();
        }
        // Arquivos ainda sendo gravados: reconcilia de novo depois do debounce.
        if !r.instaveis.is_empty() {
            let t = agora();
            let agora_i = Instant::now();
            let mut e = self.fila.travar();
            for d in r.instaveis {
                if self
                    .raizes
                    .iter()
                    .any(|x| reconciliacao::dentro_de(&d, &x.caminho))
                {
                    e.sujos.entry(d).or_insert(crate::fila::Sujo {
                        primeiro: agora_i,
                        ultimo: agora_i,
                        observado_em: t,
                    });
                }
            }
            drop(e);
            self.ultima_mudanca = Instant::now();
        }
        for (caminho, motivo) in &r.problemas {
            self.erro = Some(format!("{}: {motivo}", inventario::exibir(caminho)));
        }
        // Raiz que sumiu: lacuna aberta (nada foi removido); corrigida quando ela voltar.
        for i in r.raizes_indisponiveis {
            if self.desmontadas.insert(i) {
                self.fila.lacuna(Lacuna {
                    tipo: TipoLacuna::RaizDesmontada,
                    raiz: Some(i),
                    detalhe: "raiz indisponível (desmontada, renomeada ou outro filesystem)".into(),
                    observado_em: agora(),
                });
            }
        }
        Ok(())
    }

    /// Só com a fila vazia e sem mudança recente: roda o pós-processamento (hash etc.).
    fn talvez_pos(&mut self, cat: &mut Catalogo) {
        if !self.precisa_pos || self.ultima_mudanca.elapsed() < self.cfg.estabilidade {
            return;
        }
        if !self.fila.travar().sujos.is_empty() || !self.pendente_fast.is_empty() {
            return;
        }
        self.precisa_pos = false;
        let ids: Vec<i64> = std::mem::take(&mut self.execucoes_pos)
            .into_iter()
            .collect();
        if let Some(pos) = self.pos.as_mut()
            && let Err(e) = (*pos)(cat, &ids)
        {
            self.erro = Some(format!("etapa pós-reconciliação: {e}"));
        }
    }
}
