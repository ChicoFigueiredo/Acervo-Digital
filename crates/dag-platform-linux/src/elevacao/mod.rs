//! Elevação sob demanda (A-16): lança `pkexec <dag-helper>` só quando o usuário pediu e uma pasta
//! negou acesso. O helper lê **apenas metadados**. A senha é digitada somente no diálogo do
//! Polkit; este módulo nunca a vê nem guarda nada.
//!
//! Nada aqui roda como root: o único processo privilegiado é o `dag-helper`, curto e restrito.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dag_privileged_helper::{CAMINHO_HELPER_PADRAO, Linha, Requisicao, decodificar_bytes};
pub use dag_privileged_helper::{Metadados, TipoArquivo};
use thiserror::Error;

use crate::servico::{ExecutorReal, ExecutorSistema};

/// Alternativas oferecidas quando não há como autorizar via Polkit.
pub const ALTERNATIVAS: &str = "Alternativas: (1) iniciar um agente Polkit na sessão (ex.: polkit-gnome-authentication-agent-1, lxqt-policykit-agent, polkit-kde-authentication-agent-1) ou usar um desktop que traga um; (2) conceder leitura ao seu usuário na pasta (ex.: `setfacl -R -m u:$USER:rX <pasta>`); (3) varrer sem `--elevar` e aceitar que essas pastas fiquem como inacessíveis no Catálogo.";

#[derive(Debug, Error)]
pub enum ErroElevacao {
    #[error("`pkexec` não encontrado (pacote polkit). {}", ALTERNATIVAS)]
    SemPkexec,
    #[error("helper não instalado em {}. Veja packaging/README.md para instalar o helper e a política Polkit.", .0.display())]
    SemHelper(PathBuf),
    #[error(
        "autorização negada ou sem agente Polkit para mostrar o diálogo de senha (pkexec código {codigo}: {detalhe}). {}",
        ALTERNATIVAS
    )]
    NaoAutorizado { codigo: i32, detalhe: String },
    #[error("o helper recusou o pedido ({codigo}): {mensagem}")]
    Recusado { codigo: String, mensagem: String },
    #[error("resposta inválida do helper: {0}")]
    RespostaInvalida(String),
    #[error("falha ao executar o helper: {0}")]
    Es(#[from] std::io::Error),
}

/// Um filho de pasta devolvido pelo helper (nome e alvo em bytes brutos).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntradaElevada {
    pub nome: Vec<u8>,
    pub alvo: Option<Vec<u8>>,
    pub meta: Metadados,
}

/// Metadados da pasta e de seus filhos diretos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListagemElevada {
    pub pasta: Metadados,
    pub entradas: Vec<EntradaElevada>,
}

/// Cliente do helper via `pkexec`.
#[derive(Clone)]
pub struct ClienteHelper {
    exec: Arc<dyn ExecutorSistema>,
    pkexec: String,
    helper: PathBuf,
    verificar_helper: bool,
}

impl std::fmt::Debug for ClienteHelper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClienteHelper")
            .field("pkexec", &self.pkexec)
            .field("helper", &self.helper)
            .finish_non_exhaustive()
    }
}

impl ClienteHelper {
    /// Cliente real com o helper no caminho informado (padrão `/usr/libexec/dag/dag-helper`).
    #[must_use]
    pub fn novo(helper: Option<PathBuf>) -> Self {
        Self::com_executor(
            Arc::new(ExecutorReal),
            "pkexec".into(),
            helper.unwrap_or_else(|| PathBuf::from(CAMINHO_HELPER_PADRAO)),
            true,
        )
    }

    /// Para testes: executor falso e sem checagem de existência do helper.
    #[must_use]
    pub fn com_executor(
        exec: Arc<dyn ExecutorSistema>,
        pkexec: String,
        helper: PathBuf,
        verificar_helper: bool,
    ) -> Self {
        Self {
            exec,
            pkexec,
            helper,
            verificar_helper,
        }
    }

    /// Pede ao helper (via pkexec) os metadados de `caminho`, dentro de `raizes`.
    ///
    /// # Errors
    /// Ver [`ErroElevacao`]; `NaoAutorizado` cobre pkexec 126/127 (sem agente ou negado).
    pub fn listar(
        &self,
        raizes: &[String],
        caminho: &str,
    ) -> Result<ListagemElevada, ErroElevacao> {
        if self.verificar_helper && !self.helper.is_file() {
            return Err(ErroElevacao::SemHelper(self.helper.clone()));
        }
        let req = Requisicao {
            raizes: raizes.to_vec(),
            caminho: caminho.to_string(),
        };
        let mut corpo = serde_json::to_vec(&req).map_err(std::io::Error::other)?;
        corpo.push(b'\n');
        let helper = self.helper.to_string_lossy().into_owned();
        let s = match self.exec.executar(&self.pkexec, &[&helper], Some(&corpo)) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ErroElevacao::SemPkexec);
            }
            Err(e) => return Err(e.into()),
        };
        interpretar_saida(&s)
    }
}

