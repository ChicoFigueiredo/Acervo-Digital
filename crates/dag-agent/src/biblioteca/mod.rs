//! Curadoria Bibliográfica (Fase B) no agente: bibliotecas Calibre, sincronização da Homologada,
//! duplicatas exatas, fila persistente, extração, sugestões, revisão, cadastro na A Classificar,
//! homologação controlada, alertas e relatórios.
//!
//! Regras que valem em todo o módulo:
//! - a Biblioteca Homologada é somente leitura (SQLite `mode=ro`; escrita só pelo caminho da
//!   homologação aprovada por pessoa);
//! - nada move, apaga ou altera arquivos catalogados: o descarte segue o fluxo da Fase A
//!   (Marcação para Deleção + Lixeira do Volume) com dupla confirmação;
//! - só o agente escreve no Catálogo (ADR 0004); IA e MCP nunca iniciam descarte nem aprovam
//!   homologação;
//! - contêineres e imagens são lidos em memória (ADR 0006).

mod bibliotecas;
mod cadastro;
mod cobertura;
mod duplicatas;
mod extracao;
mod fila;
mod item;
mod processar;
mod relatorios;
mod revisao;
mod segredo;
mod vocab;
pub(crate) mod watcher;

pub use watcher::ResumoVigia;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dag_biblio::FormatoLivro;
use dag_calibre::calibredb::ClienteCalibredb;
use dag_calibre::erro::Resultado as ResultadoCalibre;
use dag_calibre::executor::{
    Comando as ComandoCalibre, ExecutorCalibre, ExecutorProcesso, SaidaProcesso,
};
use dag_calibre::trava::{PoliticaTrava, ProcessoCalibre};
use dag_core::{
    BibliotecaInfo, Comando, ConfigBiblioteca, DadosBiblioteca, PapelRegistro,
    PoliticaTravaCalibre, ProgressoBiblioteca, Resposta,
};
use dag_scanner::EstadoControle;
use dag_store::biblioteca as store;
use rusqlite::Connection;

pub use extracao::{
    ClassificadorIa, ExtratorIndisponivel, ExtratorLivro, ExtratorPadrao, IaIndisponivel, IaLocal,
    SaidaExtracao, SaidaMembro,
};

use crate::agente::{Agente, agora};
use crate::erro::ErroAgente;

/// Chave da configuração no Catálogo.
pub(crate) const CHAVE_CONFIG: &str = "biblioteca.config";
/// Instante (segundos) de `Agente::agora` para estas rotinas: injetável nos testes.
pub(crate) type Relogio = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Estado e pontos de encaixe da curadoria dentro do `Agente`.
pub(crate) struct ContextoBiblioteca {
    pub extrator: Arc<dyn ExtratorLivro>,
    /// `None` = cria a partir da configuração quando preciso.
    pub ia: Option<Arc<dyn ClassificadorIa>>,
    /// `None` = `calibredb` do `PATH`.
    pub executor: Option<Arc<dyn ExecutorCalibre>>,
    /// Processos Calibre conhecidos (testes); `None` = inspeciona `/proc`.
    pub processos: Option<Vec<ProcessoCalibre>>,
    pub relogio: Option<Relogio>,
    pub ultima_mensagem: Option<Instant>,
    /// Classificador de vocabulário treinado (cache; refeito quando o espelho ou as decisões mudam).
    pub vocab: Option<Arc<vocab::CacheVocab>>,
}

impl Default for ContextoBiblioteca {
    fn default() -> Self {
        Self {
            extrator: Arc::new(ExtratorPadrao::novo()),
            ia: None,
            executor: None,
            processos: None,
            relogio: None,
            ultima_mensagem: None,
            vocab: None,
        }
    }
}

/// `ExecutorCalibre` sobre um `Arc<dyn ExecutorCalibre>`.
#[derive(Debug, Clone)]
pub(crate) struct ExecDinamico(pub Arc<dyn ExecutorCalibre>);

impl ExecutorCalibre for ExecDinamico {
    fn executar(&self, cmd: &ComandoCalibre) -> ResultadoCalibre<SaidaProcesso> {
        self.0.executar(cmd)
    }
}

impl Agente {
    /// Troca o extrator de livros (padrão: PDF/DjVu reais, e-books via `dag-biblio-ebook`).
    #[must_use]
    pub fn com_extrator_livro(mut self, e: Arc<dyn ExtratorLivro>) -> Self {
        self.biblio.extrator = e;
        self
    }

