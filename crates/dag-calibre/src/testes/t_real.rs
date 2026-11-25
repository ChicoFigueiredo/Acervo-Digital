#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Testes com o `calibredb` real. Pulam (com aviso) se ele não estiver instalado.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use super::fixtures::{calibre, construir_real};
use crate::calibredb::{
    Alvo, COLUNA_DAG_CONFIANCA, COLUNA_DAG_HASH, CamposMetadados, ClienteCalibredb, NovoLivro,
};
use crate::erro::ErroCalibre;
use crate::executor::ExecutorProcesso;
use crate::homologacao::{
    AprovacaoHomologacao, OpcoesHomologacao, homologar, reverter_homologacao,
};
use crate::instantaneo::{Escopo, comparar, hash_arquivo, tirar};
use crate::leitura::BibliotecaLeitura;
use crate::perfil::{OrigemEstante, ler_perfil};

fn normaliza(v: &[(String, Value)]) -> BTreeMap<String, Value> {
    v.iter()
        .map(|(k, v)| {
            let v = match v {
                Value::Array(a) => {
                    let mut a = a.clone();
                    a.sort_by_key(ToString::to_string);
                    Value::Array(a)
                }
                o => o.clone(),
            };
            (k.clone(), v)
        })
        .collect()
}

fn agora() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn sem_dag(ids: &[(String, String)]) -> Vec<(String, String)> {
    ids.iter()
        .filter(|(t, _)| !t.starts_with("dag"))
        .cloned()
        .collect()
}