fn interpretar_saida(s: &crate::servico::SaidaProcesso) -> Result<ListagemElevada, ErroElevacao> {
    let texto = s.stdout_texto();
    let mut pasta = None;
    let mut entradas = Vec::new();
    let mut fim = None;
    for linha in texto.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<Linha>(linha)
            .map_err(|e| ErroElevacao::RespostaInvalida(e.to_string()))?
        {
            Linha::Dir { meta } => pasta = Some(meta),
            Linha::Entrada {
                nome,
                nome_hex,
                alvo,
                alvo_hex,
                meta,
            } => {
                let nome = decodificar_bytes(nome.as_deref(), nome_hex.as_deref())
                    .ok_or_else(|| ErroElevacao::RespostaInvalida("nome inválido".into()))?;
                let alvo = if alvo.is_some() || alvo_hex.is_some() {
                    Some(
                        decodificar_bytes(alvo.as_deref(), alvo_hex.as_deref()).ok_or_else(
                            || ErroElevacao::RespostaInvalida("alvo inválido".into()),
                        )?,
                    )
                } else {
                    None
                };
                entradas.push(EntradaElevada { nome, alvo, meta });
            }
            Linha::Fim { entradas: n } => fim = Some(n),
            Linha::Erro { codigo, mensagem } => {
                return Err(ErroElevacao::Recusado { codigo, mensagem });
            }
        }
    }
    match s.codigo {
        Some(0) => {}
        Some(c @ (126 | 127)) => {
            return Err(ErroElevacao::NaoAutorizado {
                codigo: c,
                detalhe: s.stderr_texto(),
            });
        }
        c => {
            return Err(ErroElevacao::RespostaInvalida(format!(
                "helper terminou com código {c:?}: {}",
                s.stderr_texto()
            )));
        }
    }
    let pasta = pasta.ok_or_else(|| ErroElevacao::RespostaInvalida("sem linha da pasta".into()))?;
    if fim != Some(entradas.len() as u64) {
        return Err(ErroElevacao::RespostaInvalida(
            "resposta truncada (linha final ausente ou contagem divergente)".into(),
        ));
    }
    Ok(ListagemElevada { pasta, entradas })
}

/// Resultado de [`diagnosticar`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostico {
    pub pkexec: bool,
    pub helper: bool,
    /// Heurística: algum processo de agente Polkit conhecido está rodando.
    pub agente_polkit: bool,
}

impl Diagnostico {
    #[must_use]
    pub fn pronto(&self) -> bool {
        self.pkexec && self.helper
    }

    #[must_use]
    pub fn mensagem(&self) -> String {
        let mut p = Vec::new();
        if !self.pkexec {
            p.push("pkexec ausente (instale o pacote polkit)".to_string());
        }
        if !self.helper {
            p.push("dag-helper não instalado (veja packaging/README.md)".to_string());
        }
        if !self.agente_polkit {
            p.push(
                "nenhum agente Polkit detectado na sessão: o diálogo de senha pode não aparecer"
                    .into(),
            );
        }
        if p.is_empty() {
            "elevação disponível: pkexec, dag-helper e agente Polkit detectados.".into()
        } else {
            format!("{}. {ALTERNATIVAS}", p.join("; "))
        }
    }
}

const AGENTES: [&str; 9] = [
    "polkit-gnome-au",
    "polkit-kde-auth",
    "polkit-mate-aut",
    "lxpolkit",
    "lxqt-policykit",
    "xfce-polkit",
    "gnome-shell",
    "plasmashell",
    "ukui-polkit",
];

/// Verifica pkexec, helper e (por heurística em `/proc`) um agente Polkit.
#[must_use]
pub fn diagnosticar(helper: &Path, proc_dir: &Path, caminhos_pkexec: &[&Path]) -> Diagnostico {
    let agente = std::fs::read_dir(proc_dir).is_ok_and(|rd| {
        rd.flatten().any(|e| {
            std::fs::read_to_string(e.path().join("comm"))
                .is_ok_and(|c| AGENTES.iter().any(|a| c.trim().starts_with(a)))
        })
    });
    Diagnostico {
        pkexec: caminhos_pkexec.iter().any(|p| p.is_file()),
        helper: helper.is_file(),
        agente_polkit: agente,
    }
}

/// [`diagnosticar`] no sistema real.
#[must_use]
pub fn diagnosticar_sistema(helper: Option<&Path>) -> Diagnostico {
    diagnosticar(
        helper.unwrap_or(Path::new(CAMINHO_HELPER_PADRAO)),
        Path::new("/proc"),
        &[Path::new("/usr/bin/pkexec"), Path::new("/bin/pkexec")],
    )
}
