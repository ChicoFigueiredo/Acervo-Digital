//! `dag modelos listar|benchmark`: perfis de IA local e benchmark contra casos com gabarito.
//! Não usa o agente: lê livros (casos já extraídos) e fala só com o Ollama/llama-server em
//! loopback, via `dag_biblio_ia`. O texto dos livros nunca sai da máquina.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::Subcommand;
use dag_biblio_ia::benchmark::{
    OpcoesBenchmark, RelatorioBenchmark, StatusModelo, carregar_casos, executar_benchmark,
};
use dag_biblio_ia::modelo::{ConfigIa, PapelModelo, PerfilModelo};
use dag_biblio_ia::{Classificador, ClienteLlm, ConfigCliente};
use serde::Serialize;

/// `dag modelos ...`
#[derive(Subcommand, Debug)]
pub enum Modelos {
    /// Perfis de modelo configurados e o que o servidor local responde
    Listar {
        /// JSON de perfis (formato de `dag-biblio-ia/exemplos/modelos.json`); padrão: os
        /// perfis embutidos (gemma3:12b e o segundo parecer)
        #[arg(long, value_name = "ARQUIVO.json")]
        perfis: Option<PathBuf>,
    },
    /// Roda os casos com gabarito contra um ou mais modelos e mede precisão, tempo e VRAM
    Benchmark {
        /// Pasta com os casos (JSON), por exemplo a do `dag biblioteca ouro exportar`
        #[arg(long, value_name = "DIR")]
        casos: PathBuf,
        /// Modelos a comparar (nome no servidor; se estiver nos perfis, usa o perfil)
        #[arg(long, required = true, num_args = 1.., value_name = "NOME")]
        modelo: Vec<String>,
        /// Grava o relatório (.md; com extensão .json, em JSON). Arquivo novo, nunca sobrescreve
        #[arg(long, value_name = "RELATORIO.md")]
        saida: Option<PathBuf>,
        /// JSON de perfis (como em `listar`)
        #[arg(long, value_name = "ARQUIVO.json")]
        perfis: Option<PathBuf>,
        /// Endereço do servidor para modelos que não estão nos perfis (padrão: Ollama local)
        #[arg(long, value_name = "URL")]
        endpoint: Option<String>,
    },
}

/// O que um perfil responde.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LinhaModelo {
    pub nome: String,
    pub papel: String,
    pub backend: String,
    pub endpoint: String,
    pub multimodal: bool,
    pub quantizacao: Option<String>,
    pub nota: Option<String>,
    /// `responde`, `inalcançável: ...` ou `recusado: ...`.
    pub servidor: String,
    pub modelo_no_servidor: Option<String>,
    pub contexto: Option<u64>,
    pub avisos: Vec<String>,
}

fn carregar_perfis(arq: Option<&Path>) -> Result<ConfigIa, String> {
    match arq {
        None => Ok(ConfigIa::default()),
        Some(p) => ConfigIa::de_arquivo(p).map_err(|e| format!("perfis {}: {e}", p.display())),
    }
}

fn config_sonda() -> ConfigCliente {
    ConfigCliente {
        timeout: Duration::from_secs(5),
        timeout_conexao: Duration::from_secs(2),
        tentativas: 1,
        ..ConfigCliente::default()
    }
}

/// Pergunta ao servidor de cada perfil se responde.
#[must_use]
pub fn sondar(cfg: &ConfigIa) -> Vec<LinhaModelo> {
    cfg.modelos
        .iter()
        .map(|p| {
            let mut l = LinhaModelo {
                nome: p.nome.clone(),
                papel: format!("{:?}", p.papel).to_lowercase(),
                backend: format!("{:?}", p.backend).to_lowercase(),
                endpoint: p.endpoint.clone(),
                multimodal: p.multimodal,
                quantizacao: p.quantizacao.clone(),
                nota: p.nota.clone(),
                servidor: String::new(),
                modelo_no_servidor: None,
                contexto: None,
                avisos: Vec::new(),
            };
            match ClienteLlm::novo(p.clone(), config_sonda()) {
                Err(e) => l.servidor = format!("recusado: {e}"),
                Ok(c) => match c.saude() {
                    Err(e) => l.servidor = format!("inalcançável: {e}"),
                    Ok(()) => {
                        l.servidor = "responde".into();
                        if let Ok(info) = c.info_servidor() {
                            l.modelo_no_servidor = info.modelo_id;
                            l.contexto = info.n_ctx;
                            if l.quantizacao.is_none() {
                                l.quantizacao = info.quantizacao;
                            }
                            l.avisos = info.avisos;
                        }
                    }
                },
            }
            l
        })
        .collect()
}

