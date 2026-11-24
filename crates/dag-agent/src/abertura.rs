//! Recuperação ao abrir (ADR 0003): Execução Interrompida, retomada automática, passadas de
//! verificação e Ponto de Confiança.
//!
//! Roda uma vez, na thread do escritor, quando o agente em segundo plano ou a GUI (com a trava de
//! escritor) abre o Catálogo. Ter a trava prova que nenhum outro processo está escrevendo: o que
//! o Catálogo ainda mostra como "em andamento" foi deixado por um processo morto.
//!
//! Ordem, sempre em série (nunca duas varreduras da mesma Raiz ao mesmo tempo):
//! 1. detecta as Execuções Interrompidas e, se houve queda, tira todas as Raízes do Ponto de
//!    Confiança;
//! 2. retoma, uma a uma, de onde pararam (checkpoint do inventário ou fila de hash);
//! 3. passadas de Fast Scan (o "Full Scan sem hash": revisita tudo, compara metadados e só
//!    hasheia o que é novo ou mudou) nas Raízes retomadas e nas que a política manda verificar,
//!    até uma passada sem mudança alguma (teto de passadas);
//! 4. com o monitor ligado, Raiz que convergiu entra no Ponto de Confiança.

use std::collections::HashSet;
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::str::FromStr;

use dag_core::{ExecucaoId, OpcoesHash, PassoAbertura, Resposta, TipoPasso};
use dag_store::inventario::{self, ExecucaoInterrompida};
use dag_store::{confianca, reconciliacao};

use crate::agente::{Agente, agora};
use crate::erro::ErroAgente;

/// Quando verificar as Raízes na abertura.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VerificarAoAbrir {
    /// Toda Raiz disponível, sempre (padrão): fechar o programa não garante que nada mudou.
    #[default]
    Sempre,
    /// Só as Raízes fora do Ponto de Confiança.
    NaoConfiaveis,
    /// Nenhuma (as Raízes retomadas ainda recebem as passadas de alcance).
    Nunca,
}

impl FromStr for VerificarAoAbrir {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "sempre" => Ok(Self::Sempre),
            "nao-confiaveis" | "nao_confiaveis" | "não-confiáveis" => Ok(Self::NaoConfiaveis),
            "nunca" => Ok(Self::Nunca),
            outro => Err(format!(
                "valor inválido `{outro}`; use `sempre`, `nao-confiaveis` ou `nunca`"
            )),
        }
    }
}

impl fmt::Display for VerificarAoAbrir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sempre => "sempre",
            Self::NaoConfiaveis => "nao-confiaveis",
            Self::Nunca => "nunca",
        })
    }
}

/// Política de abertura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoliticaAbertura {
    /// Retoma sozinha as Execuções Interrompidas (padrão: sim).
    pub retomar_automaticamente: bool,
    /// Quando verificar as Raízes (padrão: sempre).
    pub verificar: VerificarAoAbrir,
    /// Teto de passadas de verificação por abertura (padrão 3).
    pub max_passadas: u32,
    /// Há um monitor que vai manter as Raízes em dia depois desta rotina? Só então uma passada
    /// sem mudanças marca o Ponto de Confiança.
    pub monitor_ativo: bool,
}

impl PoliticaAbertura {
    /// Nada ao abrir (testes e quem não quer a recuperação).
    #[must_use]
    pub fn desligada() -> Self {
        Self {
            retomar_automaticamente: false,
            verificar: VerificarAoAbrir::Nunca,
            max_passadas: 1,
            monitor_ativo: false,
        }
    }
}

impl Default for PoliticaAbertura {
    fn default() -> Self {
        Self {
            retomar_automaticamente: true,
            verificar: VerificarAoAbrir::Sempre,
            max_passadas: 3,
            monitor_ativo: false,
        }
    }
}

/// Uma passada de verificação.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassadaAbertura {
    pub execucao: i64,
    /// Entradas novas, alteradas, renomeadas ou ausentes encontradas.
    pub mudancas: u64,
    /// Arquivos que tiveram o conteúdo lido nesta passada (só os novos ou alterados).
    pub hasheados: u64,
}

