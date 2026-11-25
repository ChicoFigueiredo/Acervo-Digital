//! Montagem pura dos comandos `Comando::Biblioteca*` e das decisões de revisão. Nada aqui fala
//! com o agente: cada função devolve o comando (ou o plano) que a página envia, para ser
//! testado sem janela.

use std::collections::BTreeSet;

use dag_core::{
    AcaoRevisao, AlvoRevisao, AprovacaoHumana, Comando, ConfigBiblioteca, ItemDecisao,
    ItemRevisaoInfo, MotivoFila, PapelRegistro,
};
use serde_json::{Map, Value};

/// Itens por página das listas paginadas por cursor.
pub const POR_PAGINA: u32 = 100;
/// Itens da revisão por página (cada um traz os campos lado a lado).
pub const POR_PAGINA_REVISAO: u32 = 25;

#[must_use]
pub fn listar() -> Comando {
    Comando::BibliotecaListar
}

#[must_use]
pub fn registrar(caminho: &str, papel: PapelRegistro, nome: &str) -> Comando {
    let nome = nome.trim();
    Comando::BibliotecaRegistrar {
        caminho: caminho.trim().to_string(),
        papel,
        nome: (!nome.is_empty()).then(|| nome.to_string()),
    }
}

#[must_use]
pub fn perfil(biblioteca: i64) -> Comando {
    Comando::BibliotecaPerfil {
        biblioteca: biblioteca.to_string(),
    }
}

#[must_use]
pub fn sincronizar(biblioteca: i64) -> Comando {
    Comando::BibliotecaSincronizar {
        biblioteca: biblioteca.to_string(),
    }
}

#[must_use]
pub fn duplicatas(cursor: Option<String>) -> Comando {
    Comando::BibliotecaDuplicatas {
        raiz: None,
        limite: Some(POR_PAGINA),
        cursor,
    }
}

#[must_use]
pub fn enfileirar(raizes: Vec<String>, motivo: MotivoFila) -> Comando {
    Comando::BibliotecaEnfileirar {
        raizes,
        motivo: Some(motivo),
    }
}

#[must_use]
pub fn fila(estado: Option<dag_core::EstadoFilaInfo>, cursor: Option<String>) -> Comando {
    Comando::BibliotecaFila {
        estado,
        limite: Some(POR_PAGINA),
        cursor,
    }
}

/// `usar_ia` só vale com a IA ligada na configuração: nunca consulta a IA contra a configuração.
#[must_use]
pub fn processar(usar_ia: bool, ia_ligada: bool) -> Comando {
    Comando::BibliotecaProcessar {
        limite: None,
        ia: usar_ia && ia_ligada,
    }
}

#[must_use]
pub fn revisao(cursor: Option<String>) -> Comando {
    Comando::BibliotecaRevisao {
        limite: Some(POR_PAGINA_REVISAO),
        cursor,
    }
}

#[must_use]
pub fn decidir(itens: Vec<ItemDecisao>) -> Comando {
    Comando::BibliotecaDecidir { itens, lote: None }
}

#[must_use]
pub fn desfazer(lote: &str) -> Comando {
    Comando::BibliotecaDesfazer {
        lote: lote.to_string(),
    }
}

#[must_use]
pub fn cadastrar(itens: Vec<i64>) -> Comando {
    Comando::BibliotecaCadastrar { itens }
}

/// Homologação com a aprovação de quem está na frente da GUI (o MCP nunca pode fornecê-la).
#[must_use]
pub fn homologar(cadastro: i64, quem: &str) -> Comando {
    Comando::BibliotecaHomologar {
        item: cadastro,
        aprovacao: Some(AprovacaoHumana {
            quem: quem.trim().to_string(),
            quando: None,
            motivo: Some("aprovada na GUI: o título foi digitado para confirmar".into()),
        }),
    }
}

#[must_use]
pub fn alertas(nao_vistos: bool, cursor: Option<String>) -> Comando {
    Comando::BibliotecaAlertas {
        nao_vistos,
        limite: Some(POR_PAGINA),
        cursor,
    }
}

#[must_use]
pub fn marcar_vistos(ids: Vec<i64>) -> Comando {
    Comando::BibliotecaMarcarVisto { ids }
}

#[must_use]
pub fn resumo(desde: Option<i64>) -> Comando {
    Comando::BibliotecaResumo { desde }
}

