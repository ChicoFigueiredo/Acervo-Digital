//! `dag biblioteca ...`: Curadoria Bibliográfica (Fase B). Cada subcomando vira um
//! `Comando::Biblioteca*` do protocolo; só o agente escreve no Catálogo (ADR 0004). A Biblioteca
//! Homologada é somente leitura: a única via que a altera é `homologar`, com aprovação de pessoa
//! (confirmação dupla neste CLI). Nada aqui apaga, move ou altera arquivo catalogado.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use dag_core::{
    AcaoRevisao, AlvoRevisao, AprovacaoHumana, Comando, DadosBiblioteca, EstadoFilaInfo,
    ItemDecisao, MotivoFila, PapelRegistro, Resposta,
};

/// Papel de uma biblioteca ao registrar.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PapelArg {
    /// Biblioteca Calibre do Chico: só leitura (`mode=ro`)
    Homologada,
    /// Biblioteca Calibre criada pelo DAG para receber os livros novos
    #[value(alias = "a_classificar")]
    AClassificar,
}

/// Por que o livro entra na fila.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotivoArg {
    Novo,
    Alterado,
    Reprocessar,
}

/// Estado de um item da fila.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoArg {
    Pendente,
    EmAndamento,
    Concluido,
    Falhou,
    Excecao,
}

/// Decisão sobre sugestões, por argumentos (um campo) ou em lote por arquivo JSON.
#[derive(Args, Debug)]
pub struct DecidirArg {
    /// Alvo: `arquivo:ID` (id do Catálogo; só o número também vale), `edicao:ID` ou `obra:ID`
    #[arg(
        long,
        value_name = "TIPO:ID",
        requires = "campo",
        conflicts_with = "de_arquivo"
    )]
    pub alvo: Option<String>,
    /// Campo: titulo, autores, tags, estante, editora, idioma, isbn...
    #[arg(long, requires = "alvo")]
    pub campo: Option<String>,
    /// Aceita a sugestão pendente do campo
    #[arg(long, conflicts_with_all = ["corrigir", "rejeitar"])]
    pub aceitar: bool,
    /// Corrige o campo com este valor (listas: separe com `|`)
    #[arg(long, value_name = "VALOR", conflicts_with_all = ["aceitar", "rejeitar"])]
    pub corrigir: Option<String>,
    /// Rejeita a sugestão do campo
    #[arg(long, conflicts_with_all = ["aceitar", "corrigir"])]
    pub rejeitar: bool,
    /// Lote de decisões em JSON: lista de `{alvo:{tipo,id}, campo, acao, valor?}` ou
    /// `{lote, itens:[...]}`
    #[arg(long, value_name = "ARQUIVO.json", conflicts_with_all = ["aceitar", "corrigir", "rejeitar"])]
    pub de_arquivo: Option<PathBuf>,
    /// Nome do lote (padrão: gerado); serve para `desfazer`
    #[arg(long)]
    pub lote: Option<String>,
}

/// Aprovação e confirmação da homologação.
#[derive(Args, Debug)]
pub struct HomologarArg {
    /// Id do cadastro (veja `dag biblioteca cadastrar`)
    pub cadastro: i64,
    /// Quem aprova (nome da pessoa; fica registrado)
    #[arg(long, value_name = "NOME")]
    pub aprovado_por: String,
    /// Motivo (opcional; fica registrado)
    #[arg(long)]
    pub motivo: Option<String>,
    /// Título do livro, para a segunda confirmação (digitá-lo ou digitar HOMOLOGAR)
    #[arg(long, value_name = "TÍTULO")]
    pub titulo: Option<String>,
    /// Dispensa as perguntas (scripts) e vale como a sua aprovação. Sem terminal, só com isto
    #[arg(long)]
    pub sim_eu_aprovo: bool,
}

/// Configuração da curadoria.
#[derive(Subcommand, Debug)]
pub enum AcaoConfig {
    /// Mostra a configuração atual (padrão)
    Obter,
    /// Altera chaves: `CHAVE=VALOR` (aninhadas com ponto: `ia.ativa=true`, `ocr.dpi=300`)
    Definir {
        #[arg(required = true, value_name = "CHAVE=VALOR")]
        pares: Vec<String>,
    },
}

/// Conjunto ouro do piloto (M5).
#[derive(Subcommand, Debug)]
pub enum Ouro {
    /// Monta uma amostra estratificada da revisão e grava um JSON por item (arquivos novos; a
    /// pasta precisa existir e nada é sobrescrito) para você preencher o gabarito
    Exportar {
        /// Tamanho da amostra
        #[arg(long, default_value_t = 150)]
        n: usize,
        /// Estratos (separe com vírgula; padrão: todos): matematica, xadrez, ficcao, quadrinhos,
        /// pdf_textual, pdf_imagem, epub, outros_formatos, duplicata_exata, titulo_coincidente
        #[arg(long, value_delimiter = ',')]
        estratos: Vec<String>,
        /// Pasta (já existente) onde gravar os casos
        #[arg(long, value_name = "DIR")]
        saida: PathBuf,
        /// Lê cada livro (só leitura) e inclui as páginas extraídas no caso, para o benchmark
        /// de modelos enxergar o texto (mais lento; PDFs escaneados usam OCR)
        #[arg(long)]
        com_texto: bool,
    },
    /// Calcula precisão por campo, cobertura e abstenções dos casos com gabarito preenchido
    /// contra as sugestões gravadas no export, e compara com as metas da especificação
    Avaliar {
        /// Pasta com os casos (JSON), a mesma de `exportar`
        #[arg(long, value_name = "DIR")]
        casos: PathBuf,
    },
}