/// O que a rotina de abertura fez.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelatorioAbertura {
    /// O processo anterior morreu sem encerrar (execução em andamento ou monitor sem `parado`).
    pub encerramento_nao_limpo: bool,
    /// Execuções detectadas como interrompidas.
    pub interrompidas: Vec<i64>,
    /// As que foram retomadas até o fim.
    pub retomadas: Vec<i64>,
    /// Interrompidas que ficaram para o usuário (retomada automática desligada).
    pub nao_retomadas: Vec<i64>,
    pub raizes_verificadas: Vec<String>,
    pub passadas: Vec<PassadaAbertura>,
    /// Raízes que entraram no Ponto de Confiança.
    pub raizes_confiaveis: Vec<String>,
    /// Raízes verificadas que não chegaram a uma passada sem mudanças (teto) ou com erro.
    pub raizes_sem_confianca: Vec<String>,
    /// A rotina foi interrompida (cancelamento ou encerramento); o que sobrou continua retomável.
    pub cancelada: bool,
    pub avisos: Vec<String>,
    /// Os Grupos de Duplicados estavam desatualizados e foram reconstruídos pela abertura.
    pub duplicados_reconstruidos: bool,
}

impl RelatorioAbertura {
    /// Algo aconteceu que o usuário deve ver?
    #[must_use]
    pub fn relevante(&self) -> bool {
        self.encerramento_nao_limpo
            || !self.interrompidas.is_empty()
            || !self.passadas.is_empty()
            || self.duplicados_reconstruidos
            || !self.avisos.is_empty()
    }
}

impl fmt::Display for RelatorioAbertura {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "abertura: {} interrompida(s), {} retomada(s), {} passada(s) de verificação, {} raiz(es) no Ponto de Confiança",
            self.interrompidas.len(),
            self.retomadas.len(),
            self.passadas.len(),
            self.raizes_confiaveis.len()
        )?;
        if !self.raizes_sem_confianca.is_empty() {
            write!(f, ", {} sem confiança", self.raizes_sem_confianca.len())?;
        }
        if self.cancelada {
            f.write_str(" (interrompida)")?;
        }
        Ok(())
    }
}

impl Agente {
    /// Recuperação ao abrir (ver o módulo). Só o escritor.
    ///
    /// Falhas de uma varredura individual viram avisos no relatório; só falhas do próprio
    /// Catálogo são erro.
    ///
    /// # Errors
    /// [`ErroAgente::Invalido`] se esta instância não é a escritora; falhas do Catálogo.
    pub fn recuperar_ao_abrir(
        &mut self,
        politica: &PoliticaAbertura,
    ) -> Result<RelatorioAbertura, ErroAgente> {
        let mut rel = self.recuperar_etapas(politica)?;
        // Hashes existem mas os Grupos de Duplicados nunca foram construídos (ou são mais antigos
        // que a última mudança de hash): reconstrói agora, no escritor.
        if !rel.cancelada
            && !self.controle.cancelado()
            && dag_store::hashes::grupos_desatualizados(self.catalogo.armazem().conexao())?
        {
            self.mensagem("abertura: Grupos de Duplicados desatualizados; reconstruindo...");
            match self.atualizar_duplicados() {
                Ok(_) => rel.duplicados_reconstruidos = true,
                Err(e) => rel
                    .avisos
                    .push(format!("Grupos de Duplicados não foram reconstruídos: {e}")),
            }
        }
        Ok(rel)
    }