#[must_use]
pub fn relatorio_espaco() -> Comando {
    Comando::BibliotecaRelatorioEspaco { volume: None }
}

#[must_use]
pub fn config_obter() -> Comando {
    Comando::BibliotecaConfigObter
}

#[must_use]
pub fn config_definir(alteracoes: Value) -> Comando {
    Comando::BibliotecaConfigDefinir { alteracoes }
}

// ---------------------------------------------------------------------------------------------
// Decisões da revisão (B-12)
// ---------------------------------------------------------------------------------------------

/// Campo de um item (arquivo) em revisão.
pub type ChaveCampo = (i64, String);

fn decisao(file_id: i64, campo: &str, acao: AcaoRevisao, valor: Option<String>) -> ItemDecisao {
    ItemDecisao {
        alvo: AlvoRevisao::Arquivo(file_id),
        campo: campo.to_string(),
        acao,
        valor,
    }
}

#[must_use]
pub fn aceitar(file_id: i64, campo: &str) -> ItemDecisao {
    decisao(file_id, campo, AcaoRevisao::Aceitar, None)
}

#[must_use]
pub fn rejeitar(file_id: i64, campo: &str) -> ItemDecisao {
    decisao(file_id, campo, AcaoRevisao::Rejeitar, None)
}

/// Correção por campo; o valor vazio não é uma correção (rejeite o campo em vez disso).
#[must_use]
pub fn corrigir(file_id: i64, campo: &str, valor: &str) -> Option<ItemDecisao> {
    let v = valor.trim();
    (!v.is_empty()).then(|| decisao(file_id, campo, AcaoRevisao::Corrigir, Some(v.to_string())))
}

/// O campo ainda espera decisão?
#[must_use]
pub fn campo_pendente(estado: &str) -> bool {
    estado == "pendente"
}

/// Decisões em lote dos campos escolhidos (só os que ainda estão pendentes), na ordem da lista.
#[must_use]
pub fn lote_dos_escolhidos(
    itens: &[ItemRevisaoInfo],
    escolhidos: &BTreeSet<ChaveCampo>,
    acao: AcaoRevisao,
) -> Vec<ItemDecisao> {
    let mut v = Vec::new();
    for it in itens {
        for c in &it.campos {
            if campo_pendente(&c.estado) && escolhidos.contains(&(it.file_id, c.campo.clone())) {
                v.push(decisao(it.file_id, &c.campo, acao, None));
            }
        }
    }
    v
}

/// Aceita todos os campos pendentes de confiança alta (e que não pedem revisão) dos itens dados.
#[must_use]
pub fn aceitar_confianca_alta(itens: &[ItemRevisaoInfo]) -> Vec<ItemDecisao> {
    let mut v = Vec::new();
    for it in itens {
        for c in &it.campos {
            let alta = super::texto::faixa_de_confianca(c.faixa.as_deref(), c.confianca) == "alta";
            if campo_pendente(&c.estado) && alta && !c.precisa_revisao && c.sugerido.is_some() {
                v.push(aceitar(it.file_id, &c.campo));
            }
        }
    }
    v
}