/// Tabela de perfis.
#[must_use]
pub fn texto_modelos(linhas: &[LinhaModelo]) -> String {
    let t: Vec<Vec<String>> = linhas
        .iter()
        .map(|l| {
            vec![
                l.nome.clone(),
                l.papel.clone(),
                l.backend.clone(),
                l.endpoint.clone(),
                if l.multimodal { "sim" } else { "não" }.into(),
                l.quantizacao.clone().unwrap_or_else(|| "-".into()),
                l.servidor.clone(),
            ]
        })
        .collect();
    let mut s = crate::saida::tabela(
        &[
            "MODELO", "PAPEL", "BACKEND", "ENDPOINT", "VISÃO", "QUANT.", "SERVIDOR",
        ],
        &[false; 7],
        &t,
    );
    for l in linhas {
        if let Some(m) = &l.modelo_no_servidor {
            s.push_str(&format!(
                "  {}: o servidor diz `{m}`{}\n",
                l.nome,
                l.contexto
                    .map(|c| format!(", contexto {c}"))
                    .unwrap_or_default()
            ));
        }
        for a in &l.avisos {
            s.push_str(&format!("  {}: aviso: {a}\n", l.nome));
        }
        if let Some(n) = &l.nota {
            s.push_str(&format!("  {}: {n}\n", l.nome));
        }
    }
    if linhas.iter().any(|l| l.servidor != "responde") {
        s.push_str("Servidor que não responde: inicie o Ollama (`ollama serve`) e baixe o modelo (`ollama pull NOME`).\n");
    }
    s
}

/// Monta um classificador por nome pedido (perfil configurado ou Ollama padrão).
fn classificadores(
    nomes: &[String],
    cfg: &ConfigIa,
    endpoint: Option<&str>,
) -> Result<Vec<Classificador>, String> {
    let mut v = Vec::new();
    for nome in nomes {
        let perfil = match cfg.modelos.iter().find(|m| &m.nome == nome) {
            Some(p) => p.clone(),
            None => {
                let mut p = PerfilModelo::ollama(nome, false, PapelModelo::Primario);
                if let Some(e) = endpoint {
                    p.endpoint = e.to_string();
                }
                p
            }
        };
        let cliente = ClienteLlm::novo(perfil, ConfigCliente::default())
            .map_err(|e| format!("modelo {nome}: {e}"))?;
        v.push(Classificador::novo(cliente, None));
    }
    Ok(v)
}

/// Resumo curto (uma linha por modelo) para o terminal.
#[must_use]
pub fn resumo_curto(r: &RelatorioBenchmark) -> String {
    let mut s = format!(
        "Benchmark: {} caso(s), {} modelo(s)\n",
        r.casos,
        r.modelos.len()
    );
    for m in &r.modelos {
        match (&m.status, &m.resumo) {
            (StatusModelo::NaoExecutado { motivo }, _) => {
                s.push_str(&format!("  {}: NÃO EXECUTADO ({motivo})\n", m.nome));
            }
            (StatusModelo::Executado, Some(rs)) => s.push_str(&format!(
                "  {}: {}/{} caso(s) ok, precisão global {}, tempo médio {}\n",
                m.nome,
                rs.itens_ok,
                rs.itens,
                rs.precisao_global
                    .map_or("n/d".into(), |p| format!("{:.1}%", p * 100.0)
                        .replace('.', ",")),
                rs.tempo_medio_ms
                    .map_or("n/d".into(), |t| format!("{:.1} s", t / 1000.0)
                        .replace('.', ","))
            )),
            (StatusModelo::Executado, None) => {
                s.push_str(&format!("  {}: sem resumo\n", m.nome));
            }
        }
    }
    s
}