/// `dag biblioteca ...`
#[derive(Subcommand, Debug)]
pub enum Biblioteca {
    /// Registra uma biblioteca Calibre (a A Classificar é criada pelo DAG se o caminho não for
    /// uma biblioteca)
    Registrar {
        caminho: String,
        #[arg(long, value_enum)]
        papel: PapelArg,
        #[arg(long)]
        nome: Option<String>,
    },
    /// Bibliotecas registradas, com contagens
    Listar,
    /// Perfil da biblioteca: colunas, estantes candidatas, tags e compatibilidade
    Perfil {
        /// Id ou caminho
        biblioteca: String,
    },
    /// Lê a Homologada em somente leitura e prova que nada mudou (trabalho longo)
    Sincronizar {
        /// Id ou caminho
        biblioteca: String,
    },
    /// Livros que você já tem: cópias fora da Homologada com o mesmo conteúdo
    Duplicatas {
        /// Só cópias sob esta pasta
        #[arg(long)]
        raiz: Option<String>,
        #[arg(long)]
        limite: Option<u32>,
        /// Cursor da página seguinte (impresso ao fim da página anterior)
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Espaço recuperável das duplicatas de livros (estimativa, não promessa)
    Espaco {
        /// Só este Volume (id ou rótulo)
        #[arg(long)]
        volume: Option<String>,
    },
    /// Enfileira os livros sob as pastas (barato; não lê o conteúdo)
    Enfileirar {
        #[arg(required = true)]
        raizes: Vec<String>,
        #[arg(long, value_enum)]
        motivo: Option<MotivoArg>,
    },
    /// Fila de processamento
    Fila {
        #[arg(long, value_enum)]
        estado: Option<EstadoArg>,
        #[arg(long)]
        limite: Option<u32>,
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Processa a fila (Ctrl-C pausa; rodar de novo retoma)
    Processar {
        /// Máximo de itens nesta rodada
        #[arg(long)]
        limite: Option<u32>,
        /// Consulta a IA local (precisa estar ativa na configuração)
        #[arg(long)]
        ia: bool,
    },
    /// Itens pendentes de revisão: extraído × sugestão × homologado parecido
    Revisar {
        #[arg(long)]
        limite: Option<u32>,
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Aceita, corrige ou rejeita sugestões (por argumentos ou em lote por arquivo)
    Decidir(DecidirArg),
    /// Desfaz um lote de decisões
    Desfazer { lote: String },
    /// Cadastra na A Classificar (via calibredb) os arquivos aprovados
    Cadastrar {
        #[arg(required = true, value_name = "FILE_ID")]
        file_ids: Vec<i64>,
    },
    /// Leva um cadastro da A Classificar à Homologada, com a sua aprovação (confirmação dupla)
    Homologar(HomologarArg),
    /// Alertas (padrão: só os não vistos; destaque para "você já tem este livro")
    Alertas {
        /// Inclui os já vistos
        #[arg(long)]
        todos: bool,
        #[arg(long)]
        limite: Option<u32>,
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Marca alertas como vistos
    Visto {
        #[arg(required = true)]
        ids: Vec<i64>,
    },
    /// Novidades desde um instante (padrão: 24 h)
    Resumo {
        /// `24h`, `7d`, `90m`, `AAAA-MM-DD` ou segundos Unix
        #[arg(long)]
        desde: Option<String>,
    },
    /// Configuração da curadoria (IA, OCR, homologação...)
    Config {
        #[command(subcommand)]
        acao: Option<AcaoConfig>,
    },
    /// Conjunto ouro do piloto: exportar amostra e avaliar
    #[command(subcommand)]
    Ouro(Ouro),
}

/// Opções da homologação interativa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpcoesHomologar {
    pub cadastro: i64,
    pub aprovado_por: String,
    pub motivo: Option<String>,
    pub titulo: Option<String>,
    pub sim_eu_aprovo: bool,
}

/// Opções da exportação do conjunto ouro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpcoesOuro {
    pub n: usize,
    pub estratos: Vec<String>,
    pub saida: PathBuf,
    pub com_texto: bool,
}

/// O que o subcomando faz além de mandar um único comando ao agente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fluxo {
    Simples,
    Homologar(OpcoesHomologar),
    OuroExportar(OpcoesOuro),
}

fn erro_arg(msg: impl Into<String>) -> Resposta {
    Resposta::Erro {
        codigo: "argumento_invalido".into(),
        mensagem: msg.into(),
    }
}

/// Monta o comando (o primeiro, nos fluxos de vários passos) e o fluxo.
///
/// # Errors
/// `Resposta::Erro` com código `argumento_invalido` (arquivo de lote ilegível, par `CHAVE=VALOR`
/// malformado...).
pub fn preparar(b: Biblioteca, agora: i64) -> Result<(Comando, Fluxo), Box<Resposta>> {
    let simples = |c| Ok((c, Fluxo::Simples));
    match b {
        Biblioteca::Registrar {
            caminho,
            papel,
            nome,
        } => simples(Comando::BibliotecaRegistrar {
            caminho: if caminho.is_empty() {
                caminho
            } else {
                crate::absoluto(&caminho)
            },
            papel: match papel {
                PapelArg::Homologada => PapelRegistro::Homologada,
                PapelArg::AClassificar => PapelRegistro::AClassificar,
            },
            nome,
        }),
        Biblioteca::Listar => simples(Comando::BibliotecaListar),
        Biblioteca::Perfil { biblioteca } => simples(Comando::BibliotecaPerfil {
            biblioteca: id_ou_caminho(&biblioteca),
        }),
        Biblioteca::Sincronizar { biblioteca } => simples(Comando::BibliotecaSincronizar {
            biblioteca: id_ou_caminho(&biblioteca),
        }),
        Biblioteca::Duplicatas {
            raiz,
            limite,
            cursor,
        } => simples(Comando::BibliotecaDuplicatas {
            raiz: raiz.map(|r| crate::absoluto(&r)),
            limite,
            cursor,
        }),
        Biblioteca::Espaco { volume } => simples(Comando::BibliotecaRelatorioEspaco { volume }),
        Biblioteca::Enfileirar { raizes, motivo } => simples(Comando::BibliotecaEnfileirar {
            raizes: raizes.iter().map(|r| crate::absoluto(r)).collect(),
            motivo: motivo.map(|m| match m {
                MotivoArg::Novo => MotivoFila::Novo,
                MotivoArg::Alterado => MotivoFila::Alterado,
                MotivoArg::Reprocessar => MotivoFila::Reprocessar,
            }),
        }),
        Biblioteca::Fila {
            estado,
            limite,
            cursor,
        } => simples(Comando::BibliotecaFila {
            estado: estado.map(|e| match e {
                EstadoArg::Pendente => EstadoFilaInfo::Pendente,
                EstadoArg::EmAndamento => EstadoFilaInfo::EmAndamento,
                EstadoArg::Concluido => EstadoFilaInfo::Concluido,
                EstadoArg::Falhou => EstadoFilaInfo::Falhou,
                EstadoArg::Excecao => EstadoFilaInfo::Excecao,
            }),
            limite,
            cursor,
        }),
        Biblioteca::Processar { limite, ia } => {
            simples(Comando::BibliotecaProcessar { limite, ia })
        }
        Biblioteca::Revisar { limite, cursor } => {
            simples(Comando::BibliotecaRevisao { limite, cursor })
        }
        Biblioteca::Decidir(d) => {
            let (itens, lote) = montar_decisao(&d).map_err(|m| Box::new(erro_arg(m)))?;
            simples(Comando::BibliotecaDecidir { itens, lote })
        }
        Biblioteca::Desfazer { lote } => simples(Comando::BibliotecaDesfazer { lote }),
        Biblioteca::Cadastrar { file_ids } => {
            simples(Comando::BibliotecaCadastrar { itens: file_ids })
        }
        Biblioteca::Homologar(h) => {
            if h.aprovado_por.trim().is_empty() {
                return Err(Box::new(erro_arg("--aprovado-por precisa de um nome")));
            }
            let op = OpcoesHomologar {
                cadastro: h.cadastro,
                aprovado_por: h.aprovado_por.trim().to_string(),
                motivo: h.motivo,
                titulo: h.titulo.filter(|t| !t.trim().is_empty()),
                sim_eu_aprovo: h.sim_eu_aprovo,
            };
            // O primeiro passo só PROPÕE (sem aprovação): nada é escrito na Homologada.
            Ok((
                Comando::BibliotecaHomologar {
                    item: op.cadastro,
                    aprovacao: None,
                },
                Fluxo::Homologar(op),
            ))
        }
        Biblioteca::Alertas {
            todos,
            limite,
            cursor,
        } => simples(Comando::BibliotecaAlertas {
            nao_vistos: !todos,
            limite,
            cursor,
        }),
        Biblioteca::Visto { ids } => simples(Comando::BibliotecaMarcarVisto { ids }),
        Biblioteca::Resumo { desde } => {
            let desde = desde
                .map(|d| parse_desde(&d, agora))
                .transpose()
                .map_err(|m| Box::new(erro_arg(m)))?;
            simples(Comando::BibliotecaResumo { desde })
        }
        Biblioteca::Config { acao } => match acao {
            None | Some(AcaoConfig::Obter) => simples(Comando::BibliotecaConfigObter),
            Some(AcaoConfig::Definir { pares }) => {
                let alteracoes = pares_para_json(&pares).map_err(|m| Box::new(erro_arg(m)))?;
                simples(Comando::BibliotecaConfigDefinir { alteracoes })
            }
        },
        Biblioteca::Ouro(Ouro::Exportar {
            n,
            estratos,
            saida,
            com_texto,
        }) => {
            if n == 0 {
                return Err(Box::new(erro_arg("--n precisa ser maior que zero")));
            }
            crate::ouro::validar_estratos(&estratos).map_err(|m| Box::new(erro_arg(m)))?;
            Ok((
                Comando::BibliotecaRevisao {
                    limite: Some(PAGINA_OURO),
                    cursor: None,
                },
                Fluxo::OuroExportar(OpcoesOuro {
                    n,
                    estratos,
                    saida,
                    com_texto,
                }),
            ))
        }
        Biblioteca::Ouro(Ouro::Avaliar { .. }) => Err(Box::new(erro_arg(
            "`ouro avaliar` não usa o agente (tratado antes do envio)",
        ))),
    }
}

/// Itens por página ao ler a revisão para o conjunto ouro.
pub const PAGINA_OURO: u32 = 100;

/// Número puro vira id; qualquer outra coisa é caminho (absolutizado).
fn id_ou_caminho(s: &str) -> String {
    if s.parse::<i64>().is_ok() {
        s.to_string()
    } else {
        crate::absoluto(s)
    }
}

/// `arquivo:12`, `edicao:3`, `obra:4` ou só `12` (arquivo).
fn parse_alvo(s: &str) -> Result<AlvoRevisao, String> {
    let (tipo, id) = match s.split_once(':') {
        Some((t, i)) => (t.trim().to_lowercase(), i.trim()),
        None => ("arquivo".to_string(), s.trim()),
    };
    let id: i64 = id
        .parse()
        .map_err(|_| format!("alvo inválido `{s}`: use arquivo:ID, edicao:ID ou obra:ID"))?;
    match tipo.as_str() {
        "arquivo" => Ok(AlvoRevisao::Arquivo(id)),
        "edicao" | "edição" => Ok(AlvoRevisao::Edicao(id)),
        "obra" => Ok(AlvoRevisao::Obra(id)),
        _ => Err(format!(
            "tipo de alvo desconhecido `{tipo}`: use arquivo, edicao ou obra"
        )),
    }
}

fn validar_item(i: usize, it: &ItemDecisao) -> Result<(), String> {
    if it.campo.trim().is_empty() {
        return Err(format!("item {}: campo vazio", i + 1));
    }
    match (it.acao, &it.valor) {
        (AcaoRevisao::Corrigir, None) => Err(format!(
            "item {} ({}): `corrigir` exige `valor`",
            i + 1,
            it.campo
        )),
        (AcaoRevisao::Aceitar | AcaoRevisao::Rejeitar, Some(_)) => Err(format!(
            "item {} ({}): `valor` só vale em `corrigir`",
            i + 1,
            it.campo
        )),
        _ => Ok(()),
    }
}

/// Itens e nome do lote a partir dos argumentos ou do arquivo JSON.
fn montar_decisao(d: &DecidirArg) -> Result<(Vec<ItemDecisao>, Option<String>), String> {
    if let Some(arq) = &d.de_arquivo {
        let texto = std::fs::read_to_string(arq)
            .map_err(|e| format!("não consegui ler {}: {e}", arq.display()))?;
        let (itens, lote_do_arquivo) = ler_lote(&texto)?;
        return Ok((itens, d.lote.clone().or(lote_do_arquivo)));
    }
    let (Some(alvo), Some(campo)) = (&d.alvo, &d.campo) else {
        return Err("informe --alvo e --campo (com --aceitar, --corrigir VALOR ou --rejeitar) ou --de-arquivo".into());
    };
    let (acao, valor) = match (d.aceitar, &d.corrigir, d.rejeitar) {
        (true, None, false) => (AcaoRevisao::Aceitar, None),
        (false, Some(v), false) => (AcaoRevisao::Corrigir, Some(v.clone())),
        (false, None, true) => (AcaoRevisao::Rejeitar, None),
        _ => {
            return Err(
                "escolha exatamente uma ação: --aceitar, --corrigir VALOR ou --rejeitar".into(),
            );
        }
    };
    let item = ItemDecisao {
        alvo: parse_alvo(alvo)?,
        campo: campo.trim().to_string(),
        acao,
        valor,
    };
    validar_item(0, &item)?;
    Ok((vec![item], d.lote.clone()))
}

/// Lê o JSON de um lote: lista de itens ou `{ "lote": "...", "itens": [...] }`.
///
/// # Errors
/// JSON inválido ou item inconsistente.
pub fn ler_lote(texto: &str) -> Result<(Vec<ItemDecisao>, Option<String>), String> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        #[serde(default)]
        lote: Option<String>,
        itens: Vec<ItemDecisao>,
    }
    let valor: serde_json::Value =
        serde_json::from_str(texto).map_err(|e| format!("JSON do lote inválido: {e}"))?;
    let (itens, lote) = if valor.is_array() {
        let itens: Vec<ItemDecisao> =
            serde_json::from_value(valor).map_err(|e| format!("item do lote inválido: {e}"))?;
        (itens, None)
    } else {
        let e: Envelope = serde_json::from_value(valor)
            .map_err(|e| format!("lote inválido (esperado lista ou {{lote, itens}}): {e}"))?;
        (e.itens, e.lote)
    };
    if itens.is_empty() {
        return Err("o lote não tem nenhum item".into());
    }
    for (i, it) in itens.iter().enumerate() {
        validar_item(i, it)?;
    }
    Ok((itens, lote))
}

