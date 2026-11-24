//! Etapa "Conteúdo de contêineres" (ADR 0006): depois do Hash Completo, lê em fluxo e na memória
//! os compactados do Catálogo, grava Membros e o BLAKE3 de cada um na mesma leitura.
//!
//! Fila persistente e pausável (a tabela `conteineres`), **um leitor por Dispositivo Físico**
//! como o agendador de hash, e um coordenador único que escreve no Catálogo:
//!
//! ```text
//!  conteineres (SQLite) ──reserva 1──▶ coordenador ──Job──▶ leitor do disco A ─┐ Lote(membros)
//!        ▲                                  ▲            ──▶ leitor do disco B ─┤ Fim(resultado)
//!        └── transações curtas ◀────────────┴───────── canal limitado ◀──────────┘
//! ```
//!
//! * Nada é extraído nem gravado em disco: os leitores só abrem o arquivo Contêiner (somente
//!   leitura) e entregam Membros ao coordenador.
//! * Leitura tudo-ou-nada por Contêiner: se falha, é cancelada ou pede outra senha, os Membros já
//!   gravados dele são apagados (um Contêiner relido do zero é idempotente).
//! * Cifrado: tenta a senha ligada ao Hash Completo, depois as do Cofre (limitadas); a que abrir
//!   fica ligada ao hash. Sem sucesso vira Pedido de Senha na Fila de Serviço, sem parar nada.

use std::collections::{HashMap, VecDeque};
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender, SyncSender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dag_conteineres::{
    ErroConteiner, EstadoMembro, MembroLido, Opcoes, PontoDeRetomada, Registro, ResultadoLeitura,
    ler_conteiner_com_pontos,
};
use dag_core::{LeituraEmCurso, ProgressoConteineres, ProgressoDiscoHash, ProgressoHash};
use dag_store::Catalogo;
use dag_store::conteineres::{self as store, ItemConteiner, NovoMembro};

use crate::controle::{ControleScan, EstadoControle};
use crate::erro::ErroScan;
use crate::hash::abrir_somente_leitura;
use crate::scan::DesfechoScan;

/// Guarda das senhas: a chave de cifra fica no keyring do sistema (`dag-agent` implementa); o
/// Catálogo só guarda blobs cifrados. A etapa nunca registra nem devolve uma senha em claro.
pub trait Cofre: Send + Sync {
    /// O chaveiro está destrancado e utilizável? Falso = os Contêineres cifrados esperam
    /// ("aguardando o chaveiro").
    fn disponivel(&self) -> bool;
    /// Cifra uma senha para guardar no Catálogo.
    fn cifrar(&self, senha: &str) -> Option<Vec<u8>>;
    /// Decifra um blob do Catálogo.
    fn decifrar(&self, blob: &[u8]) -> Option<String>;
}

/// Sem cofre: nenhum Contêiner cifrado é aberto sozinho (vira Pedido de Senha).
#[derive(Debug, Clone, Copy, Default)]
pub struct SemCofre;

impl Cofre for SemCofre {
    fn disponivel(&self) -> bool {
        true
    }

    fn cifrar(&self, _: &str) -> Option<Vec<u8>> {
        None
    }

    fn decifrar(&self, _: &[u8]) -> Option<String> {
        None
    }
}

/// Parâmetros da etapa.
#[derive(Debug, Clone)]
pub struct ConfigConteineres {
    /// Discos lendo ao mesmo tempo (um leitor por disco).
    pub max_discos: usize,
    /// Níveis de Contêiner aninhado abertos (padrão 3).
    pub profundidade_max: u32,
    /// Maior Membro aninhado carregado na memória (padrão 256 MiB).
    pub limite_aninhado: u64,
    /// Senhas do Cofre tentadas por Contêiner (a derivação de chave do 7z/rar é cara).
    pub max_tentativas_cofre: usize,
    /// Membros por transação.
    pub lote_membros: usize,
    /// Intervalo máximo entre gravações do cursor de retomada de uma Imagem de Disco.
    pub intervalo_checkpoint: Duration,
    pub intervalo_de_progresso: Duration,
}

