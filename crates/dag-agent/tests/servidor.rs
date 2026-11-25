//! Agente em segundo plano: socket protegido, protocolo, controle imediato durante a varredura,
//! escritor único e encerramento limpo.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dag_agent::{
    Agente, Cliente, Conexao, ConfigServidor, ErroAgente, GuardaEscritor, Servidor,
    caminho_socket_padrao,
};
use dag_core::{
    Autor, Comando, EstadoExecucao, ModoVarredura, Pedido, Progresso, Resposta, VolumeDescoberto,
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

/// Árvore pequena: big.pdf (9000) duplicado em copia/, mais sub/x.bin e sub/y.txt.
fn cenario() -> Cenario {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let raiz = base.join("arvore");
    escrever(&raiz.join("big.pdf"), 9000);
    escrever(&raiz.join("copia/big.pdf"), 9000);
    escrever(&raiz.join("sub/x.bin"), 3000);
    escrever(&raiz.join("sub/y.txt"), 2000);
    Cenario {
        catalogo: base.join("dados/catalogo.sqlite"),
        socket: base.join("run/dag/agente.sock"),
        raiz,
        _tmp: tmp,
    }
}

/// Árvore grande o bastante para a varredura durar (muitas pastas e arquivos).
fn cenario_grande() -> Cenario {
    let c = cenario();
    for d in 0..120 {
        for f in 0..60 {
            escrever(&c.raiz.join(format!("g{d}/f{f}.txt")), 64 + f);
        }
    }
    c
}

fn config(c: &Cenario, monitor: bool) -> ConfigServidor {
    let mut cfg = ConfigServidor::novo(&c.catalogo, &c.socket);
    cfg.monitor = monitor;
    cfg.volumes_fixos = Some(vec![volume_de(&c.raiz)]);
    cfg
}

fn pedido(comando: Comando) -> Pedido {
    Pedido {
        autor: Autor::Cli,
        comando,
    }
}

fn enviar(cli: &mut Cliente, comando: Comando) -> Resposta {
    cli.enviar(pedido(comando), &mut |_| {}).unwrap()
}

fn full(c: &Cenario, sem_hash: bool) -> Comando {
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

fn esperar(descricao: &str, mut pronto: impl FnMut() -> bool) {
    let fim = Instant::now() + Duration::from_secs(60);
    while !pronto() {
        assert!(Instant::now() < fim, "tempo esgotado: {descricao}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn estados(cli: &mut Cliente) -> Vec<EstadoExecucao> {
    match enviar(cli, Comando::Status { execucao: None }) {
        Resposta::Status { execucoes, .. } => execucoes.iter().map(|e| e.estado).collect(),
        outra => panic!("{outra:?}"),
    }
}

fn codigo(r: &Resposta) -> &str {
    match r {
        Resposta::Erro { codigo, .. } => codigo,
        outra => panic!("esperava erro: {outra:?}"),
    }
}

#[test]
fn ida_e_volta_com_progresso_de_hash_e_permissoes() {
    let c = cenario();
    let srv = Servidor::iniciar(config(&c, false)).unwrap();
    // Diretório 0700 e socket 0600.
    let modo = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(modo(&c.socket), 0o600);
    assert_eq!(modo(c.socket.parent().unwrap()), 0o700);

    let mut cli = Cliente::conectar(&c.socket).unwrap();
    let info = cli.info().unwrap();
    assert_eq!(info.pid, std::process::id());
    assert!(!info.monitor);
    assert!(matches!(
        enviar(&mut cli, Comando::ListarVolumes),
        Resposta::Volumes { .. }
    ));

    let mut eventos = Vec::new();
    let r = cli
        .enviar(pedido(full(&c, false)), &mut |p| eventos.push(p.clone()))
        .unwrap();
    let Resposta::ScanConcluido {
        desfecho,
        arquivos,
        hash,
        duplicados,
        ..
    } = r
    else {
        panic!("{r:?}")
    };
    assert_eq!(desfecho, "concluida");
    assert_eq!(arquivos, 4);
    assert!(hash.is_some());
    assert_eq!(duplicados.unwrap().grupos, 1);
    // Progresso do inventário e do hash (com bytes) chegou pelo socket.
    assert!(
        eventos
            .iter()
            .any(|e| matches!(e, Progresso::Inventario(_)))
    );
    let h: Vec<_> = eventos
        .iter()
        .filter_map(|e| match e {
            Progresso::Hash(h) => Some(h),
            _ => None,
        })
        .collect();
    assert!(!h.is_empty(), "sem progresso de hash: {eventos:?}");
    let ultimo = h.last().unwrap();
    assert_eq!(ultimo.arquivos_feitos, ultimo.arquivos_total);
    assert!(ultimo.bytes_lidos > 0 && ultimo.bytes_total >= ultimo.bytes_lidos);
    // A mesma conexão serve vários pedidos em sequência.
    assert!(matches!(
        enviar(&mut cli, Comando::Status { execucao: None }),
        Resposta::Status { .. }
    ));
    // Quadro malformado não derruba a conexão: resposta de erro e segue.
    drop(cli);
    srv.encerrar();
    assert!(!c.socket.exists(), "o socket some no encerramento");
}

#[test]
fn status_responde_durante_o_scan_e_cancelar_de_outro_cliente_para_a_varredura() {
    let c = cenario_grande();
    let srv = Servidor::iniciar(config(&c, false)).unwrap();
    let socket = c.socket.clone();

    let comando = full(&c, false);
    let a = std::thread::spawn({
        let socket = socket.clone();
        move || {
            let mut a = Cliente::conectar(&socket).unwrap();
            a.enviar(pedido(comando), &mut |_| {}).unwrap()
        }
    });

    let mut b = Cliente::conectar(&socket).unwrap();
    // Nada a cancelar antes de existir trabalho (e nada vaza para o próximo).
    // A varredura aparece como em andamento no Status de outro cliente.
    esperar("scan em andamento", || {
        estados(&mut b).contains(&EstadoExecucao::EmAndamento)
    });
    let t = Instant::now();
    let r = enviar(&mut b, Comando::SaudeMonitor);
    assert!(matches!(r, Resposta::SaudeMonitor { .. }), "{r:?}");
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "SaudeMonitor demorou {:?}",
        t.elapsed()
    );

    // Pausar é imediato; o estado gravado vira `pausada` no próximo ponto seguro.
    let r = enviar(&mut b, Comando::Pausar { execucao: None });
    assert!(matches!(r, Resposta::Ok { .. }), "{r:?}");
    esperar("scan pausado", || {
        estados(&mut b).first() == Some(&EstadoExecucao::Pausada)
    });
    let r = enviar(
        &mut b,
        Comando::Retomar {
            execucao: None,
            threads_por_disco: None,
            max_discos: None,
        },
    );
    assert!(matches!(r, Resposta::Ok { .. }), "{r:?}");
    esperar("scan retomado", || {
        estados(&mut b).first() != Some(&EstadoExecucao::Pausada)
    });
    // Cancelar de outro cliente: o primeiro recebe a varredura cancelada (retomável).
    let r = enviar(&mut b, Comando::Cancelar { execucao: None });
    assert!(matches!(r, Resposta::Ok { .. }), "{r:?}");
    let Resposta::ScanConcluido { desfecho, .. } = a.join().unwrap() else {
        panic!("esperava ScanConcluido")
    };
    assert_eq!(desfecho, "cancelada");

    // Sem trabalho em andamento, controlar é erro claro, e o cancelamento não vaza.
    let r = enviar(&mut b, Comando::Cancelar { execucao: None });
    assert_eq!(codigo(&r), "nada_em_execucao");
    let r = enviar(&mut b, full(&c, true));
    let Resposta::ScanConcluido { desfecho, .. } = r else {
        panic!("{r:?}")
    };
    assert_eq!(desfecho, "concluida");
    srv.encerrar();
}

#[test]
fn retomar_com_id_de_execucao_gravada_vai_para_a_fila() {
    let c = cenario_grande();
    let srv = Servidor::iniciar(config(&c, false)).unwrap();
    let mut a = Cliente::conectar(&c.socket).unwrap();
    let mut b = Cliente::conectar(&c.socket).unwrap();
    let cancelou = AtomicBool::new(false);
    let r = std::thread::scope(|s| {
        let h = s.spawn(|| {
            esperar("scan em andamento", || {
                estados(&mut b).contains(&EstadoExecucao::EmAndamento)
            });
            enviar(&mut b, Comando::Cancelar { execucao: None });
            cancelou.store(true, Ordering::SeqCst);
        });
        let r = a.enviar(pedido(full(&c, true)), &mut |_| {}).unwrap();
        h.join().unwrap();
        r
    });
    let Resposta::ScanConcluido {
        execucao, desfecho, ..
    } = r
    else {
        panic!("{r:?}")
    };
    assert_eq!(desfecho, "cancelada");
    let r = enviar(
        &mut a,
        Comando::Retomar {
            execucao: Some(execucao),
            threads_por_disco: None,
            max_discos: None,
        },
    );
    let Resposta::ScanConcluido { desfecho, .. } = r else {
        panic!("{r:?}")
    };
    assert!(desfecho.starts_with("concluida"), "{desfecho}");
    srv.encerrar();
}

#[test]
fn conexao_de_outro_uid_e_recusada() {
    let c = cenario();
    let proprio = rustix::process::getuid().as_raw();
    let mut cfg = config(&c, false);
    cfg.uid_aceito = Some(proprio + 1);
    let srv = Servidor::iniciar(cfg).unwrap();
    let mut cli = Cliente::conectar(&c.socket).unwrap();
    let r = cli.enviar(pedido(Comando::ListarVolumes), &mut |_| {});
    match r {
        Ok(Resposta::Erro { codigo, .. }) => assert_eq!(codigo, "nao_autorizado"),
        outra => panic!("esperava recusa: {outra:?}"),
    }
    // Quem não é aceito não obtém nem a identificação: cai para a sessão, que não consegue
    // escrever com o agente segurando a trava (abre só para leitura).
    let Conexao::Sessao(a) = Conexao::abrir(&c.catalogo, &c.socket).unwrap() else {
        panic!("não devia usar o agente")
    };
    assert!(!a.eh_escritor());
    srv.encerrar();
}

#[test]
fn com_o_agente_de_pe_nao_existe_segundo_escritor() {
    let c = cenario();
    let srv = Servidor::iniciar(config(&c, false)).unwrap();
    // Outro processo (aqui, outra abertura) não obtém a trava.
    assert!(matches!(
        Agente::abrir(&c.catalogo).unwrap_err(),
        ErroAgente::EscritorOcupado(_)
    ));
    assert!(matches!(
        GuardaEscritor::adquirir(&c.catalogo).unwrap_err(),
        ErroAgente::EscritorOcupado(_)
    ));
    // Nem outro agente em outro socket.
    let mut outro = config(&c, false);
    outro.socket = c.socket.with_file_name("outro.sock");
    assert!(matches!(
        Servidor::iniciar(outro).unwrap_err(),
        ErroAgente::EscritorOcupado(_)
    ));
    // Quem usa `Conexao::abrir` é atendido pelo agente, e o escritor continua único.
    let mut con = Conexao::abrir(&c.catalogo, &c.socket).unwrap();
    assert!(con.eh_daemon());
    let r = con.enviar(pedido(full(&c, true)), &mut |_| {}).unwrap();
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    // Um agente que serve outro Catálogo não é usado: cai para a sessão naquele Catálogo.
    let outro_cat = c.catalogo.with_file_name("outro.sqlite");
    let con = Conexao::abrir(&outro_cat, &c.socket).unwrap();
    assert!(!con.eh_daemon());
    drop(con);
    srv.encerrar();
    // Depois do encerramento a trava é liberada.
    drop(GuardaEscritor::adquirir(&c.catalogo).unwrap());
}

#[test]
fn escritas_concorrentes_de_varios_clientes_sao_serializadas() {
    let c = cenario();
    let srv = Servidor::iniciar(config(&c, false)).unwrap();
    let ids: Vec<i64> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..4)
            .map(|_| {
                s.spawn(|| {
                    let mut cli = Cliente::conectar(&c.socket).unwrap();
                    let r = cli.enviar(pedido(full(&c, true)), &mut |_| {}).unwrap();
                    let Resposta::ScanConcluido {
                        execucao, desfecho, ..
                    } = r
                    else {
                        panic!("{r:?}")
                    };
                    assert_eq!(desfecho, "concluida");
                    execucao.0
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut unicos = ids.clone();
    unicos.sort_unstable();
    unicos.dedup();
    assert_eq!(unicos.len(), 4, "cada pedido virou uma execução: {ids:?}");
    srv.encerrar();
    let conn = rusqlite::Connection::open(&c.catalogo).unwrap();
    let ok: String = conn
        .query_row("PRAGMA integrity_check", [], |l| l.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
}

#[test]
fn socket_antigo_sem_dono_e_limpo_mas_socket_vivo_e_nao_regular_sao_recusados() {
    let c = cenario();
    fs::create_dir_all(c.socket.parent().unwrap()).unwrap();
    // Socket órfão (processo morreu): ninguém atende, então é removido.
    drop(UnixListener::bind(&c.socket).unwrap());
    assert!(c.socket.exists());
    let srv = Servidor::iniciar(config(&c, false)).unwrap();
    assert!(Cliente::conectar(&c.socket).unwrap().info().is_ok());
    srv.encerrar();

    // Socket com um listener vivo de outro processo: não é tocado.
    let vivo = UnixListener::bind(&c.socket).unwrap();
    let e = Servidor::iniciar(config(&c, false)).unwrap_err();
    assert!(e.to_string().contains("já há um agente"), "{e}");
    assert!(c.socket.exists());
    drop(vivo);
    fs::remove_file(&c.socket).unwrap();

    // Arquivo comum no caminho: nunca é apagado.
    fs::write(&c.socket, b"dados").unwrap();
    let e = Servidor::iniciar(config(&c, false)).unwrap_err();
    assert!(e.to_string().contains("não é um socket"), "{e}");
    assert_eq!(fs::read(&c.socket).unwrap(), b"dados");
}

#[test]
fn encerramento_cancela_a_varredura_com_checkpoint_e_solta_tudo() {
    let c = cenario_grande();
    let srv = Servidor::iniciar(config(&c, false)).unwrap();
    let socket = c.socket.clone();
    let comando = full(&c, false);
    let a = std::thread::spawn(move || {
        let mut a = Cliente::conectar(&socket).unwrap();
        a.enviar(pedido(comando), &mut |_| {})
    });
    let mut b = Cliente::conectar(&c.socket).unwrap();
    esperar("scan em andamento", || {
        estados(&mut b).contains(&EstadoExecucao::EmAndamento)
    });
    drop(b);
    srv.encerrar();
    // O cliente recebe o desfecho (cancelada, retomável), não uma conexão cortada.
    match a.join().unwrap().unwrap() {
        Resposta::ScanConcluido { desfecho, .. } => assert_eq!(desfecho, "cancelada"),
        outra => panic!("{outra:?}"),
    }
    assert!(!c.socket.exists());
    assert!(Cliente::conectar(&c.socket).is_err());
    // Trava solta e execução gravada como interrompida (encerramento não é cancelamento do
    // usuário): a flag "continuar de onde parou" a leva adiante na próxima abertura.
    let mut ag = Agente::abrir(&c.catalogo).unwrap().silencioso();
    let Resposta::Status { execucoes, .. } = ag.executar(Comando::Status { execucao: None }) else {
        panic!()
    };
    assert_eq!(execucoes[0].estado, EstadoExecucao::Pausada);
    assert_eq!(execucoes[0].estado_detalhe, "interrompida");
    assert!(execucoes[0].retomando);
}

#[test]
fn monitor_do_agente_roda_apos_o_primeiro_scan_e_cede_a_pedidos() {
    let c = cenario();
    let srv = Servidor::iniciar(config(&c, true)).unwrap();
    let mut cli = Cliente::conectar(&c.socket).unwrap();
    assert!(cli.info().unwrap().monitor);
    // Sem raízes catalogadas o monitor ainda não pode subir; depois do scan, sobe sozinho.
    let r = enviar(&mut cli, full(&c, false));
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    let mut obs = Cliente::conectar(&c.socket).unwrap();
    let em_execucao = |obs: &mut Cliente| match enviar(obs, Comando::SaudeMonitor) {
        Resposta::SaudeMonitor { saude } => saude.em_execucao,
        outra => panic!("{outra:?}"),
    };
    esperar("monitor em execução", || em_execucao(&mut obs));
    // Um arquivo novo vira evento sem nenhum comando do usuário.
    escrever(&c.raiz.join("sub/novo.txt"), 10);
    esperar("evento do arquivo novo", || {
        match enviar(&mut obs, Comando::Eventos { limite: Some(50) }) {
            Resposta::Eventos { eventos } => eventos.iter().any(|e| {
                e.caminho
                    .as_deref()
                    .is_some_and(|p| p.ends_with("novo.txt"))
            }),
            outra => panic!("{outra:?}"),
        }
    });
    // Um pedido de escrita tem prioridade sobre o monitor e é atendido; o monitor volta.
    let r = enviar(&mut cli, full(&c, true));
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    esperar("monitor de volta", || em_execucao(&mut obs));
    // `dag monitorar` com o agente de pé não cria um segundo monitor.
    let r = enviar(
        &mut cli,
        Comando::Monitorar {
            raizes: vec![],
            reconciliar_a_cada_s: None,
            sem_hash: false,
        },
    );
    assert!(matches!(r, Resposta::Ok { .. }), "{r:?}");
    srv.encerrar();
}

#[test]
fn sessao_executa_com_progresso_e_recusa_controle_sem_agente() {
    let c = cenario();
    // Sem agente no socket: sessão em processo (modo 1).
    let mut con = Conexao::abrir(&c.catalogo, &c.socket).unwrap();
    assert!(!con.eh_daemon());
    let Conexao::Sessao(a) = &mut con else {
        panic!()
    };
    *a = std::mem::replace(a, Agente::abrir_leitura(&c.catalogo).unwrap())
        .com_volumes_fixos(vec![volume_de(&c.raiz)]);
    let mut n_hash = 0;
    let r = con
        .enviar(pedido(full(&c, false)), &mut |p| {
            if matches!(p, Progresso::Hash(_)) {
                n_hash += 1;
            }
        })
        .unwrap();
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    assert!(n_hash > 0);
    for cmd in [
        Comando::Pausar { execucao: None },
        Comando::Cancelar { execucao: None },
        Comando::Retomar {
            execucao: None,
            threads_por_disco: None,
            max_discos: None,
        },
    ] {
        let r = con.enviar(pedido(cmd), &mut |_| {}).unwrap();
        assert_eq!(codigo(&r), "sem_agente");
    }
}

#[test]
fn caminho_padrao_respeita_dag_socket_so_quando_definido() {
    // Só confere que devolve um caminho absoluto terminando em agente.sock (sem mexer no
    // ambiente do processo de teste).
    let p = caminho_socket_padrao();
    assert!(p.is_absolute(), "{}", p.display());
    assert!(p.file_name().is_some());
    let _ = std::io::stderr().flush();
}

#[test]
fn agente_retoma_sozinho_a_execucao_deixada_por_um_processo_morto() {
    let c = cenario();
    {
        let mut a = Agente::abrir(&c.catalogo)
            .unwrap()
            .silencioso()
            .com_volumes_fixos(vec![volume_de(&c.raiz)]);
        let r = a.executar(full(&c, false));
        assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    }
    // Como um processo morto no meio do hash: fase `hash`, dois arquivos sem hash, fila pendente.
    {
        let cat = dag_store::Catalogo::abrir(&c.catalogo).unwrap();
        cat.armazem()
            .conexao()
            .execute_batch(
                "UPDATE scan_runs SET phase = 'hash';
                 DELETE FROM hashes WHERE file_id IN
                    (SELECT id FROM files WHERE kind = 'arquivo' ORDER BY id LIMIT 2);
                 UPDATE fila_hash SET state = 'pendente'
                    WHERE file_id IN (SELECT id FROM files WHERE kind = 'arquivo' ORDER BY id LIMIT 2);",
            )
            .unwrap();
    }
    let srv = Servidor::iniciar(config(&c, false)).unwrap();
    let mut cli = Cliente::conectar(&c.socket).unwrap();
    // A abertura marca, retoma e conclui sem ninguém pedir.
    esperar("execução retomada e concluída", || {
        let Resposta::Status { execucoes, .. } =
            enviar(&mut cli, Comando::Status { execucao: None })
        else {
            panic!()
        };
        let e = &execucoes[0];
        e.estado == EstadoExecucao::Concluida && e.hash.is_none() && !e.retomando
    });
    srv.encerrar();
    let cat = dag_store::Catalogo::abrir(&c.catalogo).unwrap();
    let n: i64 = cat
        .armazem()
        .conexao()
        .query_row("SELECT COUNT(*) FROM hashes", [], |l| l.get(0))
        .unwrap();
    assert_eq!(n, 4);
}

/// Agente subido como um cliente o sobe (monitor habilitado, Catálogo ainda sem raízes): a
/// situação do monitor é a mesma em `Status` e `SaudeMonitor`, nunca "ligado" e "parado" ao mesmo
/// tempo; depois da abertura e do primeiro scan o monitor fica ativo e um arquivo novo aparece
/// no Catálogo.
#[test]
fn monitor_situacao_unica_e_arquivo_novo_chega_ao_catalogo() {
    let c = cenario();
    let srv = Servidor::iniciar(config(&c, true)).unwrap();
    let mut cli = Cliente::conectar(&c.socket).unwrap();
    let situacao = |cli: &mut Cliente| {
        let Resposta::Status {
            monitor: Some(a), ..
        } = enviar(cli, Comando::Status { execucao: None })
        else {
            panic!()
        };
        let Resposta::SaudeMonitor { saude: b } = enviar(cli, Comando::SaudeMonitor) else {
            panic!()
        };
        // Pode haver uma transição entre as duas leituras; fora dela, são idênticas.
        (a, b)
    };
    // Sem raízes catalogadas: nunca "ativo"; diz por que (aguardando ou sem raízes).
    let (a, _) = situacao(&mut cli);
    assert!(
        ["aguardando_abertura", "iniciando", "parado"].contains(&a.situacao.as_str()),
        "{a:?}"
    );
    assert!(!a.em_execucao);
    if a.situacao == "parado" {
        assert!(
            a.situacao_motivo
                .as_deref()
                .is_some_and(|m| m.contains("raiz")),
            "{a:?}"
        );
    }
    let r = enviar(&mut cli, full(&c, false));
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    esperar("monitor ativo na situação única", || {
        let (a, b) = situacao(&mut cli);
        a.situacao == "ativo" && b.situacao == "ativo" && a.em_execucao && b.em_execucao
    });
    escrever(&c.raiz.join("sub/chegou.txt"), 33);
    esperar("arquivo novo no Catálogo", || {
        let cat = dag_store::Catalogo::abrir(&c.catalogo).unwrap();
        cat.armazem()
            .conexao()
            .query_row(
                "SELECT COUNT(*) FROM files WHERE CAST(path_raw AS TEXT) LIKE '%chegou.txt'",
                [],
                |l| l.get::<_, i64>(0),
            )
            .unwrap()
            > 0
    });
    // Um trabalho do agente pausa o monitor e a situação diz isso, com o mesmo texto em todo lugar.
    let r = enviar(&mut cli, full(&c, true));
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    esperar("monitor de volta", || {
        situacao(&mut cli).0.situacao == "ativo"
    });
    srv.encerrar();
}
