#![allow(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use dag_platform_linux::servico::{
    Atividade, ConfigUnidade, ErroServico, ExecutorSistema, GerenciadorServico, ModoServico,
    NOME_UNIDADE, SaidaProcesso, de_show, diretorio_unidades, gerar_unidade, parse_linger,
};

/// Executor falso: registra as chamadas e responde por prefixo de comando.
#[derive(Default)]
struct Falso {
    chamadas: Mutex<Vec<String>>,
    respostas: Mutex<Vec<(String, VecDeque<io::Result<SaidaProcesso>>)>>,
}

impl Falso {
    fn responder(&self, prefixo: &str, codigo: i32, stdout: &str, stderr: &str) {
        let r = Ok(SaidaProcesso {
            codigo: Some(codigo),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        });
        self.respostas
            .lock()
            .unwrap()
            .push((prefixo.to_string(), VecDeque::from([r])));
    }
    fn nao_existe(&self, prefixo: &str) {
        self.respostas.lock().unwrap().push((
            prefixo.to_string(),
            VecDeque::from([Err(io::Error::from(io::ErrorKind::NotFound))]),
        ));
    }
    fn chamadas(&self) -> Vec<String> {
        self.chamadas.lock().unwrap().clone()
    }
}

impl ExecutorSistema for Falso {
    fn executar(
        &self,
        programa: &str,
        args: &[&str],
        stdin: Option<&[u8]>,
    ) -> io::Result<SaidaProcesso> {
        assert!(stdin.is_none());
        let linha = format!("{programa} {}", args.join(" "));
        self.chamadas.lock().unwrap().push(linha.clone());
        let mut rs = self.respostas.lock().unwrap();
        for (p, fila) in rs.iter_mut() {
            if linha.starts_with(p.as_str()) {
                return if fila.len() > 1 {
                    fila.pop_front().unwrap()
                } else {
                    match fila.front().unwrap() {
                        Ok(s) => Ok(s.clone()),
                        Err(e) => Err(io::Error::from(e.kind())),
                    }
                };
            }
        }
        // Padrão: tudo dá certo, sem saída.
        Ok(SaidaProcesso {
            codigo: Some(0),
            ..Default::default()
        })
    }
}

struct Ambiente {
    _t: tempfile::TempDir,
    falso: Arc<Falso>,
    g: GerenciadorServico,
    dir_unidades: PathBuf,
    dados: PathBuf,
}

fn ambiente() -> Ambiente {
    let t = tempfile::tempdir().unwrap();
    let dir_unidades = t.path().join("config/systemd/user");
    let dados = t.path().join("dados/dag");
    let falso = Arc::new(Falso::default());
    let g = GerenciadorServico::novo(
        falso.clone(),
        ConfigUnidade {
            binario: PathBuf::from("/opt/dag/bin/dag"),
            dirs_gravaveis: vec![dados.clone()],
            catalogo: None,
        },
        dir_unidades.clone(),
        1000,
    );
    Ambiente {
        _t: t,
        falso,
        g,
        dir_unidades,
        dados,
    }
}

const SHOW_ATIVO: &str = "LoadState=loaded\nActiveState=active\nSubState=running\nUnitFileState=enabled\nMainPID=4242\nActiveEnterTimestamp=Mon 2026-11-23 10:00:00 UTC\n";

#[test]
fn instalacao_limpa_nao_tem_servico() {
    let a = ambiente();
    a.falso
        .responder("loginctl show-user", 0, "Linger=no\n", "");
    let e = a.g.estado().unwrap();
    assert!(!e.instalado);
    assert!(!e.autostart);
    assert_eq!(e.atividade, Atividade::Inativo);
    assert_eq!(e.linger, Some(false));
    assert_eq!(e.modo_desejado(), ModoServico::Parado);
    assert_eq!(e.modo_atual(), ModoServico::Parado);
    assert!(e.descrever().contains("não instalado"));
    // nem sequer consultou o systemctl nem criou nada
    assert!(a.falso.chamadas().iter().all(|c| c.starts_with("loginctl")));
    assert!(!a.dir_unidades.exists());
    assert!(!a.dados.exists());
}

