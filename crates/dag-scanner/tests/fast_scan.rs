//! Fast Scan (A-12, A-18; CA-A09, CA-A14, CA-A25): reconciliação sem reler o que não mudou,
//! renomeação com id/hash preservados e remoção só com prova.
#![allow(clippy::unwrap_used)]

mod comum;

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use comum::*;
use dag_core::ExecucaoId;
use dag_scanner::{
    ConfigScan, ControleScan, DesfechoScan, ErroScan, EtapaHash, FastScan, FullScan, ResultadoScan,
};
use dag_store::Catalogo;
use dag_store::inventario::marcar_interrompidas;

fn conteudo(letra: u8, n: usize) -> Vec<u8> {
    vec![letra; n]
}

fn gravar(p: &Path, dados: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, dados).unwrap();
}

fn caminho(p: &Path) -> Vec<u8> {
    p.as_os_str().as_bytes().to_vec()
}

fn fast(raiz: &Path) -> FastScan {
    FastScan::novo(vec![raiz.to_path_buf()], vec![volume_de(raiz)])
}

fn full_com_hash(raiz: &Path, cat: &mut Catalogo) -> i64 {
    let r = FullScan::novo(vec![raiz.to_path_buf()], vec![volume_de(raiz)])
        .executar(cat)
        .unwrap();
    EtapaHash::nova(ExecucaoId(r.execucao_id))
        .executar(cat)
        .unwrap();
    r.execucao_id
}

fn fast_com_hash(raiz: &Path, cat: &mut Catalogo) -> (ResultadoScan, dag_scanner::ResultadoHash) {
    let r = fast(raiz).executar(cat).unwrap();
    let h = EtapaHash::nova(ExecucaoId(r.execucao_id))
        .executar(cat)
        .unwrap();
    (r, h)
}

fn id_de(cat: &Catalogo, p: &Path) -> i64 {
    cat.armazem()
        .conexao()
        .query_row(
            "SELECT id FROM files WHERE path_raw = ?1",
            [caminho(p)],
            |l| l.get(0),
        )
        .unwrap()
}

fn estado(cat: &Catalogo, p: &Path) -> Option<String> {
    cat.armazem()
        .conexao()
        .query_row(
            "SELECT state FROM files WHERE path_raw = ?1",
            [caminho(p)],
            |l| l.get(0),
        )
        .ok()
}

fn hash_de(cat: &Catalogo, id: i64) -> Option<Vec<u8>> {
    cat.armazem()
        .conexao()
        .query_row("SELECT hash FROM hashes WHERE file_id = ?1", [id], |l| {
            l.get(0)
        })
        .ok()
}

fn ausentes(cat: &Catalogo) -> i64 {
    n(
        cat,
        "SELECT (SELECT COUNT(*) FROM files WHERE state = 'ausente') +
                (SELECT COUNT(*) FROM folders WHERE state = 'ausente')",
    )
}

fn arvore_base(raiz: &Path) {
    gravar(&raiz.join("a.txt"), &conteudo(b'a', 1000));
    gravar(&raiz.join("sub/b.txt"), &conteudo(b'b', 2000));
    gravar(&raiz.join("sub/c.bin"), &conteudo(b'c', 3000));
    gravar(&raiz.join("d/e.txt"), &conteudo(b'e', 4000));
    gravar(&raiz.join("d/f.txt"), &conteudo(b'f', 5000));
}

