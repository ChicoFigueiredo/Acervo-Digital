//! Lado cliente do IPC do agente (spec §6/§11) e a escolha entre agente em segundo plano e
//! agente de sessão ([`Conexao`]). GUI, CLI e MCP usam a mesma API.

use std::ffi::OsString;
use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use dag_core::{Autor, Comando, Pedido, Progresso, Resposta};
use dag_scanner::ControleScan;

use crate::agente::{Agente, somente_leitura};
use crate::erro::ErroAgente;
use crate::protocolo::{InfoServidor, Quadro, Requisicao, ler_quadro, linha};

/// Variável de ambiente que escolhe o socket do agente.
pub const VAR_SOCKET: &str = "DAG_SOCKET";

/// Resolve o socket: `DAG_SOCKET`, depois `$XDG_RUNTIME_DIR/dag/agente.sock`, depois
/// `$XDG_STATE_HOME/dag/agente.sock` (padrão `~/.local/state`). Valores vazios ou relativos são
/// ignorados. Sem nenhum deles, `/tmp/dag-<uid>/agente.sock`.
#[must_use]
pub fn resolver_caminho_socket(
    env_dag: Option<OsString>,
    xdg_runtime_dir: Option<OsString>,
    xdg_state_home: Option<OsString>,
    home: Option<OsString>,
    uid: u32,
) -> PathBuf {
    let absoluto = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    if let Some(e) = env_dag.filter(|e| !e.is_empty()) {
        return PathBuf::from(e);
    }
    if let Some(r) = absoluto(xdg_runtime_dir) {
        return r.join("dag").join("agente.sock");
    }
    if let Some(s) = absoluto(xdg_state_home) {
        return s.join("dag").join("agente.sock");
    }
    if let Some(h) = absoluto(home) {
        return h.join(".local/state/dag/agente.sock");
    }
    PathBuf::from(format!("/tmp/dag-{uid}/agente.sock"))
}

/// Socket padrão do agente, lendo o ambiente do processo.
#[must_use]
pub fn caminho_socket_padrao() -> PathBuf {
    resolver_caminho_socket(
        std::env::var_os(VAR_SOCKET),
        std::env::var_os("XDG_RUNTIME_DIR"),
        std::env::var_os("XDG_STATE_HOME"),
        std::env::var_os("HOME"),
        rustix::process::getuid().as_raw(),
    )
}

/// Conexão com um agente em segundo plano (`dag agente`). Um pedido por vez; para controlar um
/// trabalho em andamento abra outra conexão.
#[derive(Debug)]
pub struct Cliente {
    socket: PathBuf,
    leitor: BufReader<UnixStream>,
    escritor: UnixStream,
    proximo_id: u64,
}

impl Cliente {
    /// Conecta ao socket.
    ///
    /// # Errors
    /// [`ErroAgente::AgenteIndisponivel`] se ninguém atende no socket.
    pub fn conectar(socket: &Path) -> Result<Cliente, ErroAgente> {
        let indisponivel = |e: std::io::Error| ErroAgente::AgenteIndisponivel {
            socket: socket.to_path_buf(),
            motivo: e.to_string(),
        };
        let escritor = UnixStream::connect(socket).map_err(indisponivel)?;
        let leitor = BufReader::new(escritor.try_clone().map_err(indisponivel)?);
        Ok(Cliente {
            socket: socket.to_path_buf(),
            leitor,
            escritor,
            proximo_id: 1,
        })
    }