    fn recuperar_etapas(
        &mut self,
        politica: &PoliticaAbertura,
    ) -> Result<RelatorioAbertura, ErroAgente> {
        if !self.eh_escritor() {
            return Err(ErroAgente::Invalido(
                "só o escritor recupera o Catálogo ao abrir".into(),
            ));
        }
        let mut rel = RelatorioAbertura::default();
        let interrompidas = self.detectar_queda(politica, &mut rel)?;
        rel.interrompidas = interrompidas.iter().map(|e| e.id).collect();
        if rel.encerramento_nao_limpo {
            self.mensagem(
                "abertura: o processo anterior não encerrou; Raízes saem do Ponto de Confiança",
            );
        }

        // 2. Retomada, uma execução por vez, a mais antiga primeiro.
        let mut alcance: Vec<PathBuf> = Vec::new();
        for e in &interrompidas {
            if !politica.retomar_automaticamente {
                rel.nao_retomadas.push(e.id);
                self.mensagem(format!(
                    "abertura: execução {} interrompida (retomada automática desligada; use `dag scan --retomar {}`)",
                    e.id, e.id
                ));
                continue;
            }
            self.passo(PassoAbertura {
                tipo: TipoPasso::Retomada,
                execucao: Some(e.id),
                modo: if e.modo == "fast" {
                    "Fast Scan".into()
                } else {
                    "Full Scan".into()
                },
                no_inventario: e.no_inventario,
                passada: 0,
                de: 0,
            });
            self.mensagem(format!(
                "abertura: etapa 1 de 2: retomando a execução #{} ({})",
                e.id,
                if e.no_inventario {
                    "inventário"
                } else {
                    "hash"
                }
            ));
            match self.retomar_scan(ExecucaoId(e.id), OpcoesHash::default()) {
                Ok(Resposta::ScanConcluido { desfecho, .. }) if desfecho == "cancelada" => {
                    rel.cancelada = true;
                    return Ok(rel);
                }
                Ok(Resposta::ScanConcluido { .. }) => {
                    rel.retomadas.push(e.id);
                    inventario::limpar_retomada_automatica(
                        self.catalogo.armazem().conexao(),
                        e.id,
                    )?;
                    alcance.extend(e.raizes.iter().map(PathBuf::from));
                }
                Ok(Resposta::Erro { mensagem, .. }) => {
                    rel.avisos
                        .push(format!("execução {} não foi retomada: {mensagem}", e.id));
                }
                Ok(_) => {}
                Err(err) => rel
                    .avisos
                    .push(format!("execução {} não foi retomada: {err}", e.id)),
            }
            if self.controle.cancelado() {
                rel.cancelada = true;
                return Ok(rel);
            }
        }

        // 3. Passadas de verificação.
        let disponiveis = self.raizes_disponiveis()?;
        let mut alvo: Vec<PathBuf> = Vec::new();
        let mut vistos: HashSet<Vec<u8>> = HashSet::new();
        let mut incluir = |c: PathBuf, alvo: &mut Vec<PathBuf>| {
            if vistos.insert(c.as_os_str().as_bytes().to_vec()) {
                alvo.push(c);
            }
        };
        for c in alcance.iter().filter(|c| c.is_dir()) {
            incluir(c.clone(), &mut alvo);
        }
        let com_alcance = !alvo.is_empty();
        for r in &disponiveis {
            let verificar = match politica.verificar {
                VerificarAoAbrir::Sempre => true,
                VerificarAoAbrir::NaoConfiaveis => !confianca::eh_confiavel(
                    self.catalogo.armazem().conexao(),
                    r.volume_id,
                    &r.path_raw,
                )?,
                VerificarAoAbrir::Nunca => false,
            };
            if verificar {
                incluir(r.caminho.clone(), &mut alvo);
            }
        }
        if alvo.is_empty() {
            return Ok(rel);
        }
        rel.raizes_verificadas = alvo.iter().map(|c| c.display().to_string()).collect();
        let strs: Vec<String> = alvo
            .iter()
            .map(|c| c.to_string_lossy().into_owned())
            .collect();
        // Sem monitor depois, uma passada basta (não há Ponto de Confiança a provar); depois de
        // uma retomada, o alcance sempre repete até estabilizar.
        let teto = if politica.monitor_ativo || com_alcance {
            politica.max_passadas.max(1)
        } else {
            1
        };
        let mut convergiu = false;
        for n in 1..=teto {
            self.passo(PassoAbertura {
                tipo: TipoPasso::Verificacao,
                execucao: None,
                modo: "Fast Scan".into(),
                no_inventario: false,
                passada: n,
                de: teto,
            });
            self.mensagem(format!(
                "abertura: etapa 2 de 2: passada {n}/{teto} de verificação (Fast Scan, hash só do que é novo ou mudou)"
            ));
            let r = self.fast_scan(&strs, false, false, OpcoesHash::default());
            match r {
                Ok(Resposta::ScanConcluido {
                    execucao,
                    desfecho,
                    hash,
                    reconciliacao,
                    ..
                }) => {
                    if desfecho == "cancelada" {
                        rel.cancelada = true;
                        return Ok(rel);
                    }
                    let mudancas = reconciliacao.as_ref().map_or(0, |x| x.mudancas());
                    rel.passadas.push(PassadaAbertura {
                        execucao: execucao.0,
                        mudancas,
                        hasheados: hash.as_ref().map_or(0, |h| h.hasheados),
                    });
                    let limpa = desfecho == "concluida"
                        && reconciliacao.as_ref().is_some_and(|x| x.sem_mudancas())
                        && hash.as_ref().is_none_or(|h| h.erros == 0);
                    if limpa {
                        convergiu = true;
                        break;
                    }
                    // Sem mudanças a perseguir (erros de leitura, por exemplo): repetir não
                    // ajuda.
                    if mudancas == 0 {
                        break;
                    }
                }
                Ok(Resposta::Erro { mensagem, .. }) => {
                    rel.avisos.push(format!("verificação falhou: {mensagem}"));
                    break;
                }
                Ok(_) => break,
                Err(e) => {
                    rel.avisos.push(format!("verificação falhou: {e}"));
                    break;
                }
            }
            if self.controle.cancelado() {
                rel.cancelada = true;
                return Ok(rel);
            }
        }

        // 4. Ponto de Confiança.
        if politica.monitor_ativo {
            let t = agora();
            for r in &disponiveis {
                if !alvo.iter().any(|c| c == &r.caminho) {
                    continue;
                }
                let nome = r.caminho.display().to_string();
                if convergiu {
                    confianca::marcar_confiavel(
                        self.catalogo.armazem().conexao(),
                        r.volume_id,
                        &r.path_raw,
                        t,
                    )?;
                    rel.raizes_confiaveis.push(nome);
                } else {
                    confianca::perder(
                        self.catalogo.armazem().conexao(),
                        r.volume_id,
                        &r.path_raw,
                        "a verificação não chegou a uma passada sem mudanças",
                        t,
                    )?;
                    rel.raizes_sem_confianca.push(nome);
                }
            }
        }
        if !convergiu {
            rel.avisos.push(format!(
                "as Raízes não estabilizaram em {teto} passada(s); sem Ponto de Confiança por ora"
            ));
        }
        Ok(rel)
    }