#[test]
fn ca_a09_e_ca_a25_adicao_alteracao_renomeacao_remocao() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    arvore_base(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    full_com_hash(&raiz, &mut cat);
    let id_b = id_de(&cat, &raiz.join("sub/b.txt"));
    let id_e = id_de(&cat, &raiz.join("d/e.txt"));
    let id_a = id_de(&cat, &raiz.join("a.txt"));
    let hash_b = hash_de(&cat, id_b).unwrap();
    let hash_e = hash_de(&cat, id_e).unwrap();
    let hash_a = hash_de(&cat, id_a).unwrap();

    // Mudanças entre os scans.
    gravar(&raiz.join("novo.txt"), &conteudo(b'n', 700)); // adição
    gravar(&raiz.join("a.txt"), &conteudo(b'A', 1500)); // modificação
    fs::rename(raiz.join("sub/b.txt"), raiz.join("sub/b2.txt")).unwrap(); // renomeação
    fs::rename(raiz.join("d/e.txt"), raiz.join("sub/e_movido.txt")).unwrap(); // mudança de pasta
    fs::remove_file(raiz.join("sub/c.bin")).unwrap(); // remoção

    let (r, h) = fast_com_hash(&raiz, &mut cat);
    assert_eq!(r.desfecho, DesfechoScan::Concluida);
    let rec = r.reconciliacao.clone().unwrap();
    assert!(rec.remocao_aplicada, "{rec:?}");
    assert_eq!(rec.arquivos_novos, 1);
    assert_eq!(rec.arquivos_alterados, 1);
    assert_eq!(rec.arquivos_renomeados, 2);
    assert_eq!(rec.arquivos_ausentes, 1);

    // Adição.
    assert_eq!(
        estado(&cat, &raiz.join("novo.txt")).as_deref(),
        Some("ativo")
    );
    // Renomeação: mesmo id, mesmo hash, caminho antigo não existe mais.
    assert_eq!(id_de(&cat, &raiz.join("sub/b2.txt")), id_b);
    assert_eq!(id_de(&cat, &raiz.join("sub/e_movido.txt")), id_e);
    assert_eq!(estado(&cat, &raiz.join("sub/b.txt")), None);
    assert_eq!(estado(&cat, &raiz.join("d/e.txt")), None);
    assert_eq!(hash_de(&cat, id_b).unwrap(), hash_b);
    assert_eq!(hash_de(&cat, id_e).unwrap(), hash_e);
    let pasta_e: String = cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT p.path_display FROM files f JOIN folders p ON p.id = f.folder_id WHERE f.id = ?1",
            [id_e],
            |l| l.get(0),
        )
        .unwrap();
    assert_eq!(pasta_e, raiz.join("sub").to_str().unwrap());
    // Modificação: mesmo id, novo tamanho, hash novo.
    assert_eq!(id_de(&cat, &raiz.join("a.txt")), id_a);
    assert_eq!(
        n(
            &cat,
            &format!("SELECT logical_bytes FROM files WHERE id = {id_a}")
        ),
        1500
    );
    assert_ne!(hash_de(&cat, id_a).unwrap(), hash_a);
    assert_eq!(
        hash_de(&cat, id_a).unwrap(),
        blake3::hash(&conteudo(b'A', 1500)).as_bytes().to_vec()
    );
    // Remoção.
    assert_eq!(
        estado(&cat, &raiz.join("sub/c.bin")).as_deref(),
        Some("ausente")
    );
    assert_eq!(ausentes(&cat), 1);
    // CA-A25: só os arquivos novos/alterados foram lidos (700 + 1500), nada mais.
    assert_eq!(h.bytes_lidos, 700 + 1500, "{h:?}");
    assert_eq!(h.hasheados, 2);
    assert_eq!(h.reaproveitados, 5 - 2);
    // Eventos: 2 renomeações com o caminho antigo, 1 removido, 1 criado, 1 alterado.
    let ev = |t: &str| {
        n(
            &cat,
            &format!(
                "SELECT COUNT(*) FROM scan_events WHERE event_type = '{t}' AND source = 'fast_scan'"
            ),
        )
    };
    assert_eq!(
        (
            ev("renomeado"),
            ev("removido"),
            ev("criado"),
            ev("alterado")
        ),
        (2, 1, 1, 1)
    );
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_events WHERE event_type = 'renomeado' AND old_path_raw IS NOT NULL AND file_id IS NOT NULL"
        ),
        2
    );
}

#[test]
fn arvore_inalterada_nao_reescreve_nada_e_nao_le_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    arvore_base(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    full_com_hash(&raiz, &mut cat);
    let antes = foto(&cat);
    let (r, h) = fast_com_hash(&raiz, &mut cat);
    let rec = r.reconciliacao.unwrap();
    assert_eq!(rec.arquivos_inalterados, 5);
    assert_eq!(
        (
            rec.arquivos_novos,
            rec.arquivos_alterados,
            rec.arquivos_renomeados,
            rec.arquivos_ausentes
        ),
        (0, 0, 0, 0)
    );
    assert_eq!((h.bytes_lidos, h.hasheados, h.reaproveitados), (0, 0, 5));
    assert_eq!(foto(&cat), antes, "o catálogo não muda");
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_events WHERE source = 'fast_scan'"
        ),
        0
    );
    // Todas as linhas foram carimbadas pela nova execução (agregados cobrem tudo).
    assert_eq!(
        n(
            &cat,
            &format!(
                "SELECT COUNT(*) FROM files WHERE seen_run_id = {}",
                r.execucao_id
            )
        ),
        5
    );
}

