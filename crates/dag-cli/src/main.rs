//! `dag`: CLI do DAG. Cada subcomando monta um [`Comando`] para o worker.
//! Incremento 1: o worker ainda não existe; o comando é apenas impresso.

use clap::{Args, Parser, Subcommand};
use dag_core::{AcaoServico, Alvo, Comando, ExecucaoId, ModoVarredura};
use std::process::ExitCode;

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

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
struct ModoArgs {
    /// Full Scan: calcula o Hash Completo de tudo
    #[arg(long)]
    full: bool,
    /// Fast Scan: reconcilia reaproveitando o que não mudou
    #[arg(long)]
    fast: bool,
}

#[derive(Subcommand, Debug)]
enum Sub {
    /// Lista Volumes
    Volumes,
    /// Inicia uma varredura
    Scan {
        #[command(flatten)]
        modo: ModoArgs,
        /// Raízes da varredura
        #[arg(required = true)]
        raizes: Vec<String>,
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
        Sub::Scan { modo, raizes } => Comando::IniciarScan {
            modo: if modo.full {
                ModoVarredura::FullScan
            } else {
                ModoVarredura::FastScan
            },
            raizes,
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    match montar(cli.comando) {
        Some(c) => {
            if cli.json {
                match serde_json::to_string(&c) {
                    Ok(j) => println!("{j}"),
                    Err(e) => {
                        eprintln!("erro ao serializar comando: {e}");
                        return ExitCode::from(1);
                    }
                }
            } else {
                println!("{c:#?}");
            }
        }
        None => println!("dag mcp: servidor MCP ainda não implementado"),
    }
    eprintln!("worker ainda não implementado (incremento 1)");
    ExitCode::from(2)
}