impl Default for ConfigConteineres {
    fn default() -> Self {
        Self {
            max_discos: 4,
            profundidade_max: 3,
            limite_aninhado: 256 << 20,
            max_tentativas_cofre: 8,
            lote_membros: 2000,
            intervalo_checkpoint: Duration::from_secs(2),
            intervalo_de_progresso: Duration::from_millis(500),
        }
    }
}

/// O que a etapa fez.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultadoConteineres {
    pub desfecho: DesfechoScan,
    /// Contêineres lidos nesta passada.
    pub lidos: u64,
    pub nao_conteineres: u64,
    /// Contêineres que reaproveitaram os Membros de uma cópia idêntica.
    pub copiados: u64,
    /// Membros gravados (leituras novas; não conta os copiados).
    pub membros: u64,
    pub bytes_lidos: u64,
    pub aguardando_senha: u64,
    pub falhas: u64,
    pub duracao: Duration,
}

impl ResultadoConteineres {
    /// Algum Membro novo (ou copiado) entrou no Catálogo: os Grupos de Duplicados mudam.
    #[must_use]
    pub fn mudou_membros(&self) -> bool {
        self.membros > 0 || self.copiados > 0
    }
}

type CallbackProgresso = Box<dyn FnMut(&ProgressoConteineres) + Send>;

/// A etapa.
pub struct EtapaConteineres {
    config: ConfigConteineres,
    controle: ControleScan,
    progresso: Option<CallbackProgresso>,
    cofre: Arc<dyn Cofre>,
    registro: Registro,
}

impl std::fmt::Debug for EtapaConteineres {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EtapaConteineres")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Default for EtapaConteineres {
    fn default() -> Self {
        Self::nova()
    }
}

fn agora() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

// ------------------------------------------------------------------ mensagens

struct Job {
    item: ItemConteiner,
    /// Tentativas em ordem; `None` = sem senha.
    senhas: Vec<Option<String>>,
}

enum Fim {
    Lido {
        resultado: ResultadoLeitura,
        senha: Option<String>,
    },
    /// O arquivo não é mais o que foi hasheado: fica para o próximo Hash Completo.
    Mudou,
    Erro(ErroConteiner),
}

enum Msg {
    Lote {
        file_id: i64,
        hash: [u8; 32],
        membros: Vec<MembroLido>,
        /// Ponto seguro desta gravação (leitores retomáveis).
        ponto: Option<PontoDeRetomada>,
    },
    Bytes {
        grupo: usize,
        file_id: i64,
        lidos: u64,
    },
    Reiniciar {
        file_id: i64,
    },
    Fim {
        grupo: usize,
        item: ItemConteiner,
        fim: Fim,
    },
}

struct Grupo {
    nome: String,
    volumes: Vec<i64>,
    total: u64,
    bytes_total: u64,
    feitos: u64,
    bytes_lidos: u64,
    /// Último `file_id` reservado (cursor).
    cursor: i64,
    esgotado: bool,
    iniciado: bool,
    em_curso: Option<EmCurso>,
    tx_job: Option<Sender<Job>>,
    janela: VecDeque<(Instant, u64)>,
}

struct EmCurso {
    item: ItemConteiner,
    /// Já tentou o Cofre.
    fase_cofre: bool,
    lidos: u64,
}

fn nome_de(path: &[u8]) -> String {
    String::from_utf8_lossy(path).into_owned()
}

fn para_novo(m: &MembroLido) -> NovoMembro {
    let (estado, motivo) = match &m.estado {
        EstadoMembro::Lido => ("lido", None),
        EstadoMembro::Cifrado => (
            "cifrado",
            Some("contêiner aninhado cifrado: a senha não abre".to_owned()),
        ),
        EstadoMembro::NaoSuportado(s) => ("nao_suportado", Some(s.clone())),
        EstadoMembro::AninhadoGrande => (
            "aninhado_grande",
            Some("contêiner aninhado grande demais para abrir na memória".to_owned()),
        ),
        EstadoMembro::Erro(s) => ("erro", Some(s.clone())),
    };
    NovoMembro {
        ordem: i64::try_from(m.ordem).unwrap_or(i64::MAX),
        pai_ordem: m.pai.map(|p| i64::try_from(p).unwrap_or(i64::MAX)),
        caminho: m.info.caminho.clone(),
        tipo: m.info.tipo.nome(),
        tamanho: m.info.tamanho.map(|t| i64::try_from(t).unwrap_or(i64::MAX)),
        mtime_ns: m.info.mtime_ns,
        cifrado: m.info.cifrado,
        hash: m.hash,
        estado,
        motivo,
        formato_interno: m.formato_interno,
    }
}

