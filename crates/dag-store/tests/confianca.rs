//! Ponto de Confiança e detecção de Execuções Interrompidas (ADR 0003).
#![allow(clippy::unwrap_used)]

use dag_store::Catalogo;
use dag_store::confianca;
use dag_store::inventario::{self, EstadoScan};

fn volume(c: &Catalogo) -> i64 {
    let con = c.armazem().conexao();
    con.execute(
        "INSERT INTO volumes (platform_id, fs_type, mounted, last_seen_at)
         VALUES ('v1', 'ext4', 1, 1)",
        [],
    )
    .unwrap();
    con.last_insert_rowid()
}

#[test]
fn confianca_marca_perde_e_o_primeiro_motivo_fica() {
    let c = Catalogo::em_memoria().unwrap();
    let v = volume(&c);
    let con = c.armazem().conexao();
    assert!(!confianca::eh_confiavel(con, v, b"/dados").unwrap());
    confianca::marcar_confiavel(con, v, b"/dados", 100).unwrap();
    assert!(confianca::eh_confiavel(con, v, b"/dados").unwrap());
    confianca::perder(con, v, b"/dados", "overflow", 200).unwrap();
    confianca::perder(con, v, b"/dados", "outro", 300).unwrap();
    let l = confianca::listar(con).unwrap();
    assert_eq!(l.len(), 1);
    assert_eq!(
        (
            l[0].confiavel_em,
            l[0].perdido_em,
            l[0].motivo_perda.as_deref()
        ),
        (None, Some(200), Some("overflow"))
    );
    // Volta a ser confiável: a perda anterior some.
    confianca::marcar_confiavel(con, v, b"/dados", 400).unwrap();
    let l = confianca::listar(con).unwrap();
    assert_eq!((l[0].confiavel_em, l[0].perdido_em), (Some(400), None));
    confianca::marcar_confiavel(con, v, b"/outra", 400).unwrap();
    assert_eq!(confianca::perder_todas(con, "queda", 500).unwrap(), 2);
    assert!(
        confianca::listar(con)
            .unwrap()
            .iter()
            .all(|r| !r.confiavel())
    );
}

#[test]
fn detectar_interrompidas_marca_e_so_devolve_full_e_fast() {
    let c = Catalogo::em_memoria().unwrap();
    let con = c.armazem().conexao();
    let full = inventario::criar_execucao(con, "full", "[\"/a\"]", None, 10).unwrap();
    let monitor = inventario::criar_execucao(con, "monitor", "[\"/a\"]", None, 11).unwrap();
    let pronta = inventario::criar_execucao(con, "fast", "[]", None, 12).unwrap();
    inventario::definir_estado(con, pronta, EstadoScan::Concluida, None, 13).unwrap();

    let achadas = inventario::detectar_interrompidas(con, true, 50).unwrap();
    assert_eq!(achadas.len(), 1);
    assert_eq!(
        (achadas[0].id, achadas[0].raizes.clone()),
        (full, vec!["/a".to_string()])
    );
    for id in [full, monitor] {
        let e = inventario::obter_execucao(con, id).unwrap().unwrap();
        assert_eq!(e.estado, EstadoScan::Interrompida);
    }
    assert!(
        inventario::obter_execucao(con, full)
            .unwrap()
            .unwrap()
            .retomada_automatica
    );
    assert!(
        !inventario::obter_execucao(con, monitor)
            .unwrap()
            .unwrap()
            .retomada_automatica
    );
    // Cair de novo durante a retomada: continua sendo devolvida; com a política desligada a flag
    // desliga, mas a execução segue `interrompida`.
    let de_novo = inventario::detectar_interrompidas(con, false, 60).unwrap();
    assert_eq!(de_novo.len(), 1);
    assert!(
        !inventario::obter_execucao(con, full)
            .unwrap()
            .unwrap()
            .retomada_automatica
    );
    // Sem flag e já `interrompida`: não é redetectada.
    assert!(
        inventario::detectar_interrompidas(con, true, 70)
            .unwrap()
            .is_empty()
    );
}
