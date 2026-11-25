#![allow(clippy::unwrap_used)]
mod comum;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use comum::{Falso, campo, chat_openai, entrada_calculo, pagina, perfil_openai, saida};
use dag_biblio::Campo;
use dag_biblio_ia::benchmark::{
    CasoBenchmark, GabaritoValor, LeituraVram, MedidorVram, OpcoesBenchmark, StatusModelo,
    carregar_casos, executar_benchmark,
};
use dag_biblio_ia::{Classificador, ClienteLlm, ConfigCliente, PapelModelo};
use serde_json::json;

#[derive(Debug)]
struct VramFalsa(AtomicU64);

impl MedidorVram for VramFalsa {
    fn ler(&self) -> Option<LeituraVram> {
        Some(LeituraVram {
            usada_mib: self.0.fetch_add(100, Ordering::SeqCst) + 1000,
            total_mib: 16376,
        })
    }
}

fn caso(id: &str, titulo_gab: Option<&str>, isbn_gab: Option<&str>) -> CasoBenchmark {
    let mut e = entrada_calculo();
    e.extracao.paginas.push(pagina(7, "Fim."));
    CasoBenchmark {
        id: id.into(),
        entrada: e,
        gabarito: [
            (
                Campo::Titulo,
                titulo_gab.map(|t| GabaritoValor::Texto(t.into())),
            ),
            (
                Campo::Isbn,
                isbn_gab.map(|t| GabaritoValor::Texto(t.into())),
            ),
            (
                Campo::Autores,
                Some(GabaritoValor::Lista(vec!["Stewart, James".into()])),
            ),
        ]
        .into_iter()
        .collect(),
    }
}

fn classificador(f: &Falso, nome: &str) -> Classificador {
    let cfg = ConfigCliente {
        tentativas: 1,
        timeout: Duration::from_secs(5),
        ..ConfigCliente::default()
    };
    Classificador::novo(
        ClienteLlm::novo(perfil_openai(&f.endpoint, nome, PapelModelo::Primario), cfg).unwrap(),
        None,
    )
}

#[test]
fn benchmark_com_servidores_falsos_mede_e_nao_inventa() {
    // Modelo bom: acerta tudo. Modelo ruim: erra o título e inventa ISBN no caso 2.
    let bom = saida(json!({
        "titulo": campo(json!("Cálculo Volume 1"), 0.9, 3, "CÁLCULO\nVolume 1"),
        "isbn": campo(json!("9788535902778"), 0.9, 3, "ISBN 978-85-359-0277-8"),
        "autores": campo(json!(["James Stewart"]), 0.9, 3, "James Stewart")
    }));
    let ruim = saida(json!({
        "titulo": campo(json!("Prefácio"), 0.9, 4, "Prefácio. Este livro"),
        "isbn": campo(json!("9788535902778"), 0.9, 3, "ISBN 978-85-359-0277-8"),
        "autores": campo(json!(["James Stewart"]), 0.9, 3, "James Stewart")
    }));
    let fb = Falso::novo(move |c, _| (c == "/v1/chat/completions").then(|| chat_openai(&bom)));
    let fr = Falso::novo(move |c, _| (c == "/v1/chat/completions").then(|| chat_openai(&ruim)));
    // Porta fechada: modelo não executado.
    let morto = {
        let f = Falso::novo(|_, _| None);
        let ep = f.endpoint.clone();
        drop(f);
        std::thread::sleep(Duration::from_millis(100));
        ep
    };
    let cfg = ConfigCliente {
        tentativas: 1,
        timeout: Duration::from_secs(1),
        ..ConfigCliente::default()
    };
    let c_morto = Classificador::novo(
        ClienteLlm::novo(
            perfil_openai(&morto, "fora-do-ar", PapelModelo::Primario),
            cfg,
        )
        .unwrap(),
        None,
    );

    // Caso 1: gabarito completo; caso 2: gabarito diz que não há ISBN (o modelo "inventou").
    let casos = vec![
        caso("c1", Some("Cálculo Volume 1"), Some("9788535902778")),
        caso("c2", Some("Cálculo Volume 1"), None),
    ];
    let opcoes = OpcoesBenchmark {
        medidor: Some(Arc::new(VramFalsa(AtomicU64::new(0)))),
        intervalo_amostra: Duration::from_millis(5),
    };
    let rel = executar_benchmark(
        &casos,
        &[
            classificador(&fb, "bom"),
            classificador(&fr, "ruim"),
            c_morto,
        ],
        &opcoes,
    );

    assert_eq!(rel.casos, 2);
    assert!(rel.vram_medida);
    assert_eq!(rel.gpu_total_mib, Some(16376));
    assert_eq!(rel.modelos.len(), 3);

    let bom = &rel.modelos[0];
    assert_eq!(bom.status, StatusModelo::Executado);
    let r = bom.resumo.as_ref().unwrap();
    assert_eq!(r.itens_ok, 2);
    assert_eq!(r.por_campo[&Campo::Titulo].corretos, 2);
    assert_eq!(r.por_campo[&Campo::Isbn].corretos, 1);
    assert_eq!(
        r.por_campo[&Campo::Isbn].inventados,
        1,
        "gabarito sem ISBN e modelo respondeu"
    );
    assert_eq!(r.por_campo[&Campo::Autores].corretos, 2);
    assert_eq!(r.tokens_por_segundo_medio, Some(50.0));
    assert!(r.vram_pico_mib.unwrap() >= 1000);
    assert!(bom.itens[0].vram_antes_mib.is_some());
    assert_eq!(bom.itens[0].tokens_saida, Some(200));
    assert_eq!(bom.itens[0].paginas_na_entrada, 3);
    assert!(bom.itens[0].ms_por_pagina.is_some());

    let ruim = &rel.modelos[1];
    let r = ruim.resumo.as_ref().unwrap();
    assert_eq!(r.por_campo[&Campo::Titulo].errados, 2);
    assert_eq!(r.por_campo[&Campo::Titulo].precisao, Some(0.0));
    assert!(r.precisao_global.unwrap() < bom.resumo.as_ref().unwrap().precisao_global.unwrap());

    let morto = &rel.modelos[2];
    assert!(
        matches!(&morto.status, StatusModelo::NaoExecutado { motivo } if motivo.contains("inacessível"))
    );
    assert!(morto.resumo.is_none() && morto.itens.is_empty());

    let md = rel.para_markdown();
    assert!(md.contains("# Benchmark"));
    assert!(md.contains("NÃO EXECUTADO"));
    assert!(md.contains("| titulo |"));
    let json: serde_json::Value = serde_json::from_str(&rel.para_json().unwrap()).unwrap();
    assert_eq!(json["modelos"][2]["status"]["estado"], "nao_executado");
}

