//! `dag`: CLI do DAG. Cada subcomando monta um [`Comando`] e o executa em processo pelo
//! `dag-agent` (escritor único do Catálogo). O worker de longa duração com IPC vem depois.
//!
//! Códigos de saída: 0 ok; 1 erro; 2 ainda não implementado; 130 varredura cancelada (Ctrl-C).

mod saida;

use clap::{Args, Parser, Subcommand, ValueEnum};
use dag_agent::{Agente, caminho_catalogo_do_ambiente, incremento_pendente, somente_leitura};
use dag_core::{
    AcaoServico, Alvo, ArquivoId, Comando, ExecucaoId, GrupoId, MarcacaoId, ModoVarredura,
    NivelEspaco, OrdemDuplicados, Resposta, TipoRelatorio,
};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Parser, Debug)]
#[command(
    name = "dag",
    version,
    about = "DAG: catalogador de acervo digital (somente leitura)"
)]
struct Cli {
    /// Saída em JSON
    #[arg(long, global = true)]
    json: bool,
    /// Arquivo do Catálogo (padrão: $DAG_CATALOGO, senão $XDG_DATA_HOME/dag/catalogo.sqlite)
    #[arg(long, global = true, value_name = "ARQUIVO")]
    catalogo: Option<PathBuf>,
    #[command(subcommand)]
    comando: Sub,
}

#[derive(Args, Debug)]
struct ExecArg {
    /// Id da execução (padrão: a única em andamento)
    #[arg(long)]
    execucao: Option<i64>,
}

/// Alvo de uma Marcação: exatamente um dos quatro.
#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
struct AlvoMarcacao {
    /// Arquivo: caminho catalogado ou id numérico (hard link vale pelo arquivo físico)
    #[arg(long, value_name = "CAMINHO|ID")]
    arquivo: Option<String>,
    /// Grupo de Duplicados (id). Backup: todas as cópias; Deleção: todas menos a Cópia Principal
    #[arg(long, value_name = "ID")]
    grupo: Option<i64>,
    /// Regra para o Volume inteiro (id, rótulo ou ponto de montagem)
    #[arg(long, value_name = "VOLUME")]
    volume: Option<String>,
    /// Regra para a pasta catalogada inteira
    #[arg(long, value_name = "PASTA")]
    pasta: Option<String>,
}

impl AlvoMarcacao {
    fn alvo(self) -> Alvo {
        if let Some(a) = self.arquivo {
            Alvo::from_arquivo_arg(&a)
        } else if let Some(g) = self.grupo {
            Alvo::Grupo(GrupoId(g))
        } else if let Some(v) = self.volume {
            Alvo::Volume(v)
        } else {
            Alvo::Pasta(self.pasta.unwrap_or_default())
        }
    }
}

/// Extensão local: `--arquivo` aceita id numérico ou caminho (absolutizado).
trait AlvoExt {
    fn from_arquivo_arg(s: &str) -> Alvo;
}

impl AlvoExt for Alvo {
    fn from_arquivo_arg(s: &str) -> Alvo {
        match s.parse::<i64>() {
            Ok(id) if !s.is_empty() => Alvo::Arquivo(ArquivoId(id)),
            _ => Alvo::Caminho(absoluto(s)),
        }
    }
}

fn absoluto(s: &str) -> String {
    std::path::absolute(s)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| s.to_string())
}

/// Tamanho com unidade binária opcional: `10`, `500K`, `10MB`, `1,5GiB`.
fn parse_tamanho(s: &str) -> Result<u64, String> {
    let t = s.trim().replace(',', ".");
    let pos = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let (num, un) = t.split_at(pos);
    let n: f64 = num
        .parse()
        .map_err(|_| format!("tamanho inválido: `{s}`"))?;
    let mult: f64 = match un.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024.0_f64.powi(4),
        _ => return Err(format!("unidade desconhecida em `{s}` (use B, K, M, G, T)")),
    };
    Ok((n * mult) as u64)
}