#[test]
fn estado_sem_loginctl_nem_systemd_ainda_diz_nao_instalado() {
    let a = ambiente();
    a.falso.nao_existe("loginctl");
    a.falso.nao_existe("systemctl");
    let e = a.g.estado().unwrap();
    assert!(!e.instalado);
    assert_eq!(e.linger, None);
}

#[test]
fn instalar_escreve_unidade_sem_habilitar() {
    let a = ambiente();
    let msg = a.g.instalar().unwrap();
    assert!(msg.contains("NÃO habilitado"));
    let u = std::fs::read_to_string(a.dir_unidades.join(NOME_UNIDADE)).unwrap();
    assert!(u.contains("ExecStart=\"/opt/dag/bin/dag\" agente"));
    assert!(a.dados.is_dir(), "diretório de dados criado");
    let c = a.falso.chamadas();
    assert_eq!(c, vec!["systemctl --user daemon-reload"]);
    assert!(
        c.iter()
            .all(|x| !x.contains("enable") && !x.contains("start")),
        "{c:?}"
    );
    // idempotente
    a.g.instalar().unwrap();
}

#[test]
fn conteudo_da_unidade() {
    let u = gerar_unidade(&ConfigUnidade {
        binario: PathBuf::from("/usr/bin/dag"),
        dirs_gravaveis: vec![
            PathBuf::from("/home/u/.local/share/dag"),
            PathBuf::from("/home/u/dir com espaço"),
            PathBuf::from("/run/user/1000/dag"),
        ],
        catalogo: Some(PathBuf::from("/home/u/cat%.sqlite")),
    })
    .unwrap();
    for esperado in [
        "[Service]",
        "ExecStart=\"/usr/bin/dag\" --catalogo \"/home/u/cat%%.sqlite\" agente\n",
        "TimeoutStopSec=120",
        "Restart=on-failure",
        "NoNewPrivileges=yes",
        "ProtectSystem=full",
        "PrivateTmp=yes",
        "RuntimeDirectory=dag\n",
        "Nice=10",
        "IOSchedulingClass=idle",
        "ReadWritePaths=\"/home/u/.local/share/dag\" \"/home/u/dir com espaço\" \"/run/user/1000/dag\"\n",
        "WantedBy=default.target",
    ] {
        assert!(u.contains(esperado), "falta `{esperado}` em:\n{u}");
    }
    // ADR 0005: o agente precisa mover (rename) dentro dos Volumes catalogados.
    assert!(!u.contains("ProtectHome"), "{u}");
    assert!(!u.contains("ProtectSystem=strict"), "{u}");
    assert!(!u.contains("somente leitura sobre o acervo"), "{u}");
    assert!(u.contains("nunca apaga"), "{u}");
    // Instalar não habilita: não há símbolos de enable no texto, e a unidade é de usuário.
    assert!(!u.contains("User="));
    assert!(!u.contains("multi-user.target"));
}

#[test]
fn unidade_recusa_entradas_invalidas() {
    let base = ConfigUnidade {
        binario: PathBuf::from("dag"),
        dirs_gravaveis: vec![],
        catalogo: None,
    };
    assert!(gerar_unidade(&base).is_err(), "binário relativo");
    let c = ConfigUnidade {
        binario: PathBuf::from("/usr/bin/dag\nExecStartPre=/bin/evil"),
        ..base.clone()
    };
    assert!(gerar_unidade(&c).is_err(), "quebra de linha");
    let c = ConfigUnidade {
        binario: PathBuf::from("/usr/bin/dag"),
        dirs_gravaveis: vec![PathBuf::from("relativo")],
        catalogo: None,
    };
    assert!(gerar_unidade(&c).is_err());
}

#[test]
fn acoes_exigem_instalacao() {
    let a = ambiente();
    for r in [
        a.g.iniciar(),
        a.g.parar(),
        a.g.reiniciar(),
        a.g.autostart(true),
        a.g.autostart(false),
    ] {
        assert!(matches!(r, Err(ErroServico::NaoInstalado)));
    }
    assert!(a.falso.chamadas().is_empty());
}