#[test]
fn real_leitura_perfil_clone_round_trip_idempotencia_e_recusas() {
    let Some((_g, cli)) = calibre() else { return };
    let t = tempfile::tempdir().unwrap();
    let h = t.path().join("Homologada");
    let arquivos = t.path().join("arquivos");
    let exec = super::fixtures::executor_real().unwrap();
    construir_real(&exec, &h, &arquivos);
    let versao = cli.versao().unwrap();
    eprintln!("calibredb {}", versao.texto);

    // ---- CA-B01: ler tudo não muda nada ----
    let antes = tirar(&h, Escopo::Completo, None).unwrap();
    assert!(antes.arquivos.contains_key("metadata.db"));
    let bib = BibliotecaLeitura::abrir(&h).unwrap();
    let livros: Vec<_> = bib.livros(2).map(Result::unwrap).collect();
    let perfil = ler_perfil(&h).unwrap();
    drop(bib);
    assert_eq!(
        comparar(&antes, &tirar(&h, Escopo::Completo, Some(&antes)).unwrap()),
        vec![]
    );

    // ---- leitura completa (CA-B21: Unicode, multi-autor, série, colunas) ----
    assert_eq!(livros.len(), 3);
    let l = &livros[0];
    assert_eq!(l.titulo, "Cálculo — Vol. 1: 微分");
    assert_eq!(l.autores, ["José Álvaro Pereira", "Maria Çelik"]);
    assert_eq!(l.serie.as_deref(), Some("Série Única"));
    assert_eq!(l.indice_serie, Some(2.0));
    assert_eq!(l.editora.as_deref(), Some("Editora São Jorge"));
    assert_eq!(l.idiomas, ["por", "eng"]);
    assert_eq!(l.avaliacao, Some(8));
    assert_eq!(
        l.data_publicacao.as_deref(),
        Some("2020-05-06T00:00:00+00:00")
    );
    assert!(l.tags.contains(&"!Catalogar".to_owned()) && l.tags.len() == 3);
    assert_eq!(l.formatos.len(), 2);
    for f in &l.formatos {
        assert!(
            Path::new(&f.caminho_absoluto).is_file(),
            "{}",
            f.caminho_absoluto
        );
        assert_eq!(
            f.tamanho,
            std::fs::metadata(&f.caminho_absoluto).unwrap().len()
        );
    }
    let p: BTreeMap<_, _> = l.personalizados.iter().cloned().collect();
    assert_eq!(p["pasta"], json!("Matemática"));
    assert_eq!(p["tagsx"], json!(["um", "dois"]));
    assert_eq!(p["serie2"], json!({"serie":"Coleção Azul","indice":2.5}));
    assert_eq!(p["enu"], json!("b"));
    assert_eq!(p["inteiro"], json!(300));
    assert_eq!(p["flt"], json!(1.5));
    assert_eq!(p["flag"], json!(true));
    assert_eq!(p["nota"], json!(6));
    assert_eq!(p["quando"], json!("2021-03-04T00:00:00+00:00"));
    assert_eq!(p["texto"], json!("<p>oi</p>"));
    assert!(!p.contains_key("calc"));
    assert_eq!(livros[2].tags, Vec::<String>::new());

    // ---- perfil ----
    assert_eq!(perfil.colunas.len(), 11);
    assert_eq!(perfil.bibliotecas_virtuais.len(), 3);
    assert_eq!(perfil.buscas_salvas[0].nome, "so-xadrez");
    assert_eq!(perfil.vocabulario.tags_controle[0].nome, "!Catalogar");
    assert_eq!(
        perfil.candidatos_estante[0].rotulo.as_deref(),
        Some("#pasta")
    );
    assert_eq!(
        perfil.candidatos_estante[0].origem,
        OrigemEstante::ColunaPersonalizada
    );
    let vg = perfil
        .versao_calibre_gravacao
        .as_ref()
        .expect("OPFs de backup têm a versão");
    assert!(
        vg.versao
            .starts_with(&format!("{}.{}", versao.maior, versao.menor)),
        "{vg:?}"
    );
    assert!(perfil.library_id.is_some());

    // ---- A Classificar: criar, clonar colunas, reabrir ----
    let cli = cli.proteger_homologada(&h).unwrap();
    let ac = cli
        .criar_a_classificar(&t.path().join("AClassificar"), &h)
        .unwrap();
    assert_ne!(Some(ac.library_id().to_owned()), perfil.library_id);
    let clone = cli.clonar_estrutura(&ac, &perfil).unwrap();
    assert_eq!(clone.criadas.len(), 11, "{clone:?}");
    assert!(clone.divergencias.is_empty(), "{clone:?}");
    assert!(
        clone
            .buscas_salvas_criadas
            .contains(&"so-xadrez".to_owned())
    );
    assert!(
        clone
            .nao_clonadas
            .iter()
            .any(|(e, _)| e.starts_with("bibliotecas virtuais"))
    );
    // segunda execução é no-op
    let de_novo = cli.clonar_estrutura(&ac, &perfil).unwrap();
    assert!(de_novo.criadas.is_empty() && de_novo.ja_existiam.len() == 11);
    assert_eq!(cli.garantir_colunas_dag(&ac).unwrap().len(), 2);
    let reaberta = BibliotecaLeitura::abrir(ac.raiz()).unwrap();
    for c in &perfil.colunas {
        let n = reaberta
            .colunas_personalizadas()
            .iter()
            .find(|x| x.rotulo == c.rotulo)
            .unwrap();
        assert_eq!(
            (&n.tipo, n.e_multipla, &n.nome),
            (&c.tipo, c.e_multipla, &c.nome)
        );
        assert_eq!(n.display, c.display, "display de #{}", c.rotulo);
    }
    drop(reaberta);

    // ---- round-trip + idempotência 3x (CA-B21, CA-B11) ----
    let meta = CamposMetadados::de_livro(l, &perfil.colunas);
    let novo = NovoLivro {
        arquivo: l.formatos[0].caminho_absoluto.clone().into(),
        hash_blake3: hash_arquivo(Path::new(&l.formatos[0].caminho_absoluto)).unwrap(),
        capa: None,
        confianca: Some(0.75),
        formatos_extras: vec![l.formatos[1].caminho_absoluto.clone().into()],
        metadados: meta,
    };
    let r: Vec<_> = (0..3).map(|_| cli.adicionar(&ac, &novo).unwrap()).collect();
    assert!(r[0].criado && !r[1].criado && !r[2].criado);
    assert!(r.iter().all(|x| x.id == r[0].id));
    let bac = BibliotecaLeitura::abrir(ac.raiz()).unwrap();
    assert_eq!(
        bac.total_livros().unwrap(),
        1,
        "reprocessar 3 vezes não duplica"
    );
    let c = bac.livro(r[0].id).unwrap().unwrap();
    assert_eq!(c.titulo, l.titulo);
    assert_eq!(c.titulo_ordenacao, l.titulo_ordenacao);
    assert_eq!(c.autores, l.autores);
    assert_eq!(c.autor_ordenacao, l.autor_ordenacao);
    let (mut t1, mut t2) = (c.tags.clone(), l.tags.clone());
    t1.sort();
    t2.sort();
    assert_eq!(t1, t2);
    assert_eq!((&c.serie, c.indice_serie), (&l.serie, l.indice_serie));
    assert_eq!(c.editora, l.editora);
    assert_eq!(c.idiomas, l.idiomas);
    assert_eq!(c.comentarios, l.comentarios);
    assert_eq!(c.avaliacao, l.avaliacao);
    assert_eq!(c.data_publicacao, l.data_publicacao);
    assert_eq!(sem_dag(&c.identificadores), sem_dag(&l.identificadores));
    assert!(
        c.identificadores
            .iter()
            .any(|(t, v)| t == "dag" && *v == novo.hash_blake3)
    );
    assert!(!c.identificadores.iter().any(|(t, _)| t == "dagpend"));
    let mut cp = normaliza(&c.personalizados);
    assert_eq!(cp.remove(COLUNA_DAG_HASH), Some(json!(novo.hash_blake3)));
    assert_eq!(cp.remove(COLUNA_DAG_CONFIANCA), Some(json!(0.75)));
    assert_eq!(cp, normaliza(&l.personalizados));
    let mut fo: Vec<_> = l
        .formatos
        .iter()
        .map(|f| (f.formato.clone(), f.tamanho))
        .collect();
    let mut fc: Vec<_> = c
        .formatos
        .iter()
        .map(|f| (f.formato.clone(), f.tamanho))
        .collect();
    fo.sort();
    fc.sort();
    assert_eq!(fc, fo, "múltiplos formatos preservados");
    for f in &c.formatos {
        let o = l.formatos.iter().find(|x| x.formato == f.formato).unwrap();
        assert_eq!(
            hash_arquivo(Path::new(&f.caminho_absoluto)).unwrap(),
            hash_arquivo(Path::new(&o.caminho_absoluto)).unwrap()
        );
    }
    drop(bac);

    // listar --for-machine e busca por hash
    let lista = cli
        .listar(&ac, &["id", "title", "identifiers"], None, None)
        .unwrap();
    assert_eq!(lista.len(), 1);
    assert_eq!(
        cli.buscar_por_hash(&ac, &novo.hash_blake3).unwrap(),
        vec![(r[0].id, false)]
    );

    // ---- rollback na A Classificar ----
    cli.remover(&ac, &[r[0].id]).unwrap();
    assert_eq!(
        BibliotecaLeitura::abrir(ac.raiz())
            .unwrap()
            .total_livros()
            .unwrap(),
        0
    );
    assert!(
        cli.buscar_por_hash(&ac, &novo.hash_blake3)
            .unwrap()
            .is_empty()
    );

    // ---- recusas duras ----
    assert!(matches!(
        cli.criar_a_classificar(&h, &h).unwrap_err(),
        ErroCalibre::CaminhoProibido(_)
    ));
    assert!(matches!(
        cli.rodar(&Alvo::Local(h.clone()), "list", &[], false)
            .unwrap_err(),
        ErroCalibre::CaminhoProibido(_)
    ));
    assert!(matches!(
        BibliotecaAClassificarCopia::tentar(&h, t.path()).unwrap_err(),
        ErroCalibre::CaminhoProibido(_)
    ));

    // ---- CA-B01/CA-B22: depois de TUDO, a Homologada é idêntica ----
    let fim = tirar(&h, Escopo::Completo, Some(&antes)).unwrap();
    assert_eq!(
        comparar(&antes, &fim),
        vec![],
        "a Homologada não pode mudar"
    );
}

