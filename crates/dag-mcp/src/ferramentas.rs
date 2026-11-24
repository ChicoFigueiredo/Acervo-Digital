//! Catálogo de ferramentas MCP: nome, descrição, JSON Schema e tradução para [`Comando`].

use dag_core::Comando;
use serde_json::{Map, Value, json};

const SEGURO: &str = "Somente leitura sobre o acervo: nenhum arquivo é apagado, movido ou \
alterado no disco; marcações só mudam o Catálogo.";
const ELEVACAO: &str = "Pode abrir um diálogo do sistema (Polkit) em que o usuário digita a \
senha; o MCP nunca recebe senhas.";

/// Uma ferramenta exposta em `tools/list`.
#[derive(Debug, Clone)]
pub struct Ferramenta {
    pub nome: &'static str,
    pub descricao: String,
    pub schema: Value,
    /// Valor do campo `comando` serializado de [`Comando`].
    tag: &'static str,
}

fn alvo_schema(desc: &str) -> Value {
    json!({
        "type": "object",
        "description": desc,
        "properties": {
            "tipo": {"type": "string", "enum": ["caminho", "arquivo", "grupo", "volume", "pasta"],
                "description": "caminho: arquivo pelo caminho catalogado; arquivo: id numérico; grupo: id do Grupo de Duplicados; volume: id, rótulo ou ponto de montagem; pasta: caminho de pasta catalogada"},
            "valor": {"description": "String (caminho, volume, pasta) ou inteiro (id de arquivo ou de grupo)",
                "type": ["string", "integer"]}
        },
        "required": ["tipo", "valor"],
        "additionalProperties": false
    })
}

fn objeto(props: Value, obrigatorios: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": props,
        "required": obrigatorios,
        "additionalProperties": false
    })
}

fn execucao() -> Value {
    json!({"execucao": {"type": "integer", "description": "Id da execução (padrão: a única em andamento)"}})
}

fn opcoes_hash() -> Value {
    json!({
        "threads_por_disco": {"type": "integer", "minimum": 1, "maximum": 8,
            "description": "Leitores simultâneos por disco no Hash Completo (1..8). Vence a flag do agente e o valor persistido; padrão 1 (HDD: 1 é o melhor; SSD/NVMe podem ganhar com 2 a 4)"},
        "max_discos": {"type": "integer", "minimum": 1, "maximum": 16,
            "description": "Discos lendo ao mesmo tempo no Hash Completo (1..16; padrão 16)"}
    })
}

fn com_hash(props: Value) -> Value {
    let mut m = props.as_object().cloned().unwrap_or_default();
    m.append(&mut opcoes_hash().as_object().cloned().unwrap_or_default());
    Value::Object(m)
}

fn raizes(desc: &str) -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": desc})
}

fn f(nome: &'static str, tag: &'static str, desc: &str, schema: Value) -> Ferramenta {
    Ferramenta {
        nome,
        descricao: desc.to_string(),
        schema,
        tag,
    }
}