/// Roda o benchmark. Devolve o texto para o terminal.
///
/// # Errors
/// Casos ilegíveis ou sem gabarito, perfil inválido ou falha ao gravar o relatório.
pub fn benchmark(
    casos: &Path,
    modelos: &[String],
    saida: Option<&Path>,
    perfis: Option<&Path>,
    endpoint: Option<&str>,
) -> Result<String, String> {
    let todos = carregar_casos(casos).map_err(|e| format!("casos: {e}"))?;
    let total = todos.len();
    let validos: Vec<_> = todos
        .into_iter()
        .filter(|c| !c.gabarito.is_empty())
        .collect();
    if validos.is_empty() {
        return Err(format!(
            "nenhum caso com gabarito preenchido em {} ({total} arquivo(s) lido(s)); preencha `gabarito` nos JSON",
            casos.display()
        ));
    }
    let cfg = carregar_perfis(perfis)?;
    let cls = classificadores(modelos, &cfg, endpoint)?;
    eprintln!(
        "benchmark: {} caso(s) com gabarito ({} sem, ignorados) x {} modelo(s); cada chamada ao modelo local pode levar segundos...",
        validos.len(),
        total - validos.len(),
        cls.len()
    );
    let rel = executar_benchmark(&validos, &cls, &OpcoesBenchmark::default());
    let mut s = resumo_curto(&rel);
    match saida {
        Some(p) => {
            let conteudo = if p.extension().is_some_and(|e| e == "json") {
                rel.para_json().map_err(|e| e.to_string())?
            } else {
                rel.para_markdown()
            };
            crate::escrita::criar_novo(p, conteudo.as_bytes())?;
            s.push_str(&format!("Relatório gravado em {}\n", p.display()));
        }
        None => {
            s.push('\n');
            s.push_str(&rel.para_markdown());
        }
    }
    Ok(s)
}

/// Executa `dag modelos ...` (sem agente).
pub fn executar(m: Modelos, json: bool) -> ExitCode {
    match m {
        Modelos::Listar { perfis } => match carregar_perfis(perfis.as_deref()) {
            Ok(cfg) => {
                let linhas = sondar(&cfg);
                if json {
                    println!(
                        "{}",
                        serde_json::to_string(&linhas).unwrap_or_else(|_| "[]".into())
                    );
                } else {
                    print!("{}", texto_modelos(&linhas));
                }
                ExitCode::SUCCESS
            }
            Err(e) => erro(&e, json),
        },
        Modelos::Benchmark {
            casos,
            modelo,
            saida,
            perfis,
            endpoint,
        } => match benchmark(
            &casos,
            &modelo,
            saida.as_deref(),
            perfis.as_deref(),
            endpoint.as_deref(),
        ) {
            Ok(s) => {
                if json {
                    println!("{}", serde_json::json!({ "relatorio": s }));
                } else {
                    print!("{s}");
                }
                ExitCode::SUCCESS
            }
            Err(e) => erro(&e, json),
        },
    }
}

