//! Serviço opcional em segundo plano e elevação sob demanda, do lado do agente (A-14, A-16).
//!
//! * [`executar`] traduz [`AcaoServico`] em chamadas ao [`GerenciadorServico`] (systemd de
//!   usuário) e persiste o modo desejado/atual em `service_state` quando há conexão de escritor.
//! * Nada é instalado ou habilitado sem uma ação explícita; o linger é uma ação separada.
//! * [`leitor_elevado`] entrega ao scanner um [`LeitorElevado`] que usa `pkexec dag-helper`, e só
//!   quando a elevação foi pedida ([`habilitar_elevacao`] ou `DAG_ELEVAR=1`).

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dag_core::{AcaoServico, Resposta};
use dag_platform_linux::elevacao::{ClienteHelper, EntradaElevada as EntradaHelper, TipoArquivo};
use dag_platform_linux::servico::{
    ConfigUnidade, ErroServico, EstadoServico, ExecutorReal, GerenciadorServico, diretorio_unidades,
};
use dag_scanner::{EntradaElevada, LeitorElevado, TipoElevado};
use rusqlite::Connection;

use crate::erro::ErroAgente;

/// Variável que liga a elevação (qualquer valor diferente de vazio/`0`).
pub const VAR_ELEVAR: &str = "DAG_ELEVAR";
/// Variável que troca o caminho do helper (padrão `/usr/libexec/dag/dag-helper`).
pub const VAR_HELPER: &str = "DAG_HELPER";

static ELEVAR: AtomicBool = AtomicBool::new(false);

/// Liga (ou desliga) a elevação para as varreduras deste processo (flag `--elevar`).
pub fn habilitar_elevacao(ativo: bool) {
    ELEVAR.store(ativo, Ordering::Relaxed);
}

/// `true` se `--elevar` foi dado neste processo ou `DAG_ELEVAR` está definido.
#[must_use]
pub fn elevacao_habilitada() -> bool {
    ELEVAR.load(Ordering::Relaxed)
        || std::env::var_os(VAR_ELEVAR).is_some_and(|v| !v.is_empty() && v != "0")
}

fn tipo(t: TipoArquivo) -> TipoElevado {
    match t {
        TipoArquivo::File => TipoElevado::Arquivo,
        TipoArquivo::Dir => TipoElevado::Pasta,
        TipoArquivo::Symlink => TipoElevado::Symlink,
        TipoArquivo::Fifo => TipoElevado::Fifo,
        TipoArquivo::Socket => TipoElevado::Socket,
        TipoArquivo::Block => TipoElevado::DispositivoBloco,
        TipoArquivo::Char => TipoElevado::DispositivoChar,
        TipoArquivo::Other => TipoElevado::Outro,
    }
}

fn converter(e: EntradaHelper) -> EntradaElevada {
    EntradaElevada {
        nome: e.nome,
        tipo: tipo(e.meta.tipo_arquivo),
        dev: e.meta.dev,
        ino: e.meta.ino,
        nlink: e.meta.nlink,
        mode: e.meta.mode,
        uid: e.meta.uid,
        gid: e.meta.gid,
        tamanho: e.meta.size,
        blocos: e.meta.blocks,
        mtime_ns: e.meta.mtime_ns,
        ctime_ns: e.meta.ctime_ns,
        alvo_symlink: e.alvo,
    }
}

/// [`LeitorElevado`] apoiado em `pkexec dag-helper`. Nunca guarda senha: a autorização é
/// pedida pelo Polkit, na janela do sistema.
#[derive(Debug)]
pub struct LeitorPkexec {
    cliente: ClienteHelper,
    raizes: Vec<String>,
}

impl LeitorPkexec {
    #[must_use]
    pub fn novo(cliente: ClienteHelper, raizes: Vec<String>) -> Self {
        Self { cliente, raizes }
    }
}