impl EtapaConteineres {
    #[must_use]
    pub fn nova() -> Self {
        Self {
            config: ConfigConteineres::default(),
            controle: ControleScan::novo(),
            progresso: None,
            cofre: Arc::new(SemCofre),
            registro: Registro::completo(),
        }
    }

    #[must_use]
    pub fn com_config(mut self, c: ConfigConteineres) -> Self {
        self.config = c;
        self
    }

    #[must_use]
    pub fn com_controle(mut self, c: ControleScan) -> Self {
        self.controle = c;
        self
    }

    #[must_use]
    pub fn com_cofre(mut self, c: Arc<dyn Cofre>) -> Self {
        self.cofre = c;
        self
    }

    /// Registro de formatos (para plugar leitores de Imagem de Disco).
    #[must_use]
    pub fn com_registro(mut self, r: Registro) -> Self {
        self.registro = r;
        self
    }

    #[must_use]
    pub fn com_progresso(mut self, f: impl FnMut(&ProgressoConteineres) + Send + 'static) -> Self {
        self.progresso = Some(Box::new(f));
        self
    }

    /// Executa a etapa até esgotar a fila ou ser cancelada.
    ///
    /// # Errors
    /// Falha do Catálogo.
    #[allow(clippy::too_many_lines)]
    pub fn executar(mut self, cat: &mut Catalogo) -> Result<ResultadoConteineres, ErroScan> {
        let inicio = Instant::now();
        let t0 = agora();
        let emitir = |p: ProgressoConteineres, cb: &mut Option<CallbackProgresso>| {
            if let Some(f) = cb.as_mut() {
                f(&p);
            }
        };
        let cofre_ok = self.cofre.disponivel();
        let fila = cat
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<_, ErroScan> {
                if cofre_ok {
                    store::reenfileirar_aguardando_chaveiro(tx, t0)?;
                }
                Ok(store::preparar_fila(tx, t0)?)
            })?;
        let volumes = store::volumes_pendentes(cat.armazem().conexao())?;
        let mut grupos: Vec<Grupo> = Vec::new();
        let mut chave_grupo: HashMap<i64, usize> = HashMap::new();
        for v in volumes {
            let chave = v.dispositivo_id.unwrap_or(-v.volume_id);
            let idx = *chave_grupo.entry(chave).or_insert_with(|| {
                grupos.push(Grupo {
                    nome: v.nome_disco.clone(),
                    volumes: Vec::new(),
                    total: 0,
                    bytes_total: 0,
                    feitos: 0,
                    bytes_lidos: 0,
                    cursor: 0,
                    esgotado: false,
                    iniciado: false,
                    em_curso: None,
                    tx_job: None,
                    janela: VecDeque::new(),
                });
                grupos.len() - 1
            });
            let g = &mut grupos[idx];
            g.volumes.push(v.volume_id);
            g.total += v.conteineres;
            g.bytes_total += v.bytes;
        }
        let total: u64 = grupos.iter().map(|g| g.total).sum();
        let bytes_total: u64 = grupos.iter().map(|g| g.bytes_total).sum();
        let mut res = ResultadoConteineres {
            desfecho: DesfechoScan::Concluida,
            lidos: 0,
            nao_conteineres: 0,
            copiados: fila.copiados,
            membros: 0,
            bytes_lidos: 0,
            aguardando_senha: 0,
            falhas: 0,
            duracao: Duration::ZERO,
        };
        let cfg = self.config.clone();
        let controle = self.controle.clone();
        let registro = &self.registro;
        let cofre = Arc::clone(&self.cofre);
        let mut progresso = self.progresso.take();
        let mut cache_cofre: Option<Vec<String>> = None;
        let mut ultimo_progresso = Instant::now();
        let mut ocupados = 0usize;
        let abort = AtomicBool::new(false);