/// `ia.ativa=true ocr.dpi=300` vira `{"ia":{"ativa":true},"ocr":{"dpi":300}}` (merge-patch).
/// O valor é JSON se for JSON válido (`true`, `300`, `["por"]`); senão, texto.
///
/// # Errors
/// Par sem `=`, chave vazia ou chave repetida com valores incompatíveis.
pub fn pares_para_json(pares: &[String]) -> Result<serde_json::Value, String> {
    let mut raiz = serde_json::Map::new();
    for p in pares {
        let (chave, valor) = p
            .split_once('=')
            .ok_or_else(|| format!("`{p}`: use CHAVE=VALOR"))?;
        let partes: Vec<&str> = chave.trim().split('.').collect();
        if partes.iter().any(|x| x.is_empty()) {
            return Err(format!("`{p}`: chave vazia"));
        }
        let v = serde_json::from_str::<serde_json::Value>(valor.trim())
            .unwrap_or_else(|_| serde_json::Value::String(valor.to_string()));
        let mut atual = &mut raiz;
        for (i, parte) in partes.iter().enumerate() {
            if i + 1 == partes.len() {
                atual.insert((*parte).to_string(), v.clone());
            } else {
                let entrada = atual
                    .entry((*parte).to_string())
                    .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                atual = entrada
                    .as_object_mut()
                    .ok_or_else(|| format!("`{p}`: `{parte}` já recebeu um valor simples"))?;
            }
        }
    }
    Ok(serde_json::Value::Object(raiz))
}