#[test]
fn sem_nvidia_smi_e_sem_casos_o_relatorio_diz_que_nao_rodou() {
    struct Sem;
    impl std::fmt::Debug for Sem {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("Sem")
        }
    }
    impl MedidorVram for Sem {
        fn ler(&self) -> Option<LeituraVram> {
            None
        }
    }
    let f = Falso::novo(|_, _| None);
    let opcoes = OpcoesBenchmark {
        medidor: Some(Arc::new(Sem)),
        intervalo_amostra: Duration::from_millis(5),
    };
    let rel = executar_benchmark(&[], &[classificador(&f, "m")], &opcoes);
    assert!(!rel.vram_medida);
    let md = rel.para_markdown();
    assert!(md.contains("VRAM medida: NÃO"));
    assert!(md.contains("Nenhum caso"));
    assert!(md.contains("NÃO EXECUTADO"));
    assert_eq!(f.contagem("/v1/chat/completions"), 0);

    // Com casos mas sem medidor, VRAM fica n/d, não zero.
    let fb =
        Falso::novo(|c, _| (c == "/v1/chat/completions").then(|| chat_openai(&saida(json!({})))));
    let rel = executar_benchmark(
        &[caso("c", None, None)],
        &[classificador(&fb, "m")],
        &OpcoesBenchmark {
            medidor: None,
            intervalo_amostra: Duration::from_millis(5),
        },
    );
    assert_eq!(rel.modelos[0].itens[0].vram_pico_mib, None);
    assert!(rel.para_markdown().contains("n/d"));
}

#[test]
fn carrega_casos_de_diretorio() {
    let d = tempfile::tempdir().unwrap();
    let c = caso("", Some("X"), None);
    std::fs::write(
        d.path().join("b-segundo.json"),
        serde_json::to_string(&c).unwrap(),
    )
    .unwrap();
    std::fs::write(
        d.path().join("a-primeiro.json"),
        serde_json::to_string(&c).unwrap(),
    )
    .unwrap();
    std::fs::write(d.path().join("ignorar.txt"), "x").unwrap();
    let casos = carregar_casos(d.path()).unwrap();
    assert_eq!(casos.len(), 2);
    assert_eq!(casos[0].id, "a-primeiro");
    assert_eq!(casos[0].entrada.id, "a-primeiro");
    assert_eq!(casos[0].gabarito[&Campo::Isbn], None);
    std::fs::write(d.path().join("quebrado.json"), "{").unwrap();
    assert!(carregar_casos(d.path()).is_err());
}