#[test]
fn renomear_pasta_preserva_ids_de_pasta_e_arquivos() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    arvore_base(&raiz);
    gravar(&raiz.join("d/fundo/g.txt"), &conteudo(b'g', 100));
    let mut cat = Catalogo::em_memoria().unwrap();
    full_com_hash(&raiz, &mut cat);
    let pasta_d: i64 = cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT id FROM folders WHERE path_raw = ?1",
            [caminho(&raiz.join("d"))],
            |l| l.get(0),
        )
        .unwrap();
    let ids: Vec<i64> = ["d/e.txt", "d/f.txt", "d/fundo/g.txt"]
        .iter()
        .map(|p| id_de(&cat, &raiz.join(p)))
        .collect();
    fs::rename(raiz.join("d"), raiz.join("d_novo")).unwrap();
    let (r, h) = fast_com_hash(&raiz, &mut cat);
    let rec = r.reconciliacao.unwrap();
    assert_eq!(ausentes(&cat), 0, "{rec:?}");
    assert_eq!(rec.pastas_renomeadas, 2, "{rec:?}");
    assert_eq!(rec.arquivos_renomeados, 3, "{rec:?}");
    assert_eq!(
        id_de(&cat, &raiz.join("d_novo/e.txt")),
        ids[0],
        "arquivo mantém o id"
    );
    assert_eq!(id_de(&cat, &raiz.join("d_novo/f.txt")), ids[1]);
    assert_eq!(id_de(&cat, &raiz.join("d_novo/fundo/g.txt")), ids[2]);
    let (id_novo, path): (i64, String) = cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT id, path_display FROM folders WHERE path_raw = ?1",
            [caminho(&raiz.join("d_novo"))],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )
        .unwrap();
    assert_eq!(id_novo, pasta_d, "{path}");
    assert_eq!(h.bytes_lidos, 0, "renomear não relê conteúdo");
    assert_eq!(
        n(&cat, "SELECT COUNT(*) FROM files WHERE state <> 'ativo'"),
        0
    );
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM folders f WHERE f.parent_folder_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM folders p WHERE p.id = f.parent_folder_id)"
        ),
        0
    );
}

#[test]
fn hard_link_novo_nao_vira_renomeacao() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    arvore_base(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    full_com_hash(&raiz, &mut cat);
    // O original continua existindo (visto), logo o novo caminho é só um novo link.
    fs::hard_link(raiz.join("sub/b.txt"), raiz.join("d/b_link.txt")).unwrap();
    let r = fast(&raiz).executar(&mut cat).unwrap();
    let rec = r.reconciliacao.unwrap();
    assert_eq!((rec.arquivos_renomeados, rec.arquivos_ausentes), (0, 0));
    assert_eq!(
        estado(&cat, &raiz.join("sub/b.txt")).as_deref(),
        Some("ativo")
    );
    assert_eq!(
        estado(&cat, &raiz.join("d/b_link.txt")).as_deref(),
        Some("ativo")
    );
}

#[test]
fn raiz_indisponivel_nao_remove_nada() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    arvore_base(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    full_com_hash(&raiz, &mut cat);
    let antes = foto(&cat);
    let volume = volume_de(&raiz);
    // Raiz renomeada (equivale a ponto de montagem que sumiu): o caminho não existe mais.
    let outro = tmp.path().join("acervo_fora");
    fs::rename(&raiz, &outro).unwrap();
    let e = FastScan::novo(vec![raiz.clone()], vec![volume.clone()])
        .executar(&mut cat)
        .unwrap_err();
    assert!(matches!(e, ErroScan::SemRaizes), "{e:?}");
    assert_eq!(ausentes(&cat), 0);
    assert_eq!(foto(&cat), antes);
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_errors WHERE severity = 'impeditivo'"
        ),
        1
    );
    // Raiz esvaziada por trás de um mount que sumiu: raiz existe mas é outra: continua sendo o
    // mesmo diretório, então aqui a remoção é legítima. Volta o conteúdo: tudo reativa.
    fs::rename(&outro, &raiz).unwrap();
    let r = FastScan::novo(vec![raiz.clone()], vec![volume])
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r.reconciliacao.unwrap().arquivos_ausentes, 0);
    assert_eq!(ausentes(&cat), 0);
}

