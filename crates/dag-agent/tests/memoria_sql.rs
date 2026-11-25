//! Medição de memória (VmHWM) das fases SQL do Catálogo sobre um banco sintético grande
//! (`DAG_MEM_BANCO`, gerado por script). `#[ignore]`: rode em release.
#![allow(clippy::unwrap_used)]

use std::fs;

use dag_core::Autor;
use dag_store::duplicados::{self, Escopo};
use dag_store::inventario::agregacao::agregar_execucao;
use dag_store::{Catalogo, hashes};

fn kb(campo: &str) -> u64 {
    fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with(campo))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn fase(nome: &str, t: std::time::Instant) {
    println!(
        "MEM {nome}: VmHWM {:.0} MiB, VmRSS {:.0} MiB, {:.1}s",
        kb("VmHWM:") as f64 / 1024.0,
        kb("VmRSS:") as f64 / 1024.0,
        t.elapsed().as_secs_f64()
    );
}

#[test]
#[ignore = "medição de escala"]
fn memoria_fases_sql() {
    // Fases em processos separados para o pico ser o da fase: DAG_MEM_FASE=agregacao|fila|grupos.
    let banco = std::env::var("DAG_MEM_BANCO").unwrap();
    let fase_alvo = std::env::var("DAG_MEM_FASE").unwrap_or_default();
    let mut cat = Catalogo::abrir(banco).unwrap();
    let run: i64 = cat
        .armazem()
        .conexao()
        .query_row("SELECT MAX(id) FROM scan_runs", [], |l| l.get(0))
        .unwrap();
    fase("base", std::time::Instant::now());
    let t = std::time::Instant::now();
    match fase_alvo.as_str() {
        "agregacao" => {
            agregar_execucao(cat.armazem_mut().conexao_mut(), run, 5000).unwrap();
        }
        "fila" => {
            cat.armazem_mut()
                .transacao_imediata(|tx| hashes::preparar_fila_hash(tx, run, 5000))
                .unwrap();
        }
        "grupos" => {
            duplicados::reconstruir_grupos(&mut cat, Escopo::Tudo, Autor::Cli, 5000).unwrap();
        }
        "resumo" => {
            let r = duplicados::resumo_duplicados(cat.armazem().conexao()).unwrap();
            println!("resumo: {r:?}");
        }
        "reavaliar" => {
            duplicados::reavaliar_marcacoes(&mut cat, Autor::Cli, 5000).unwrap();
        }
        _ => panic!("DAG_MEM_FASE"),
    }
    fase(&fase_alvo, t);
}