        let resultado_cat: Result<(), ErroScan> = std::thread::scope(|s| {
            let (tx_msg, rx_msg) = mpsc::sync_channel::<Msg>(64);
            let mut falha: Option<ErroScan> = None;
            'principal: loop {
                let cancelado = controle.cancelado();
                let pausado = controle.estado() == EstadoControle::Pausado;
                if !cancelado && !pausado {
                    // Inicia discos até o limite de paralelismo e despacha para os livres.
                    let ativos = grupos.iter().filter(|g| g.iniciado && !g.esgotado).count();
                    let mut vagas = cfg.max_discos.max(1).saturating_sub(ativos);
                    #[allow(clippy::needless_range_loop)]
                    for gi in 0..grupos.len() {
                        if grupos[gi].esgotado || grupos[gi].em_curso.is_some() {
                            continue;
                        }
                        if !grupos[gi].iniciado {
                            if vagas == 0 {
                                continue;
                            }
                            vagas -= 1;
                            grupos[gi].iniciado = true;
                            let (tj, rj) = mpsc::channel::<Job>();
                            grupos[gi].tx_job = Some(tj);
                            let tx = tx_msg.clone();
                            let (c, r, ct, ab) = (&cfg, registro, &controle, &abort);
                            s.spawn(move || trabalhar(gi, rj, tx, r, c, ct, ab));
                        }
                        let reservado = cat.armazem_mut().transacao_imediata(
                            |tx| -> Result<Option<ItemConteiner>, ErroScan> {
                                let mut v = store::reservar(
                                    tx,
                                    &grupos[gi].volumes,
                                    grupos[gi].cursor,
                                    1,
                                    agora(),
                                )?;
                                Ok(v.pop())
                            },
                        );
                        let item = match reservado {
                            Ok(i) => i,
                            Err(e) => {
                                falha = Some(e);
                                break 'principal;
                            }
                        };
                        let Some(item) = item else {
                            grupos[gi].esgotado = true;
                            grupos[gi].tx_job = None;
                            continue;
                        };
                        grupos[gi].cursor = item.file_id;
                        // Senha ligada ao hash: tentativa extra depois de "sem senha".
                        let mut senhas = vec![None];
                        if cofre.disponivel()
                            && let Ok(Some(blob)) =
                                store::senha_cifrada(cat.armazem().conexao(), &item.hash)
                            && let Some(p) = cofre.decifrar(&blob)
                        {
                            senhas.push(Some(p));
                        }
                        despachar(&mut grupos[gi], item, senhas, false);
                        ocupados += 1;
                    }
                }
                if (cancelado || grupos.iter().all(|g| g.esgotado)) && ocupados == 0 {
                    break;
                }
                if pausado && ocupados == 0 {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                match rx_msg.recv_timeout(Duration::from_millis(100)) {
                    Ok(Msg::Lote {
                        file_id,
                        hash,
                        membros,
                        ponto,
                    }) => {
                        let novos: Vec<NovoMembro> = membros.iter().map(para_novo).collect();
                        let n = novos.len() as u64;
                        let t = agora();
                        let r =
                            cat.armazem_mut()
                                .transacao_imediata(|tx| -> Result<(), ErroScan> {
                                    match ponto {
                                        // Membros e cursor na mesma transação.
                                        Some(p) => store::gravar_lote_retomavel(
                                            tx,
                                            file_id,
                                            &novos,
                                            store::Retomada {
                                                indice: p.indice,
                                                proxima_ordem: p.proxima_ordem,
                                                bytes: p.bytes,
                                            },
                                            &hash,
                                            t,
                                        )?,
                                        None => store::inserir_membros(tx, file_id, &novos)?,
                                    }
                                    Ok(())
                                });
                        if let Err(e) = r {
                            falha = Some(e);
                            break;
                        }
                        res.membros += n;
                    }
                    Ok(Msg::Bytes {
                        grupo,
                        file_id,
                        lidos,
                    }) => {
                        let g = &mut grupos[grupo];
                        if let Some(c) = g.em_curso.as_mut()
                            && c.item.file_id == file_id
                        {
                            let delta = lidos.saturating_sub(c.lidos);
                            c.lidos = lidos;
                            g.bytes_lidos += delta;
                            res.bytes_lidos += delta;
                            g.janela.push_back((Instant::now(), g.bytes_lidos));
                            while g.janela.len() > 2
                                && g.janela
                                    .front()
                                    .is_some_and(|(t, _)| t.elapsed() > Duration::from_secs(8))
                            {
                                g.janela.pop_front();
                            }
                        }
                    }
                    Ok(Msg::Reiniciar { file_id }) => {
                        let r =
                            cat.armazem_mut()
                                .transacao_imediata(|tx| -> Result<(), ErroScan> {
                                    store::apagar_membros(tx, file_id)?;
                                    Ok(())
                                });
                        if let Err(e) = r {
                            falha = Some(e);
                            break;
                        }
                    }
                    Ok(Msg::Fim { grupo, item, fim }) => {
                        ocupados -= 1;
                        let em = grupos[grupo].em_curso.take();
                        let fase_cofre = em.as_ref().is_some_and(|e| e.fase_cofre);
                        let r = tratar_fim(
                            cat,
                            &cfg,
                            &*cofre,
                            &mut cache_cofre,
                            &mut grupos[grupo],
                            &mut res,
                            &mut ocupados,
                            item,
                            fim,
                            fase_cofre,
                        );
                        if let Err(e) = r {
                            falha = Some(e);
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                if ultimo_progresso.elapsed() >= cfg.intervalo_de_progresso {
                    ultimo_progresso = Instant::now();
                    emitir(
                        monta_progresso(&grupos, &res, total, bytes_total, &cfg, pausado, inicio),
                        &mut progresso,
                    );
                }
            }
            // Encerra os leitores (soltando os canais de job) e espera o que ainda lê abortar.
            for g in &mut grupos {
                g.tx_job = None;
            }
            if falha.is_some() {
                abort.store(true, Ordering::SeqCst);
            }
            drop(tx_msg);
            // Drena para os leitores em curso poderem terminar sem travar no canal cheio.
            while rx_msg.recv_timeout(Duration::from_millis(50)).is_ok() {}
            match falha {
                Some(e) => Err(e),
                None => Ok(()),
            }
        });
        // Itens que ficaram `lendo` (cancelamento, erro) voltam à fila.
        let t1 = agora();
        let _ = cat
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroScan> {
                store::devolver_lendo(tx, t1)?;
                Ok(())
            });
        resultado_cat?;
        if controle.cancelado() {
            res.desfecho = DesfechoScan::Cancelada;
        } else if res.falhas > 0 {
            res.desfecho = DesfechoScan::ConcluidaComErros;
        }
        res.duracao = inicio.elapsed();
        let pausado = false;
        // Sem nada a ler a etapa fica em silêncio: não assume o painel do trabalho.
        if total > 0 || res.copiados > 0 {
            emitir(
                monta_progresso(&grupos, &res, total, bytes_total, &cfg, pausado, inicio),
                &mut progresso,
            );
        }
        Ok(res)
    }
}

fn despachar(g: &mut Grupo, item: ItemConteiner, senhas: Vec<Option<String>>, fase_cofre: bool) {
    g.em_curso = Some(EmCurso {
        // Leitura retomada: os bytes já lidos antes não contam de novo no progresso.
        lidos: item.retomada.map_or(0, |r| r.bytes),
        item: item.clone(),
        fase_cofre,
    });
    if let Some(tx) = &g.tx_job {
        let _ = tx.send(Job { item, senhas });
    }
}

#[allow(clippy::too_many_arguments)]
fn tratar_fim(
    cat: &mut Catalogo,
    cfg: &ConfigConteineres,
    cofre: &dyn Cofre,
    cache_cofre: &mut Option<Vec<String>>,
    g: &mut Grupo,
    res: &mut ResultadoConteineres,
    ocupados: &mut usize,
    item: ItemConteiner,
    fim: Fim,
    fase_cofre: bool,
) -> Result<(), ErroScan> {
    let t = agora();
    let f = item.file_id;
    match fim {
        Fim::Lido { resultado, senha } => {
            let bytes = i64::try_from(resultado.bytes).unwrap_or(i64::MAX);
            let copiados = cat
                .armazem_mut()
                .transacao_imediata(|tx| -> Result<u64, ErroScan> {
                    let mut copiados = 0;
                    match resultado.formato {
                        None => {
                            store::apagar_membros(tx, f)?;
                            store::marcar_nao_conteiner(tx, f, &item.hash, t)?;
                        }
                        Some(formato) => {
                            store::concluir(
                                tx,
                                f,
                                &item.hash,
                                formato,
                                i64::try_from(resultado.membros).unwrap_or(i64::MAX),
                                bytes,
                                senha.is_some(),
                                t,
                            )?;
                            if let Some(p) = &senha
                                && let Some(blob) = cofre.cifrar(p)
                            {
                                store::guardar_senha(tx, &item.hash, &blob, t)?;
                            }
                            // Cópias idênticas que ainda esperam reaproveitam a lista.
                            copiados = store::propagar_para_identicas(tx, f, &item.hash, t)?;
                            if !resultado.avisos.is_empty() {
                                // Lido, mas com problemas: um Pedido agregado por imagem.
                                store::criar_pedido(
                                    tx,
                                    "falha_leitura",
                                    f,
                                    &resumir_avisos(&resultado.avisos),
                                    t,
                                )?;
                            }
                        }
                    }
                    Ok(copiados)
                })?;
            res.copiados += copiados;
            g.feitos += 1;
            if resultado.formato.is_some() {
                res.lidos += 1;
            } else {
                res.nao_conteineres += 1;
            }
        }
        Fim::Mudou => {
            // Fica pendente (o cursor do grupo impede de ser reservado de novo agora).
            cat.armazem_mut()
                .transacao_imediata(|tx| -> Result<(), ErroScan> {
                    store::apagar_membros(tx, f)?;
                    tx.execute(
                        "UPDATE conteineres SET estado = 'pendente', atualizado_em = ?2 WHERE file_id = ?1",
                        rusqlite::params![f, t],
                    )?;
                    Ok(())
                })?;
            g.feitos += 1;
        }
        Fim::Erro(ErroConteiner::Cancelado) => {
            cat.armazem_mut()
                .transacao_imediata(|tx| -> Result<(), ErroScan> {
                    if store::tem_retomada(tx, f)? {
                        // Leitor retomável: mantém Membros e cursor; continua daqui.
                        store::devolver_preservando(tx, f, t)?;
                        return Ok(());
                    }
                    store::apagar_membros(tx, f)?;
                    tx.execute(
                        "UPDATE conteineres SET estado = 'pendente', atualizado_em = ?2 WHERE file_id = ?1",
                        rusqlite::params![f, t],
                    )?;
                    Ok(())
                })?;
        }
        Fim::Erro(e @ (ErroConteiner::SenhaNecessaria | ErroConteiner::SenhaIncorreta)) => {
            cat.armazem_mut()
                .transacao_imediata(|tx| -> Result<(), ErroScan> {
                    store::apagar_membros(tx, f)?;
                    Ok(())
                })?;
            if !fase_cofre && cofre.disponivel() {
                // Segunda chance: as senhas do Cofre.
                if cache_cofre.is_none() {
                    let blobs = store::cofre_cifradas(cat.armazem().conexao())?;
                    *cache_cofre = Some(
                        blobs
                            .iter()
                            .filter_map(|(_, b)| cofre.decifrar(b))
                            .collect(),
                    );
                }
                let candidatas: Vec<Option<String>> = cache_cofre
                    .as_ref()
                    .map(|v| {
                        v.iter()
                            .take(cfg.max_tentativas_cofre)
                            .cloned()
                            .map(Some)
                            .collect()
                    })
                    .unwrap_or_default();
                if !candidatas.is_empty() {
                    despachar(g, item, candidatas, true);
                    *ocupados += 1;
                    return Ok(());
                }
            }
            let (chaveiro, motivo) = if cofre.disponivel() {
                let m = if matches!(e, ErroConteiner::SenhaIncorreta) || fase_cofre {
                    "contêiner cifrado: nenhuma senha guardada nem do Cofre abre; informe a senha"
                } else {
                    "contêiner cifrado: informe a senha"
                };
                (false, m)
            } else {
                (
                    true,
                    "aguardando o chaveiro do sistema (trancado) para tentar as senhas guardadas",
                )
            };
            cat.armazem_mut()
                .transacao_imediata(|tx| -> Result<(), ErroScan> {
                    store::marcar_aguardando(tx, f, chaveiro, motivo, t)?;
                    Ok(())
                })?;
            g.feitos += 1;
            res.aguardando_senha += 1;
        }
        Fim::Erro(e) => {
            let motivo = e.to_string();
            cat.armazem_mut()
                .transacao_imediata(|tx| -> Result<(), ErroScan> {
                    store::apagar_membros(tx, f)?;
                    store::marcar_falha(tx, f, &motivo, t)?;
                    Ok(())
                })?;
            g.feitos += 1;
            res.falhas += 1;
        }
    }
    Ok(())
}

/// Aviso agregado de uma imagem: os primeiros e a contagem do resto.
fn resumir_avisos(avisos: &[String]) -> String {
    const MOSTRAR: usize = 5;
    let mut m = format!("lido com {} problema(s)", avisos.len());
    for a in avisos.iter().take(MOSTRAR) {
        m.push_str("; ");
        m.push_str(a);
    }
    if avisos.len() > MOSTRAR {
        m.push_str(&format!("; e mais {}", avisos.len() - MOSTRAR));
    }
    m
}

fn monta_progresso(
    grupos: &[Grupo],
    res: &ResultadoConteineres,
    total: u64,
    bytes_total: u64,
    cfg: &ConfigConteineres,
    pausado: bool,
    inicio: Instant,
) -> ProgressoConteineres {
    let feitos: u64 = grupos.iter().map(|g| g.feitos).sum();
    let janela_bps = |g: &Grupo| -> f64 {
        match (g.janela.front(), g.janela.back()) {
            (Some((t0, b0)), Some((t1, b1))) if t1 > t0 => {
                (*b1 - *b0) as f64 / t1.duration_since(*t0).as_secs_f64()
            }
            _ => 0.0,
        }
    };
    let bps: f64 = grupos.iter().map(janela_bps).sum();
    let janela_ms = grupos
        .iter()
        .filter_map(|g| g.janela.front().map(|(t, _)| t.elapsed().as_millis()))
        .max()
        .unwrap_or(0);
    let discos = grupos
        .iter()
        .map(|g| ProgressoDiscoHash {
            disco: g.nome.clone(),
            volumes: g.volumes.clone(),
            threads: 1,
            leitores_ativos: u32::from(g.em_curso.is_some() && !pausado),
            arquivos_feitos: g.feitos,
            arquivos_total: g.total,
            bytes_lidos: g.bytes_lidos,
            bytes_total: g.bytes_total,
            bytes_por_seg: janela_bps(g),
            ativo: g.iniciado && !g.esgotado,
            concluido: g.esgotado && g.em_curso.is_none(),
            lendo: g
                .em_curso
                .as_ref()
                .map(|c| LeituraEmCurso {
                    caminho: nome_de(&c.item.path_raw),
                    bytes_lidos: c.lidos,
                    bytes_total: u64::try_from(c.item.tamanho).unwrap_or(0),
                })
                .into_iter()
                .collect(),
        })
        .collect();
    ProgressoConteineres {
        andamento: ProgressoHash {
            arquivos_feitos: feitos,
            arquivos_total: total,
            bytes_lidos: res.bytes_lidos,
            bytes_total,
            bytes_por_seg: bps,
            threads_por_disco: 1,
            max_discos: u32::try_from(cfg.max_discos).unwrap_or(u32::MAX),
            erros: res.falhas,
            janela_ms: u64::try_from(janela_ms).unwrap_or(u64::MAX),
            decorrido_ms: u64::try_from(inicio.elapsed().as_millis()).unwrap_or(u64::MAX),
            discos,
            ..ProgressoHash::default()
        },
        membros: res.membros,
        aguardando_senha: res.aguardando_senha,
        falhas: res.falhas,
        copiados: res.copiados,
    }
}

// ------------------------------------------------------------------ leitor de um disco

fn trabalhar(
    grupo: usize,
    rx: mpsc::Receiver<Job>,
    tx: SyncSender<Msg>,
    registro: &Registro,
    cfg: &ConfigConteineres,
    controle: &ControleScan,
    abort: &AtomicBool,
) {
    while let Ok(job) = rx.recv() {
        let item = job.item.clone();
        let fim = ler_um(grupo, &job, &tx, registro, cfg, controle, abort);
        if tx.send(Msg::Fim { grupo, item, fim }).is_err() {
            return;
        }
    }
}

fn ler_um(
    grupo: usize,
    job: &Job,
    tx: &SyncSender<Msg>,
    registro: &Registro,
    cfg: &ConfigConteineres,
    controle: &ControleScan,
    abort: &AtomicBool,
) -> Fim {
    use std::os::unix::ffi::OsStrExt;
    let caminho = PathBuf::from(std::ffi::OsStr::from_bytes(&job.item.path_raw));
    match std::fs::symlink_metadata(&caminho) {
        Ok(m) if m.is_file() && i64::try_from(m.len()).ok() == Some(job.item.tamanho) => {}
        Ok(_) => return Fim::Mudou,
        Err(e) => return Fim::Erro(ErroConteiner::Io(e)),
    }
    let file_id = job.item.file_id;
    let mut ultimo_erro = ErroConteiner::SenhaNecessaria;
    let mut retomada = job.item.retomada;
    let hash = job.item.hash;
    for senha in &job.senhas {
        let arquivo = match abrir_somente_leitura(&caminho) {
            Ok(f) => f,
            Err(e) => return Fim::Erro(ErroConteiner::Io(e)),
        };
        let fonte: dag_conteineres::FonteBox = Box::new(BufReader::with_capacity(1 << 20, arquivo));
        let opcoes = Opcoes {
            profundidade_max: cfg.profundidade_max,
            limite_aninhado: cfg.limite_aninhado,
            senha: senha.clone(),
            retomar: retomada.map(|r| PontoDeRetomada {
                indice: r.indice,
                proxima_ordem: r.proxima_ordem,
                bytes: r.bytes,
            }),
        };
        let lote: std::cell::RefCell<Vec<MembroLido>> = std::cell::RefCell::new(Vec::new());
        let perdeu_canal = std::cell::Cell::new(false);
        let mut lidos: u64 = retomada.map_or(0, |r| r.bytes);
        let mut ultimo = Instant::now();
        let mut ultimo_lote = Instant::now();
        let r = ler_conteiner_com_pontos(
            registro,
            fonte,
            &opcoes,
            &|| controle.cancelado() || abort.load(Ordering::Relaxed) || perdeu_canal.get(),
            &mut |n| {
                lidos += n;
                // Pausa: segura a leitura aqui (o arquivo continua aberto).
                while controle.estado() == EstadoControle::Pausado {
                    std::thread::sleep(Duration::from_millis(100));
                }
                if ultimo.elapsed() >= Duration::from_millis(250) {
                    ultimo = Instant::now();
                    let _ = tx.try_send(Msg::Bytes {
                        grupo,
                        file_id,
                        lidos,
                    });
                }
            },
            &mut |m| {
                lote.borrow_mut().push(m);
                Ok(())
            },
            // Ponto seguro (Membro do nível raiz concluído, com os filhos): é aqui que o lote
            // sai. Leitor retomável também grava por tempo, para um Membro enorme não deixar
            // horas de trabalho sem cursor; o cursor viaja com os Membros.
            &mut |ponto| {
                let cheio = lote.borrow().len() >= cfg.lote_membros
                    || (ponto.is_some() && ultimo_lote.elapsed() >= cfg.intervalo_checkpoint);
                if cheio {
                    ultimo_lote = Instant::now();
                    let membros = std::mem::take(&mut *lote.borrow_mut());
                    let enviado = tx.send(Msg::Lote {
                        file_id,
                        hash,
                        membros,
                        ponto,
                    });
                    if enviado.is_err() {
                        perdeu_canal.set(true);
                    }
                }
            },
        );
        match r {
            Ok(resultado) => {
                let restante = lote.into_inner();
                if !restante.is_empty()
                    && tx
                        .send(Msg::Lote {
                            file_id,
                            hash,
                            membros: restante,
                            ponto: None,
                        })
                        .is_err()
                {
                    return Fim::Erro(ErroConteiner::Cancelado);
                }
                let _ = tx.try_send(Msg::Bytes {
                    grupo,
                    file_id,
                    lidos,
                });
                return Fim::Lido {
                    resultado,
                    senha: senha.clone(),
                };
            }
            Err(e @ (ErroConteiner::SenhaNecessaria | ErroConteiner::SenhaIncorreta)) => {
                if tx.send(Msg::Reiniciar { file_id }).is_err() {
                    return Fim::Erro(ErroConteiner::Cancelado);
                }
                retomada = None;
                ultimo_erro = e;
            }
            Err(e) => return Fim::Erro(e),
        }
    }
    Fim::Erro(ultimo_erro)
}