/// Cópia da Homologada (mesmo library_id) não pode virar A Classificar.
struct BibliotecaAClassificarCopia;
impl BibliotecaAClassificarCopia {
    fn tentar(h: &Path, tmp: &Path) -> Result<(), ErroCalibre> {
        let c = tmp.join("copia-da-homologada");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::copy(h.join("metadata.db"), c.join("metadata.db")).unwrap();
        crate::calibredb::BibliotecaAClassificar::abrir(&c, h).map(|_| ())
    }
}

#[test]
fn real_homologacao_via_calibredb_com_verificacao_e_rollback() {
    let Some((_g, cli)) = calibre() else { return };
    let t = tempfile::tempdir().unwrap();
    let h = t.path().join("H");
    let arquivos = t.path().join("arq");
    let exec = super::fixtures::executor_real().unwrap();
    construir_real(&exec, &h, &arquivos);
    let perfil = ler_perfil(&h).unwrap();
    let cli: ClienteCalibredb<ExecutorProcesso> = cli.proteger_homologada(&h).unwrap();
    let ac = cli.criar_a_classificar(&t.path().join("AC"), &h).unwrap();
    cli.clonar_estrutura(&ac, &perfil).unwrap();
    cli.garantir_colunas_dag(&ac).unwrap();

    let f1 = arquivos.join("novo.txt");
    std::fs::write(&f1, "Livro novo para homologar — acentuação\n").unwrap();
    let f2 = arquivos.join("novo.html");
    std::fs::write(&f2, "<html>novo</html>").unwrap();
    let hash = hash_arquivo(&f1).unwrap();
    let novo = NovoLivro {
        arquivo: f1.clone(),
        hash_blake3: hash.clone(),
        capa: None,
        confianca: Some(0.9),
        formatos_extras: vec![f2.clone()],
        metadados: CamposMetadados {
            titulo: Some("Livro Novo Ünico".into()),
            titulo_ordenacao: Some("Livro Novo Unico".into()),
            autores: Some(vec!["Zé Novo".into()]),
            autor_ordenacao: Some("Novo, Zé".into()),
            tags: Some(vec!["Xadrez".into()]),
            personalizados: vec![crate::calibredb::CampoPersonalizado {
                rotulo: "pasta".into(),
                tipo: crate::leitura::TipoColuna::Text,
                valor: json!("Xadrez"),
            }],
            ..CamposMetadados::default()
        },
    };
    let id_ac = cli.adicionar(&ac, &novo).unwrap().id;

    let ap = AprovacaoHomologacao {
        aprovado_por: "Chico".into(),
        aprovado_em_unix: agora(),
        hash_blake3: hash.clone(),
        motivo: "teste".into(),
    };
    let alvo = Alvo::Local(h.clone());
    let antes_total = BibliotecaLeitura::abrir(&h)
        .unwrap()
        .total_livros()
        .unwrap();
    let reg = homologar(
        &cli,
        &ac,
        id_ac,
        &h,
        &alvo,
        &ap,
        &OpcoesHomologacao::default(),
    )
    .unwrap();
    assert!(reg.integro(), "anomalias: {:?}", reg.anomalias);
    assert_eq!(reg.formatos_verificados.len(), 2);
    assert!(reg.campos_ignorados.iter().any(|(c, _)| c == "#dag_hash"));
    let bh = BibliotecaLeitura::abrir(&h).unwrap();
    assert_eq!(bh.total_livros().unwrap(), antes_total + 1);
    let novo_h = bh.livro(reg.id_criado).unwrap().unwrap();
    assert_eq!(novo_h.titulo, "Livro Novo Ünico");
    assert_eq!(novo_h.autores, ["Zé Novo"]);
    assert!(
        novo_h
            .personalizados
            .contains(&("pasta".to_owned(), json!("Xadrez")))
    );
    assert!(
        novo_h
            .identificadores
            .contains(&("dag".to_owned(), hash.clone()))
    );
    drop(bh);

    // idempotência: homologar de novo recusa
    let e = homologar(
        &cli,
        &ac,
        id_ac,
        &h,
        &alvo,
        &ap,
        &OpcoesHomologacao::default(),
    )
    .unwrap_err();
    assert!(matches!(e, ErroCalibre::JaExisteNaHomologada(_)), "{e}");

    // rollback
    reverter_homologacao(&cli, &h, &alvo, &reg).unwrap();
    let bh = BibliotecaLeitura::abrir(&h).unwrap();
    assert_eq!(bh.total_livros().unwrap(), antes_total);
    assert!(
        bh.livros_por_identificador("dag", &hash)
            .unwrap()
            .is_empty()
    );
}