#[test]
fn volume_desconectado_nao_remove_e_reconectar_restaura() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    arvore_base(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    full_com_hash(&raiz, &mut cat);
    assert_eq!(n(&cat, "SELECT mounted FROM volumes"), 1);
    let visto: i64 = n(&cat, "SELECT last_seen_at FROM volumes");
    // O volume some da descoberta (desconectado); o caminho da raiz ainda existe no disco.
    let e = FastScan::novo(vec![raiz.clone()], vec![])
        .executar(&mut cat)
        .unwrap_err();
    assert!(matches!(e, ErroScan::SemRaizes), "{e:?}");
    assert_eq!(n(&cat, "SELECT mounted FROM volumes"), 0);
    assert_eq!(
        n(&cat, "SELECT last_seen_at FROM volumes"),
        visto,
        "preserva a última vez visto"
    );
    assert_eq!(ausentes(&cat), 0);
    assert_eq!(
        n(&cat, "SELECT COUNT(*) FROM files WHERE state = 'ativo'"),
        5
    );
    // Nas visões, tudo aparece como indisponível (sem reescrever as linhas).
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM v_arquivos WHERE estado_efetivo = 'indisponivel'"
        ),
        5
    );
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM v_pastas WHERE estado_efetivo = 'indisponivel'"
        ),
        n(&cat, "SELECT COUNT(*) FROM folders")
    );
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_events WHERE event_type = 'desmontado'"
        ),
        1
    );
    // Reconecta: o scan marca como montado e tudo volta a ficar disponível.
    let r = fast(&raiz).executar(&mut cat).unwrap();
    assert_eq!(r.reconciliacao.unwrap().arquivos_inalterados, 5);
    assert_eq!(n(&cat, "SELECT mounted FROM volumes"), 1);
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM v_arquivos WHERE estado_efetivo = 'ativo'"
        ),
        5
    );
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_events WHERE event_type = 'montado'"
        ),
        1
    );
}

#[test]
fn diretorio_ilegivel_protege_a_subarvore_dele() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    arvore_base(&raiz);
    gravar(&raiz.join("privado/segredo.txt"), &conteudo(b's', 10));
    let mut cat = Catalogo::em_memoria().unwrap();
    full_com_hash(&raiz, &mut cat);
    let priv_dir = raiz.join("privado");
    fs::set_permissions(&priv_dir, fs::Permissions::from_mode(0o000)).unwrap();
    let restaurar = || fs::set_permissions(&priv_dir, fs::Permissions::from_mode(0o755)).unwrap();
    if fs::read_dir(&priv_dir).is_ok() {
        // Rodando como root: permissão não bloqueia; o cenário não existe.
        restaurar();
        return;
    }
    fs::remove_file(raiz.join("sub/c.bin")).unwrap();
    let r = fast(&raiz).executar(&mut cat).unwrap();
    restaurar();
    let rec = r.reconciliacao.unwrap();
    // Remoção legítima fora da pasta bloqueada; o que está dentro dela nunca é dado como removido.
    assert_eq!(
        estado(&cat, &raiz.join("sub/c.bin")).as_deref(),
        Some("ausente")
    );
    assert_eq!(
        estado(&cat, &raiz.join("privado/segredo.txt")).as_deref(),
        Some("ativo")
    );
    assert!(rec.bloqueios >= 1);
    assert_eq!(r.desfecho, DesfechoScan::ConcluidaComAvisos);
}

