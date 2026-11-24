//! Erros esperados e aceitos: migração aditiva, regras do usuário, classificação gravada,
//! estado final da execução e resumo.
#![allow(clippy::unwrap_used)]

use dag_core::erros::ClasseErro;
use dag_store::erros::{
    OpcoesResumo, adicionar_exclusao, adicionar_regra, classificar_execucao, contar_desfecho,
    listar_exclusoes, listar_regras, marcar_esperado, remover_exclusao, remover_regra, resumir,
};
use dag_store::inventario::{self, EstadoScan, NovoProblema, Severidade};
use dag_store::{Catalogo, MIGRACOES};

const HOME: Option<&str> = Some("/home/u");

fn registrar(c: &Catalogo, run: i64, fase: &str, codigo: &str, caminho: &str, sev: Severidade) {
    inventario::registrar_problema(
        c.armazem().conexao(),
        &NovoProblema {
            run_id: run,
            pasta_id: None,
            path_raw: Some(caminho.as_bytes()),
            fase,
            codigo,
            motivo: Some("motivo"),
            severidade: sev,
            observado_em: 1,
        },
    )
    .unwrap();
}

fn execucao(c: &Catalogo) -> i64 {
    inventario::criar_execucao(c.armazem().conexao(), "full", "[]", None, 1).unwrap()
}

#[test]
fn migracao_e_aditiva_e_aceita_o_novo_status() {
    // Banco na versão 9 com uma execução `concluida_com_erros` e um problema antigo.
    let c = Catalogo::em_memoria().unwrap();
    assert_eq!(c.versao().unwrap() as usize, MIGRACOES.len());
    let run = execucao(&c);
    registrar(&c, run, "hash", "hash_ausente", "/x/a", Severidade::Aviso);
    inventario::definir_estado(
        c.armazem().conexao(),
        run,
        EstadoScan::ConcluidaComAvisos,
        None,
        5,
    )
    .unwrap();
    let st: String = c
        .armazem()
        .conexao()
        .query_row("SELECT status FROM scan_runs WHERE id = ?1", [run], |l| {
            l.get(0)
        })
        .unwrap();
    assert_eq!(st, "concluida_com_avisos");
    // Valor inválido continua barrado pelo CHECK.
    assert!(
        c.armazem()
            .conexao()
            .execute(
                "UPDATE scan_runs SET status = 'qualquer' WHERE id = ?1",
                [run]
            )
            .is_err()
    );
}

#[test]
fn migracao_de_banco_antigo_preserva_dados() {
    use bancada_armazem::{Armazem, Escada, Pragmas};
    let mut a = Armazem::em_memoria(&Pragmas::default()).unwrap();
    // Aplica só as 9 primeiras migrações (o esquema que o usuário já tem).
    let antigas = &MIGRACOES[..9];
    a.migrar(&Escada::nova(antigas)).unwrap();
    a.conexao()
        .execute(
            "INSERT INTO scan_runs (mode, roots, started_at, status) VALUES ('full','[]',1,'concluida_com_erros')",
            [],
        )
        .unwrap();
    a.conexao()
        .execute(
            "INSERT INTO scan_errors (run_id, path_raw, path_display, phase, error_code, observed_at, severity)
             VALUES (1, x'2f61', '/a', 'hash', 'hash_permissao', 1, 'aviso')",
            [],
        )
        .unwrap();
    a.migrar(&Escada::nova(MIGRACOES)).unwrap();
    // A execução antiga mantém o estado; a linha antiga fica sem classe.
    let (st, classe): (String, Option<String>) = a
        .conexao()
        .query_row(
            "SELECT r.status, e.classe FROM scan_runs r JOIN scan_errors e ON e.run_id = r.id",
            [],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )
        .unwrap();
    assert_eq!(st, "concluida_com_erros");
    assert_eq!(classe, None);
    // O novo status é aceito e as chaves estrangeiras seguem íntegras.
    a.conexao()
        .execute("UPDATE scan_runs SET status = 'concluida_com_avisos'", [])
        .unwrap();
    let fk: i64 = a
        .conexao()
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |l| {
            l.get(0)
        })
        .unwrap();
    assert_eq!(fk, 0);
    let ok: String = a
        .conexao()
        .query_row("PRAGMA integrity_check", [], |l| l.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
}