/// Todas as ferramentas, na ordem apresentada ao cliente.
#[allow(clippy::vec_init_then_push)]
pub fn todas() -> Vec<Ferramenta> {
    let pag = json!({
        "limite": {"type": "integer", "minimum": 1, "description": "Máximo de itens da página"},
        "cursor": {"type": "string", "description": "proximo_cursor da página anterior"}
    });
    let mut pag_props = pag.as_object().cloned().unwrap_or_default();
    let mut v = Vec::new();
    v.push(f(
        "listar_volumes",
        "listar_volumes",
        &format!("Lista os Volumes conhecidos (conectados ou não). {SEGURO}"),
        objeto(json!({}), &[]),
    ));
    v.push(f(
        "scan_full",
        "iniciar_scan",
        &format!(
            "Full Scan: inventário, Hash Completo, Grupos de Duplicados e reavaliação das Marcações \
             sobre as raízes dadas. Pode demorar. {SEGURO}"
        ),
        objeto(
            com_hash(json!({"raizes": raizes("Pastas a varrer (caminhos absolutos)"),
                "sem_hash": {"type": "boolean", "description": "Pula hash, duplicados e reavaliação (inventário rápido)"},
                "releitura_total": {"type": "boolean", "description": "Lê o conteúdo de todos os arquivos de novo (Full Scan com hash de tudo), em vez de reaproveitar o hash dos que não mudaram"}})),
            &["raizes"],
        ),
    ));
    v.push(f(
        "scan_fast",
        "iniciar_scan",
        &format!(
            "Fast Scan: reconcilia o Catálogo reaproveitando o que não mudou. Sem raízes, vale todas \
             as raízes já catalogadas cujo Volume está conectado. {SEGURO}"
        ),
        objeto(
            com_hash(json!({"raizes": raizes("Pastas a reconciliar (opcional)"),
                "sem_hash": {"type": "boolean", "description": "Pula hash, duplicados e reavaliação"}})),
            &[],
        ),
    ));
    v.push(f(
        "retomar",
        "retomar",
        "Retoma uma execução de varredura pausada, cancelada ou interrompida. Para execução gravada no Catálogo, `threads_por_disco`/`max_discos` valem a partir desta retomada.",
        objeto(com_hash(execucao()), &[]),
    ));
    v.push(f(
        "pausar",
        "pausar",
        "Pausa uma varredura em andamento.",
        objeto(execucao(), &[]),
    ));
    v.push(f(
        "cancelar",
        "cancelar",
        "Cancela uma varredura (fica retomável). Não toca nos arquivos do acervo.",
        objeto(execucao(), &[]),
    ));
    v.push(f(
        "status",
        "status",
        "Progresso das varreduras, totais do Catálogo e saúde do monitor.",
        objeto(execucao(), &[]),
    ));
    let mut ex =
        json!({"pasta": {"type": "string", "description": "Pasta catalogada (padrão: as Raízes)"}})
            .as_object()
            .cloned()
            .unwrap_or_default();
    ex.append(&mut pag_props.clone());
    v.push(f(
        "explorar",
        "explorar",
        &format!(
            "Lista os filhos diretos de uma pasta catalogada, do maior para o menor. {SEGURO}"
        ),
        objeto(Value::Object(ex), &[]),
    ));
    let mut dup = json!({
        "volume": {"type": "string", "description": "Só grupos com cópia neste Volume (id, rótulo ou ponto de montagem)"},
        "tamanho_minimo": {"type": "integer", "minimum": 0, "description": "Tamanho mínimo do arquivo, em bytes"},
        "ordem": {"type": "string", "enum": ["recuperavel", "tamanho"], "description": "Padrão: maior espaço recuperável primeiro"}
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    dup.append(&mut pag_props.clone());
    v.push(f(
        "listar_duplicados",
        "listar_duplicados",
        &format!("Lista Grupos de Duplicados (paginado por cursor). {SEGURO}"),
        objeto(Value::Object(dup), &[]),
    ));
    v.push(f(
        "detalhar_grupo",
        "detalhar_grupo",
        "Um Grupo de Duplicados com todas as cópias, Marcações e Cobertura.",
        objeto(
            json!({"grupo": {"type": "integer", "description": "Id do grupo"}}),
            &["grupo"],
        ),
    ));
    v.push(f(
        "definir_copia_principal",
        "definir_copia_principal",
        &format!(
            "Troca a Cópia Principal de um grupo (sem `arquivo`, volta à escolha automática pela \
             Prioridade de Volume). Só muda o Catálogo. {SEGURO}"
        ),
        objeto(
            json!({"grupo": {"type": "integer", "description": "Id do grupo"},
                "arquivo": alvo_schema("Arquivo (tipo caminho ou arquivo) que vira a Cópia Principal")}),
            &["grupo"],
        ),
    ));
    v.push(f(
        "marcar_backup",
        "marcar_backup",
        &format!("Cria uma Marcação de Backup (a cópia é intencional e deve ser preservada). Só muda o Catálogo. {SEGURO}"),
        objeto(json!({"alvo": alvo_schema("O que marcar")}), &["alvo"]),
    ));
    v.push(f(
        "marcar_delecao",
        "marcar_delecao",
        &format!(
            "Cria uma Marcação para Deleção: apenas uma candidata a descarte registrada no Catálogo. \
             NADA é apagado nem movido no disco. {SEGURO}"
        ),
        objeto(json!({"alvo": alvo_schema("O que marcar")}), &["alvo"]),
    ));
    v.push(f(
        "desmarcar",
        "desmarcar",
        &format!("Remove a Marcação do alvo (arquivo, grupo ou regra de Volume/pasta). {SEGURO}"),
        objeto(
            json!({"alvo": alvo_schema("Alvo cuja Marcação será removida")}),
            &["alvo"],
        ),
    ));
    v.push(f(
        "confirmar_marcacao",
        "confirmar_marcacao",
        "Confirma de novo uma Marcação Pendente (conteúdo mudou depois de marcada), adotando o Hash Completo atual.",
        objeto(
            json!({"marcacao": {"type": "integer", "description": "Id da Marcação (veja o relatório de marcações)"}}),
            &["marcacao"],
        ),
    ));
    v.push(f(
        "definir_prioridade",
        "definir_prioridade_volume",
        "Define a Prioridade de Volume, do mais ao menos prioritário (decide a Cópia Principal automática).",
        objeto(
            json!({"ordem": {"type": "array", "items": {"type": "string"}, "minItems": 1,
                "description": "Volumes (id, rótulo, ponto de montagem) ou pastas catalogadas"}}),
            &["ordem"],
        ),
    ));
    let mut rel = json!({
        "tipo": {"type": "string", "enum": ["cobertura", "sem_cobertura", "espaco", "marcacoes"]},
        "nivel": {"type": "string", "enum": ["computador", "volume", "pasta", "grupo"], "description": "Só para o tipo espaco"},
        "saida": {"type": "string", "description": "Caminho .csv ou .json para gravar o relatório inteiro (não sobrescreve arquivo existente). Sem isto, devolve uma página"}
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    rel.append(&mut pag_props);
    v.push(f(
        "relatorio",
        "relatorio",
        &format!(
            "Relatório de Cobertura, arquivos sem Cobertura, espaço recuperável ou Marcações. Com `saida`, \
             grava um arquivo novo no caminho indicado. {SEGURO}"
        ),
        objeto(Value::Object(rel), &["tipo"]),
    ));
    v.push(f(
        "verificar_integridade",
        "verificar_integridade",
        &format!("Verificação de Integridade (relê e confere hashes) sobre as raízes (todas, se omitido). {SEGURO}"),
        objeto(com_hash(json!({"raizes": raizes("Raízes a verificar")})), &[]),
    ));
    v.push(f(
        "config_hash_obter",
        "config_hash",
        "Paralelismo padrão do Hash Completo persistido no Catálogo (threads por disco e discos em paralelo). Somente leitura.",
        objeto(json!({}), &[]),
    ));
    v.push(f(
        "config_hash_definir",
        "config_hash",
        "Define o paralelismo padrão do Hash Completo, persistido no Catálogo (threads_por_disco 1..8, padrão 1; max_discos 1..16, padrão 16). Vale a partir da próxima execução ou retomada; a que está rodando mantém o valor com que começou. Só muda o Catálogo.",
        objeto(opcoes_hash(), &[]),
    ));
    v.push(f(
        "eventos",
        "eventos",
        "Últimos eventos persistidos (criado, alterado, removido, renomeado, lacunas...).",
        objeto(
            json!({"limite": {"type": "integer", "minimum": 1, "description": "Máximo de eventos (padrão 50)"}}),
            &[],
        ),
    ));
    v.push(f(
        "saude_monitor",
        "saude_monitor",
        "Saúde do monitor e pendências de reconciliação.",
        objeto(json!({}), &[]),
    ));
    v.push(f(
        "servico",
        "servico",
        &format!(
            "Gerencia o serviço em segundo plano: instalar, remover, iniciar, parar ou autostart \
             (liga/desliga a inicialização automática). {ELEVACAO} Nenhum arquivo do acervo é tocado."
        ),
        objeto(
            json!({"acao": {"type": "string", "enum": ["instalar", "remover", "iniciar", "parar", "autostart"]},
                "ativo": {"type": "boolean", "description": "Só para autostart: true liga, false desliga"}}),
            &["acao"],
        ),
    ));
    v.push(f(
        "backup_catalogo",
        "backup_catalogo",
        "Backup consistente do Catálogo (SQLite) para o destino indicado. Não toca no acervo.",
        objeto(
            json!({"destino": {"type": "string", "description": "Caminho do arquivo de backup"}}),
            &["destino"],
        ),
    ));
    v
}

/// Traduz os argumentos de uma chamada no [`Comando`] correspondente.
pub fn montar(nome: &str, args: &Value) -> Result<Comando, String> {
    let ferr = todas()
        .into_iter()
        .find(|t| t.nome == nome)
        .ok_or_else(|| format!("ferramenta desconhecida: {nome}"))?;
    let mut obj: Map<String, Value> = match args {
        Value::Null => Map::new(),
        Value::Object(m) => m.clone(),
        _ => return Err("`arguments` deve ser um objeto".into()),
    };
    // Rejeita propriedades fora do schema (additionalProperties: false).
    let props = ferr
        .schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(k) = obj.keys().find(|k| !props.contains_key(*k)) {
        return Err(format!("argumento desconhecido: `{k}`"));
    }
    obj.retain(|_, v| !v.is_null());
    match nome {
        "scan_full" => {
            if !obj.contains_key("raizes") {
                return Err("faltou o argumento `raizes`".into());
            }
            obj.insert("modo".into(), json!("full_scan"));
        }
        "scan_fast" => {
            obj.insert("modo".into(), json!("fast_scan"));
            obj.entry("raizes").or_insert_with(|| json!([]));
        }
        "servico" => {
            let acao = obj
                .get("acao")
                .and_then(Value::as_str)
                .ok_or("faltou o argumento `acao`")?
                .to_string();
            let a = match acao.as_str() {
                "instalar" | "remover" | "iniciar" | "parar" => json!(acao),
                "autostart" => {
                    let ativo = obj
                        .get("ativo")
                        .and_then(Value::as_bool)
                        .ok_or("autostart exige `ativo` (booleano)")?;
                    json!({"autostart": {"ativo": ativo}})
                }
                o => return Err(format!("ação de serviço inválida: `{o}`")),
            };
            obj.clear();
            obj.insert("acao".into(), a);
        }
        "verificar_integridade" => {
            obj.entry("raizes").or_insert_with(|| json!([]));
        }
        "config_hash_definir" if obj.is_empty() => {
            return Err("informe `threads_por_disco` e/ou `max_discos`".into());
        }
        _ => {}
    }
    obj.insert("comando".into(), json!(ferr.tag));
    serde_json::from_value(Value::Object(obj)).map_err(|e| format!("argumentos inválidos: {e}"))
}
