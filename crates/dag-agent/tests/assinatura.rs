//! Assinatura de eventos (ADR 0004): todo cliente vê e controla todo trabalho do agente, não
//! importa quem o iniciou; quem assina tarde recebe o instantâneo; o observador reconecta.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dag_agent::{
    Assinatura, Cliente, ConfigServidor, EventoObservado, Observador, OpcoesObservador, Servidor,
};
use dag_core::{
    Alvo, Autor, Comando, EstadoJob, EventoAgente, JobInfo, ModoVarredura, OrigemJob, Pedido,
    Progresso, Resposta, TipoJob, VisaoJobs, VolumeDescoberto,
};

fn volume_de(p: &Path) -> VolumeDescoberto {
    let dev = fs::metadata(p).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("teste-dev-{dev}"),
        st_dev: dev,
        rotulo: Some("teste".into()),
        fs_type: "teste".into(),
        capacidade_bytes: Some(1000),
        livre_bytes: Some(400),
        pontos_de_montagem: vec![p.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: None,
    }
}

fn escrever(p: &Path, bytes: usize) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, vec![b'x'; bytes]).unwrap();
}

struct Cenario {
    _tmp: tempfile::TempDir,
    raiz: PathBuf,
    catalogo: PathBuf,
    socket: PathBuf,
}

/// Árvore grande o bastante para a varredura durar enquanto os clientes a controlam.
fn cenario() -> Cenario {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let raiz = base.join("arvore");
    escrever(&raiz.join("big.pdf"), 9000);
    escrever(&raiz.join("copia/big.pdf"), 9000);
    for d in 0..120 {
        for f in 0..60 {
            escrever(&raiz.join(format!("g{d}/f{f}.txt")), 64 + f);
        }
    }
    Cenario {
        catalogo: base.join("dados/catalogo.sqlite"),
        socket: base.join("run/dag/agente.sock"),
        raiz,
        _tmp: tmp,
    }
}

fn config(c: &Cenario) -> ConfigServidor {
    let mut cfg = ConfigServidor::novo(&c.catalogo, &c.socket);
    cfg.monitor = false;
    cfg.volumes_fixos = Some(vec![volume_de(&c.raiz)]);
    cfg
}

fn scan(c: &Cenario, sem_hash: bool) -> Comando {
    Comando::IniciarScan {
        modo: ModoVarredura::FullScan,
        raizes: vec![c.raiz.to_str().unwrap().into()],
        sem_hash,
        elevar: false,
        releitura_total: false,
        threads_por_disco: None,
        max_discos: None,
    }
}

fn enviar(socket: &Path, autor: Autor, comando: Comando) -> Resposta {
    Cliente::conectar(socket)
        .unwrap()
        .enviar(Pedido { autor, comando }, &mut |_| {})
        .unwrap()
}

/// Um assinante que aplica os eventos e guarda tudo o que recebeu.
struct Escuta {
    ass: Assinatura,
    visao: VisaoJobs,
    recebidos: Vec<EventoAgente>,
}

impl Escuta {
    fn nova(socket: &Path) -> Self {
        Self {
            ass: Cliente::conectar(socket).unwrap().assinar().unwrap(),
            visao: VisaoJobs::default(),
            recebidos: Vec::new(),
        }
    }

    /// Lê até um evento que satisfaz `pred` (já aplicado à visão).
    fn ate(
        &mut self,
        descricao: &str,
        pred: impl Fn(&EventoAgente, &VisaoJobs) -> bool,
    ) -> EventoAgente {
        let fim = Instant::now() + Duration::from_secs(60);
        loop {
            assert!(
                Instant::now() < fim,
                "tempo esgotado: {descricao}; recebidos: {:#?}",
                self.recebidos.iter().rev().take(6).collect::<Vec<_>>()
            );
            if let Some(e) = self.ass.proximo(Duration::from_millis(200)).unwrap() {
                self.visao.aplicar(&e);
                self.recebidos.push(e.clone());
                if pred(&e, &self.visao) {
                    return e;
                }
            }
        }
    }