#[derive(ValueEnum, Debug, Clone, Copy)]
enum OrdemArg {
    Recuperavel,
    Tamanho,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
enum TipoRel {
    Cobertura,
    SemCobertura,
    Espaco,
    Marcacoes,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
enum NivelArg {
    Computador,
    Volume,
    Pasta,
    Grupo,
}

#[derive(Subcommand, Debug)]
enum Sub {
    /// Lista Volumes
    Volumes,
    /// Inicia uma varredura
    Scan {
        /// Full Scan: inventário, Hash Completo, Grupos de Duplicados e reavaliação das Marcações
        #[arg(long, conflicts_with = "fast", required_unless_present_any = ["fast", "retomar"])]
        full: bool,
        /// Pula Hash, duplicados e reavaliação (inventário rápido, sem leitura do conteúdo)
        #[arg(long, conflicts_with_all = ["fast", "retomar"])]
        sem_hash: bool,
        /// Fast Scan: reconcilia reaproveitando o que não mudou
        #[arg(long, required_unless_present_any = ["full", "retomar"])]
        fast: bool,
        /// Retoma a execução ID (cancelada, pausada ou interrompida)
        #[arg(long, value_name = "ID", conflicts_with_all = ["full", "fast", "raizes"])]
        retomar: Option<i64>,
        /// Raízes da varredura
        /// (Fast Scan sem raízes: todas as raízes já varridas)
        #[arg(required_unless_present_any = ["retomar", "fast"])]
        raizes: Vec<String>,
    },
    /// Monitora as raízes (inotify) e reconcilia o Catálogo; roda até Ctrl-C
    Monitorar {
        /// Raízes a monitorar (padrão: todas as raízes já varridas)
        raizes: Vec<String>,
        /// Intervalo da reconciliação periódica (ex.: 30m, 90s, 2h, 1d; 0 ou nunca desliga)
        #[arg(long, value_name = "INTERVALO", value_parser = parse_intervalo)]
        reconciliar_a_cada: Option<u64>,
        /// Não calcula Hash Completo após as reconciliações
        #[arg(long)]
        sem_hash: bool,
    },
    /// Lista os eventos de arquivos/volumes persistidos (mais recentes primeiro)
    Eventos {
        /// Máximo de eventos (padrão 50)
        #[arg(long)]
        limite: Option<u32>,
    },
    /// Lista os filhos de uma pasta catalogada, do maior para o menor
    Explorar {
        /// Pasta (padrão: as Raízes catalogadas)
        pasta: Option<String>,
        /// Máximo de entradas (padrão 50)
        #[arg(long)]
        limite: Option<u32>,
        /// Cursor da página seguinte (impresso ao fim da página anterior)
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Pausa uma varredura
    Pausar(ExecArg),
    /// Retoma uma varredura pausada
    Retomar(ExecArg),
    /// Cancela uma varredura
    Cancelar(ExecArg),
    /// Mostra o progresso das varreduras
    Status(ExecArg),
    /// Lista Grupos de Duplicados (maior espaço recuperável primeiro)
    Duplicados {
        /// Só grupos com cópia neste Volume (id, rótulo ou ponto de montagem)
        #[arg(long)]
        volume: Option<String>,
        /// Tamanho mínimo do arquivo (ex.: 10M, 1,5G; unidades binárias)
        #[arg(long, value_name = "TAMANHO", value_parser = parse_tamanho)]
        min_tamanho: Option<u64>,
        #[arg(long, value_enum)]
        ordem: Option<OrdemArg>,
        /// Máximo de grupos (padrão 50)
        #[arg(long)]
        limite: Option<u32>,
        /// Cursor da página seguinte (impresso ao fim da página anterior)
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Mostra um Grupo de Duplicados: cópias, Marcações, Cobertura
    Grupo {
        /// Id do grupo (veja `dag duplicados`)
        id: i64,
    },
    /// Define a Cópia Principal de um grupo (persiste entre varreduras)
    Principal {
        grupo: i64,
        /// Arquivo (caminho catalogado ou id) que passa a ser a Cópia Principal
        #[arg(required_unless_present = "auto")]
        arquivo: Option<String>,
        /// Volta à escolha automática pela Prioridade de Volume
        #[arg(long, conflicts_with = "arquivo")]
        auto: bool,
    },
    /// Cria uma Marcação (somente no Catálogo; nada é apagado nem movido)
    #[command(subcommand)]
    Marcar(Marcar),
    /// Remove uma Marcação
    Desmarcar(AlvoMarcacao),
    /// Confirma de novo uma Marcação Pendente (conteúdo mudou depois de marcada)
    Confirmar {
        /// Id da Marcação (veja `dag relatorio marcacoes`)
        marcacao: i64,
    },
    /// Define a Prioridade de Volume (do mais ao menos prioritário): Volumes ou pastas
    Prioridade {
        #[arg(required = true, value_name = "VOLUME|PASTA")]
        ordem: Vec<String>,
    },
    /// Relatórios: cobertura, sem-cobertura, espaco, marcacoes
    Relatorio {
        #[arg(value_enum)]
        tipo: TipoRel,
        /// Nível do relatório de espaço
        #[arg(long, value_enum)]
        nivel: Option<NivelArg>,
        /// Grava o relatório inteiro neste arquivo (.csv ou .json; não sobrescreve)
        #[arg(long, value_name = "ARQUIVO")]
        saida: Option<PathBuf>,
        #[arg(long)]
        limite: Option<u32>,
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Verificação de Integridade
    Verificar {
        /// Raízes (padrão: todas)
        raizes: Vec<String>,
    },
    /// Gerencia o serviço em segundo plano
    #[command(subcommand)]
    Servico(Servico),
    /// Operações sobre o Catálogo
    #[command(subcommand)]
    Catalogo(Catalogo),
    /// Servidor MCP por stdio
    Mcp,
}

#[derive(Subcommand, Debug)]
enum Marcar {
    /// Marcação de Backup: a cópia é intencional e deve ser preservada
    Backup(AlvoMarcacao),
    /// Marcação para Deleção: candidata a descarte (não apaga nada)
    Delecao(AlvoMarcacao),
}

#[derive(Subcommand, Debug)]
enum Servico {
    Instalar,
    Remover,
    Iniciar,
    Parar,
    /// Liga ou desliga a inicialização automática
    Autostart {
        #[arg(value_parser = ["on", "off"])]
        estado: String,
    },
}

#[derive(Subcommand, Debug)]
enum Catalogo {
    /// Backup consistente do Catálogo
    Backup { destino: String },
}

/// `30m`, `90s`, `2h`, `1d` ou segundos puros; `0`/`nunca` desliga (devolve 0).
fn parse_intervalo(t: &str) -> Result<u64, String> {
    let t = t.trim().to_lowercase();
    if t == "nunca" || t == "0" {
        return Ok(0);
    }
    let (num, mult) = match t.chars().last() {
        Some('s') => (&t[..t.len() - 1], 1),
        Some('m') => (&t[..t.len() - 1], 60),
        Some('h') => (&t[..t.len() - 1], 3600),
        Some('d') => (&t[..t.len() - 1], 86_400),
        _ => (t.as_str(), 1),
    };
    let n: u64 = num
        .trim()
        .parse()
        .map_err(|_| format!("intervalo inválido: {t} (use 30m, 90s, 2h, 1d)"))?;
    n.checked_mul(mult)
        .ok_or_else(|| "intervalo grande demais".to_string())
}

fn exec(a: &ExecArg) -> Option<ExecucaoId> {
    a.execucao.map(ExecucaoId)
}

/// `None` para `dag mcp`, que não é um `Comando`.
fn montar(sub: Sub) -> Option<Comando> {
    Some(match sub {
        Sub::Volumes => Comando::ListarVolumes,
        Sub::Scan {
            retomar: Some(id), ..
        } => Comando::Retomar {
            execucao: Some(ExecucaoId(id)),
        },
        Sub::Scan {
            full,
            raizes,
            sem_hash,
            ..
        } => Comando::IniciarScan {
            modo: if full {
                ModoVarredura::FullScan
            } else {
                ModoVarredura::FastScan
            },
            raizes,
            sem_hash,
        },
        Sub::Monitorar {
            raizes,
            reconciliar_a_cada,
            sem_hash,
        } => Comando::Monitorar {
            raizes,
            reconciliar_a_cada_s: reconciliar_a_cada,
            sem_hash,
        },
        Sub::Eventos { limite } => Comando::Eventos { limite },
        Sub::Explorar {
            pasta,
            limite,
            cursor,
        } => Comando::Explorar {
            pasta,
            limite,
            cursor,
        },
        Sub::Pausar(a) => Comando::Pausar { execucao: exec(&a) },
        Sub::Retomar(a) => Comando::Retomar { execucao: exec(&a) },
        Sub::Cancelar(a) => Comando::Cancelar { execucao: exec(&a) },
        Sub::Status(a) => Comando::Status { execucao: exec(&a) },
        Sub::Duplicados {
            volume,
            min_tamanho,
            ordem,
            limite,
            cursor,
        } => Comando::ListarDuplicados {
            volume,
            tamanho_minimo: min_tamanho,
            ordem: ordem.map(|o| match o {
                OrdemArg::Recuperavel => OrdemDuplicados::Recuperavel,
                OrdemArg::Tamanho => OrdemDuplicados::Tamanho,
            }),
            limite,
            cursor,
        },
        Sub::Grupo { id } => Comando::DetalharGrupo { grupo: GrupoId(id) },
        Sub::Principal {
            grupo,
            arquivo,
            auto,
        } => Comando::DefinirCopiaPrincipal {
            grupo: GrupoId(grupo),
            arquivo: if auto {
                None
            } else {
                arquivo.as_deref().map(Alvo::from_arquivo_arg)
            },
        },
        Sub::Marcar(Marcar::Backup(a)) => Comando::MarcarBackup { alvo: a.alvo() },
        Sub::Marcar(Marcar::Delecao(a)) => Comando::MarcarDelecao { alvo: a.alvo() },
        Sub::Desmarcar(a) => Comando::Desmarcar { alvo: a.alvo() },
        Sub::Confirmar { marcacao } => Comando::ConfirmarMarcacao {
            marcacao: MarcacaoId(marcacao),
        },
        Sub::Prioridade { ordem } => Comando::DefinirPrioridadeVolume { ordem },
        Sub::Relatorio {
            tipo,
            nivel,
            saida,
            limite,
            cursor,
        } => Comando::Relatorio {
            tipo: match tipo {
                TipoRel::Cobertura => TipoRelatorio::Cobertura,
                TipoRel::SemCobertura => TipoRelatorio::SemCobertura,
                TipoRel::Espaco => TipoRelatorio::Espaco,
                TipoRel::Marcacoes => TipoRelatorio::Marcacoes,
            },
            nivel: nivel.map(|n| match n {
                NivelArg::Computador => NivelEspaco::Computador,
                NivelArg::Volume => NivelEspaco::Volume,
                NivelArg::Pasta => NivelEspaco::Pasta,
                NivelArg::Grupo => NivelEspaco::Grupo,
            }),
            limite,
            cursor,
            saida: saida.map(|p| absoluto(&p.to_string_lossy())),
        },
        Sub::Verificar { raizes } => Comando::VerificarIntegridade {
            raizes: raizes.iter().map(|r| absoluto(r)).collect(),
        },
        Sub::Servico(s) => Comando::Servico {
            acao: match s {
                Servico::Instalar => AcaoServico::Instalar,
                Servico::Remover => AcaoServico::Remover,
                Servico::Iniciar => AcaoServico::Iniciar,
                Servico::Parar => AcaoServico::Parar,
                Servico::Autostart { estado } => AcaoServico::Autostart {
                    ativo: estado == "on",
                },
            },
        },
        Sub::Catalogo(Catalogo::Backup { destino }) => Comando::BackupCatalogo { destino },
        Sub::Mcp => return None,
    })
}

static INTERROMPIDO: AtomicBool = AtomicBool::new(false);

/// Primeiro Ctrl-C cancela a varredura com limpeza (a fila fica retomável); o segundo aborta.
fn instalar_ctrl_c(agente: &Agente) {
    let controle = agente.controle();
    let r = ctrlc::set_handler(move || {
        if INTERROMPIDO.swap(true, Ordering::SeqCst) {
            std::process::exit(130);
        }
        eprintln!("\ncancelando: gravando o que já foi lido (Ctrl-C de novo aborta)...");
        controle.cancelar();
    });
    if let Err(e) = r {
        eprintln!("aviso: Ctrl-C não tratado ({e})");
    }
}

fn codigo_de_saida(r: &Resposta) -> u8 {
    match r {
        Resposta::Erro { codigo, .. } if codigo == "nao_implementado" => 2,
        Resposta::Erro { .. } => 1,
        Resposta::ScanConcluido { desfecho, .. } if desfecho == "cancelada" => 130,
        _ => 0,
    }
}

fn executar(cli: Cli) -> Resposta {
    let Some(comando) = montar(cli.comando) else {
        return Resposta::Erro {
            codigo: "nao_implementado".into(),
            mensagem: "dag mcp: servidor MCP ainda não implementado (incremento 6)".into(),
        };
    };
    if let Some(n) = incremento_pendente(&comando) {
        return Resposta::Erro {
            codigo: "nao_implementado".into(),
            mensagem: format!("ainda não implementado (incremento {n})"),
        };
    }
    let caminho = match caminho_catalogo_do_ambiente(cli.catalogo.as_deref()) {
        Ok(c) => c,
        Err(e) => return e.para_resposta(),
    };
    let agente = if somente_leitura(&comando) {
        Agente::abrir_leitura(&caminho)
    } else {
        Agente::abrir(&caminho)
    };
    let mut agente = match agente {
        Ok(a) => a,
        Err(e) => return e.para_resposta(),
    };
    instalar_ctrl_c(&agente);
    agente.executar(comando)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    let resposta = executar(cli);
    let codigo = codigo_de_saida(&resposta);
    if json {
        match serde_json::to_string(&resposta) {
            Ok(j) => println!("{j}"),
            Err(e) => {
                eprintln!("erro ao serializar resposta: {e}");
                return ExitCode::from(1);
            }
        }
        if let Resposta::Erro { mensagem, .. } = &resposta {
            eprintln!("erro: {mensagem}");
        }
    } else {
        saida::imprimir(&resposta);
    }
    ExitCode::from(codigo)
}