#[test]
fn iniciar_parar_reiniciar_autostart() {
    let a = ambiente();
    a.g.instalar().unwrap();
    a.falso.respostas.lock().unwrap().clear();
    a.falso
        .responder("loginctl show-user", 0, "Linger=no\n", "");
    a.g.iniciar().unwrap();
    a.g.parar().unwrap();
    a.g.reiniciar().unwrap();
    let m = a.g.autostart(true).unwrap();
    assert!(m.contains("linger"), "avisa do linger: {m}");
    a.g.autostart(false).unwrap();
    let c = a.falso.chamadas();
    let u = NOME_UNIDADE;
    for esperado in [
        format!("systemctl --user start {u}"),
        format!("systemctl --user stop {u}"),
        format!("systemctl --user restart {u}"),
        format!("systemctl --user enable {u}"),
        format!("systemctl --user disable {u}"),
    ] {
        assert!(c.contains(&esperado), "{esperado} em {c:?}");
    }
}

#[test]
fn falha_do_systemctl_vira_erro_com_hint() {
    let a = ambiente();
    a.g.instalar().unwrap();
    a.falso
        .responder("systemctl --user start", 1, "", "Unit failed to load");
    let e = a.g.iniciar().unwrap_err();
    assert!(matches!(e, ErroServico::Comando { .. }));
    a.falso.respostas.lock().unwrap().clear();
    a.falso.responder(
        "systemctl --user stop",
        1,
        "",
        "Failed to connect to bus: No medium found",
    );
    let e = a.g.parar().unwrap_err();
    assert_eq!(e.codigo(), "systemd_indisponivel");
    a.falso.respostas.lock().unwrap().clear();
    a.falso.nao_existe("systemctl");
    assert_eq!(
        a.g.reiniciar().unwrap_err().codigo(),
        "systemd_indisponivel"
    );
}

#[test]
fn remover_para_desabilita_apaga_e_recarrega() {
    let a = ambiente();
    a.g.instalar().unwrap();
    a.falso.respostas.lock().unwrap().clear();
    a.falso.chamadas.lock().unwrap().clear();
    a.g.remover().unwrap();
    assert!(!a.dir_unidades.join(NOME_UNIDADE).exists());
    let u = NOME_UNIDADE;
    assert_eq!(
        a.falso.chamadas(),
        vec![
            format!("systemctl --user stop {u}"),
            format!("systemctl --user disable {u}"),
            "systemctl --user daemon-reload".to_string()
        ]
    );
    // de novo: idempotente, sem chamar nada
    a.falso.chamadas.lock().unwrap().clear();
    assert!(a.g.remover().unwrap().contains("já não"));
    assert!(a.falso.chamadas().is_empty());
}

#[test]
fn remover_funciona_mesmo_sem_systemd() {
    let a = ambiente();
    a.g.instalar().unwrap();
    a.falso.nao_existe("systemctl");
    a.g.remover().unwrap();
    assert!(!a.dir_unidades.join(NOME_UNIDADE).exists());
}

#[test]
fn estado_instalado_e_ativo() {
    let a = ambiente();
    a.g.instalar().unwrap();
    a.falso
        .responder("systemctl --user show", 0, SHOW_ATIVO, "");
    a.falso
        .responder("loginctl show-user", 0, "Linger=yes\n", "");
    let e = a.g.estado().unwrap();
    assert!(e.instalado && e.autostart);
    assert_eq!(e.atividade, Atividade::Ativo);
    assert_eq!(e.main_pid, Some(4242));
    assert_eq!(e.desde.as_deref(), Some("Mon 2026-11-23 10:00:00 UTC"));
    assert_eq!(e.linger, Some(true));
    assert_eq!(e.modo_desejado(), ModoServico::Continuo);
    let d = e.descrever();
    assert!(d.contains("4242") && d.contains("linger do usuário: ligado"));
    let show = a
        .falso
        .chamadas()
        .into_iter()
        .find(|c| c.contains(" show "))
        .unwrap();
    for p in [
        "LoadState",
        "ActiveState",
        "UnitFileState",
        "MainPID",
        "ActiveEnterTimestamp",
    ] {
        assert!(show.contains(&format!("-p {p}")), "{show}");
    }
}