    fn job_iniciado(&mut self, origem: OrigemJob, tipo: TipoJob) -> JobInfo {
        let EventoAgente::JobIniciado { job } = self.ate("job iniciado", |e, _| {
            matches!(e, EventoAgente::JobIniciado { job } if job.origem == origem && job.tipo == tipo)
        }) else {
            unreachable!()
        };
        job
    }
}

fn esperar(descricao: &str, mut pronto: impl FnMut() -> bool) {
    let fim = Instant::now() + Duration::from_secs(60);
    while !pronto() {
        assert!(Instant::now() < fim, "tempo esgotado: {descricao}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn jobs(socket: &Path) -> Vec<JobInfo> {
    match enviar(socket, Autor::Gui, Comando::ListarJobs) {
        Resposta::Jobs { jobs } => jobs,
        outra => panic!("{outra:?}"),
    }
}

/// Espera a rotina de abertura (no instantâneo ou já terminada) sair do caminho.
fn sem_abertura(socket: &Path) {
    esperar("abertura terminar", || {
        !jobs(socket).iter().any(|j| j.tipo == TipoJob::Abertura)
    });
}

#[test]
fn cliente_gui_ve_o_job_de_outro_cliente_com_progresso_e_o_pausa_retoma_e_cancela() {
    let c = cenario();
    let srv = Servidor::iniciar(config(&c)).unwrap();
    sem_abertura(&c.socket);
    let mut escuta = Escuta::nova(&c.socket);
    assert!(matches!(
        escuta.ate("instantâneo", |e, _| matches!(
            e,
            EventoAgente::Instantaneo { .. }
        )),
        EventoAgente::Instantaneo { .. }
    ));

    // Quem inicia é a CLI; a GUI (assinante) só observa e controla.
    let socket = c.socket.clone();
    let comando = scan(&c, false);
    let cli = std::thread::spawn(move || enviar(&socket, Autor::Cli, comando));

    let job = escuta.job_iniciado(OrigemJob::Cli, TipoJob::FullScan);
    assert_eq!(job.estado, EstadoJob::Executando);
    assert_eq!(job.raizes, vec![c.raiz.to_str().unwrap().to_string()]);
    escuta.ate("progresso do inventário", |e, _| {
        matches!(
            e,
            EventoAgente::JobProgresso { job: j, progresso: Progresso::Inventario(_) } if *j == job.id
        )
    });
    // Qualquer cliente lista o mesmo trabalho, com quem o iniciou.
    let lista = jobs(&c.socket);
    let visto = lista.iter().find(|j| j.id == job.id).unwrap();
    assert_eq!(visto.origem, OrigemJob::Cli);
    assert!(visto.execucao.is_some(), "a execução já é conhecida");

    let ok = |r: Resposta| assert!(matches!(r, Resposta::Ok { .. }), "{r:?}");
    ok(enviar(
        &c.socket,
        Autor::Gui,
        Comando::PausarJob { job: job.id },
    ));
    escuta.ate("job pausado", |e, _| {
        matches!(e, EventoAgente::JobAtualizado { job: j } if j.id == job.id && j.estado == EstadoJob::Pausado)
    });
    ok(enviar(
        &c.socket,
        Autor::Gui,
        Comando::RetomarJob { job: job.id },
    ));
    escuta.ate("job retomado", |e, _| {
        matches!(e, EventoAgente::JobAtualizado { job: j } if j.id == job.id && j.estado == EstadoJob::Executando)
    });
    // Pausar pela execução (o id que a CLI mostra) vale igual.
    let execucao = dag_core::ExecucaoId(visto.execucao.unwrap());
    ok(enviar(
        &c.socket,
        Autor::Mcp,
        Comando::Pausar {
            execucao: Some(execucao),
        },
    ));
    escuta.ate("pausado pela execução", |e, _| {
        matches!(e, EventoAgente::JobAtualizado { job: j } if j.id == job.id && j.estado == EstadoJob::Pausado)
    });
    ok(enviar(
        &c.socket,
        Autor::Gui,
        Comando::CancelarJob { job: job.id },
    ));
    let EventoAgente::JobFinalizado { desfecho, .. } = escuta.ate(
        "job finalizado",
        |e, _| matches!(e, EventoAgente::JobFinalizado { job: j, .. } if j.id == job.id),
    ) else {
        unreachable!()
    };
    assert_eq!(desfecho, "cancelada");
    let Resposta::ScanConcluido { desfecho, .. } = cli.join().unwrap() else {
        panic!("a CLI devia receber ScanConcluido")
    };
    assert_eq!(desfecho, "cancelada");
    assert!(escuta.visao.jobs.is_empty(), "{:?}", escuta.visao.jobs);
    assert!(jobs(&c.socket).is_empty());
    srv.encerrar();
}

#[test]
fn fila_de_jobs_e_cancelamento_de_quem_ainda_nao_comecou() {
    let c = cenario();
    let srv = Servidor::iniciar(config(&c)).unwrap();
    sem_abertura(&c.socket);
    let mut escuta = Escuta::nova(&c.socket);

    let (s1, c1) = (c.socket.clone(), scan(&c, false));
    let primeiro = std::thread::spawn(move || enviar(&s1, Autor::Cli, c1));
    let j1 = escuta.job_iniciado(OrigemJob::Cli, TipoJob::FullScan);

    // O MCP pede outra varredura enquanto a primeira roda: entra na fila.
    let (s2, c2) = (c.socket.clone(), scan(&c, true));
    let segundo = std::thread::spawn(move || enviar(&s2, Autor::Mcp, c2));
    let EventoAgente::JobEnfileirado { job: j2 } = escuta.ate(
        "job enfileirado",
        |e, _| matches!(e, EventoAgente::JobEnfileirado { job } if job.origem == OrigemJob::Mcp),
    ) else {
        unreachable!()
    };
    assert_eq!(j2.estado, EstadoJob::Enfileirado);
    assert_ne!(j1.id, j2.id);
    assert_eq!(jobs(&c.socket).len(), 2);

    // Pausar quem está na fila não faz sentido; cancelar, sim, e quem pediu é avisado.
    let r = enviar(&c.socket, Autor::Gui, Comando::PausarJob { job: j2.id });
    assert!(matches!(r, Resposta::Erro { .. }), "{r:?}");
    let r = enviar(&c.socket, Autor::Gui, Comando::CancelarJob { job: j2.id });
    assert!(matches!(r, Resposta::Ok { .. }), "{r:?}");
    let EventoAgente::JobFinalizado { desfecho, .. } = escuta.ate(
        "fila cancelada",
        |e, _| matches!(e, EventoAgente::JobFinalizado { job, .. } if job.id == j2.id),
    ) else {
        unreachable!()
    };
    assert_eq!(desfecho, "erro:cancelado");
    assert!(
        matches!(segundo.join().unwrap(), Resposta::Erro { codigo, .. } if codigo == "cancelado")
    );

    // O primeiro segue e termina normalmente quando ninguém o cancela.
    let r = enviar(&c.socket, Autor::Gui, Comando::CancelarJob { job: j1.id });
    assert!(matches!(r, Resposta::Ok { .. }), "{r:?}");
    assert!(matches!(
        primeiro.join().unwrap(),
        Resposta::ScanConcluido { desfecho, .. } if desfecho == "cancelada"
    ));
    // O trabalhador não executa o cancelado depois.
    std::thread::sleep(Duration::from_millis(300));
    assert!(jobs(&c.socket).is_empty());
    let r = enviar(&c.socket, Autor::Gui, Comando::CancelarJob { job: 999 });
    assert!(matches!(r, Resposta::Erro { codigo, .. } if codigo == "nao_encontrado"));
    srv.encerrar();
}

#[test]
fn assinante_tardio_recebe_o_instantaneo_do_que_ja_esta_rodando() {
    let c = cenario();
    let srv = Servidor::iniciar(config(&c)).unwrap();
    sem_abertura(&c.socket);
    let (s1, c1) = (c.socket.clone(), scan(&c, false));
    let cli = std::thread::spawn(move || enviar(&s1, Autor::Cli, c1));
    // Só assina depois de o trabalho ter progresso.
    esperar("progresso do job", || {
        jobs(&c.socket)
            .iter()
            .any(|j| j.tipo == TipoJob::FullScan && !j.progresso.is_empty())
    });
    let mut tardio = Escuta::nova(&c.socket);
    let EventoAgente::Instantaneo { jobs, .. } = tardio.ate("instantâneo", |e, _| {
        matches!(e, EventoAgente::Instantaneo { .. })
    }) else {
        unreachable!()
    };
    assert_eq!(tardio.recebidos.len(), 1, "o instantâneo vem primeiro");
    let j = jobs.iter().find(|j| j.tipo == TipoJob::FullScan).unwrap();
    assert_eq!(j.origem, OrigemJob::Cli);
    assert!(matches!(
        j.estado,
        EstadoJob::Executando | EstadoJob::Pausado
    ));
    assert!(!j.progresso.is_empty());
    // E segue recebendo os eventos seguintes.
    tardio.ate(
        "progresso depois do instantâneo",
        |e, _| matches!(e, EventoAgente::JobProgresso { job, .. } if *job == j.id),
    );
    enviar(&c.socket, Autor::Gui, Comando::CancelarJob { job: j.id });
    tardio.ate("fim", |e, _| {
        matches!(e, EventoAgente::JobFinalizado { .. })
    });
    cli.join().unwrap();
    srv.encerrar();
}

#[test]
fn escrita_do_mcp_funciona_com_a_gui_conectada_e_ambos_veem_o_mesmo_agente() {
    let c = cenario();
    let srv = Servidor::iniciar(config(&c)).unwrap();
    sem_abertura(&c.socket);
    // A "GUI" fica conectada assinando.
    let mut gui = Escuta::nova(&c.socket);
    let r = enviar(&c.socket, Autor::Gui, scan(&c, false));
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    // O MCP escreve no Catálogo pelo mesmo agente (um só escritor).
    let alvo = Alvo::Caminho(c.raiz.join("big.pdf").to_str().unwrap().into());
    let r = enviar(&c.socket, Autor::Mcp, Comando::MarcarBackup { alvo });
    assert!(matches!(r, Resposta::Marcacao { .. }), "{r:?}");
    let r = enviar(
        &c.socket,
        Autor::Mcp,
        Comando::ConfigHash {
            threads_por_disco: Some(2),
            max_discos: Some(3),
        },
    );
    assert!(matches!(r, Resposta::ConfigHash { .. }), "{r:?}");
    // A GUI viu o scan da própria GUI terminar, com a origem certa.
    let EventoAgente::JobFinalizado { job, .. } = gui.ate("scan da gui", |e, _| {
        matches!(e, EventoAgente::JobFinalizado { job, .. } if job.tipo == TipoJob::FullScan)
    }) else {
        unreachable!()
    };
    assert_eq!(job.origem, OrigemJob::Gui);
    srv.encerrar();
}

#[test]
fn a_rotina_de_abertura_e_um_job_visivel_a_quem_assina() {
    let c = cenario();
    {
        let mut a = dag_agent::Agente::abrir(&c.catalogo)
            .unwrap()
            .silencioso()
            .com_volumes_fixos(vec![volume_de(&c.raiz)]);
        let r = a.executar(scan(&c, false));
        assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    }
    // A abertura verifica as Raízes (Fast Scan de milhares de arquivos): dá tempo de assinar.
    let srv = Servidor::iniciar(config(&c)).unwrap();
    let mut escuta = Escuta::nova(&c.socket);
    let EventoAgente::Instantaneo { jobs, .. } = escuta.ate("instantâneo", |e, _| {
        matches!(e, EventoAgente::Instantaneo { .. })
    }) else {
        unreachable!()
    };
    let abertura = jobs.iter().find(|j| j.tipo == TipoJob::Abertura).unwrap();
    assert_eq!(abertura.origem, OrigemJob::Abertura);
    let EventoAgente::JobFinalizado { job, .. } = escuta.ate("fim da abertura", |e, _| {
        matches!(e, EventoAgente::JobFinalizado { job, .. } if job.tipo == TipoJob::Abertura)
    }) else {
        unreachable!()
    };
    assert_eq!(job.id, abertura.id);
    // Passos da abertura chegaram como etapa/progresso do job.
    assert!(
        escuta.recebidos.iter().any(|e| matches!(
            e,
            EventoAgente::JobProgresso { job: j, .. } if *j == abertura.id
        )),
        "{:#?}",
        escuta.recebidos
    );
    srv.encerrar();
}

#[test]
fn mudanca_no_ponto_de_confianca_vira_evento() {
    let c = cenario();
    let srv = Servidor::iniciar(config(&c)).unwrap();
    sem_abertura(&c.socket);
    let r = enviar(&c.socket, Autor::Cli, scan(&c, true));
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    let mut escuta = Escuta::nova(&c.socket);
    escuta.ate("instantâneo", |e, _| {
        matches!(e, EventoAgente::Instantaneo { .. })
    });
    // Outra conexão ao Catálogo (leitor WAL ao lado do escritor) muda a confiança; o agente
    // percebe na próxima leitura periódica e avisa.
    {
        let cat = dag_store::Catalogo::abrir(&c.catalogo).unwrap();
        dag_store::confianca::marcar_confiavel(
            cat.armazem().conexao(),
            1,
            c.raiz.as_os_str().as_bytes(),
            1_700_000_000,
        )
        .unwrap();
    }
    escuta.ate("ponto de confiança", |e, _| {
        matches!(e, EventoAgente::PontoConfianca { confianca }
            if confianca.iter().any(|r| r.confiavel_em == Some(1_700_000_000)))
    });
    assert!(
        escuta.visao.monitor.is_some(),
        "o instantâneo traz o monitor"
    );
    srv.encerrar();
}

#[test]
fn observador_reconecta_quando_o_agente_reinicia() {
    let c = cenario();
    let srv = Servidor::iniciar(config(&c)).unwrap();
    let obs = Observador::iniciar(
        OpcoesObservador {
            intervalo: Duration::from_millis(50),
            ..OpcoesObservador::novo(c.catalogo.clone(), c.socket.clone())
        },
        None,
    );
    let esperar_obs = |pred: &dyn Fn(&EventoObservado) -> bool, desc: &str| {
        let fim = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < fim, "tempo esgotado: {desc}");
            if let Some(e) = obs.proximo(Duration::from_millis(100))
                && pred(&e)
            {
                return e;
            }
        }
    };
    let pid = std::process::id();
    let EventoObservado::Conectado(info) =
        esperar_obs(&|e| matches!(e, EventoObservado::Conectado(_)), "conectar")
    else {
        unreachable!()
    };
    assert_eq!(info.pid, pid, "o agente do teste roda neste processo");
    esperar_obs(
        &|e| matches!(e, EventoObservado::Evento(EventoAgente::Instantaneo { .. })),
        "instantâneo",
    );

    // O agente cai (como num `kill`): o observador avisa e segue tentando.
    srv.encerrar();
    esperar_obs(
        &|e| matches!(e, EventoObservado::Desconectado { .. }),
        "desconexão",
    );
    // O agente volta e o observador reconecta sozinho, com um instantâneo novo.
    let srv = Servidor::iniciar(config(&c)).unwrap();
    esperar_obs(
        &|e| matches!(e, EventoObservado::Conectado(_)),
        "reconectar",
    );
    esperar_obs(
        &|e| matches!(e, EventoObservado::Evento(EventoAgente::Instantaneo { .. })),
        "novo instantâneo",
    );
    obs.parar();
    srv.encerrar();
}
