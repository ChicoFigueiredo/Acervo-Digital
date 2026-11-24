//! `dag`: CLI do DAG. Cada subcomando monta um [`Comando`] e o envia ao agente em segundo plano
//! (`dag agente`), se houver um servindo o Catálogo, ou o executa numa sessão em processo pelo
//! `dag-agent` (escritor único do Catálogo).
//!
//! Códigos de saída: 0 ok; 1 erro; 2 ainda não implementado; 130 varredura cancelada (Ctrl-C).

mod saida;
mod seguir;

use clap::{Args, Parser, Subcommand, ValueEnum};
use dag_agent::{
    Cancelador, Conexao, ConfigServidor, ExibidorStderr, OpcoesConexao, Servidor,
    caminho_catalogo_do_ambiente, caminho_socket_padrao, incremento_pendente,
};
use dag_core::{
    AcaoServico, Alvo, ArquivoId, Autor, Comando, ExecucaoId, GrupoId, MarcacaoId, ModoVarredura,
    NivelEspaco, OrdemDuplicados, Pedido, Resposta, TipoRelatorio,
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
    /// Socket do agente em segundo plano (padrão: $DAG_SOCKET, senão $XDG_RUNTIME_DIR/dag/agente.sock)
    #[arg(long, global = true, value_name = "ARQUIVO")]
    socket: Option<PathBuf>,
    /// Não usa o agente em segundo plano: executa em processo e vira o escritor do Catálogo
    /// (testes e máquinas sem como subir um processo; também `DAG_SEM_AGENTE=1`). Não enxerga nem
    /// controla o que outros processos fazem
    #[arg(long, global = true)]
    sem_agente: bool,
    #[command(subcommand)]
    comando: Sub,
}

#[derive(Args, Debug)]
struct ExecArg {
    /// Id da execução (padrão: a única em andamento)
    #[arg(long)]
    execucao: Option<i64>,
}

/// Qual trabalho do agente controlar: a execução ou o id do trabalho (`dag jobs`).
#[derive(Args, Debug)]
struct ControleArg {
    /// Id da execução (padrão: a única em andamento)
    #[arg(long, conflicts_with = "job")]
    execucao: Option<i64>,
    /// Id do trabalho do agente, como aparece em `dag jobs` (inclui os que ainda estão na fila)
    #[arg(long)]
    job: Option<u64>,
}

#[derive(Args, Debug)]
struct StatusArg {
    /// Id da execução (padrão: as mais recentes)
    #[arg(long)]
    execucao: Option<i64>,
    /// Fica acompanhando o fluxo de eventos do agente (trabalhos, progresso, monitor) até Ctrl-C
    #[arg(long, conflicts_with = "execucao")]
    seguir: bool,
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

/// Paralelismo do Hash Completo (vence a flag do agente e o valor persistido).
#[derive(clap::Args, Debug, Clone, Copy, Default)]
struct ArgsHash {
    /// Leitores simultâneos por disco no Hash Completo (1 a 8; padrão 1: o melhor para HDD)
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=8))]
    threads_por_disco: Option<u32>,
    /// Discos lendo ao mesmo tempo no Hash Completo (1 a 16; padrão 16)
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=16))]
    max_discos: Option<u32>,
}

#[derive(Subcommand, Debug)]
enum Config {
    /// Mostra ou define o paralelismo padrão do Hash Completo (persistido no Catálogo; vale a
    /// partir da próxima execução ou retomada)
    Hash(ArgsHash),
}