    /// Conecta só se há um agente em `socket` servindo este `catalogo` (confere pela
    /// identificação); `None` em qualquer outro caso.
    #[must_use]
    pub fn conectar_se_serve(socket: &Path, catalogo: &Path) -> Option<Cliente> {
        tentar_agente(catalogo, socket)
    }

    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Envia o pedido e devolve a resposta; cada evento de progresso chama `progresso`.
    ///
    /// # Errors
    /// [`ErroAgente::Ipc`] se a conexão cair ou o agente violar o protocolo. Falhas do comando
    /// chegam como `Ok(Resposta::Erro { .. })`.
    pub fn enviar(
        &mut self,
        pedido: Pedido,
        progresso: &mut dyn FnMut(&Progresso),
    ) -> Result<Resposta, ErroAgente> {
        let id = self.novo_id();
        let envio = self.escrever(&Requisicao {
            id,
            pedido: Some(pedido),
            ola: false,
        });
        if let Err(e) = envio {
            return self.recusa_pendente().ok_or(e);
        }
        loop {
            let q = self.ler()?;
            if q.id != id && q.id != 0 {
                continue;
            }
            if let Some(p) = &q.progresso {
                progresso(p);
            }
            if let Some(r) = q.resposta {
                return Ok(r);
            }
        }
    }

    /// Identificação do agente (qual Catálogo serve).
    ///
    /// # Errors
    /// [`ErroAgente::Ipc`] se o agente não responder como esperado.
    pub fn info(&mut self) -> Result<InfoServidor, ErroAgente> {
        let id = self.novo_id();
        let envio = self.escrever(&Requisicao {
            id,
            pedido: None,
            ola: true,
        });
        if let Err(e) = envio {
            return Err(match self.recusa_pendente() {
                Some(Resposta::Erro { mensagem, .. }) => ErroAgente::Ipc(mensagem),
                _ => e,
            });
        }
        loop {
            let q = self.ler()?;
            if let Some(s) = q.servidor {
                return Ok(s);
            }
            if let Some(Resposta::Erro { mensagem, .. }) = q.resposta {
                return Err(ErroAgente::Ipc(mensagem));
            }
        }
    }

    /// Uma recusa do agente (uid não autorizado) que chegou antes de o envio falhar.
    fn recusa_pendente(&mut self) -> Option<Resposta> {
        self.ler().ok().and_then(|q| q.resposta)
    }

    fn novo_id(&mut self) -> u64 {
        let id = self.proximo_id;
        self.proximo_id += 1;
        id
    }

    fn escrever(&mut self, r: &Requisicao) -> Result<(), ErroAgente> {
        let bytes = linha(r).map_err(|e| ErroAgente::Ipc(e.to_string()))?;
        self.escritor
            .write_all(&bytes)
            .and_then(|()| self.escritor.flush())
            .map_err(|e| ErroAgente::Ipc(format!("envio: {e}")))
    }

    fn ler(&mut self) -> Result<Quadro, ErroAgente> {
        let mut buf = Vec::new();
        let bytes = ler_quadro(&mut self.leitor, &mut buf)
            .map_err(|e| ErroAgente::Ipc(format!("leitura: {e}")))?
            .ok_or_else(|| ErroAgente::Ipc("o agente encerrou a conexão".into()))?;
        serde_json::from_slice(&bytes).map_err(|e| ErroAgente::Ipc(format!("quadro inválido: {e}")))
    }
}

/// Pede o cancelamento do trabalho em andamento (Ctrl-C), seja em processo ou no agente.
#[derive(Debug, Clone)]
pub enum Cancelador {
    Local(ControleScan),
    Remoto(PathBuf),
}

impl Cancelador {
    /// Cancela com checkpoint; falhas de conexão são ignoradas (nada a cancelar).
    pub fn cancelar(&self) {
        match self {
            Self::Local(c) => c.cancelar(),
            Self::Remoto(socket) => {
                if let Ok(mut c) = Cliente::conectar(socket) {
                    let _ = c.enviar(
                        Pedido {
                            autor: Autor::Cli,
                            comando: Comando::Cancelar { execucao: None },
                        },
                        &mut |_| {},
                    );
                }
            }
        }
    }
}

/// Agente em segundo plano, se houver um servindo o mesmo Catálogo; senão, um agente de sessão
/// em processo (modo 1).
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Conexao {
    Daemon(Cliente),
    Sessao(Agente),
}