impl LeitorElevado for LeitorPkexec {
    fn listar(&self, pasta: &[u8]) -> io::Result<Vec<EntradaElevada>> {
        let pasta = std::str::from_utf8(pasta).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "caminho não UTF-8 não é aceito pelo helper",
            )
        })?;
        let l = self
            .cliente
            .listar(&self.raizes, pasta)
            .map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e.to_string()))?;
        Ok(l.entradas.into_iter().map(converter).collect())
    }
}

/// Leitor elevado para as `raizes` da varredura, se a elevação foi pedida; senão `None`.
/// As raízes são canonicalizadas (o helper exige caminhos canônicos).
#[must_use]
pub fn leitor_elevado<P: AsRef<Path>>(raizes: &[P]) -> Option<Arc<dyn LeitorElevado>> {
    leitor_elevado_se(raizes, false)
}

/// Como [`leitor_elevado`], mas `pedido` (o `elevar` do comando) também liga a elevação, sem
/// depender do estado global do processo (o agente em segundo plano serve vários clientes).
#[must_use]
pub fn leitor_elevado_se<P: AsRef<Path>>(
    raizes: &[P],
    pedido: bool,
) -> Option<Arc<dyn LeitorElevado>> {
    if !pedido && !elevacao_habilitada() {
        return None;
    }
    let raizes: Vec<String> = raizes
        .iter()
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .filter_map(|p| p.to_str().map(str::to_string))
        .collect();
    if raizes.is_empty() {
        return None;
    }
    let helper = std::env::var_os(VAR_HELPER)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    Some(Arc::new(LeitorPkexec::novo(
        ClienteHelper::novo(helper),
        raizes,
    )))
}

fn absoluto(v: Option<OsString>) -> Option<PathBuf> {
    v.map(PathBuf::from).filter(|p| p.is_absolute())
}

/// Diretórios graváveis do serviço: dados (pai do Catálogo), estado e runtime do DAG.
fn dirs_gravaveis(
    catalogo: &Path,
    xdg_state: Option<OsString>,
    home: Option<OsString>,
    xdg_runtime: Option<OsString>,
) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = catalogo.parent().filter(|p| p.is_absolute()) {
        v.push(d.to_path_buf());
    }
    if let Some(s) = absoluto(xdg_state).or_else(|| absoluto(home).map(|h| h.join(".local/state")))
    {
        v.push(s.join("dag"));
    }
    if let Some(r) = absoluto(xdg_runtime) {
        v.push(r.join("dag"));
    }
    v.sort();
    v.dedup();
    v
}

/// Quanto esperar o agente em execução terminar a parada limpa antes de desistir (nunca se mata
/// à força: um hash em curso grava a fila de retomada ao parar).
const ESPERA_ENTREGA: Duration = Duration::from_secs(180);

/// Quem segura o Catálogo, para decidir a entrega ao serviço.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisaoEntrega {
    /// Ninguém (ou o próprio serviço) escreve: pode iniciar.
    Nada,
    /// Há um agente `dag` de outro caminho (GUI, setsid, systemd-run): pedir parada limpa.
    Encerrar(u32),
    /// O escritor não é um agente (GUI antiga, `dag --sem-agente`): não mexemos nele.
    Recusar(String),
}

/// Decisão pura da entrega. `dono`: pid gravado na trava de escritor; `vivo`/`eh_agente`: o que
/// `/proc` diz dele; `pid_servico`: `MainPID` da unidade, se ativa.
#[must_use]
pub fn decidir_entrega(
    dono: Option<u32>,
    vivo: bool,
    eh_agente: bool,
    pid_servico: Option<u32>,
) -> DecisaoEntrega {
    match dono {
        None => DecisaoEntrega::Nada,
        Some(_) if !vivo => DecisaoEntrega::Nada,
        Some(p) if Some(p) == pid_servico => DecisaoEntrega::Nada,
        Some(p) if eh_agente => DecisaoEntrega::Encerrar(p),
        Some(p) => DecisaoEntrega::Recusar(format!(
            "o Catálogo tem outro escritor que não é um agente (pid {p}), provavelmente uma GUI antiga ou `dag --sem-agente`: feche-o e rode de novo"
        )),
    }
}