#[test]
fn parsing_de_estados() {
    let e = de_show(
        "LoadState=loaded\nActiveState=failed\nUnitFileState=disabled\nMainPID=0\nActiveEnterTimestamp=\n",
        "x".into(),
        None,
    );
    assert_eq!(e.atividade, Atividade::Falhou);
    assert!(!e.autostart);
    assert_eq!(e.desde, None);
    assert_eq!(e.modo_desejado(), ModoServico::SobDemanda);
    assert_eq!(e.modo_atual(), ModoServico::Parado);
    let e = de_show(
        "LoadState=loaded\nActiveState=activating\n",
        "x".into(),
        None,
    );
    assert_eq!(e.atividade, Atividade::Outro("activating".into()));
    let e = de_show(
        "LoadState=not-found\nActiveState=inactive\n",
        "x".into(),
        None,
    );
    assert!(!e.instalado);
    assert_eq!(parse_linger("Linger=yes\n"), Some(true));
    assert_eq!(parse_linger("Linger=no\n"), Some(false));
    assert_eq!(parse_linger("lixo"), None);
    assert_eq!(ModoServico::SobDemanda.como_texto(), "sob_demanda");
}

#[test]
fn linger_so_quando_pedido() {
    let a = ambiente();
    a.g.instalar().unwrap();
    a.g.autostart(true).unwrap();
    a.g.iniciar().unwrap();
    assert!(
        a.falso
            .chamadas()
            .iter()
            .all(|c| !c.contains("enable-linger")),
        "nenhuma ação implícita de linger"
    );
    a.g.persistir_sem_sessao(true).unwrap();
    a.g.persistir_sem_sessao(false).unwrap();
    let c = a.falso.chamadas();
    assert!(c.contains(&"loginctl enable-linger".to_string()));
    assert!(c.contains(&"loginctl disable-linger".to_string()));
}

#[test]
fn linger_sem_autorizacao_orienta() {
    let a = ambiente();
    a.falso.responder(
        "loginctl enable-linger",
        1,
        "",
        "Failed to enable linger: Interactive authentication required.",
    );
    let e = a.g.persistir_sem_sessao(true).unwrap_err();
    assert_eq!(e.codigo(), "polkit");
    assert!(e.to_string().contains("sudo loginctl enable-linger"));
}

#[test]
fn diretorio_de_unidades_segue_xdg() {
    use std::ffi::OsString;
    let s = |x: &str| Some(OsString::from(x));
    assert_eq!(
        diretorio_unidades(s("/cfg"), s("/home/u")).unwrap(),
        PathBuf::from("/cfg/systemd/user")
    );
    assert_eq!(
        diretorio_unidades(None, s("/home/u")).unwrap(),
        PathBuf::from("/home/u/.config/systemd/user")
    );
    assert_eq!(
        diretorio_unidades(s("relativo"), s("/home/u")).unwrap(),
        PathBuf::from("/home/u/.config/systemd/user")
    );
    assert!(diretorio_unidades(None, None).is_none());
}

/// Teste no sistema real: precisa de systemd de usuário (não existe no contêiner).
#[test]
#[ignore = "requer systemd de usuário ativo"]
fn real_estado_nao_falha() {
    let t = tempfile::tempdir().unwrap();
    let g = GerenciadorServico::novo(
        Arc::new(dag_platform_linux::servico::ExecutorReal),
        ConfigUnidade {
            binario: PathBuf::from("/usr/bin/dag"),
            dirs_gravaveis: vec![],
            catalogo: None,
        },
        t.path().to_path_buf(),
        rustix::process::getuid().as_raw(),
    );
    let e = g.estado().unwrap();
    assert!(!e.instalado);
}