impl Conexao {
    /// Conecta ao agente em `socket` se ele serve `catalogo`; senão abre uma sessão como
    /// escritor. Se outro processo (sem socket) já é o escritor, a sessão abre só para leitura.
    ///
    /// # Errors
    /// Falhas ao abrir o Catálogo.
    pub fn abrir(catalogo: &Path, socket: &Path) -> Result<Conexao, ErroAgente> {
        if let Some(c) = tentar_agente(catalogo, socket) {
            return Ok(Conexao::Daemon(c));
        }
        match Agente::abrir(catalogo) {
            Ok(a) => Ok(Conexao::Sessao(a.silencioso())),
            Err(ErroAgente::EscritorOcupado(p)) => match Agente::abrir_leitura(catalogo) {
                Ok(a) => Ok(Conexao::Sessao(a.silencioso())),
                Err(_) => Err(ErroAgente::EscritorOcupado(p)),
            },
            Err(e) => Err(e),
        }
    }

    /// Como [`abrir`](Self::abrir), mas conforme o comando: os que só leem não criam nem
    /// travam nada (`dag status` sem Catálogo diz que não há Catálogo); os que escrevem exigem
    /// ser o escritor (`escritor_ocupado` se outro processo for); pausar, retomar e cancelar só
    /// existem com um agente em segundo plano (`sem_agente`).
    ///
    /// # Errors
    /// Falhas ao abrir o Catálogo.
    pub fn abrir_para(
        catalogo: &Path,
        socket: &Path,
        comando: &Comando,
    ) -> Result<Conexao, ErroAgente> {
        if matches!(
            comando,
            Comando::Pausar { .. }
                | Comando::Cancelar { .. }
                | Comando::Retomar { execucao: None, .. }
        ) {
            // Só atuam sobre um agente em segundo plano; nada a abrir (nem criar) sem ele.
            return tentar_agente(catalogo, socket)
                .map(Conexao::Daemon)
                .ok_or(ErroAgente::SemAgente);
        }
        if somente_leitura(comando) {
            if let Some(c) = tentar_agente(catalogo, socket) {
                return Ok(Conexao::Daemon(c));
            }
            return Ok(Conexao::Sessao(
                Agente::abrir_leitura(catalogo)?.silencioso(),
            ));
        }
        if let Some(c) = tentar_agente(catalogo, socket) {
            return Ok(Conexao::Daemon(c));
        }
        // Escrita sem agente: a sessão precisa ser o escritor (ou falha com `escritor_ocupado`).
        Ok(Conexao::Sessao(Agente::abrir(catalogo)?.silencioso()))
    }

    /// Executa o pedido. Na sessão, o trabalho roda numa thread e o progresso chega aqui.
    ///
    /// # Errors
    /// [`ErroAgente::Ipc`] no modo agente em segundo plano se a conexão falhar.
    pub fn enviar(
        &mut self,
        pedido: Pedido,
        progresso: &mut dyn FnMut(&Progresso),
    ) -> Result<Resposta, ErroAgente> {
        match self {
            Self::Daemon(c) => c.enviar(pedido, progresso),
            Self::Sessao(a) => Ok(executar_na_sessao(a, pedido, progresso)),
        }
    }

    /// `true` se há um agente em segundo plano atendendo.
    #[must_use]
    pub fn eh_daemon(&self) -> bool {
        matches!(self, Self::Daemon(_))
    }

    /// Identificação do agente em segundo plano (`None` na sessão).
    pub fn info_daemon(&mut self) -> Option<InfoServidor> {
        match self {
            Self::Daemon(c) => c.info().ok(),
            Self::Sessao(_) => None,
        }
    }