#[test]
fn ca_a14_cancelar_no_meio_nao_remove_e_retomar_conclui() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    criar_arvore(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    full_com_hash(&raiz, &mut cat);
    let total_arquivos = n(&cat, "SELECT COUNT(*) FROM files");
    let removidos = 1 + percorrer_fs(&raiz.join("t3/m4"))
        .values()
        .filter(|m| m.kind == "arquivo")
        .count() as i64;
    fs::remove_file(raiz.join("t0/a0.bin")).unwrap();
    fs::remove_dir_all(raiz.join("t3/m4")).unwrap();

    let controle = ControleScan::novo();
    let c2 = controle.clone();
    let vistos = Arc::new(AtomicU64::new(0));
    let v2 = Arc::clone(&vistos);
    let scan = FastScan::novo(vec![raiz.clone()], vec![volume_de(&raiz)])
        .com_config(config_miuda())
        .com_controle(controle)
        .com_progresso(move |p| {
            v2.store(p.arquivos, Ordering::SeqCst);
            if p.arquivos >= 300 {
                c2.cancelar();
            }
        });
    let r = scan.executar(&mut cat).unwrap();
    assert_eq!(r.desfecho, DesfechoScan::Cancelada);
    assert!(r.reconciliacao.is_none());
    assert!(vistos.load(Ordering::SeqCst) < total_arquivos as u64);
    // Interrompido: nenhuma remoção falsa, nem do que de fato sumiu (ainda não há prova).
    assert_eq!(ausentes(&cat), 0);

    // "Morte" do processo: o estado persistido continua retomável.
    cat.armazem_mut()
        .transacao_imediata(|tx| -> Result<(), dag_store::Erro> {
            tx.execute(
                "UPDATE scan_runs SET status = 'em_andamento' WHERE id = ?1",
                [r.execucao_id],
            )?;
            marcar_interrompidas(tx, 4_000_000_000)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(ausentes(&cat), 0);
    let r2 = FastScan::novo(vec![], vec![volume_de(&raiz)])
        .retomar(&mut cat, r.execucao_id)
        .unwrap();
    assert_eq!(r2.desfecho, DesfechoScan::Concluida);
    let rec = r2.reconciliacao.unwrap();
    assert!(rec.remocao_aplicada, "{rec:?}");
    assert_eq!(rec.arquivos_ausentes as i64, removidos, "{rec:?}");
    assert_eq!(
        estado(&cat, &raiz.join("t0/a0.bin")).as_deref(),
        Some("ausente")
    );
    assert_eq!(
        n(&cat, "SELECT COUNT(*) FROM files WHERE state = 'ativo'"),
        total_arquivos - removidos
    );
    // Os contadores de inalterados sobreviveram à retomada.
    assert_eq!(
        rec.arquivos_inalterados as i64,
        total_arquivos - removidos,
        "{rec:?}"
    );
}

#[test]
fn primeiro_fast_scan_em_raiz_nova_cataloga_tudo_como_novo() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    arvore_base(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    let (r, h) = fast_com_hash(&raiz, &mut cat);
    let rec = r.reconciliacao.unwrap();
    assert_eq!(rec.arquivos_novos, 5);
    assert_eq!(h.hasheados, 5);
    assert_eq!(
        n(&cat, "SELECT COUNT(*) FROM scan_runs WHERE mode = 'fast'"),
        1
    );
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_events WHERE event_type = 'criado'"
        ),
        5
    );
    // O agregado cobre a execução.
    assert_eq!(
        n(
            &cat,
            "SELECT recursive_file_count FROM folder_stats s JOIN folders f ON f.id = s.folder_id WHERE f.parent_folder_id IS NULL"
        ),
        5
    );
}

#[test]
fn raiz_aninhada_mantem_o_pai_e_seu_proprio_escopo() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    arvore_base(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    full_com_hash(&raiz, &mut cat);
    // Fast Scan só de "sub": o resto do acervo não é tocado (nem dado como removido).
    fs::remove_file(raiz.join("d/f.txt")).unwrap();
    fs::remove_file(raiz.join("sub/c.bin")).unwrap();
    let r = FastScan::novo(vec![raiz.join("sub")], vec![volume_de(&raiz)])
        .executar(&mut cat)
        .unwrap();
    let rec = r.reconciliacao.unwrap();
    assert_eq!(rec.arquivos_ausentes, 1);
    assert_eq!(
        estado(&cat, &raiz.join("sub/c.bin")).as_deref(),
        Some("ausente")
    );
    assert_eq!(
        estado(&cat, &raiz.join("d/f.txt")).as_deref(),
        Some("ativo")
    );
    let pai: Option<i64> = cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT parent_folder_id FROM folders WHERE path_raw = ?1",
            [caminho(&raiz.join("sub"))],
            |l| l.get(0),
        )
        .unwrap();
    assert!(
        pai.is_some(),
        "a pasta raiz do Fast Scan continua pendurada na raiz maior"
    );
}

#[test]
fn config_miuda_e_concorrencia_dao_o_mesmo_resultado() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    criar_arvore(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    full_com_hash(&raiz, &mut cat);
    fs::rename(raiz.join("t1/m2"), raiz.join("t1/m2_renomeada")).unwrap();
    fs::remove_file(raiz.join("t2/a1.bin")).unwrap();
    escrever(&raiz.join("t4/novo.dat"), 123);
    let r = FastScan::novo(vec![raiz.clone()], vec![volume_de(&raiz)])
        .com_config(ConfigScan {
            concorrencia: 4,
            ..config_miuda()
        })
        .executar(&mut cat)
        .unwrap();
    let rec = r.reconciliacao.unwrap();
    assert_eq!(rec.arquivos_ausentes, 1, "{rec:?}");
    assert_eq!(rec.arquivos_novos, 1, "{rec:?}");
    assert_eq!(rec.pastas_renomeadas, 6, "m2 + 5 folhas: {rec:?}");
    assert_eq!(rec.arquivos_renomeados, 4 + 5 * 8, "{rec:?}");
    assert_eq!(ausentes(&cat), 1);
}