#[derive(Subcommand, Debug)]
enum Sub {
    /// Lista Volumes (padrão: só os reais: físicos, removíveis e de rede)
    Volumes {
        /// Mostra também os volumes de imagem/virtuais (snap, loop, zram...), com a coluna CLASSE
        #[arg(long)]
        todos: bool,
    },
    /// Inicia uma varredura
    Scan {
        /// Full Scan: inventário, Hash Completo, Grupos de Duplicados e reavaliação das Marcações
        #[arg(long, conflicts_with = "fast", required_unless_present_any = ["fast", "retomar"])]
        full: bool,
        /// Pula Hash, duplicados e reavaliação (inventário rápido, sem leitura do conteúdo)
        #[arg(long, conflicts_with_all = ["fast", "retomar"])]
        sem_hash: bool,
        /// Com --full: lê o conteúdo de todos os arquivos de novo, em vez de reaproveitar o hash
        /// dos que não mudaram (a abertura do agente nunca faz isso sozinha)
        #[arg(long, requires = "full", conflicts_with = "sem_hash")]
        reler_tudo: bool,
        /// Fast Scan: reconcilia reaproveitando o que não mudou
        #[arg(long, required_unless_present_any = ["full", "retomar"])]
        fast: bool,
        /// Pastas que negam acesso: pede ao Polkit (pkexec + dag-helper) a leitura só dos
        /// metadados delas. Nunca roda o DAG como root; a senha vai só ao diálogo do sistema
        #[arg(long, conflicts_with = "retomar")]
        elevar: bool,
        /// Retoma a execução ID (cancelada, pausada ou interrompida)
        #[arg(long, value_name = "ID", conflicts_with_all = ["full", "fast", "raizes"])]
        retomar: Option<i64>,
        #[command(flatten)]
        hash: ArgsHash,
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
    /// Pausa uma varredura (de qualquer cliente: GUI, CLI, MCP ou a abertura do agente)
    Pausar(ControleArg),
    /// Retoma uma varredura pausada
    Retomar(ControleArg),
    /// Cancela uma varredura (ou um trabalho ainda na fila)
    Cancelar(ControleArg),
    /// Mostra o progresso das varreduras (`--seguir` acompanha os eventos do agente)
    Status(StatusArg),
    /// Acompanha os eventos do agente: trabalhos iniciados por qualquer cliente, progresso,
    /// monitor e Ponto de Confiança (o mesmo que `status --seguir`)
    Acompanhar,
    /// Lista os trabalhos do agente (em andamento e na fila) e quem os iniciou
    Jobs,
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
        #[command(flatten)]
        hash: ArgsHash,
    },
    /// Configurações persistidas no Catálogo
    #[command(subcommand)]
    Config(Config),
    /// Gerencia o serviço em segundo plano
    #[command(subcommand)]
    Servico(Servico),
    /// Operações sobre o Catálogo
    #[command(subcommand)]
    Catalogo(Catalogo),
    /// Servidor MCP por stdio
    Mcp,
    /// Agente em segundo plano: único escritor do Catálogo, serve o socket local e monitora as
    /// raízes (processo em primeiro plano; a unidade systemd o usa)
    Agente {
        /// Não monitora as raízes catalogadas
        #[arg(long)]
        sem_monitor: bool,
        /// Não retoma sozinho as execuções interrompidas (queda, falta de energia); elas ficam
        /// `interrompida` até `dag scan --retomar <id>`
        #[arg(long)]
        sem_retomar_automatico: bool,
        /// Quando verificar as raízes ao abrir (Fast Scan, hash só do que é novo ou mudou):
        /// `sempre` (padrão), `nao-confiaveis` (só as fora do Ponto de Confiança) ou `nunca`
        #[arg(long, value_name = "QUANDO", default_value = "sempre")]
        verificar_ao_abrir: dag_agent::VerificarAoAbrir,
        /// Teto de passadas de verificação por abertura até uma passada sem mudanças
        #[arg(long, value_name = "N", default_value_t = 3)]
        max_passadas: u32,
        /// Leitores por disco no hash deste agente (vence o persistido; o pedido vence este)
        #[command(flatten)]
        hash: ArgsHash,
    },
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
    /// Estado do serviço (instalado, autostart, ativo, PID, linger)
    Estado,
    /// Instala a unidade systemd de usuário (não habilita nem inicia)
    Instalar,
    /// Para, desabilita e remove a unidade
    Remover,
    Iniciar,
    Parar,
    Reiniciar,
    /// Liga ou desliga a inicialização automática (no login)
    Autostart {
        #[arg(value_parser = ["on", "off"])]
        estado: String,
    },
    /// Linger: mantém o serviço de usuário desde o boot e após o último logout
    /// (`loginctl enable-linger`; pode pedir autorização do Polkit)
    Linger {
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

fn exec_controle(a: &ControleArg) -> Option<ExecucaoId> {
    a.execucao.map(ExecucaoId)
}

/// `None` para `dag mcp`, que não é um `Comando`.
fn montar(sub: Sub) -> Option<Comando> {
    Some(match sub {
        Sub::Volumes { .. } => Comando::ListarVolumes,
        Sub::Scan {
            retomar: Some(id),
            hash,
            ..
        } => Comando::Retomar {
            execucao: Some(ExecucaoId(id)),
            threads_por_disco: hash.threads_por_disco,
            max_discos: hash.max_discos,
        },
        Sub::Scan {
            full,
            raizes,
            sem_hash,
            elevar,
            reler_tudo,
            hash,
            ..
        } => Comando::IniciarScan {
            modo: if full {
                ModoVarredura::FullScan
            } else {
                ModoVarredura::FastScan
            },
            raizes,
            sem_hash,
            elevar,
            releitura_total: reler_tudo,
            threads_por_disco: hash.threads_por_disco,
            max_discos: hash.max_discos,
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
        Sub::Pausar(a) => match a.job {
            Some(job) => Comando::PausarJob { job },
            None => Comando::Pausar {
                execucao: exec_controle(&a),
            },
        },
        Sub::Retomar(a) => match a.job {
            Some(job) => Comando::RetomarJob { job },
            None => Comando::Retomar {
                execucao: exec_controle(&a),
                threads_por_disco: None,
                max_discos: None,
            },
        },
        Sub::Cancelar(a) => match a.job {
            Some(job) => Comando::CancelarJob { job },
            None => Comando::Cancelar {
                execucao: exec_controle(&a),
            },
        },
        Sub::Status(a) => Comando::Status {
            execucao: a.execucao.map(ExecucaoId),
        },
        Sub::Jobs => Comando::ListarJobs,
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
        Sub::Verificar { raizes, hash } => Comando::VerificarIntegridade {
            raizes: raizes.iter().map(|r| absoluto(r)).collect(),
            threads_por_disco: hash.threads_por_disco,
            max_discos: hash.max_discos,
        },
        Sub::Config(Config::Hash(h)) => Comando::ConfigHash {
            threads_por_disco: h.threads_por_disco,
            max_discos: h.max_discos,
        },
        Sub::Servico(s) => Comando::Servico {
            acao: match s {
                Servico::Estado => AcaoServico::Estado,
                Servico::Reiniciar => AcaoServico::Reiniciar,
                Servico::Linger { estado } => AcaoServico::PersistirSemSessao {
                    ativo: estado == "on",
                },
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
        Sub::Mcp | Sub::Agente { .. } | Sub::Acompanhar => return None,
    })
}

static INTERROMPIDO: AtomicBool = AtomicBool::new(false);

/// Primeiro Ctrl-C cancela a varredura com limpeza (a fila fica retomável); o segundo aborta.
/// Com o agente em segundo plano, o cancelamento vai pelo socket e o trabalho dele é que para.
fn instalar_ctrl_c(cancelador: Cancelador) {
    let r = ctrlc::set_handler(move || {
        if INTERROMPIDO.swap(true, Ordering::SeqCst) {
            std::process::exit(130);
        }
        eprintln!("\ncancelando: gravando o que já foi lido (Ctrl-C de novo aborta)...");
        cancelador.cancelar();
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

/// Resultado de um comando e, para `dag status`, quem o atendeu.
struct Execucao {
    resposta: Resposta,
    /// `Some(None)`: sem agente (sessão); `Some(Some(..))`: agente em segundo plano.
    agente: Option<Option<dag_agent::InfoServidor>>,
    /// Só em `dag status` com agente: os trabalhos dele.
    jobs: Option<Vec<dag_core::JobInfo>>,
}

fn executar(cli: Cli) -> Execucao {
    let sem = |resposta| Execucao {
        resposta,
        agente: None,
        jobs: None,
    };
    let Some(comando) = montar(cli.comando) else {
        return sem(Resposta::Erro {
            codigo: "nao_implementado".into(),
            mensagem: "subcomando sem comando associado".into(),
        });
    };
    if let Some(n) = incremento_pendente(&comando) {
        return sem(Resposta::Erro {
            codigo: "nao_implementado".into(),
            mensagem: format!("ainda não implementado (incremento {n})"),
        });
    }
    let caminho = match caminho_catalogo_do_ambiente(cli.catalogo.as_deref()) {
        Ok(c) => c,
        Err(e) => return sem(e.para_resposta()),
    };
    // O serviço é gerido direto no systemd do usuário: não abre o Catálogo (que pode estar com o
    // agente em segundo plano) nem cria nada.
    if let Comando::Servico { acao } = &comando {
        return sem(dag_agent::servico::executar_avulso(*acao, &caminho));
    }
    let socket = cli.socket.unwrap_or_else(caminho_socket_padrao);
    let opcoes = OpcoesConexao::novo(cli.sem_agente);
    let mut conexao = match Conexao::abrir_para_com(&caminho, &socket, &comando, &opcoes) {
        Ok(c) => c,
        Err(e) => return sem(e.para_resposta()),
    };
    instalar_ctrl_c(conexao.cancelador());
    let eh_status = matches!(comando, Comando::Status { .. });
    let mut exibidor = ExibidorStderr::novo();
    let resposta = match conexao.enviar(
        Pedido {
            autor: Autor::Cli,
            comando,
        },
        &mut |p| exibidor.mostrar(p),
    ) {
        Ok(r) => r,
        Err(e) => e.para_resposta(),
    };
    let agente = eh_status.then(|| conexao.info_daemon());
    // O estado de um agente inclui o que ele está fazendo, seja quem for que pediu.
    let jobs = (eh_status && conexao.eh_daemon())
        .then(|| {
            conexao
                .enviar(
                    Pedido {
                        autor: Autor::Cli,
                        comando: Comando::ListarJobs,
                    },
                    &mut |_| {},
                )
                .ok()
        })
        .flatten()
        .and_then(|r| match r {
            Resposta::Jobs { jobs } => Some(jobs),
            _ => None,
        });
    Execucao {
        resposta,
        agente,
        jobs,
    }
}

/// `dag agente`: roda o agente em primeiro plano até SIGTERM/SIGINT e encerra com checkpoint.
fn rodar_agente(
    catalogo: Option<&std::path::Path>,
    socket: Option<PathBuf>,
    sem_monitor: bool,
    abertura: dag_agent::PoliticaAbertura,
    hash: ArgsHash,
) -> ExitCode {
    let falha = |e: &dyn std::fmt::Display| {
        eprintln!("erro: {e}");
        ExitCode::from(1)
    };
    let caminho = match caminho_catalogo_do_ambiente(catalogo) {
        Ok(c) => c,
        Err(e) => return falha(&e),
    };
    let socket = socket.unwrap_or_else(caminho_socket_padrao);
    let mut cfg = ConfigServidor::novo(caminho, socket);
    cfg.monitor = !sem_monitor;
    cfg.abertura = abertura;
    cfg.opcoes_hash = dag_core::OpcoesHash {
        threads_por_disco: hash.threads_por_disco,
        max_discos: hash.max_discos,
    };
    let servidor = match Servidor::iniciar(cfg) {
        Ok(s) => s,
        Err(e) => return falha(&e),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    // `termination`: SIGINT, SIGTERM e SIGHUP.
    if let Err(e) = ctrlc::set_handler(move || {
        let _ = tx.send(());
    }) {
        return falha(&e);
    }
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(()) => {
                eprintln!("agente: sinal recebido; encerrando com checkpoint...");
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if servidor.ativo() => {}
            Err(_) => break,
        }
    }
    servidor.encerrar();
    ExitCode::SUCCESS
}

/// `dag mcp`: servidor MCP por stdio (só quadros do protocolo no stdout; logs no stderr).
fn servir_mcp(
    catalogo: Option<&std::path::Path>,
    socket: Option<PathBuf>,
    sem_agente: bool,
) -> ExitCode {
    let caminho = match caminho_catalogo_do_ambiente(catalogo) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("erro: {e}");
            return ExitCode::from(1);
        }
    };
    let socket = socket.unwrap_or_else(caminho_socket_padrao);
    match dag_mcp::servir_stdio_com(&caminho, &socket, OpcoesConexao::novo(sem_agente)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::from(1)
        }
    }
}

/// `dag status --seguir` e `dag acompanhar`.
fn acompanhar(
    catalogo: Option<&std::path::Path>,
    socket: Option<&std::path::Path>,
    json: bool,
) -> ExitCode {
    match caminho_catalogo_do_ambiente(catalogo) {
        Ok(c) => ExitCode::from(seguir::seguir(&c, socket, json)),
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::from(1)
        }
    }
}

fn agente_json(info: Option<&dag_agent::InfoServidor>) -> serde_json::Value {
    match info {
        Some(i) => serde_json::json!({
            "servindo": true,
            "pid": i.pid,
            "catalogo": i.catalogo,
            "monitor": i.monitor,
            "versao": i.versao,
        }),
        None => serde_json::json!({ "servindo": false }),
    }
}

fn agente_texto(info: Option<&dag_agent::InfoServidor>) -> String {
    match info {
        Some(i) => format!(
            "Agente: servindo (pid {}, monitor {})\n",
            i.pid,
            if i.monitor { "ligado" } else { "desligado" }
        ),
        None => "Agente: nenhum em execução (este comando rodou numa sessão em processo)\n".into(),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if matches!(cli.comando, Sub::Mcp) {
        return servir_mcp(cli.catalogo.as_deref(), cli.socket, cli.sem_agente);
    }
    if let Sub::Agente {
        sem_monitor,
        sem_retomar_automatico,
        verificar_ao_abrir,
        max_passadas,
        hash,
    } = cli.comando
    {
        let abertura = dag_agent::PoliticaAbertura {
            retomar_automaticamente: !sem_retomar_automatico,
            verificar: verificar_ao_abrir,
            max_passadas,
            monitor_ativo: false,
        };
        return rodar_agente(
            cli.catalogo.as_deref(),
            cli.socket,
            sem_monitor,
            abertura,
            hash,
        );
    }
    let json = cli.json;
    if matches!(cli.comando, Sub::Acompanhar)
        || matches!(cli.comando, Sub::Status(StatusArg { seguir: true, .. }))
    {
        return acompanhar(cli.catalogo.as_deref(), cli.socket.as_deref(), json);
    }
    let todos = matches!(cli.comando, Sub::Volumes { todos: true });
    let Execucao {
        resposta,
        agente,
        jobs,
    } = executar(cli);
    let codigo = codigo_de_saida(&resposta);
    if json {
        let mut valor = match serde_json::to_value(&resposta) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("erro ao serializar resposta: {e}");
                return ExitCode::from(1);
            }
        };
        if let (Some(info), Some(obj)) = (&agente, valor.as_object_mut()) {
            obj.insert("agente".into(), agente_json(info.as_ref()));
        }
        if let (Some(j), Some(obj)) = (&jobs, valor.as_object_mut()) {
            obj.insert(
                "jobs".into(),
                serde_json::to_value(j).unwrap_or(serde_json::Value::Null),
            );
        }
        println!("{valor}");
        if let Resposta::Erro { mensagem, .. } = &resposta {
            eprintln!("erro: {mensagem}");
        }
    } else {
        if let Some(info) = &agente {
            println!("{}", agente_texto(info.as_ref()));
        }
        saida::imprimir(&resposta, todos);
        if let Some(j) = &jobs {
            print!("\n{}", saida::jobs(j));
        }
    }
    ExitCode::from(codigo)
}
