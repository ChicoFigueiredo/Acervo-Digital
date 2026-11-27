//! Lado cliente do IPC do agente (spec §6/§11) e a escolha entre agente em segundo plano e
//! agente de sessão ([`Conexao`]). GUI, CLI e MCP usam a mesma API.

use std::ffi::OsString;
use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use std::time::Duration;

use dag_core::{Autor, Comando, EventoAgente, Pedido, Progresso, Resposta};
use dag_scanner::ControleScan;

use crate::agente::{Agente, somente_leitura};
use crate::erro::ErroAgente;
use crate::lancar::{OpcoesLancamento, garantir_agente, sem_agente_do_ambiente};
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
    /// O pedido em curso já começou a rodar no agente (chegou progresso dele). Enquanto está
    /// na fila, o Ctrl-C deste cliente não pode cancelar o trabalho de outro.
    em_curso: Arc<AtomicBool>,
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
            em_curso: Arc::new(AtomicBool::new(false)),
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

    /// Alça para cancelar (Ctrl-C) de outra thread o pedido deste cliente, e só ele.
    #[must_use]
    pub fn cancelador(&self) -> Cancelador {
        Cancelador::Remoto(self.socket.clone(), Arc::clone(&self.em_curso))
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
        self.em_curso.store(false, Ordering::SeqCst);
        let envio = self.escrever(&Requisicao {
            id,
            pedido: Some(pedido),
            ola: false,
            assinar: false,
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
                self.em_curso.store(true, Ordering::SeqCst);
                progresso(p);
            }
            if let Some(r) = q.resposta {
                self.em_curso.store(false, Ordering::SeqCst);
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
            assinar: false,
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

    /// Assina o fluxo de eventos do agente (ADR 0004): consome esta conexão, que passa a só
    /// receber. O primeiro evento é sempre o [`EventoAgente::Instantaneo`].
    ///
    /// # Errors
    /// [`ErroAgente::Ipc`] se o envio falhar.
    pub fn assinar(mut self) -> Result<Assinatura, ErroAgente> {
        let id = self.novo_id();
        let envio = self.escrever(&Requisicao {
            id,
            pedido: None,
            ola: false,
            assinar: true,
        });
        if let Err(e) = envio {
            return Err(match self.recusa_pendente() {
                Some(Resposta::Erro { mensagem, .. }) => ErroAgente::Ipc(mensagem),
                _ => e,
            });
        }
        Ok(Assinatura {
            socket: self.socket,
            leitor: self.leitor,
            buf: Vec::new(),
        })
    }

    /// O agente ainda responde? (Uma volta de ida e volta barata.)
    #[must_use]
    pub fn viva(&mut self) -> bool {
        self.info().is_ok()
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

/// Fluxo de eventos de um agente (resultado de [`Cliente::assinar`]).
#[derive(Debug)]
pub struct Assinatura {
    socket: PathBuf,
    leitor: BufReader<UnixStream>,
    buf: Vec<u8>,
}

impl Assinatura {
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Espera o próximo evento por até `espera`. `Ok(None)` = nada chegou nesse tempo.
    ///
    /// # Errors
    /// [`ErroAgente::Ipc`] se o agente fechar a conexão (encerrou ou desligou este assinante por
    /// lentidão) ou violar o protocolo: o cliente deve reassinar.
    pub fn proximo(&mut self, espera: Duration) -> Result<Option<EventoAgente>, ErroAgente> {
        let _ = self
            .leitor
            .get_ref()
            .set_read_timeout(Some(espera.max(Duration::from_millis(1))));
        loop {
            let bytes = match ler_quadro(&mut self.leitor, &mut self.buf) {
                Ok(Some(b)) => b,
                Ok(None) => return Err(ErroAgente::Ipc("o agente encerrou a conexão".into())),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(ErroAgente::Ipc(format!("leitura: {e}"))),
            };
            let q: Quadro = serde_json::from_slice(&bytes)
                .map_err(|e| ErroAgente::Ipc(format!("quadro inválido: {e}")))?;
            if let Some(e) = q.evento {
                return Ok(Some(e));
            }
            if let Some(Resposta::Erro { mensagem, .. }) = q.resposta {
                return Err(ErroAgente::Ipc(mensagem));
            }
        }
    }
}

/// Pede o cancelamento do trabalho em andamento (Ctrl-C), seja em processo ou no agente.
#[derive(Debug, Clone)]
pub enum Cancelador {
    Local(ControleScan),
    /// Socket do agente e se o pedido deste cliente já começou a rodar lá.
    Remoto(PathBuf, Arc<AtomicBool>),
}

impl Cancelador {
    /// Cancela com checkpoint; falhas de conexão são ignoradas (nada a cancelar).
    ///
    /// Devolve `false` sem pedir nada ao agente quando o pedido deste cliente ainda espera na
    /// fila: o trabalho em andamento é de outro cliente (a curadoria, um scan) e não pode parar
    /// porque este cliente desistiu de esperar.
    pub fn cancelar(&self) -> bool {
        match self {
            Self::Local(c) => {
                c.cancelar();
                true
            }
            Self::Remoto(_, em_curso) if !em_curso.load(Ordering::SeqCst) => false,
            Self::Remoto(socket, _) => {
                if let Ok(mut c) = Cliente::conectar(socket) {
                    let _ = c.enviar(
                        Pedido {
                            autor: Autor::Cli,
                            comando: Comando::Cancelar { execucao: None },
                        },
                        &mut |_| {},
                    );
                }
                true
            }
        }
    }
}

/// Como o cliente chega ao agente (ADR 0004).
#[derive(Debug, Clone, Default)]
pub struct OpcoesConexao {
    /// Sem agente: abre o Catálogo em processo (a sessão vira o escritor). `--sem-agente`.
    pub sem_agente: bool,
    /// Como lançar o agente; `None` = valores do ambiente.
    pub lancamento: Option<OpcoesLancamento>,
}

impl OpcoesConexao {
    /// `sem_agente` vem da flag ou de `DAG_SEM_AGENTE`.
    #[must_use]
    pub fn novo(flag_sem_agente: bool) -> Self {
        Self {
            sem_agente: flag_sem_agente || sem_agente_do_ambiente(),
            lancamento: None,
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
                | Comando::PausarJob { .. }
                | Comando::RetomarJob { .. }
                | Comando::CancelarJob { .. }
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

    /// Como [`abrir_para`](Self::abrir_para), mas o agente em segundo plano é a regra (ADR 0004):
    /// o que escreve ou varre sobe o agente se não houver um (`opcoes.lancamento`) e só cai no
    /// modo em processo com `opcoes.sem_agente`. Leituras e o controle de trabalhos não lançam
    /// nada: sem agente, as leituras abrem o Catálogo só para leitura e o controle diz `sem_agente`.
    ///
    /// # Errors
    /// Falhas ao abrir o Catálogo ou ao lançar o agente.
    pub fn abrir_para_com(
        catalogo: &Path,
        socket: &Path,
        comando: &Comando,
        opcoes: &OpcoesConexao,
    ) -> Result<Conexao, ErroAgente> {
        if opcoes.sem_agente {
            return Self::abrir_para_sem_agente(catalogo, socket, comando);
        }
        let controle = matches!(
            comando,
            Comando::Pausar { .. }
                | Comando::Cancelar { .. }
                | Comando::Retomar { execucao: None, .. }
                | Comando::PausarJob { .. }
                | Comando::RetomarJob { .. }
                | Comando::CancelarJob { .. }
        );
        if controle || somente_leitura(comando) {
            return Self::abrir_para(catalogo, socket, comando);
        }
        let lanc = opcoes
            .lancamento
            .clone()
            .unwrap_or_else(|| OpcoesLancamento::do_ambiente(catalogo, socket));
        garantir_agente(&lanc).map(Conexao::Daemon)
    }

    /// O comportamento anterior ao agente único: agente em segundo plano se houver, senão sessão
    /// em processo (`--sem-agente`, testes, máquinas sem como subir um processo).
    fn abrir_para_sem_agente(
        catalogo: &Path,
        socket: &Path,
        comando: &Comando,
    ) -> Result<Conexao, ErroAgente> {
        Self::abrir_para(catalogo, socket, comando)
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
            Self::Daemon(c) => c.cancelador(),
            Self::Sessao(a) => Cancelador::Local(a.controle()),
        }
    }
}

/// Conecta e confere que o agente serve este Catálogo.
pub(crate) fn tentar_agente(catalogo: &Path, socket: &Path) -> Option<Cliente> {
    tentar_agente_com_info(catalogo, socket).map(|(c, _)| c)
}

/// Como [`tentar_agente`], devolvendo também a identificação do agente.
pub(crate) fn tentar_agente_com_info(
    catalogo: &Path,
    socket: &Path,
) -> Option<(Cliente, InfoServidor)> {
    let mut c = Cliente::conectar(socket).ok()?;
    let info = c.info().ok()?;
    mesmo_arquivo(Path::new(&info.catalogo), catalogo).then_some((c, info))
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
        Comando::Pausar { .. }
            | Comando::Cancelar { .. }
            | Comando::Retomar { execucao: None, .. }
            | Comando::PausarJob { .. }
            | Comando::RetomarJob { .. }
            | Comando::CancelarJob { .. }
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