    /// Troca a IA (padrão: Ollama local, criada da configuração quando a IA está ativa).
    #[must_use]
    pub fn com_classificador_ia(mut self, c: Arc<dyn ClassificadorIa>) -> Self {
        self.biblio.ia = Some(c);
        self
    }

    /// Troca o executor do `calibredb` (testes).
    #[must_use]
    pub fn com_executor_calibre(mut self, e: Arc<dyn ExecutorCalibre>) -> Self {
        self.biblio.executor = Some(e);
        self
    }

    /// Fixa os processos Calibre conhecidos (testes da trava de escrita).
    #[must_use]
    pub fn com_processos_calibre(mut self, p: Vec<ProcessoCalibre>) -> Self {
        self.biblio.processos = Some(p);
        self
    }

    /// Relógio injetável (testes de debounce): segundos Unix.
    #[must_use]
    pub fn com_relogio_biblioteca(mut self, r: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.biblio.relogio = Some(Arc::new(r));
        self
    }

    pub(crate) fn agora_biblio(&self) -> i64 {
        self.biblio.relogio.as_ref().map_or_else(agora, |r| r())
    }

    pub(crate) fn conexao(&self) -> &Connection {
        self.catalogo.armazem().conexao()
    }

    pub(crate) fn config_biblioteca(&self) -> ConfigBiblioteca {
        ler_config(self.conexao())
    }

    /// A configuração como vai às respostas: `homologacao.senha_no_cofre` diz se há senha do
    /// servidor guardada; a senha em si nunca sai.
    pub(crate) fn config_biblioteca_publica(&self) -> ConfigBiblioteca {
        let mut c = self.config_biblioteca();
        c.homologacao.senha_no_cofre = segredo::tem_senha_servidor(self.conexao());
        c
    }

    /// Fluxo do trabalho: segue, ou para (cancelado). Pausado espera sem consumir CPU.
    pub(crate) fn seguir_biblioteca(&self) -> bool {
        loop {
            match self.controle.estado() {
                EstadoControle::Executando => return true,
                EstadoControle::Pausado => std::thread::sleep(Duration::from_millis(100)),
                EstadoControle::Cancelado => return false,
            }
        }
    }