fn erro(msg: &str, json: bool) -> ExitCode {
    if json {
        println!(
            "{}",
            serde_json::json!({"tipo":"erro","codigo":"argumento_invalido","mensagem":msg})
        );
    }
    eprintln!("erro: {msg}");
    ExitCode::from(1)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod testes {
    use super::*;
    use crate::{Cli, Sub};
    use clap::Parser;

    #[test]
    fn parse_dos_subcomandos() {
        let cli = Cli::try_parse_from([
            "dag",
            "modelos",
            "benchmark",
            "--casos",
            "/c",
            "--modelo",
            "gemma3:12b",
            "qwen3:14b",
            "--saida",
            "r.md",
        ])
        .unwrap();
        match cli.comando {
            Sub::Modelos(Modelos::Benchmark {
                casos,
                modelo,
                saida,
                ..
            }) => {
                assert_eq!(casos, PathBuf::from("/c"));
                assert_eq!(modelo, vec!["gemma3:12b", "qwen3:14b"]);
                assert_eq!(saida, Some(PathBuf::from("r.md")));
            }
            o => panic!("{o:?}"),
        }
        assert!(Cli::try_parse_from(["dag", "modelos", "benchmark", "--casos", "/c"]).is_err());
        assert!(matches!(
            Cli::try_parse_from(["dag", "modelos", "listar"])
                .unwrap()
                .comando,
            Sub::Modelos(Modelos::Listar { perfis: None })
        ));
    }

    fn perfis_inalcancaveis(dir: &Path) -> PathBuf {
        let p = dir.join("perfis.json");
        std::fs::write(
            &p,
            r#"{"modelos":[
                {"nome":"m1","endpoint":"http://127.0.0.1:1","backend":"ollama","papel":"primario","multimodal":true,"quantizacao":"Q4_K_M","nota":"teste"},
                {"nome":"m2","endpoint":"http://exemplo.com:11434","backend":"ollama","papel":"secundario"}
            ]}"#,
        )
        .unwrap();
        p
    }

    #[test]
    fn listar_mostra_inalcancavel_e_recusa_endpoint_nao_local() {
        let t = tempfile::tempdir().unwrap();
        let cfg = carregar_perfis(Some(&perfis_inalcancaveis(t.path()))).unwrap();
        let l = sondar(&cfg);
        assert_eq!(l.len(), 2);
        assert!(l[0].servidor.starts_with("inalcançável"), "{:?}", l[0]);
        assert!(l[1].servidor.starts_with("recusado"), "{:?}", l[1]);
        let txt = texto_modelos(&l);
        assert!(
            txt.contains("m1") && txt.contains("Q4_K_M") && txt.contains("sim"),
            "{txt}"
        );
        assert!(
            txt.contains("m1: teste") && txt.contains("ollama serve"),
            "{txt}"
        );
        assert!(carregar_perfis(Some(&t.path().join("nao.json"))).is_err());
    }

    #[test]
    fn benchmark_exige_casos_com_gabarito() {
        let t = tempfile::tempdir().unwrap();
        let e = benchmark(&t.path().join("x"), &["m".into()], None, None, None).unwrap_err();
        assert!(e.starts_with("casos:"), "{e}");
        // Caso sem gabarito preenchido.
        let caso = serde_json::json!({
            "id": "c1",
            "entrada": {"nome_arquivo": "a.pdf", "extracao": {"formato": "pdf", "estado": {"estado": "lido"}}},
            "gabarito": {}
        });
        std::fs::write(t.path().join("c1.json"), caso.to_string()).unwrap();
        let e = benchmark(t.path(), &["m".into()], None, None, None).unwrap_err();
        assert!(e.contains("nenhum caso com gabarito preenchido"), "{e}");
    }

    #[test]
    fn benchmark_sem_servidor_marca_nao_executado_e_grava_sem_sobrescrever() {
        let t = tempfile::tempdir().unwrap();
        let casos = t.path().join("casos");
        std::fs::create_dir(&casos).unwrap();
        let caso = serde_json::json!({
            "id": "c1",
            "entrada": {"nome_arquivo": "a.pdf", "extracao": {"formato": "pdf", "estado": {"estado": "lido"}}},
            "gabarito": {"titulo": "A"}
        });
        std::fs::write(casos.join("c1.json"), caso.to_string()).unwrap();
        let rel = t.path().join("r.md");
        let s = benchmark(
            &casos,
            &["m1".into()],
            Some(&rel),
            None,
            Some("http://127.0.0.1:1"),
        )
        .unwrap();
        assert!(
            s.contains("1 caso(s), 1 modelo(s)") && s.contains("Relatório gravado"),
            "{s}"
        );
        assert!(s.contains("NÃO EXECUTADO") || s.contains("0/1"), "{s}");
        assert!(std::fs::read_to_string(&rel).unwrap().contains("Benchmark"));
        let e = benchmark(
            &casos,
            &["m1".into()],
            Some(&rel),
            None,
            Some("http://127.0.0.1:1"),
        )
        .unwrap_err();
        assert!(e.contains("já existe"), "{e}");
    }
}