/// `/proc/<pid>/cmdline` é um `dag ... agente`?
fn cmdline_de_agente(pid: u32) -> bool {
    let Ok(b) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    let mut partes = b.split(|c| *c == 0).filter(|p| !p.is_empty());
    let Some(bin) = partes.next() else {
        return false;
    };
    let bin = String::from_utf8_lossy(bin);
    bin.rsplit('/').next().is_some_and(|n| n == "dag") && partes.any(|p| p == b"agente")
}

/// O processo existe e não é zumbi?
fn processo_vivo(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|t| {
        t.rfind(')')
            .and_then(|i| t[i + 1..].split_whitespace().next().map(|s| s != "Z"))
            .unwrap_or(false)
    })
}

/// Espera `pid` sair até `espera`. `true` se saiu.
fn esperar_saida(pid: u32, espera: Duration) -> bool {
    let fim = std::time::Instant::now() + espera;
    while processo_vivo(pid) {
        if std::time::Instant::now() >= fim {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    true
}

/// Entrega do Catálogo ao serviço: se um agente lançado por GUI/setsid/systemd-run segura a
/// trava de escritor, pede a ele a parada limpa (SIGTERM: o trabalho em curso vira `interrompida`
/// e o serviço o retoma ao abrir) e espera. Nunca usa SIGKILL. `autorizado` permite a um
/// chamador interativo recusar a entrega.
///
/// # Errors
/// [`ErroServico::Invalido`] se o escritor não é um agente ou não saiu a tempo.
pub fn entregar_catalogo(
    catalogo: &Path,
    espera: Duration,
    autorizado: &dyn Fn() -> bool,
) -> Result<Option<String>, ErroServico> {
    let dono = std::fs::read_to_string(crate::caminho::caminho_do_lock(catalogo))
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok());
    let vivo = dono.is_some_and(processo_vivo);
    let eh = dono.is_some_and(cmdline_de_agente);
    match decidir_entrega(dono, vivo, eh, None) {
        DecisaoEntrega::Nada => Ok(None),
        DecisaoEntrega::Recusar(m) => Err(ErroServico::Invalido(m)),
        DecisaoEntrega::Encerrar(pid) => {
            if !autorizado() {
                return Err(ErroServico::Invalido("entrega recusada".into()));
            }
            let alvo = i32::try_from(pid)
                .ok()
                .and_then(rustix::process::Pid::from_raw)
                .ok_or_else(|| ErroServico::Invalido(format!("pid inválido: {pid}")))?;
            rustix::process::kill_process(alvo, rustix::process::Signal::TERM).map_err(|e| {
                ErroServico::Invalido(format!("não consegui avisar o agente (pid {pid}): {e}"))
            })?;
            if esperar_saida(pid, espera) {
                Ok(Some(format!(
                    "o agente em execução (pid {pid}) foi encerrado com limpeza; o trabalho em curso fica interrompido e o serviço o retoma ao abrir."
                )))
            } else {
                Err(ErroServico::Invalido(format!(
                    "o agente (pid {pid}) ainda está parando após {}s; nada foi morto à força. Tente de novo em instantes",
                    espera.as_secs()
                )))
            }
        }
    }
}

/// Gerenciador do sistema real (binário atual, ambiente atual).
///
/// # Errors
/// [`ErroAgente::Invalido`] se não achar o binário, o `HOME` ou o Catálogo for relativo.
pub fn gerenciador_real(catalogo: &Path) -> Result<GerenciadorServico, ErroAgente> {
    let binario = std::env::current_exe()
        .map_err(|e| ErroAgente::es("descobrir o caminho do binário dag", e))?;
    let catalogo = std::path::absolute(catalogo)
        .map_err(|e| ErroAgente::es("resolver o caminho do Catálogo", e))?;
    let dir = diretorio_unidades(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .ok_or_else(|| {
        ErroAgente::Invalido(
            "não foi possível achar o diretório de unidades de usuário (defina HOME)".into(),
        )
    })?;
    let unidade = ConfigUnidade {
        binario,
        dirs_gravaveis: dirs_gravaveis(
            &catalogo,
            std::env::var_os("XDG_STATE_HOME"),
            std::env::var_os("HOME"),
            std::env::var_os("XDG_RUNTIME_DIR"),
        ),
        // Sempre explícito: o serviço serve exatamente o Catálogo que o cliente usa.
        catalogo: Some(catalogo),
    };
    Ok(GerenciadorServico::novo(
        Arc::new(ExecutorReal),
        unidade,
        dir,
        rustix::process::getuid().as_raw(),
    ))
}

fn erro(e: &ErroServico) -> Resposta {
    Resposta::Erro {
        codigo: e.codigo().to_string(),
        mensagem: e.to_string(),
    }
}

fn persistir(conn: &Connection, estado: Option<&EstadoServico>, falha: Option<&str>) {
    let (d, a) = estado.map_or(("parado", "parado"), |e| {
        (e.modo_desejado().como_texto(), e.modo_atual().como_texto())
    });
    // Falha em gravar o estado não invalida a ação já feita no systemd.
    let _ = conn.execute(
        "UPDATE service_state SET desired_mode = ?1, current_mode = ?2, error = ?3 WHERE id = 1",
        rusqlite::params![d, a, falha],
    );
}

/// Executa a ação com o gerenciador dado (testável com executor falso). `conn`: conexão do
/// escritor, se houver, para persistir o modo em `service_state`.
pub fn executar_com(
    g: &GerenciadorServico,
    acao: AcaoServico,
    conn: Option<&Connection>,
) -> Resposta {
    executar_com_entrega(g, acao, conn, &|| Ok(None))
}

/// [`executar_com`] com a `entrega` do Catálogo usada por `InstalarEAtivar` (veja
/// [`entregar_catalogo`]).
pub fn executar_com_entrega(
    g: &GerenciadorServico,
    acao: AcaoServico,
    conn: Option<&Connection>,
    entrega: &dyn Fn() -> Result<Option<String>, ErroServico>,
) -> Resposta {
    let r = match acao {
        AcaoServico::Instalar => g.instalar(),
        AcaoServico::InstalarEAtivar { boot } => g.instalar_e_ativar(boot, entrega),
        AcaoServico::Remover => g.remover(),
        AcaoServico::Iniciar => g.iniciar(),
        AcaoServico::Parar => g.parar(),
        AcaoServico::Reiniciar => g.reiniciar(),
        AcaoServico::Autostart { ativo } => g.autostart(ativo),
        AcaoServico::PersistirSemSessao { ativo } => g.persistir_sem_sessao(ativo),
        AcaoServico::Estado => g.estado().map(|e| e.descrever()),
    };
    match r {
        Ok(mensagem) => {
            if let Some(c) = conn {
                persistir(c, g.estado().ok().as_ref(), None);
            }
            Resposta::Ok {
                mensagem: Some(mensagem),
            }
        }
        Err(e) => {
            if let Some(c) = conn {
                persistir(c, g.estado().ok().as_ref(), Some(&e.to_string()));
            }
            erro(&e)
        }
    }
}

/// [`executar_com`] no sistema real.
#[must_use]
pub fn executar(acao: AcaoServico, catalogo: &Path, conn: Option<&Connection>) -> Resposta {
    match gerenciador_real(catalogo) {
        Ok(g) => executar_com_entrega(&g, acao, conn, &|| {
            entregar_catalogo(catalogo, ESPERA_ENTREGA, &|| true)
        }),
        Err(e) => e.para_resposta(),
    }
}

/// Executa sem exigir o Catálogo (o serviço pode estar rodando como escritor). Só persiste o
/// modo se o Catálogo já existir e a trava de escritor estiver livre; nunca cria nada.
#[must_use]
pub fn executar_avulso(acao: AcaoServico, catalogo: &Path) -> Resposta {
    // Quem inicia o serviço não pode segurar a trava de escritor enquanto ele sobe.
    let inicia = matches!(
        acao,
        AcaoServico::InstalarEAtivar { .. } | AcaoServico::Iniciar | AcaoServico::Reiniciar
    );
    let agente = if catalogo.is_file() && !inicia {
        crate::Agente::abrir(catalogo).ok()
    } else {
        None
    };
    executar(
        acao,
        catalogo,
        agente.as_ref().map(|a| a.catalogo().armazem().conexao()),
    )
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_platform_linux::servico::{ExecutorSistema, SaidaProcesso};
    use dag_store::Catalogo;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Falso {
        chamadas: Mutex<Vec<String>>,
        show: Mutex<String>,
    }

    impl ExecutorSistema for Falso {
        fn executar(
            &self,
            programa: &str,
            args: &[&str],
            _stdin: Option<&[u8]>,
        ) -> io::Result<SaidaProcesso> {
            let linha = format!("{programa} {}", args.join(" "));
            self.chamadas.lock().unwrap().push(linha.clone());
            let stdout = if linha.contains(" show ") {
                self.show.lock().unwrap().clone()
            } else if linha.starts_with("loginctl show-user") {
                "Linger=no\n".into()
            } else {
                String::new()
            };
            Ok(SaidaProcesso {
                codigo: Some(0),
                stdout: stdout.into_bytes(),
                stderr: Vec::new(),
            })
        }
    }

    fn montar(t: &Path) -> (GerenciadorServico, Arc<Falso>) {
        let falso = Arc::new(Falso::default());
        let g = GerenciadorServico::novo(
            falso.clone(),
            ConfigUnidade {
                binario: PathBuf::from("/usr/bin/dag"),
                dirs_gravaveis: vec![t.join("dados")],
                catalogo: None,
            },
            t.join("unidades"),
            1000,
        );
        (g, falso)
    }

    fn modos(c: &Catalogo) -> (String, String, Option<String>) {
        c.armazem()
            .conexao()
            .query_row(
                "SELECT desired_mode, current_mode, error FROM service_state WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    }

    #[test]
    fn estado_limpo_e_persistencia_de_modos() {
        let t = tempfile::tempdir().unwrap();
        let (g, falso) = montar(t.path());
        let cat = Catalogo::em_memoria().unwrap();
        let con = cat.armazem().conexao();

        let r = executar_com(&g, AcaoServico::Estado, Some(con));
        match r {
            Resposta::Ok { mensagem } => assert!(mensagem.unwrap().contains("não instalado")),
            o => panic!("{o:?}"),
        }
        assert_eq!(modos(&cat).0, "parado");

        let r = executar_com(&g, AcaoServico::Instalar, Some(con));
        assert!(matches!(r, Resposta::Ok { .. }));
        *falso.show.lock().unwrap() =
            "LoadState=loaded\nActiveState=inactive\nUnitFileState=disabled\nMainPID=0\n".into();
        executar_com(&g, AcaoServico::Instalar, Some(con));
        assert_eq!(modos(&cat), ("sob_demanda".into(), "parado".into(), None));

        *falso.show.lock().unwrap() =
            "LoadState=loaded\nActiveState=active\nUnitFileState=enabled\nMainPID=7\n".into();
        executar_com(&g, AcaoServico::Autostart { ativo: true }, Some(con));
        assert_eq!(modos(&cat), ("continuo".into(), "continuo".into(), None));

        executar_com(&g, AcaoServico::Remover, Some(con));
        assert_eq!(modos(&cat).0, "parado");
        assert!(
            !falso
                .chamadas
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.contains("enable-linger"))
        );
    }

    #[test]
    fn falha_vira_erro_e_e_registrada() {
        let t = tempfile::tempdir().unwrap();
        let (g, _) = montar(t.path());
        let cat = Catalogo::em_memoria().unwrap();
        let r = executar_com(&g, AcaoServico::Iniciar, Some(cat.armazem().conexao()));
        match r {
            Resposta::Erro { codigo, .. } => assert_eq!(codigo, "servico_nao_instalado"),
            o => panic!("{o:?}"),
        }
        assert!(modos(&cat).2.unwrap().contains("não está instalado"));
    }

    #[test]
    fn dirs_gravaveis_sao_so_os_do_dag() {
        let v = dirs_gravaveis(
            Path::new("/home/u/.local/share/dag/catalogo.sqlite"),
            None,
            Some("/home/u".into()),
            Some("/run/user/1000".into()),
        );
        assert_eq!(
            v,
            vec![
                PathBuf::from("/home/u/.local/share/dag"),
                PathBuf::from("/home/u/.local/state/dag"),
                PathBuf::from("/run/user/1000/dag"),
            ]
        );
    }

    #[test]
    fn leitor_so_existe_com_elevacao_pedida() {
        habilitar_elevacao(false);
        if std::env::var_os(VAR_ELEVAR).is_none() {
            assert!(leitor_elevado(&["/tmp"]).is_none());
            habilitar_elevacao(true);
            assert!(leitor_elevado(&["/tmp"]).is_some());
            assert!(leitor_elevado::<&str>(&[]).is_none());
            habilitar_elevacao(false);
        }
    }

    #[test]
    fn decisao_de_entrega() {
        use DecisaoEntrega::*;
        assert_eq!(decidir_entrega(None, false, false, None), Nada);
        assert_eq!(decidir_entrega(Some(5), false, true, None), Nada);
        assert_eq!(decidir_entrega(Some(5), true, true, Some(5)), Nada);
        assert_eq!(decidir_entrega(Some(5), true, true, Some(9)), Encerrar(5));
        assert!(matches!(
            decidir_entrega(Some(5), true, false, None),
            Recusar(_)
        ));
    }

    #[test]
    fn esperar_saida_de_processo_real() {
        let mut f = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = f.id();
        assert!(processo_vivo(pid));
        assert!(!cmdline_de_agente(pid));
        assert!(!esperar_saida(pid, Duration::from_millis(200)));
        f.kill().unwrap();
        f.wait().unwrap();
        assert!(esperar_saida(pid, Duration::from_secs(2)));
    }

    #[test]
    fn instalar_e_ativar_com_entrega_e_linger() {
        use std::cell::Cell;
        let t = tempfile::tempdir().unwrap();
        let (g, falso) = montar(t.path());
        let chamou = Cell::new(0);
        let entrega = || {
            chamou.set(chamou.get() + 1);
            Ok(Some("entregue".to_string()))
        };
        let m = g.instalar_e_ativar(true, &entrega).unwrap();
        assert_eq!(chamou.get(), 1);
        assert!(m.starts_with("entregue"));
        let c = falso.chamadas.lock().unwrap().clone();
        let pos = |s: &str| c.iter().position(|x| x.contains(s)).unwrap();
        assert!(pos("daemon-reload") < pos("enable --now dag-agente.service"));
        assert!(c.iter().any(|x| x == "loginctl enable-linger"));
        // idempotente
        g.instalar_e_ativar(false, &entrega).unwrap();
        // entrega que falha impede o início
        let (g2, falso2) = montar(&t.path().join("b"));
        let r = g2.instalar_e_ativar(false, &|| Err(ErroServico::Invalido("x".into())));
        assert!(r.is_err());
        assert!(
            !falso2
                .chamadas
                .lock()
                .unwrap()
                .iter()
                .any(|x| x.contains("enable"))
        );
    }
}