    /// Enquanto a guarda viver, o cancelamento (Ctrl-C, `dag cancelar`) interrompe a consulta
    /// SQL em curso (`sqlite3_interrupt`). Só para consultas de leitura longas: a escrita não
    /// passa por aqui.
    pub(crate) fn guarda_interrupcao(&self) -> GuardaInterrupcao {
        let alca = self.conexao().get_interrupt_handle();
        let controle = self.controle.clone();
        let fim = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fim2 = Arc::clone(&fim);
        let vigia = std::thread::spawn(move || {
            while !fim2.load(std::sync::atomic::Ordering::Relaxed) {
                if controle.estado() == EstadoControle::Cancelado {
                    alca.interrupt();
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        GuardaInterrupcao {
            fim,
            vigia: Some(vigia),
        }
    }

    /// Progresso no painel de trabalhos (vai como mensagem de etapa; no máximo 2 por segundo).
    pub(crate) fn progresso_biblioteca(&mut self, p: &ProgressoBiblioteca, forcar: bool) {
        let ok = forcar
            || self
                .biblio
                .ultima_mensagem
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(500));
        if ok {
            self.biblio.ultima_mensagem = Some(Instant::now());
            self.mensagem(p.texto());
        }
    }

    /// Cliente do `calibredb` conforme a configuração (prefixo `unshare`, política de trava).
    pub(crate) fn cliente_calibre(
        &self,
        cfg: &ConfigBiblioteca,
        homologada: Option<&Path>,
    ) -> Result<ClienteCalibredb<ExecDinamico>, ErroAgente> {
        let exec: Arc<dyn ExecutorCalibre> = match &self.biblio.executor {
            Some(e) => Arc::clone(e),
            None => {
                let e = ExecutorProcesso::disponivel().ok_or_else(|| {
                    ErroAgente::biblio(
                        "calibredb_ausente",
                        "o `calibredb` não foi encontrado no PATH; instale o Calibre",
                    )
                })?;
                let e = if cfg.usar_unshare {
                    e.com_prefixo(ExecutorProcesso::prefixo_unshare())
                } else {
                    e
                };
                Arc::new(e)
            }
        };
        let politica = match cfg.politica_trava {
            PoliticaTravaCalibre::Estrita => PoliticaTrava::Estrita,
            PoliticaTravaCalibre::ApenasServidor => PoliticaTrava::ApenasServidor,
        };
        let mut c = ClienteCalibredb::novo(ExecDinamico(exec)).com_politica(politica);
        if let Some(p) = &self.biblio.processos {
            c = c.com_processos(p.clone());
        }
        if let Some(h) = homologada {
            c = c.proteger_homologada(h)?;
        }
        Ok(c)
    }

    /// Despacha os comandos `Biblioteca*`.
    pub(crate) fn executar_biblioteca(&mut self, comando: Comando) -> Result<Resposta, ErroAgente> {
        match comando {
            Comando::BibliotecaRegistrar {
                caminho,
                papel,
                nome,
            } => self.registrar_biblioteca(&caminho, papel, nome.as_deref()),
            Comando::BibliotecaListar => self.listar_bibliotecas_cmd(),
            Comando::BibliotecaPerfil { biblioteca } => self.perfil_biblioteca(&biblioteca),
            Comando::BibliotecaSincronizar { biblioteca } => {
                self.sincronizar_biblioteca(&biblioteca)
            }
            Comando::BibliotecaDuplicatas {
                raiz,
                limite,
                cursor,
            } => self.duplicatas(raiz.as_deref(), limite, cursor.as_deref()),
            Comando::BibliotecaEnfileirar { raizes, motivo } => {
                self.enfileirar_biblioteca(&raizes, motivo)
            }
            Comando::BibliotecaFila {
                estado,
                limite,
                cursor,
            } => self.fila_biblioteca(estado, limite, cursor.as_deref()),
            Comando::BibliotecaProcessar { limite, ia } => self.processar_biblioteca(limite, ia),
            Comando::BibliotecaRevisao { limite, cursor } => {
                self.revisao_biblioteca(limite, cursor.as_deref())
            }
            Comando::BibliotecaDecidir { itens, lote } => {
                self.decidir_biblioteca(&itens, lote.as_deref())
            }
            Comando::BibliotecaDesfazer { lote } => self.desfazer_biblioteca(&lote),
            Comando::BibliotecaCadastrar { itens } => self.cadastrar_biblioteca(&itens),
            Comando::BibliotecaCadastros {
                estado,
                limite,
                cursor,
            } => self.cadastros_biblioteca(estado, limite, cursor.as_deref()),
            Comando::BibliotecaHomologar { item, aprovacao } => {
                self.homologar_biblioteca(item, aprovacao)
            }
            Comando::BibliotecaAlertas {
                nao_vistos,
                limite,
                cursor,
            } => self.alertas_biblioteca(nao_vistos, limite, cursor.as_deref()),
            Comando::BibliotecaMarcarVisto { ids } => self.marcar_vistos_biblioteca(&ids),
            Comando::BibliotecaResumo { desde } => self.resumo_biblioteca(desde),
            Comando::BibliotecaRelatorioEspaco { volume } => {
                self.relatorio_espaco_biblioteca(volume.as_deref())
            }
            Comando::BibliotecaVerificarCobertura {
                biblioteca,
                amostra,
            } => self.verificar_cobertura(&biblioteca, amostra),
            Comando::BibliotecaConfigObter => Ok(dados(DadosBiblioteca::Config {
                config: self.config_biblioteca_publica(),
            })),
            Comando::BibliotecaConfigDefinir { alteracoes } => self.definir_config(&alteracoes),
            _ => Err(ErroAgente::NaoImplementado(0)),
        }
    }
}

/// O comando é da curadoria bibliográfica?
#[must_use]
pub(crate) fn e_biblioteca(c: &Comando) -> bool {
    matches!(
        c,
        Comando::BibliotecaRegistrar { .. }
            | Comando::BibliotecaListar
            | Comando::BibliotecaPerfil { .. }
            | Comando::BibliotecaSincronizar { .. }
            | Comando::BibliotecaDuplicatas { .. }
            | Comando::BibliotecaEnfileirar { .. }
            | Comando::BibliotecaFila { .. }
            | Comando::BibliotecaProcessar { .. }
            | Comando::BibliotecaRevisao { .. }
            | Comando::BibliotecaDecidir { .. }
            | Comando::BibliotecaDesfazer { .. }
            | Comando::BibliotecaCadastrar { .. }
            | Comando::BibliotecaCadastros { .. }
            | Comando::BibliotecaHomologar { .. }
            | Comando::BibliotecaAlertas { .. }
            | Comando::BibliotecaMarcarVisto { .. }
            | Comando::BibliotecaResumo { .. }
            | Comando::BibliotecaRelatorioEspaco { .. }
            | Comando::BibliotecaVerificarCobertura { .. }
            | Comando::BibliotecaConfigObter
            | Comando::BibliotecaConfigDefinir { .. }
    )
}

/// Comandos da curadoria que só leem o Catálogo (valem num agente sem a trava de escritor).
/// Perfil, listagens e revisão leem; `Duplicatas`, `Alertas` (ao ler também não escrevem) e os
/// demais seguem o que cada um faz: `Duplicatas` grava candidatos e alertas, então não é leitura.
#[must_use]
pub(crate) fn somente_leitura_biblioteca(c: &Comando) -> bool {
    matches!(
        c,
        Comando::BibliotecaListar
            | Comando::BibliotecaPerfil { .. }
            | Comando::BibliotecaFila { .. }
            | Comando::BibliotecaRevisao { .. }
            | Comando::BibliotecaCadastros { .. }
            | Comando::BibliotecaAlertas { .. }
            | Comando::BibliotecaResumo { .. }
            | Comando::BibliotecaRelatorioEspaco { .. }
            | Comando::BibliotecaConfigObter
    )
}

/// Comandos que são trabalho longo (job `TipoJob::Biblioteca` no agente em segundo plano).
#[must_use]
pub(crate) fn e_trabalho_longo_biblioteca(c: &Comando) -> bool {
    matches!(
        c,
        Comando::BibliotecaSincronizar { .. }
            | Comando::BibliotecaDuplicatas { .. }
            | Comando::BibliotecaEnfileirar { .. }
            | Comando::BibliotecaProcessar { .. }
            | Comando::BibliotecaCadastrar { .. }
            | Comando::BibliotecaHomologar { .. }
            | Comando::BibliotecaVerificarCobertura { .. }
    )
}

pub(crate) fn dados(d: DadosBiblioteca) -> Resposta {
    Resposta::Biblioteca { dados: d }
}

/// Configuração persistida (campos ausentes ou ilegíveis voltam ao padrão).
pub(crate) fn ler_config(conn: &Connection) -> ConfigBiblioteca {
    dag_store::configuracao::ler(conn, CHAVE_CONFIG)
        .ok()
        .flatten()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub(crate) fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

pub(crate) fn hash_de_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// Formato bibliográfico pela extensão do nome, só os que o DAG cataloga como livro (exclui
/// texto puro, RTF, DOCX e HTML, que não são livros por si).
pub(crate) fn formato_do_nome(nome: &str) -> Option<FormatoLivro> {
    let ext = Path::new(nome).extension()?.to_str()?;
    match FormatoLivro::da_extensao(ext)? {
        FormatoLivro::Txt | FormatoLivro::Rtf | FormatoLivro::Docx | FormatoLivro::Html => None,
        f => Some(f),
    }
}

/// BLAKE3 de um arquivo (leitura em fluxo, só leitura).
pub(crate) fn blake3_arquivo(caminho: &Path) -> std::io::Result<[u8; 32]> {
    use std::io::Read;
    let mut f = std::fs::File::open(caminho)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(*h.finalize().as_bytes())
}

/// Resolve uma biblioteca por id numérico ou caminho.
pub(crate) fn resolver_biblioteca(
    conn: &Connection,
    ref_: &str,
) -> Result<store::BibliotecaCalibre, ErroAgente> {
    let achada = if let Ok(id) = ref_.trim().parse::<i64>() {
        store::obter_biblioteca(conn, id)?
    } else {
        let c = std::fs::canonicalize(ref_.trim()).map_or_else(
            |_| ref_.trim().to_owned(),
            |p| p.to_string_lossy().into_owned(),
        );
        store::biblioteca_por_caminho(conn, &c)?
    };
    achada.ok_or_else(|| {
        ErroAgente::biblio(
            "biblioteca_nao_registrada",
            format!("a biblioteca `{ref_}` não está registrada (use `BibliotecaRegistrar`)"),
        )
    })
}

pub(crate) fn papel_de(p: dag_biblio::PapelBiblioteca) -> PapelRegistro {
    match p {
        dag_biblio::PapelBiblioteca::Homologada => PapelRegistro::Homologada,
        dag_biblio::PapelBiblioteca::AClassificar => PapelRegistro::AClassificar,
    }
}

/// Primeira Biblioteca Homologada ativa.
pub(crate) fn homologada_ativa(
    conn: &Connection,
) -> Result<Option<store::BibliotecaCalibre>, ErroAgente> {
    Ok(
        store::listar_bibliotecas(conn, Some(dag_biblio::PapelBiblioteca::Homologada))?
            .into_iter()
            .find(|b| b.estado == store::EstadoBiblioteca::Ativa),
    )
}

pub(crate) fn info_biblioteca(
    conn: &Connection,
    b: &store::BibliotecaCalibre,
) -> Result<BibliotecaInfo, ErroAgente> {
    let (livros, formatos) = match b.papel {
        dag_biblio::PapelBiblioteca::Homologada => (
            store::contar_livros(conn, b.id)?,
            store::contar_formatos(conn, b.id)?,
        ),
        dag_biblio::PapelBiblioteca::AClassificar => {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM cadastros_a_classificar WHERE biblioteca_id = ?1 AND estado = 'cadastrado'",
                [b.id],
                |l| l.get(0),
            )?;
            (n, n)
        }
    };
    let ult = store::ultimo_instantaneo(conn, b.id)?;
    Ok(BibliotecaInfo {
        id: b.id,
        caminho: b.caminho.clone(),
        papel: papel_de(b.papel),
        nome: b.nome.clone(),
        estado: format!("{:?}", b.estado).to_lowercase(),
        versao_calibre: b.versao_calibre.clone(),
        library_uuid: b.library_uuid.clone(),
        livros,
        formatos,
        ultima_sincronizacao: ult.as_ref().map(|u| u.quando),
        ultimo_resultado: ult.map(|u| format!("{:?}", u.resultado).to_lowercase()),
    })
}

/// Cursor opaco `a:b` (inteiros).
pub(crate) fn cursor_par(c: Option<&str>) -> Option<(i64, i64)> {
    let (a, b) = c?.split_once(':')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Cursor inteiro simples.
pub(crate) fn cursor_i64(c: Option<&str>) -> Option<i64> {
    c?.parse().ok()
}

pub(crate) const PAGINA_BIBLIO: u32 = 50;
pub(crate) const PAGINA_BIBLIO_MAX: u32 = 500;

pub(crate) fn limite_pagina(l: Option<u32>) -> usize {
    l.unwrap_or(PAGINA_BIBLIO).clamp(1, PAGINA_BIBLIO_MAX) as usize
}

/// Ver [`Agente::guarda_interrupcao`]. Ao sair de escopo, para o vigia.
pub(crate) struct GuardaInterrupcao {
    fim: Arc<std::sync::atomic::AtomicBool>,
    vigia: Option<std::thread::JoinHandle<()>>,
}

impl Drop for GuardaInterrupcao {
    fn drop(&mut self) {
        self.fim.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(v) = self.vigia.take() {
            let _ = v.join();
        }
    }
}

#[cfg(test)]
mod testes_interrupcao {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn cancelamento_interrompe_consulta_longa() {
        let t = tempfile::tempdir().unwrap();
        let a = Agente::abrir(t.path().join("c.sqlite")).unwrap();
        let controle = a.controle();
        let guarda = a.guarda_interrupcao();
        let cancelador = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            controle.cancelar();
        });
        let inicio = Instant::now();
        // Consulta artificial que levaria minutos.
        let r: rusqlite::Result<i64> = a.conexao().query_row(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000000000)
             SELECT count(*) FROM n",
            [],
            |l| l.get(0),
        );
        drop(guarda);
        cancelador.join().unwrap();
        assert!(r.is_err(), "devia ter sido interrompida: {r:?}");
        assert!(
            inicio.elapsed() < Duration::from_secs(10),
            "{:?}",
            inicio.elapsed()
        );
    }
}
