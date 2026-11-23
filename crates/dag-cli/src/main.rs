//! `dag`: CLI do DAG. Cada subcomando monta um [`Comando`] e o executa em processo pelo
//! `dag-agent` (escritor único do Catálogo). O worker de longa duração com IPC vem depois.
//!
//! Códigos de saída: 0 ok; 1 erro; 2 ainda não implementado; 130 varredura cancelada (Ctrl-C).

mod saida;

use clap::{Args, Parser, Subcommand};
use dag_agent::{Agente, caminho_catalogo_do_ambiente, incremento_pendente, somente_leitura};
use dag_core::{AcaoServico, Alvo, Comando, ExecucaoId, ModoVarredura, Resposta};
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

#[derive(Args, Debug)]
struct AlvoArg {
    /// Caminho do arquivo
    alvo: String,
}

#[derive(Subcommand, Debug)]
enum Sub {
    /// Lista Volumes
    Volumes,
    /// Inicia uma varredura
    Scan {
        /// Full Scan: inventário completo (o Hash Completo vem em incremento posterior)
        #[arg(long, conflicts_with = "fast", required_unless_present_any = ["fast", "retomar"])]
        full: bool,
        /// Fast Scan: reconcilia reaproveitando o que não mudou
        #[arg(long, required_unless_present_any = ["full", "retomar"])]
        fast: bool,
        /// Retoma a execução ID (cancelada, pausada ou interrompida)
        #[arg(long, value_name = "ID", conflicts_with_all = ["full", "fast", "raizes"])]
        retomar: Option<i64>,
        /// Raízes da varredura
        #[arg(required_unless_present = "retomar")]
        raizes: Vec<String>,
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
    /// Lista Grupos de Duplicados
    Duplicados {
        #[arg(long)]
        limite: Option<u32>,
        #[arg(long)]
        deslocamento: Option<u32>,
    },
    /// Cria uma Marcação
    #[command(subcommand)]
    Marcar(Marcar),
    /// Remove a Marcação de um arquivo
    Desmarcar(AlvoArg),
    /// Define a Prioridade de Volume (do mais ao menos prioritário)
    Prioridade {
        #[arg(required = true)]
        ordem: Vec<String>,
    },
    /// Gera relatório
    Relatorio,
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
    /// Marcação de Backup
    Backup(AlvoArg),
    /// Marcação para Deleção
    Delecao(AlvoArg),
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

fn exec(a: &ExecArg) -> Option<ExecucaoId> {
    a.execucao.map(ExecucaoId)
}

fn caminho(a: AlvoArg) -> Alvo {
    Alvo::Caminho(a.alvo)
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
        Sub::Scan { full, raizes, .. } => Comando::IniciarScan {
            modo: if full {
                ModoVarredura::FullScan
            } else {
                ModoVarredura::FastScan
            },
            raizes,
        },
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
            limite,
            deslocamento,
        } => Comando::ListarDuplicados {
            limite,
            deslocamento,
        },
        Sub::Marcar(Marcar::Backup(a)) => Comando::MarcarBackup { alvo: caminho(a) },
        Sub::Marcar(Marcar::Delecao(a)) => Comando::MarcarDelecao { alvo: caminho(a) },
        Sub::Desmarcar(a) => Comando::Desmarcar { alvo: caminho(a) },
        Sub::Prioridade { ordem } => Comando::DefinirPrioridadeVolume { ordem },
        Sub::Relatorio => Comando::Relatorio,
        Sub::Verificar { raizes } => Comando::VerificarIntegridade { raizes },
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