/// Todos os campos pendentes de um item.
#[must_use]
pub fn pendentes_do_item(item: &ItemRevisaoInfo, acao: AcaoRevisao) -> Vec<ItemDecisao> {
    item.campos
        .iter()
        .filter(|c| campo_pendente(&c.estado))
        .map(|c| decisao(item.file_id, &c.campo, acao, None))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Configuração: merge-patch mínimo
// ---------------------------------------------------------------------------------------------

/// Merge-patch (RFC 7386) que leva `antes` a `depois`, restrito às chaves de topo `chaves`
/// (`[]` = todas). Objetos aninhados só levam o que mudou; um valor que some vira `null`.
#[must_use]
pub fn patch_de_config(
    antes: &ConfigBiblioteca,
    depois: &ConfigBiblioteca,
    chaves: &[&str],
) -> Value {
    let a = serde_json::to_value(antes).unwrap_or(Value::Null);
    let d = serde_json::to_value(depois).unwrap_or(Value::Null);
    let Value::Object(mut p) = diferenca(&a, &d) else {
        return Value::Object(Map::new());
    };
    if !chaves.is_empty() {
        p.retain(|k, _| chaves.contains(&k.as_str()));
    }
    Value::Object(p)
}

fn diferenca(antes: &Value, depois: &Value) -> Value {
    match (antes, depois) {
        (Value::Object(a), Value::Object(d)) => {
            let mut p = Map::new();
            for (k, v) in d {
                match a.get(k) {
                    Some(x) if x == v => {}
                    Some(x) if x.is_object() && v.is_object() => {
                        p.insert(k.clone(), diferenca(x, v));
                    }
                    _ => {
                        p.insert(k.clone(), v.clone());
                    }
                }
            }
            for k in a.keys().filter(|k| !d.contains_key(*k)) {
                p.insert(k.clone(), Value::Null);
            }
            Value::Object(p)
        }
        (_, d) => d.clone(),
    }
}

/// Um patch sem nenhuma mudança.
#[must_use]
pub fn patch_vazio(p: &Value) -> bool {
    p.as_object().is_none_or(Map::is_empty)
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_core::CampoLadoALado;

    fn campo(nome: &str, estado: &str, conf: f32, faixa: &str, rev: bool) -> CampoLadoALado {
        CampoLadoALado {
            campo: nome.into(),
            extraido: Some("ext".into()),
            sugerido: Some("sug".into()),
            sugerido_lista: Vec::new(),
            confianca: Some(conf),
            faixa: Some(faixa.into()),
            origem: None,
            modelo: None,
            evidencias: Vec::new(),
            precisa_revisao: rev,
            motivos: Vec::new(),
            homologado: None,
            estado: estado.into(),
            sugestao_id: Some(1),
        }
    }

    fn item(id: i64, campos: Vec<CampoLadoALado>) -> ItemRevisaoInfo {
        ItemRevisaoInfo {
            membro: None,
            proposta_homologada: None,
            paginas: vec![],
            paginas_ocr: 0,
            file_id: id,
            caminho: Some(format!("/l/{id}.pdf")),
            tamanho: Some(10),
            hash: None,
            formato: Some("PDF".into()),
            pendentes: campos.len() as i64,
            campos,
            homologados_parecidos: Vec::new(),
            obra_id: None,
            edicao_id: None,
            avisos: Vec::new(),
        }
    }

    #[test]
    fn comandos_simples_tem_a_forma_do_protocolo() {
        assert!(matches!(listar(), Comando::BibliotecaListar));
        match registrar("  /lib ", PapelRegistro::AClassificar, " ") {
            Comando::BibliotecaRegistrar {
                caminho,
                papel,
                nome,
            } => {
                assert_eq!(caminho, "/lib");
                assert_eq!(papel, PapelRegistro::AClassificar);
                assert_eq!(nome, None);
            }
            o => panic!("{o:?}"),
        }
        match perfil(3) {
            Comando::BibliotecaPerfil { biblioteca } => assert_eq!(biblioteca, "3"),
            o => panic!("{o:?}"),
        }
        match duplicatas(Some("c1".into())) {
            Comando::BibliotecaDuplicatas {
                raiz: None,
                limite: Some(POR_PAGINA),
                cursor: Some(c),
            } => assert_eq!(c, "c1"),
            o => panic!("{o:?}"),
        }
        match revisao(None) {
            Comando::BibliotecaRevisao {
                limite: Some(POR_PAGINA_REVISAO),
                cursor: None,
            } => {}
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn processar_com_ia_so_se_a_configuracao_liga() {
        assert!(matches!(
            processar(true, true),
            Comando::BibliotecaProcessar { ia: true, .. }
        ));
        assert!(matches!(
            processar(true, false),
            Comando::BibliotecaProcessar { ia: false, .. }
        ));
        assert!(matches!(
            processar(false, true),
            Comando::BibliotecaProcessar { ia: false, .. }
        ));
    }

    #[test]
    fn homologar_leva_a_aprovacao_de_quem_aprova() {
        match homologar(7, "  Chico ") {
            Comando::BibliotecaHomologar {
                item: 7,
                aprovacao: Some(a),
            } => {
                assert_eq!(a.quem, "Chico");
                assert!(a.quando.is_none());
                assert!(a.motivo.is_some());
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn corrigir_exige_valor_e_aparda() {
        assert!(corrigir(1, "titulo", "   ").is_none());
        let d = corrigir(1, "autores", " A|B ").unwrap();
        assert_eq!(d.acao, AcaoRevisao::Corrigir);
        assert_eq!(d.valor.as_deref(), Some("A|B"));
        assert_eq!(d.alvo, AlvoRevisao::Arquivo(1));
        assert_eq!(aceitar(2, "tags").valor, None);
        assert_eq!(rejeitar(2, "tags").acao, AcaoRevisao::Rejeitar);
    }

    #[test]
    fn lote_so_leva_o_pendente_escolhido_na_ordem() {
        let itens = vec![
            item(
                1,
                vec![
                    campo("titulo", "pendente", 0.95, "alta", false),
                    campo("autores", "aceita", 0.95, "alta", false),
                    campo("tags", "pendente", 0.5, "baixa", true),
                ],
            ),
            item(2, vec![campo("titulo", "pendente", 0.7, "media", true)]),
        ];
        let mut esc = BTreeSet::new();
        esc.insert((1, "titulo".to_string()));
        esc.insert((1, "autores".to_string()));
        esc.insert((2, "titulo".to_string()));
        let l = lote_dos_escolhidos(&itens, &esc, AcaoRevisao::Rejeitar);
        let v: Vec<_> = l.iter().map(|d| (d.alvo, d.campo.as_str())).collect();
        assert_eq!(
            v,
            [
                (AlvoRevisao::Arquivo(1), "titulo"),
                (AlvoRevisao::Arquivo(2), "titulo")
            ]
        );
        assert!(l.iter().all(|d| d.acao == AcaoRevisao::Rejeitar));
    }

    #[test]
    fn aceitar_alta_ignora_media_baixa_e_o_que_pede_revisao() {
        let itens = vec![item(
            1,
            vec![
                campo("titulo", "pendente", 0.95, "alta", false),
                campo("autores", "pendente", 0.95, "alta", true),
                campo("tags", "pendente", 0.7, "media", false),
                campo("idioma", "rejeitada", 0.99, "alta", false),
            ],
        )];
        let l = aceitar_confianca_alta(&itens);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].campo, "titulo");
        let todos = pendentes_do_item(&itens[0], AcaoRevisao::Aceitar);
        assert_eq!(todos.len(), 3);
    }

    #[test]
    fn patch_de_config_leva_so_o_que_mudou_e_so_as_chaves_pedidas() {
        let base = ConfigBiblioteca::default();
        let mut novo = base.clone();
        novo.ia.ativa = true;
        novo.ia.modelo_secundario = Some("qwen".into());
        novo.debounce_s = 60;
        let p = patch_de_config(&base, &novo, &["ia"]);
        assert_eq!(
            p,
            serde_json::json!({"ia": {"ativa": true, "modelo_secundario": "qwen"}})
        );
        let tudo = patch_de_config(&base, &novo, &[]);
        assert_eq!(tudo["debounce_s"], 60);
        assert!(patch_vazio(&patch_de_config(&base, &base, &[])));
        // Apagar o modelo secundário vira null (merge-patch).
        let mut sem = novo.clone();
        sem.ia.modelo_secundario = None;
        let p = patch_de_config(&novo, &sem, &["ia"]);
        assert_eq!(p, serde_json::json!({"ia": {"modelo_secundario": null}}));
    }

    #[test]
    fn patch_aplicado_pelo_agente_reproduz_a_edicao() {
        // Mesma regra do agente: objetos mesclam, o resto substitui; null vira None.
        fn mesclar(base: &mut Value, patch: &Value) {
            match (base, patch) {
                (Value::Object(b), Value::Object(p)) => {
                    for (k, v) in p {
                        match b.get_mut(k) {
                            Some(x) if v.is_object() => mesclar(x, v),
                            _ => {
                                b.insert(k.clone(), v.clone());
                            }
                        }
                    }
                }
                (b, p) => *b = p.clone(),
            }
        }
        let base = ConfigBiblioteca::default();
        let mut novo = base.clone();
        novo.ocr.idiomas = vec!["por".into()];
        novo.ia.modelo_secundario = Some("x".into());
        novo.homologacao.url = Some("http://127.0.0.1:17790".into());
        novo.pastas_a_classificar.push("novos".into());
        let p = patch_de_config(&base, &novo, &[]);
        let mut v = serde_json::to_value(&base).unwrap();
        mesclar(&mut v, &p);
        let volta: ConfigBiblioteca = serde_json::from_value(v).unwrap();
        assert_eq!(volta, novo);
    }
}