    /// Alça para cancelar (Ctrl-C) de outra thread.
    #[must_use]
    pub fn cancelador(&self) -> Cancelador {
        match self {
            Self::Daemon(c) => Cancelador::Remoto(c.socket.clone()),
            Self::Sessao(a) => Cancelador::Local(a.controle()),
        }
    }
}

/// Conecta e confere que o agente serve este Catálogo.
fn tentar_agente(catalogo: &Path, socket: &Path) -> Option<Cliente> {
    let mut c = Cliente::conectar(socket).ok()?;
    let info = c.info().ok()?;
    mesmo_arquivo(Path::new(&info.catalogo), catalogo).then_some(c)
}

/// Compara caminhos mesmo que o arquivo ainda não exista (resolve o diretório pai).
fn mesmo_arquivo(a: &Path, b: &Path) -> bool {
    fn canonico(p: &Path) -> PathBuf {
        if let Ok(c) = std::fs::canonicalize(p) {
            return c;
        }
        let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        match (abs.parent(), abs.file_name()) {
            (Some(pai), Some(nome)) => {
                std::fs::canonicalize(pai).map_or_else(|_| abs.clone(), |pai| pai.join(nome))
            }
            _ => abs,
        }
    }
    canonico(a) == canonico(b)
}

/// Roda o pedido na thread atual do agente de sessão; o progresso (que pode vir de threads do
/// trabalho) é repassado ao chamador por um canal.
fn executar_na_sessao(
    agente: &mut Agente,
    pedido: Pedido,
    progresso: &mut dyn FnMut(&Progresso),
) -> Resposta {
    if matches!(
        pedido.comando,
        Comando::Pausar { .. } | Comando::Cancelar { .. } | Comando::Retomar { execucao: None, .. }
    ) {
        return ErroAgente::SemAgente.para_resposta();
    }
    let (tx, rx) = std::sync::mpsc::channel::<Progresso>();
    agente.definir_progresso(Some(std::sync::Arc::new(move |p: &Progresso| {
        let _ = tx.send(p.clone());
    })));
    agente.controle().reiniciar();
    let resposta = std::thread::scope(|s| {
        let h = s.spawn(|| agente.executar_pedido(pedido));
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(p) => progresso(&p),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if h.is_finished() {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        h.join().unwrap_or_else(|_| Resposta::Erro {
            codigo: "panico".into(),
            mensagem: "o agente de sessão falhou inesperadamente".into(),
        })
    });
    // Eventos que chegaram entre o último recebimento e o fim.
    while let Ok(p) = rx.try_recv() {
        progresso(&p);
    }
    agente.definir_progresso(None);
    resposta
}

#[cfg(test)]
mod testes {
    use super::*;

    fn o(s: &str) -> Option<OsString> {
        Some(s.into())
    }

    #[test]
    fn precedencia_do_socket() {
        assert_eq!(
            resolver_caminho_socket(o("/e/s"), o("/run/u"), o("/st"), o("/h"), 1000),
            PathBuf::from("/e/s")
        );
        assert_eq!(
            resolver_caminho_socket(o(""), o("/run/u"), o("/st"), o("/h"), 1000),
            PathBuf::from("/run/u/dag/agente.sock")
        );
        assert_eq!(
            resolver_caminho_socket(None, o("relativo"), None, o("/h"), 1000),
            PathBuf::from("/h/.local/state/dag/agente.sock")
        );
        assert_eq!(
            resolver_caminho_socket(None, None, o("/st"), o("/h"), 1000),
            PathBuf::from("/st/dag/agente.sock")
        );
        assert_eq!(
            resolver_caminho_socket(None, None, None, None, 1000),
            PathBuf::from("/tmp/dag-1000/agente.sock")
        );
    }

    #[test]
    fn sem_agente_conectar_falha_com_erro_claro() {
        let tmp = tempfile::tempdir().unwrap();
        let e = Cliente::conectar(&tmp.path().join("nada.sock")).unwrap_err();
        assert_eq!(e.codigo(), "agente_indisponivel");
    }
}
