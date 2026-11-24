//! Reteste único do fim da etapa de Hash Completo e desfecho `concluida_com_avisos`.
#![allow(clippy::unwrap_used)]

mod comum;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use comum::*;
use dag_core::ExecucaoId;
use dag_scanner::{Desfecho, EtapaHash};
use dag_store::Catalogo;

fn gravar(p: &Path, dados: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, dados).unwrap();
}

fn estado(cat: &Catalogo, run: i64) -> String {
    cat.armazem()
        .conexao()
        .query_row("SELECT status FROM scan_runs WHERE id = ?1", [run], |l| {
            l.get(0)
        })
        .unwrap()
}

/// Acrescenta bytes ao arquivo `alvo` na primeira vez (ou em toda leitura, se `sempre`).
fn mexer(alvo: PathBuf, sempre: bool) -> impl Fn(&Path) + Send + Sync + 'static {
    let n = Mutex::new(0u32);
    move |p: &Path| {
        if p != alvo {
            return;
        }
        let mut v = n.lock().unwrap();
        *v += 1;
        if sempre || *v == 1 {
            let mut f = OpenOptions::new().append(true).open(p).unwrap();
            f.write_all(b"mais").unwrap();
        }
    }
}

#[test]
fn vivo_que_se_acalma_e_hasheado_no_reteste_sem_nenhum_erro() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    let a = raiz.join("a.log");
    gravar(&a, &[7u8; 50_000]);
    gravar(&raiz.join("b.bin"), &[9u8; 1000]);
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = scan(&raiz).executar(&mut cat).unwrap().execucao_id;
    let r = EtapaHash::nova(ExecucaoId(run))
        .com_gancho_apos_leitura(mexer(a.clone(), false))
        .executar(&mut cat)
        .unwrap();
    // Mudou na 1ª leitura, estável na 2ª (o reteste): hash gravado e problema removido.
    assert_eq!(r.desfecho, Desfecho::Concluida);
    assert_eq!((r.hasheados, r.erros), (2, 0));
    assert_eq!(n(&cat, "SELECT COUNT(*) FROM hashes"), 2);
    assert_eq!(n(&cat, "SELECT COUNT(*) FROM scan_errors"), 0);
    // O inventário da linha ganhou o tamanho de agora.
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM files WHERE path_raw LIKE '%a.log' AND logical_bytes = 50004"
        ),
        1
    );
    assert_eq!(estado(&cat, run), "concluida");
}

#[test]
fn vivo_que_continua_mudando_vira_esperado_e_so_avisos() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    let a = raiz.join("a.log");
    gravar(&a, &[7u8; 50_000]);
    gravar(&raiz.join("b.bin"), &[9u8; 1000]);
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = scan(&raiz).executar(&mut cat).unwrap().execucao_id;
    let r = EtapaHash::nova(ExecucaoId(run))
        .com_gancho_apos_leitura(mexer(a.clone(), true))
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r.desfecho, Desfecho::ConcluidaComAvisos);
    assert_eq!((r.hasheados, r.erros), (1, 1));
    assert_eq!(n(&cat, "SELECT COUNT(*) FROM hashes"), 1);
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_errors WHERE classe = 'esperado' AND regra = 'embutida:reteste'"
        ),
        1
    );
    assert_eq!(estado(&cat, run), "concluida_com_avisos");
}

#[test]
fn permissao_negada_no_hash_e_so_aviso() {
    // Como root o kernel ignora o modo 000.
    if fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0) == 0 {
        eprintln!("PULADO: executando como root");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    gravar(&raiz.join("ok"), &[1u8; 1000]);
    gravar(&raiz.join("trancado"), &[2u8; 1000]);
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = scan(&raiz).executar(&mut cat).unwrap().execucao_id;
    fs::set_permissions(raiz.join("trancado"), fs::Permissions::from_mode(0o000)).unwrap();
    let r = EtapaHash::nova(ExecucaoId(run)).executar(&mut cat).unwrap();
    fs::set_permissions(raiz.join("trancado"), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(r.desfecho, Desfecho::ConcluidaComAvisos);
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_errors WHERE error_code = 'hash_permissao' AND severity = 'aviso'"
        ),
        1
    );
}