/// Segundos Unix de `--desde`: número puro (segundos Unix), `30m`, `24h`, `7d` (para trás a
/// partir de `agora`), `AAAA-MM-DD` ou `AAAA-MM-DD HH:MM` (UTC).
///
/// # Errors
/// Formato não reconhecido.
pub fn parse_desde(s: &str, agora: i64) -> Result<i64, String> {
    let t = s.trim();
    let invalido =
        || format!("--desde inválido `{s}`: use 24h, 7d, 90m, AAAA-MM-DD ou segundos Unix");
    if let Ok(n) = t.parse::<i64>() {
        return Ok(n);
    }
    if let Some(ult) = t.chars().last()
        && let Some(mult) = match ult {
            'm' => Some(60),
            'h' => Some(3600),
            'd' => Some(86_400),
            _ => None,
        }
        && let Ok(n) = t[..t.len() - 1].trim().parse::<i64>()
    {
        return Ok(agora - n.saturating_mul(mult));
    }
    let (data, hora) = t.split_once([' ', 'T']).unwrap_or((t, "00:00"));
    let mut d = data.split('-');
    let (Some(a), Some(m), Some(di), None) = (d.next(), d.next(), d.next(), d.next()) else {
        return Err(invalido());
    };
    let (a, m, di): (i64, i64, i64) = (
        a.parse().map_err(|_| invalido())?,
        m.parse().map_err(|_| invalido())?,
        di.parse().map_err(|_| invalido())?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&di) {
        return Err(invalido());
    }
    let mut h = hora.split(':');
    let hh: i64 = h.next().unwrap_or("0").parse().map_err(|_| invalido())?;
    let mm: i64 = h.next().unwrap_or("0").parse().map_err(|_| invalido())?;
    if !(0..24).contains(&hh) || !(0..60).contains(&mm) {
        return Err(invalido());
    }
    Ok(dias_desde_epoca(a, m, di) * 86_400 + hh * 3600 + mm * 60)
}