    /// Passo 1: marca as Execuções Interrompidas e, se o processo anterior caiu, limpa o Ponto de
    /// Confiança e a saúde do monitor.
    fn detectar_queda(
        &mut self,
        politica: &PoliticaAbertura,
        rel: &mut RelatorioAbertura,
    ) -> Result<Vec<ExecucaoInterrompida>, ErroAgente> {
        let t = agora();
        let auto = politica.retomar_automaticamente;
        let (interrompidas, marcadas) = self.catalogo.armazem_mut().transacao_imediata(
            |tx| -> Result<(Vec<ExecucaoInterrompida>, usize), ErroAgente> {
                let antes: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM scan_runs WHERE status IN ('em_andamento','pausada')",
                    [],
                    |l| l.get(0),
                )?;
                let lista = inventario::detectar_interrompidas(tx, auto, t)?;
                Ok((lista, usize::try_from(antes).unwrap_or(0)))
            },
        )?;
        let saude = reconciliacao::ler_saude(self.catalogo.armazem().conexao())?;
        let monitor_morto = saude.estado != "parado";
        rel.encerramento_nao_limpo = marcadas > 0 || !interrompidas.is_empty() || monitor_morto;
        if rel.encerramento_nao_limpo {
            let motivo = "encerramento não limpo do processo anterior";
            confianca::perder_todas(self.catalogo.armazem().conexao(), motivo, t)?;
            if monitor_morto {
                let mut s = saude;
                s.estado = "parado".into();
                s.erro = Some(motivo.into());
                reconciliacao::gravar_saude(self.catalogo.armazem().conexao(), &s)?;
            }
        }
        Ok(interrompidas)
    }
}