#[test]
fn regras_do_usuario_e_exclusoes() {
    let c = Catalogo::em_memoria().unwrap();
    let con = c.armazem().conexao();
    let a = adicionar_regra(con, "/mnt/p2/Dev", Some("hash_permissao"), "SQL Server", 1).unwrap();
    let b = adicionar_regra(con, "~/.cache", None, "cache", 2).unwrap();
    // Repetir só atualiza o motivo.
    assert_eq!(
        adicionar_regra(con, "/mnt/p2/Dev", Some("hash_permissao"), "novo motivo", 3).unwrap(),
        a
    );
    let regras = listar_regras(con).unwrap();
    assert_eq!(regras.len(), 2);
    assert_eq!(regras[0].motivo, "novo motivo");
    assert_eq!(regras[1].codigo, "*");
    assert!(remover_regra(con, b).unwrap());
    assert!(!remover_regra(con, b).unwrap());

    let e = adicionar_exclusao(con, "~/.cache", "cache do usuário").unwrap();
    assert_eq!(adicionar_exclusao(con, "~/.cache", "outra").unwrap(), e);
    let ex = listar_exclusoes(con).unwrap();
    assert_eq!(ex.len(), 1);
    assert_eq!(ex[0].motivo, "outra");
    assert!(remover_exclusao(con, e).unwrap());
    assert!(listar_exclusoes(con).unwrap().is_empty());
}

#[test]
fn estado_final_por_desfecho() {
    let c = Catalogo::em_memoria().unwrap();
    let con = c.armazem().conexao();

    // Sem problemas: concluída.
    let r0 = execucao(&c);
    assert_eq!(
        inventario::estado_final(con, r0).unwrap(),
        EstadoScan::Concluida
    );

    // Só informativos continuam "concluída".
    let r1 = execucao(&c);
    registrar(
        &c,
        r1,
        "inventario",
        "limite_de_volume",
        "/mnt",
        Severidade::Info,
    );
    assert_eq!(
        inventario::estado_final(con, r1).unwrap(),
        EstadoScan::Concluida
    );

    // Permissão negada (aviso) e arquivo vivo: com avisos.
    let r2 = execucao(&c);
    registrar(
        &c,
        r2,
        "hash",
        "hash_permissao",
        "/etc/shadow",
        Severidade::Aviso,
    );
    registrar(
        &c,
        r2,
        "hash",
        "hash_ausente",
        "/var/log/x",
        Severidade::Aviso,
    );
    assert_eq!(
        inventario::estado_final(con, r2).unwrap(),
        EstadoScan::ConcluidaComAvisos
    );

    // Erro real: com erros.
    let r3 = execucao(&c);
    registrar(
        &c,
        r3,
        "inventario",
        "erro_es",
        "/mnt/x/y",
        Severidade::Erro,
    );
    assert_eq!(
        inventario::estado_final(con, r3).unwrap(),
        EstadoScan::ConcluidaComErros
    );

    // Uma regra aceita vale para as próximas classificações: o erro vira aceito.
    adicionar_regra(con, "/mnt/x", Some("erro_es"), "ntfs", 1).unwrap();
    let r4 = execucao(&c);
    registrar(
        &c,
        r4,
        "inventario",
        "erro_es",
        "/mnt/x/y",
        Severidade::Erro,
    );
    assert_eq!(
        inventario::estado_final(con, r4).unwrap(),
        EstadoScan::ConcluidaComAvisos
    );
    let c4 = contar_desfecho(con, r4).unwrap();
    assert_eq!((c4.reais, c4.avisos), (0, 1));
}