/// Dias desde 1970-01-01 (calendário gregoriano proléptico).
fn dias_desde_epoca(a: i64, m: i64, d: i64) -> i64 {
    let a = if m <= 2 { a - 1 } else { a };
    let era = a.div_euclid(400);
    let yoe = a - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ---------------------------------------------------------------------------------------------
// Homologação com confirmação dupla
// ---------------------------------------------------------------------------------------------

/// Palavra aceita na segunda confirmação quando o título não é digitado.
pub const PALAVRA_HOMOLOGAR: &str = "HOMOLOGAR";

fn linha(entrada: &mut dyn BufRead) -> Result<String, String> {
    let mut s = String::new();
    match entrada.read_line(&mut s) {
        Ok(0) => Err("sem confirmação (entrada encerrada); nada foi escrito na Homologada".into()),
        Ok(_) => Ok(s.trim().to_string()),
        Err(e) => Err(format!("não consegui ler a confirmação: {e}")),
    }
}

/// Confirmação dupla da homologação. `Ok(())` só com as duas respostas (ou `sim_eu_aprovo`).
/// Sem terminal e sem `sim_eu_aprovo`, recusa: a aprovação é de uma pessoa.
///
/// # Errors
/// Texto explicando por que nada foi escrito.
pub fn confirmar_homologacao(
    titulo: Option<&str>,
    tem_terminal: bool,
    sim_eu_aprovo: bool,
    entrada: &mut dyn BufRead,
    saida: &mut dyn Write,
) -> Result<(), String> {
    if sim_eu_aprovo {
        return Ok(());
    }
    if !tem_terminal {
        return Err("sem terminal: a homologação ESCREVE na Biblioteca Homologada e exige a confirmação de uma pessoa. Rode num terminal ou, sabendo o que faz, passe --sim-eu-aprovo".into());
    }
    let _ = write!(
        saida,
        "Escrever este livro na Biblioteca Homologada? [s/N] "
    );
    let _ = saida.flush();
    let r = linha(entrada)?;
    if !matches!(r.to_lowercase().as_str(), "s" | "sim") {
        return Err("cancelado na primeira confirmação; nada foi escrito".into());
    }
    match titulo {
        Some(t) => {
            let _ = write!(
                saida,
                "Segunda confirmação. Digite o título do livro (\"{t}\") ou {PALAVRA_HOMOLOGAR}: "
            );
        }
        None => {
            let _ = write!(
                saida,
                "Segunda confirmação. Digite {PALAVRA_HOMOLOGAR} para escrever na Homologada: "
            );
        }
    }
    let _ = saida.flush();
    let r = linha(entrada)?;
    let confere = r == PALAVRA_HOMOLOGAR
        || titulo.is_some_and(|t| !r.is_empty() && r.to_lowercase() == t.trim().to_lowercase());
    if !confere {
        return Err("cancelado na segunda confirmação; nada foi escrito".into());
    }
    Ok(())
}

/// O que será escrito, antes de pedir a aprovação.
fn plano_homologacao(
    op: &OpcoesHomologar,
    calibre_id: Option<i64>,
    destino: Option<&dag_core::BibliotecaInfo>,
    modo: &str,
) -> String {
    let mut s = format!("Homologar o cadastro {}", op.cadastro);
    if let Some(c) = calibre_id {
        s.push_str(&format!(" (livro {c} na A Classificar)"));
    }
    s.push_str("\nIsto ESCREVE na Biblioteca Homologada, e só com a sua aprovação:\n");
    match destino {
        Some(b) => s.push_str(&format!(
            "  destino:  {} (id {}), {}\n",
            b.nome, b.id, b.caminho
        )),
        None => s.push_str("  destino:  (Homologada não encontrada entre as registradas)\n"),
    }
    s.push_str(&format!("  como:     {modo}\n"));
    if let Some(t) = &op.titulo {
        s.push_str(&format!("  livro:    {t}\n"));
    }
    s.push_str(
        "  o que:    o arquivo e os metadados já aprovados na A Classificar entram como um livro\n            novo na Homologada; nenhum livro existente é alterado ou apagado\n",
    );
    s.push_str(&format!("  quem:     {}", op.aprovado_por));
    if let Some(m) = &op.motivo {
        s.push_str(&format!(" (motivo: {m})"));
    }
    s.push_str(
        "\n  depois:   a aprovação e o que foi escrito ficam registrados no Catálogo, com o\n            necessário para reverter\n",
    );
    s
}

/// Homologação em três passos: propõe (nada é escrito), mostra o plano, pede a confirmação dupla
/// e só então envia a aprovação. `enviar` fala com o agente.
pub fn fluxo_homologar(
    op: &OpcoesHomologar,
    enviar: &mut dyn FnMut(Comando) -> Resposta,
    tem_terminal: bool,
    entrada: &mut dyn BufRead,
    saida: &mut dyn Write,
) -> Resposta {
    let proposta = enviar(Comando::BibliotecaHomologar {
        item: op.cadastro,
        aprovacao: None,
    });
    let Resposta::Biblioteca {
        dados: DadosBiblioteca::Homologacao { homologacao },
    } = &proposta
    else {
        return proposta;
    };
    if homologacao.estado == "aplicada" {
        let _ = writeln!(saida, "Este cadastro já foi homologado; nada a fazer.");
        return proposta;
    }
    // Contexto para o plano (falhas aqui não impedem: o plano só fica mais pobre).
    let destino = match enviar(Comando::BibliotecaListar) {
        Resposta::Biblioteca {
            dados: DadosBiblioteca::Bibliotecas { itens },
        } => itens
            .into_iter()
            .find(|b| b.papel == PapelRegistro::Homologada),
        _ => None,
    };
    let modo = match enviar(Comando::BibliotecaConfigObter) {
        Resposta::Biblioteca {
            dados: DadosBiblioteca::Config { config },
        } => match config.homologacao.modo {
            dag_core::ModoHomologacao::Local => {
                "calibredb direto na pasta (o Calibre precisa estar fechado)".to_string()
            }
            dag_core::ModoHomologacao::Servidor => format!(
                "calibre-server em {}",
                config
                    .homologacao
                    .url
                    .as_deref()
                    .unwrap_or("(url não configurada)")
            ),
        },
        _ => "calibredb (modo não conferido)".to_string(),
    };
    let _ = write!(
        saida,
        "{}",
        plano_homologacao(op, homologacao.calibre_id, destino.as_ref(), &modo)
    );
    if let Err(m) = confirmar_homologacao(
        op.titulo.as_deref(),
        tem_terminal,
        op.sim_eu_aprovo,
        entrada,
        saida,
    ) {
        return Resposta::Erro {
            codigo: "cancelado".into(),
            mensagem: m,
        };
    }
    enviar(Comando::BibliotecaHomologar {
        item: op.cadastro,
        aprovacao: Some(AprovacaoHumana {
            quem: op.aprovado_por.clone(),
            quando: None,
            motivo: op.motivo.clone(),
        }),
    })
}

// ---------------------------------------------------------------------------------------------
// Conjunto ouro
// ---------------------------------------------------------------------------------------------

/// Extração completa (com páginas) de um livro, só leitura, para os casos do conjunto ouro.
fn extrair_para_ouro(
    item: &dag_core::ItemRevisaoInfo,
    cfg: &dag_core::ConfigBiblioteca,
) -> Result<Option<dag_biblio::ResultadoExtracao>, String> {
    use dag_agent::ExtratorLivro;
    let caminho = item
        .caminho
        .as_deref()
        .ok_or("o Catálogo não informou o caminho")?;
    let formato = dag_biblio::FormatoLivro::da_extensao(
        std::path::Path::new(caminho)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default(),
    )
    .unwrap_or(dag_biblio::FormatoLivro::Outro);
    dag_agent::ExtratorPadrao::novo()
        .extrair(std::path::Path::new(caminho), formato, cfg)
        .map(|s| Some(s.resultado))
}

/// `ouro exportar`: lê a revisão pendente (páginas até `MAX_POOL`), monta a amostra estratificada
/// e grava um JSON novo por item. `enviar` fala com o agente.
pub fn fluxo_ouro_exportar(
    op: &OpcoesOuro,
    enviar: &mut dyn FnMut(Comando) -> Resposta,
) -> Resposta {
    let estratos = match crate::ouro::validar_estratos(&op.estratos) {
        Ok(e) => e,
        Err(m) => return erro_arg(m),
    };
    let mut itens = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        match enviar(Comando::BibliotecaRevisao {
            limite: Some(PAGINA_OURO),
            cursor: cursor.clone(),
        }) {
            Resposta::Biblioteca {
                dados:
                    DadosBiblioteca::Revisao {
                        itens: mut pagina,
                        proximo_cursor,
                        ..
                    },
            } => {
                itens.append(&mut pagina);
                cursor = proximo_cursor;
            }
            outra @ Resposta::Erro { .. } => return outra,
            _ => return erro_arg("resposta inesperada do agente à revisão"),
        }
        if cursor.is_none() || itens.len() >= crate::ouro::MAX_POOL {
            break;
        }
    }
    let cfg = if op.com_texto {
        match enviar(Comando::BibliotecaConfigObter) {
            Resposta::Biblioteca {
                dados: DadosBiblioteca::Config { config },
            } => config,
            _ => dag_core::ConfigBiblioteca::default(),
        }
    } else {
        dag_core::ConfigBiblioteca::default()
    };
    let com_texto = op.com_texto;
    let mut extrair = |i: &dag_core::ItemRevisaoInfo| {
        if com_texto {
            extrair_para_ouro(i, &cfg)
        } else {
            Ok(None)
        }
    };
    match crate::ouro::exportar(&itens, &estratos, op.n, &op.saida, &mut extrair) {
        Ok(mut rel) => {
            if cursor.is_some() {
                rel.avisos.push(format!(
                    "a amostra saiu dos primeiros {} itens em revisão (há mais)",
                    crate::ouro::MAX_POOL
                ));
            }
            Resposta::Ok {
                mensagem: Some(crate::ouro::texto_exportacao(&rel, &op.saida)),
            }
        }
        Err(m) => erro_arg(m),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod testes {
    use super::*;
    use crate::{Cli, Sub};
    use clap::Parser;
    use dag_core::{BibliotecaInfo, ConfigBiblioteca, HomologacaoInfo};

    const AGORA: i64 = 1_700_000_000;

    fn prep(args: &[&str]) -> (Comando, Fluxo) {
        let mut v = vec!["dag", "biblioteca"];
        v.extend_from_slice(args);
        let cli = Cli::try_parse_from(v).unwrap();
        match cli.comando {
            Sub::Biblioteca(b) => preparar(b, AGORA).unwrap(),
            o => panic!("{o:?}"),
        }
    }

    fn prep_erro(args: &[&str]) -> String {
        let mut v = vec!["dag", "biblioteca"];
        v.extend_from_slice(args);
        let cli = Cli::try_parse_from(v).unwrap();
        match cli.comando {
            Sub::Biblioteca(b) => match *preparar(b, AGORA).unwrap_err() {
                Resposta::Erro { mensagem, .. } => mensagem,
                o => panic!("{o:?}"),
            },
            o => panic!("{o:?}"),
        }
    }

    fn cmd(args: &[&str]) -> Comando {
        prep(args).0
    }

    #[test]
    fn registrar_papeis() {
        match cmd(&[
            "registrar",
            "/mnt/e/Livros",
            "--papel",
            "homologada",
            "--nome",
            "Livros",
        ]) {
            Comando::BibliotecaRegistrar {
                caminho,
                papel,
                nome,
            } => {
                assert_eq!(caminho, "/mnt/e/Livros");
                assert_eq!(papel, PapelRegistro::Homologada);
                assert_eq!(nome.as_deref(), Some("Livros"));
            }
            o => panic!("{o:?}"),
        }
        match cmd(&["registrar", "/mnt/e2/X", "--papel", "a-classificar"]) {
            Comando::BibliotecaRegistrar { papel, nome, .. } => {
                assert_eq!(papel, PapelRegistro::AClassificar);
                assert!(nome.is_none());
            }
            o => panic!("{o:?}"),
        }
        assert!(
            Cli::try_parse_from(["dag", "biblioteca", "registrar", "/x"]).is_err(),
            "--papel é obrigatório"
        );
        assert!(
            Cli::try_parse_from(["dag", "biblioteca", "registrar", "/x", "--papel", "outra"])
                .is_err()
        );
    }

    #[test]
    fn leitura_e_paginacao() {
        assert_eq!(cmd(&["listar"]), Comando::BibliotecaListar);
        assert_eq!(
            cmd(&["perfil", "3"]),
            Comando::BibliotecaPerfil {
                biblioteca: "3".into()
            }
        );
        assert_eq!(
            cmd(&["sincronizar", "/mnt/e/Livros"]),
            Comando::BibliotecaSincronizar {
                biblioteca: "/mnt/e/Livros".into()
            }
        );
        assert_eq!(
            cmd(&[
                "duplicatas",
                "--raiz",
                "/mnt/d",
                "--limite",
                "20",
                "--cursor",
                "abc"
            ]),
            Comando::BibliotecaDuplicatas {
                raiz: Some("/mnt/d".into()),
                limite: Some(20),
                cursor: Some("abc".into())
            }
        );
        assert_eq!(
            cmd(&["espaco", "--volume", "D"]),
            Comando::BibliotecaRelatorioEspaco {
                volume: Some("D".into())
            }
        );
        assert_eq!(
            cmd(&["fila", "--estado", "em-andamento", "--limite", "5"]),
            Comando::BibliotecaFila {
                estado: Some(EstadoFilaInfo::EmAndamento),
                limite: Some(5),
                cursor: None
            }
        );
        assert_eq!(
            cmd(&["revisar", "--limite", "3"]),
            Comando::BibliotecaRevisao {
                limite: Some(3),
                cursor: None
            }
        );
    }

    #[test]
    fn fila_e_processamento() {
        assert_eq!(
            cmd(&[
                "enfileirar",
                "/mnt/e/_classifica",
                "/mnt/d/novos",
                "--motivo",
                "reprocessar"
            ]),
            Comando::BibliotecaEnfileirar {
                raizes: vec!["/mnt/e/_classifica".into(), "/mnt/d/novos".into()],
                motivo: Some(MotivoFila::Reprocessar)
            }
        );
        assert!(Cli::try_parse_from(["dag", "biblioteca", "enfileirar"]).is_err());
        assert_eq!(
            cmd(&["processar", "--limite", "10", "--ia"]),
            Comando::BibliotecaProcessar {
                limite: Some(10),
                ia: true
            }
        );
        assert_eq!(
            cmd(&["processar"]),
            Comando::BibliotecaProcessar {
                limite: None,
                ia: false
            }
        );
    }

    #[test]
    fn decidir_por_argumentos() {
        match cmd(&[
            "decidir",
            "--alvo",
            "arquivo:12",
            "--campo",
            "titulo",
            "--aceitar",
        ]) {
            Comando::BibliotecaDecidir { itens, lote } => {
                assert_eq!(
                    itens,
                    vec![ItemDecisao {
                        alvo: AlvoRevisao::Arquivo(12),
                        campo: "titulo".into(),
                        acao: AcaoRevisao::Aceitar,
                        valor: None
                    }]
                );
                assert!(lote.is_none());
            }
            o => panic!("{o:?}"),
        }
        match cmd(&[
            "decidir",
            "--alvo",
            "edicao:3",
            "--campo",
            "tags",
            "--corrigir",
            "xadrez|finais",
            "--lote",
            "L1",
        ]) {
            Comando::BibliotecaDecidir { itens, lote } => {
                assert_eq!(itens[0].alvo, AlvoRevisao::Edicao(3));
                assert_eq!(itens[0].acao, AcaoRevisao::Corrigir);
                assert_eq!(itens[0].valor.as_deref(), Some("xadrez|finais"));
                assert_eq!(lote.as_deref(), Some("L1"));
            }
            o => panic!("{o:?}"),
        }
        match cmd(&["decidir", "--alvo", "7", "--campo", "estante", "--rejeitar"]) {
            Comando::BibliotecaDecidir { itens, .. } => {
                assert_eq!(itens[0].alvo, AlvoRevisao::Arquivo(7));
                assert_eq!(itens[0].acao, AcaoRevisao::Rejeitar);
            }
            o => panic!("{o:?}"),
        }
        // Ações conflitantes e alvo sem campo são recusados já no parse.
        assert!(
            Cli::try_parse_from([
                "dag",
                "biblioteca",
                "decidir",
                "--alvo",
                "1",
                "--campo",
                "titulo",
                "--aceitar",
                "--rejeitar"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["dag", "biblioteca", "decidir", "--alvo", "1"]).is_err());
        // Sem ação nenhuma: erro de argumento do preparar.
        let m = prep_erro(&["decidir", "--alvo", "1", "--campo", "titulo"]);
        assert!(m.contains("exatamente uma ação"), "{m}");
        let m = prep_erro(&[
            "decidir",
            "--alvo",
            "livro:1",
            "--campo",
            "titulo",
            "--aceitar",
        ]);
        assert!(m.contains("tipo de alvo desconhecido"), "{m}");
    }

    #[test]
    fn decidir_em_lote_por_arquivo() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("lote.json");
        std::fs::write(
            &p,
            r#"{"lote":"piloto-1","itens":[
                {"alvo":{"tipo":"arquivo","id":1},"campo":"titulo","acao":"aceitar"},
                {"alvo":{"tipo":"obra","id":9},"campo":"tags","acao":"corrigir","valor":"a|b"}
            ]}"#,
        )
        .unwrap();
        match cmd(&["decidir", "--de-arquivo", p.to_str().unwrap()]) {
            Comando::BibliotecaDecidir { itens, lote } => {
                assert_eq!(itens.len(), 2);
                assert_eq!(itens[1].alvo, AlvoRevisao::Obra(9));
                assert_eq!(lote.as_deref(), Some("piloto-1"));
            }
            o => panic!("{o:?}"),
        }
        // --lote da linha de comando vence o do arquivo.
        match cmd(&[
            "decidir",
            "--de-arquivo",
            p.to_str().unwrap(),
            "--lote",
            "outro",
        ]) {
            Comando::BibliotecaDecidir { lote, .. } => assert_eq!(lote.as_deref(), Some("outro")),
            o => panic!("{o:?}"),
        }
        // Lista pura e erros.
        let (itens, lote) =
            ler_lote(r#"[{"alvo":{"tipo":"arquivo","id":1},"campo":"titulo","acao":"rejeitar"}]"#)
                .unwrap();
        assert_eq!((itens.len(), lote), (1, None));
        assert!(ler_lote("[]").unwrap_err().contains("nenhum item"));
        assert!(ler_lote("{").unwrap_err().contains("inválido"));
        let e =
            ler_lote(r#"[{"alvo":{"tipo":"arquivo","id":1},"campo":"titulo","acao":"corrigir"}]"#)
                .unwrap_err();
        assert!(e.contains("exige `valor`"), "{e}");
        let e = ler_lote(
            r#"[{"alvo":{"tipo":"arquivo","id":1},"campo":"titulo","acao":"aceitar","valor":"x"}]"#,
        )
        .unwrap_err();
        assert!(e.contains("só vale em `corrigir`"), "{e}");
        let m = prep_erro(&["decidir", "--de-arquivo", "/nao/existe.json"]);
        assert!(m.contains("não consegui ler"), "{m}");
    }

    #[test]
    fn cadastro_alertas_visto_resumo() {
        assert_eq!(
            cmd(&["cadastrar", "10", "11"]),
            Comando::BibliotecaCadastrar {
                itens: vec![10, 11]
            }
        );
        assert_eq!(
            cmd(&["desfazer", "L1"]),
            Comando::BibliotecaDesfazer { lote: "L1".into() }
        );
        assert_eq!(
            cmd(&["alertas"]),
            Comando::BibliotecaAlertas {
                nao_vistos: true,
                limite: None,
                cursor: None
            }
        );
        assert_eq!(
            cmd(&["alertas", "--todos", "--limite", "9"]),
            Comando::BibliotecaAlertas {
                nao_vistos: false,
                limite: Some(9),
                cursor: None
            }
        );
        assert_eq!(
            cmd(&["visto", "1", "2", "3"]),
            Comando::BibliotecaMarcarVisto { ids: vec![1, 2, 3] }
        );
        assert_eq!(cmd(&["resumo"]), Comando::BibliotecaResumo { desde: None });
        assert_eq!(
            cmd(&["resumo", "--desde", "24h"]),
            Comando::BibliotecaResumo {
                desde: Some(AGORA - 86_400)
            }
        );
        assert!(prep_erro(&["resumo", "--desde", "ontem"]).contains("--desde inválido"));
    }

    #[test]
    fn desde_aceita_relativo_data_e_unix() {
        assert_eq!(parse_desde("90m", 10_000).unwrap(), 10_000 - 5_400);
        assert_eq!(parse_desde("7d", 1_000_000).unwrap(), 1_000_000 - 604_800);
        assert_eq!(parse_desde("1700000000", 5).unwrap(), 1_700_000_000);
        assert_eq!(parse_desde("1970-01-02", 0).unwrap(), 86_400);
        assert_eq!(parse_desde("2024-02-29 12:30", 0).unwrap(), 1_709_209_800);
        assert!(parse_desde("2024-13-01", 0).is_err());
        assert!(parse_desde("", 0).is_err());
    }

    #[test]
    fn config_obter_e_definir() {
        assert_eq!(cmd(&["config"]), Comando::BibliotecaConfigObter);
        assert_eq!(cmd(&["config", "obter"]), Comando::BibliotecaConfigObter);
        match cmd(&[
            "config",
            "definir",
            "ia.ativa=true",
            "ocr.dpi=300",
            "ia.modelo_primario=gemma3:12b",
            "destino_a_classificar=/mnt/e2/L",
            "ocr.idiomas=[\"por\"]",
        ]) {
            Comando::BibliotecaConfigDefinir { alteracoes } => assert_eq!(
                alteracoes,
                serde_json::json!({
                    "ia": {"ativa": true, "modelo_primario": "gemma3:12b"},
                    "ocr": {"dpi": 300, "idiomas": ["por"]},
                    "destino_a_classificar": "/mnt/e2/L"
                })
            ),
            o => panic!("{o:?}"),
        }
        assert!(prep_erro(&["config", "definir", "semigual"]).contains("CHAVE=VALOR"));
        assert!(prep_erro(&["config", "definir", "a..b=1"]).contains("chave vazia"));
        assert!(prep_erro(&["config", "definir", "a=1", "a.b=2"]).contains("valor simples"));
    }

    #[test]
    fn homologar_propoe_primeiro_e_exige_quem() {
        let (c, f) = prep(&[
            "homologar",
            "12",
            "--aprovado-por",
            "Chico",
            "--motivo",
            "conferi",
        ]);
        assert_eq!(
            c,
            Comando::BibliotecaHomologar {
                item: 12,
                aprovacao: None
            }
        );
        assert_eq!(
            f,
            Fluxo::Homologar(OpcoesHomologar {
                cadastro: 12,
                aprovado_por: "Chico".into(),
                motivo: Some("conferi".into()),
                titulo: None,
                sim_eu_aprovo: false
            })
        );
        assert!(Cli::try_parse_from(["dag", "biblioteca", "homologar", "12"]).is_err());
        assert!(prep_erro(&["homologar", "12", "--aprovado-por", "  "]).contains("nome"));
        let (_, f) = prep(&["homologar", "1", "--aprovado-por", "C", "--sim-eu-aprovo"]);
        assert!(matches!(f, Fluxo::Homologar(o) if o.sim_eu_aprovo));
    }

    #[test]
    fn ouro_exportar_valida_estratos() {
        let (c, f) = prep(&[
            "ouro",
            "exportar",
            "--n",
            "20",
            "--estratos",
            "xadrez,epub",
            "--saida",
            "/tmp/ouro",
        ]);
        assert!(matches!(c, Comando::BibliotecaRevisao { .. }));
        assert_eq!(
            f,
            Fluxo::OuroExportar(OpcoesOuro {
                n: 20,
                estratos: vec!["xadrez".into(), "epub".into()],
                saida: "/tmp/ouro".into(),
                com_texto: false
            })
        );
        let m = prep_erro(&[
            "ouro",
            "exportar",
            "--estratos",
            "poesia",
            "--saida",
            "/tmp/o",
        ]);
        assert!(m.contains("estrato desconhecido"), "{m}");
        assert!(prep_erro(&["ouro", "exportar", "--n", "0", "--saida", "/tmp/o"]).contains("--n"));
    }

    // ---- homologação com confirmação dupla -------------------------------------------------

    fn rodar_conf(
        titulo: Option<&str>,
        tty: bool,
        sim: bool,
        entrada: &str,
    ) -> (Result<(), String>, String) {
        let mut ent = entrada.as_bytes();
        let mut sai = Vec::new();
        let r = confirmar_homologacao(titulo, tty, sim, &mut ent, &mut sai);
        (r, String::from_utf8(sai).unwrap())
    }

    #[test]
    fn confirmacao_dupla_com_palavra_ou_titulo() {
        let (r, saida) = rodar_conf(None, true, false, "s\nHOMOLOGAR\n");
        assert!(r.is_ok());
        assert!(saida.contains("[s/N]") && saida.contains("Segunda confirmação"));
        // Com o título conhecido, vale digitá-lo (sem diferenciar caixa) ou a palavra.
        assert!(
            rodar_conf(Some("O Jogo da Vida"), true, false, "sim\no jogo da vida\n")
                .0
                .is_ok()
        );
        assert!(
            rodar_conf(Some("O Jogo da Vida"), true, false, "s\nHOMOLOGAR\n")
                .0
                .is_ok()
        );
        let (_, saida) = rodar_conf(Some("O Jogo da Vida"), true, false, "s\nx\n");
        assert!(saida.contains("O Jogo da Vida"));
    }

    #[test]
    fn confirmacao_recusa_resposta_errada_e_entrada_vazia() {
        for ent in [
            "",
            "n\nHOMOLOGAR\n",
            "s\n",
            "s\nhomologar\n",
            "s\n\n",
            "s\nOutro\n",
        ] {
            let r = rodar_conf(Some("Título"), true, false, ent).0;
            assert!(r.is_err(), "entrada {ent:?} deveria cancelar");
        }
        // Título vazio digitado não confere nem com título vazio configurado.
        assert!(rodar_conf(None, true, false, "s\n\n").0.is_err());
    }

    #[test]
    fn sem_terminal_recusa_a_menos_que_sim_eu_aprovo() {
        let (r, saida) = rodar_conf(None, false, false, "s\nHOMOLOGAR\n");
        let e = r.unwrap_err();
        assert!(
            e.contains("sem terminal") && e.contains("--sim-eu-aprovo"),
            "{e}"
        );
        assert!(saida.is_empty(), "nem pergunta sem terminal");
        let (r, saida) = rodar_conf(None, false, true, "");
        assert!(r.is_ok() && saida.is_empty());
    }

    fn hom(estado: &str) -> Resposta {
        Resposta::Biblioteca {
            dados: DadosBiblioteca::Homologacao {
                homologacao: HomologacaoInfo {
                    resumo: None,
                    id: 1,
                    cadastro_id: 12,
                    estado: estado.into(),
                    aprovado_por: None,
                    aprovado_em: None,
                    calibre_id: Some(345),
                    erro: None,
                },
            },
        }
    }

    fn homologada() -> BibliotecaInfo {
        BibliotecaInfo {
            id: 1,
            caminho: "/mnt/e/Livros".into(),
            papel: PapelRegistro::Homologada,
            nome: "Livros".into(),
            estado: "ativa".into(),
            versao_calibre: None,
            library_uuid: None,
            livros: 10,
            formatos: 12,
            ultima_sincronizacao: None,
            ultimo_resultado: None,
        }
    }

    /// Agente simulado: registra o que recebeu.
    fn simulado(vistos: &mut Vec<Comando>, c: Comando) -> Resposta {
        vistos.push(c.clone());
        match c {
            Comando::BibliotecaHomologar {
                aprovacao: None, ..
            } => hom("proposta"),
            Comando::BibliotecaHomologar {
                aprovacao: Some(_), ..
            } => hom("aplicada"),
            Comando::BibliotecaListar => Resposta::Biblioteca {
                dados: DadosBiblioteca::Bibliotecas {
                    itens: vec![homologada()],
                },
            },
            Comando::BibliotecaConfigObter => Resposta::Biblioteca {
                dados: DadosBiblioteca::Config {
                    config: ConfigBiblioteca::default(),
                },
            },
            o => panic!("{o:?}"),
        }
    }

    fn op(sim: bool) -> OpcoesHomologar {
        OpcoesHomologar {
            cadastro: 12,
            aprovado_por: "Chico".into(),
            motivo: Some("conferi".into()),
            titulo: Some("O Jogo".into()),
            sim_eu_aprovo: sim,
        }
    }

    fn aprovacoes(vistos: &[Comando]) -> usize {
        vistos
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    Comando::BibliotecaHomologar {
                        aprovacao: Some(_),
                        ..
                    }
                )
            })
            .count()
    }

    #[test]
    fn fluxo_mostra_o_plano_e_so_aprova_depois_das_duas_respostas() {
        let mut vistos = Vec::new();
        let mut ent = "s\nHOMOLOGAR\n".as_bytes();
        let mut sai = Vec::new();
        let r = fluxo_homologar(
            &op(false),
            &mut |c| simulado(&mut vistos, c),
            true,
            &mut ent,
            &mut sai,
        );
        let saida = String::from_utf8(sai).unwrap();
        assert!(
            saida.contains("ESCREVE na Biblioteca Homologada"),
            "{saida}"
        );
        assert!(
            saida.contains("/mnt/e/Livros") && saida.contains("Chico"),
            "{saida}"
        );
        assert!(saida.contains("livro 345 na A Classificar"), "{saida}");
        assert!(
            saida.contains("O Jogo") && saida.contains("conferi"),
            "{saida}"
        );
        assert_eq!(aprovacoes(&vistos), 1);
        // A proposta vem primeiro, sem aprovação; a aprovação leva quem e o motivo.
        assert!(matches!(
            vistos[0],
            Comando::BibliotecaHomologar {
                aprovacao: None,
                ..
            }
        ));
        match vistos.last().unwrap() {
            Comando::BibliotecaHomologar {
                item,
                aprovacao: Some(a),
            } => {
                assert_eq!(*item, 12);
                assert_eq!(a.quem, "Chico");
                assert_eq!(a.motivo.as_deref(), Some("conferi"));
            }
            o => panic!("{o:?}"),
        }
        assert!(matches!(
            r,
            Resposta::Biblioteca {
                dados: DadosBiblioteca::Homologacao { ref homologacao }
            } if homologacao.estado == "aplicada"
        ));
    }

    #[test]
    fn fluxo_cancelado_nao_envia_aprovacao() {
        for (tty, ent) in [
            (true, "n\n"),
            (true, "s\nnao\n"),
            (true, ""),
            (false, "s\nHOMOLOGAR\n"),
        ] {
            let mut vistos = Vec::new();
            let mut e = ent.as_bytes();
            let mut sai = Vec::new();
            let r = fluxo_homologar(
                &op(false),
                &mut |c| simulado(&mut vistos, c),
                tty,
                &mut e,
                &mut sai,
            );
            assert_eq!(aprovacoes(&vistos), 0, "entrada {ent:?}");
            match r {
                Resposta::Erro { codigo, .. } => assert_eq!(codigo, "cancelado"),
                o => panic!("{o:?}"),
            }
        }
    }

    #[test]
    fn fluxo_com_sim_eu_aprovo_aprova_sem_perguntar() {
        let mut vistos = Vec::new();
        let mut e = "".as_bytes();
        let mut sai = Vec::new();
        fluxo_homologar(
            &op(true),
            &mut |c| simulado(&mut vistos, c),
            false,
            &mut e,
            &mut sai,
        );
        assert_eq!(aprovacoes(&vistos), 1);
        // O plano continua sendo mostrado.
        assert!(String::from_utf8(sai).unwrap().contains("ESCREVE"));
    }

    #[test]
    fn fluxo_devolve_o_erro_da_proposta_e_nao_repete_aplicada() {
        let mut e = "".as_bytes();
        let mut sai = Vec::new();
        let r = fluxo_homologar(
            &op(true),
            &mut |_| Resposta::Erro {
                codigo: "cadastro_nao_pronto".into(),
                mensagem: "falta cadastrar".into(),
            },
            true,
            &mut e,
            &mut sai,
        );
        assert!(matches!(r, Resposta::Erro { codigo, .. } if codigo == "cadastro_nao_pronto"));
        let mut n = 0;
        let r = fluxo_homologar(
            &op(true),
            &mut |_| {
                n += 1;
                hom("aplicada")
            },
            true,
            &mut e,
            &mut sai,
        );
        assert_eq!(n, 1, "já aplicada: só a consulta inicial");
        assert!(matches!(r, Resposta::Biblioteca { .. }));
    }

    fn item_rev(id: i64, caminho: &str) -> dag_core::ItemRevisaoInfo {
        dag_core::ItemRevisaoInfo {
            membro: None,
            proposta_homologada: None,
            paginas: vec![],
            paginas_ocr: 0,
            file_id: id,
            caminho: Some(caminho.into()),
            tamanho: Some(1),
            hash: None,
            formato: caminho.rsplit('.').next().map(str::to_uppercase),
            pendentes: 1,
            campos: vec![],
            homologados_parecidos: vec![],
            obra_id: None,
            edicao_id: None,
            avisos: vec![],
        }
    }

    #[test]
    fn ouro_le_todas_as_paginas_da_revisao_e_grava_casos_novos() {
        let t = tempfile::tempdir().unwrap();
        let op = OpcoesOuro {
            n: 3,
            estratos: vec!["epub".into(), "pdf_textual".into()],
            saida: t.path().to_path_buf(),
            com_texto: false,
        };
        let mut pedidos = Vec::new();
        let r = fluxo_ouro_exportar(&op, &mut |c| {
            let Comando::BibliotecaRevisao { limite, cursor } = c else {
                panic!("{c:?}")
            };
            assert_eq!(limite, Some(PAGINA_OURO));
            pedidos.push(cursor.clone());
            let (itens, proximo) = match cursor.as_deref() {
                None => (
                    vec![item_rev(1, "/l/a.epub"), item_rev(2, "/l/b.epub")],
                    Some("2".to_string()),
                ),
                Some(_) => (vec![item_rev(3, "/l/c.pdf")], None),
            };
            Resposta::Biblioteca {
                dados: DadosBiblioteca::Revisao {
                    itens,
                    total_pendentes: 3,
                    proximo_cursor: proximo,
                },
            }
        });
        assert_eq!(pedidos, vec![None, Some("2".to_string())]);
        let Resposta::Ok { mensagem: Some(m) } = r else {
            panic!("{r:?}")
        };
        assert!(m.contains("3 caso(s) novo(s)"), "{m}");
        for id in [1, 2, 3] {
            assert!(t.path().join(format!("ouro-{id:06}.json")).is_file());
        }
        // Erro do agente passa direto; estrato inválido é argumento inválido.
        let e = fluxo_ouro_exportar(&op, &mut |_| Resposta::Erro {
            codigo: "x".into(),
            mensagem: "y".into(),
        });
        assert!(matches!(e, Resposta::Erro { codigo, .. } if codigo == "x"));
        let ruim = OpcoesOuro {
            estratos: vec!["poesia".into()],
            ..op
        };
        let e = fluxo_ouro_exportar(&ruim, &mut |_| unreachable!());
        assert!(matches!(e, Resposta::Erro { codigo, .. } if codigo == "argumento_invalido"));
    }
}