#[test]
fn classificacao_gravada_e_marca_do_reteste() {
    let c = Catalogo::em_memoria().unwrap();
    let con = c.armazem().conexao();
    let run = execucao(&c);
    registrar(
        &c,
        run,
        "hash",
        "hash_alterado_durante_leitura",
        "/home/u/.cache/a",
        Severidade::Aviso,
    );
    registrar(
        &c,
        run,
        "hash",
        "hash_alterado_durante_leitura",
        "/home/u/docs/b.odt",
        Severidade::Aviso,
    );
    assert_eq!(classificar_execucao(con, run, HOME).unwrap(), 2);
    // Idempotente: nada novo a classificar.
    assert_eq!(classificar_execucao(con, run, HOME).unwrap(), 0);
    let classes: Vec<(String, String, Option<String>)> = con
        .prepare(
            "SELECT path_display, classe, regra FROM scan_errors WHERE run_id = ?1 ORDER BY id",
        )
        .unwrap()
        .query_map([run], |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(classes[0].1, ClasseErro::Esperado.como_str());
    assert_eq!(classes[0].2.as_deref(), Some("embutida:volateis"));
    assert_eq!(classes[1].1, ClasseErro::Erro.como_str());

    // O reteste do fim da etapa de hash marca o segundo como esperado, em qualquer pasta.
    let id: i64 = con
        .query_row(
            "SELECT id FROM scan_errors WHERE path_display = '/home/u/docs/b.odt'",
            [],
            |l| l.get(0),
        )
        .unwrap();
    marcar_esperado(con, id, "reteste", "continua mudando").unwrap();
    let r = resumir(con, run, &OpcoesResumo::default(), HOME).unwrap();
    assert_eq!(r.esperados, 2);
    assert_eq!(r.reais + r.avisos, 0);
}

#[test]
fn resumo_agrupa_por_codigo_e_pasta() {
    let c = Catalogo::em_memoria().unwrap();
    let con = c.armazem().conexao();
    let run = execucao(&c);
    for i in 0..5 {
        registrar(
            &c,
            run,
            "hash",
            "hash_permissao",
            &format!("/mnt/p2/Dev/db/f{i}.mdf"),
            Severidade::Aviso,
        );
    }
    registrar(
        &c,
        run,
        "hash",
        "hash_permissao",
        "/etc/shadow",
        Severidade::Aviso,
    );
    registrar(
        &c,
        run,
        "inventario",
        "erro_es",
        "/mnt/n/node_modules/.bin/x",
        Severidade::Erro,
    );
    registrar(
        &c,
        run,
        "inventario",
        "symlink_alvo_ilegivel",
        "/mnt/n/node_modules/.bin/y",
        Severidade::Info,
    );
    registrar(
        &c,
        run,
        "hash",
        "hash_ausente",
        "/home/u/.cache/z",
        Severidade::Aviso,
    );

    let r = resumir(con, run, &OpcoesResumo::default(), HOME).unwrap();
    assert_eq!(r.total, 9);
    assert_eq!(
        (r.reais, r.avisos, r.esperados, r.aceitos, r.informativos),
        (1, 6, 1, 0, 1)
    );
    // Erros reais primeiro; depois avisos, ordenados por quantidade.
    assert_eq!(r.por_codigo[0].codigo, "erro_es");
    assert_eq!(r.por_codigo[0].rotulo, "erro");
    assert_eq!(r.por_codigo[1].codigo, "hash_permissao");
    assert_eq!(r.por_codigo[1].total, 6);
    assert!(
        r.por_codigo[1]
            .dica
            .as_deref()
            .unwrap()
            .contains("--elevar")
    );
    // A pasta com mais problemas é a do SQL Server.
    assert_eq!(r.por_pasta[0].pasta, "/mnt/p2/Dev/db");
    assert_eq!(r.por_pasta[0].total, 5);
    assert!(!r.amostras.is_empty());

    // Filtros por código e pasta.
    let f = resumir(
        con,
        run,
        &OpcoesResumo {
            codigo: Some("hash_permissao"),
            pasta: Some("/mnt/p2"),
            ..OpcoesResumo::default()
        },
        HOME,
    )
    .unwrap();
    assert_eq!(f.total, 5);

    // Aceitar a pasta: o resumo passa a mostrar "aceito" sem mexer nas linhas gravadas.
    adicionar_regra(con, "/mnt/p2/Dev", Some("hash_permissao"), "SQL Server", 1).unwrap();
    let r2 = resumir(con, run, &OpcoesResumo::default(), HOME).unwrap();
    assert_eq!(r2.aceitos, 5);
    assert_eq!(r2.avisos, 1);
}
