//! Testes com montagens reais (tmpfs, bind). Exigem poder montar (root em container); sem isso
//! são pulados com aviso. Toda montagem é desfeita antes de o diretório temporário ser removido;
//! se o `umount` falhar, o diretório é deixado para trás de propósito (nunca apagar através de
//! um bind mount).
#![allow(clippy::unwrap_used)]

mod comum;

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use comum::*;
use dag_scanner::{DesfechoScan, FullScan};
use dag_store::Catalogo;
use dag_store::inventario::consulta::listar_problemas;
use tempfile::TempDir;

struct Cenario {
    dir: Option<TempDir>,
    montagens: Vec<PathBuf>,
}

impl Cenario {
    fn novo() -> Self {
        Self {
            dir: Some(tempfile::tempdir().unwrap()),
            montagens: Vec::new(),
        }
    }

    fn caminho(&self) -> &Path {
        self.dir.as_ref().unwrap().path()
    }

    /// `false` se o ambiente não permite montar.
    fn montar(&mut self, args: &[&str], destino: &Path) -> bool {
        let ok = Command::new("mount")
            .args(args)
            .arg(destino)
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            self.montagens.push(destino.to_path_buf());
        }
        ok
    }
}

impl Drop for Cenario {
    fn drop(&mut self) {
        let mut tudo_ok = true;
        for m in self.montagens.iter().rev() {
            let ok = Command::new("umount")
                .arg("-l")
                .arg(m)
                .status()
                .is_ok_and(|s| s.success());
            tudo_ok &= ok;
        }
        if !tudo_ok {
            // Não arriscar remover conteúdo real através de uma montagem que ficou.
            if let Some(d) = self.dir.take() {
                let _ = d.keep();
            }
        }
    }
}

fn dev(p: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(p).unwrap().dev()
}

fn volume_dev(p: &Path, id: &str, pseudo: bool) -> dag_core::VolumeDescoberto {
    let mut v = volume_de(p);
    v.platform_id = id.into();
    v.pseudo = pseudo;
    v
}

#[test]
fn fronteira_de_volume_nao_e_cruzada_e_pseudo_e_decidido_pelo_flag() {
    let mut c = Cenario::novo();
    let raiz = c.caminho().join("raiz");
    escrever(&raiz.join("normal/a.txt"), 10);
    let pm = raiz.join("montado");
    fs::create_dir_all(&pm).unwrap();
    if !c.montar(&["-t", "tmpfs", "tmpfs"], &pm) {
        eprintln!("PULADO: sem permissão para montar tmpfs");
        return;
    }
    escrever(&pm.join("dentro.txt"), 99);
    let dev_raiz = dev(&raiz);
    let dev_mont = dev(&pm);
    assert_ne!(dev_raiz, dev_mont);

    // Caso 1: o outro volume é conhecido e real, mas não é raiz selecionada.
    let vols = vec![
        volume_dev(&raiz, "vol-raiz", false),
        volume_dev(&pm, "vol-tmpfs", false),
    ];
    let mut cat = Catalogo::em_memoria().unwrap();
    let r = FullScan::novo(vec![raiz.clone()], vols.clone())
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r.desfecho, DesfechoScan::Concluida);
    let f = foto(&cat);
    assert!(
        !f.arquivos.keys().any(|p| p.ends_with(b"dentro.txt")),
        "não cruzou a montagem"
    );
    assert!(
        f.pastas.contains_key(pm.as_os_str().as_bytes()),
        "a pasta de montagem é registrada"
    );
    let ign = listar_problemas(cat.armazem().conexao(), r.execucao_id, 0, 10, Some(true)).unwrap();
    assert_eq!(ign.len(), 1);
    assert_eq!(ign[0].codigo, "limite_de_volume");
    assert_eq!(ign[0].path_raw.as_deref(), Some(pm.as_os_str().as_bytes()));
    assert!(
        ign[0]
            .motivo
            .as_deref()
            .unwrap()
            .contains("não selecionado")
    );
    assert_eq!(n(&cat, "SELECT COUNT(*) FROM volumes"), 2);
    let attrs: String = cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT attrs FROM folders WHERE name_display = 'montado'",
            [],
            |l| l.get(0),
        )
        .unwrap();
    assert!(attrs.contains("\"limite_de_volume\":true"));

    // Caso 2: o outro volume também é raiz: cada um é varrido no seu volume, sem duplicar.
    let mut cat = Catalogo::em_memoria().unwrap();
    let r = FullScan::novo(vec![raiz.clone(), pm.clone()], vols)
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r.desfecho, DesfechoScan::Concluida);
    let con = cat.armazem().conexao();
    let vol_tmpfs: i64 = con
        .query_row(
            "SELECT id FROM volumes WHERE platform_id = 'vol-tmpfs'",
            [],
            |l| l.get(0),
        )
        .unwrap();
    let dentro: i64 = con
        .query_row(
            "SELECT COUNT(*) FROM files WHERE name_display = 'dentro.txt' AND volume_id = ?1",
            [vol_tmpfs],
            |l| l.get(0),
        )
        .unwrap();
    assert_eq!(dentro, 1);
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM files WHERE name_display = 'dentro.txt'"
        ),
        1
    );
    let ign = listar_problemas(con, r.execucao_id, 0, 10, Some(true)).unwrap();
    assert!(
        ign[0]
            .motivo
            .as_deref()
            .unwrap()
            .contains("selecionado como raiz")
    );
    assert_eq!(n(&cat, "SELECT COUNT(*) FROM volume_stats"), 2);

    // Caso 3: o outro volume é pseudo (pelo flag): ignorado e nem registrado como volume.
    let vols = vec![
        volume_dev(&raiz, "vol-raiz", false),
        volume_dev(&pm, "vol-pseudo", true),
    ];
    let mut cat = Catalogo::em_memoria().unwrap();
    let r = FullScan::novo(vec![raiz.clone()], vols)
        .executar(&mut cat)
        .unwrap();
    let ign = listar_problemas(cat.armazem().conexao(), r.execucao_id, 0, 10, Some(true)).unwrap();
    assert_eq!(ign[0].codigo, "volume_pseudo");
    assert_eq!(n(&cat, "SELECT COUNT(*) FROM volumes"), 1);
    assert!(
        !foto(&cat)
            .arquivos
            .keys()
            .any(|p| p.ends_with(b"dentro.txt"))
    );
    assert!(
        foto(&cat).arquivos.keys().any(|p| p.ends_with(b"a.txt")),
        "pasta normal continua sendo varrida"
    );

    // Caso 4: st_dev que a descoberta não conhece.
    let mut cat = Catalogo::em_memoria().unwrap();
    let r = FullScan::novo(
        vec![raiz.clone()],
        vec![volume_dev(&raiz, "vol-raiz", false)],
    )
    .executar(&mut cat)
    .unwrap();
    let ign = listar_problemas(cat.armazem().conexao(), r.execucao_id, 0, 10, Some(true)).unwrap();
    assert_eq!(ign[0].codigo, "volume_desconhecido");
}

#[test]
fn bind_mount_alias_e_bind_de_ancestral_nao_geram_laco_nem_contagem_dupla() {
    let mut c = Cenario::novo();
    let raiz = c.caminho().join("raiz");
    escrever(&raiz.join("a/f1.txt"), 10);
    escrever(&raiz.join("a/f2.txt"), 20);
    fs::create_dir_all(raiz.join("alias")).unwrap();
    fs::create_dir_all(raiz.join("a/laco")).unwrap();
    let ok1 = Command::new("mount")
        .arg("--bind")
        .arg(raiz.join("a"))
        .arg(raiz.join("alias"))
        .status()
        .is_ok_and(|s| s.success());
    if !ok1 {
        eprintln!("PULADO: sem permissão para bind mount");
        return;
    }
    c.montagens.push(raiz.join("alias"));
    // Bind do próprio ancestral dentro de si: ciclo clássico.
    let ok2 = Command::new("mount")
        .arg("--bind")
        .arg(&raiz)
        .arg(raiz.join("a/laco"))
        .status()
        .is_ok_and(|s| s.success());
    if ok2 {
        c.montagens.push(raiz.join("a/laco"));
    }

    let mut cat = Catalogo::em_memoria().unwrap();
    let r = scan(&raiz).executar(&mut cat).unwrap();
    assert_eq!(r.desfecho, DesfechoScan::Concluida);
    // f1 e f2 aparecem uma única vez, por um dos dois caminhos (a ou alias).
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM files WHERE name_display = 'f1.txt'"
        ),
        1
    );
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM files WHERE name_display = 'f2.txt'"
        ),
        1
    );
    assert_eq!(r.contadores.arquivos, 2);
    let ign = listar_problemas(cat.armazem().conexao(), r.execucao_id, 0, 10, Some(true)).unwrap();
    assert!(
        ign.iter().any(|p| p.codigo == "diretorio_duplicado"),
        "{ign:?}"
    );
    // O bind do ancestral só é alcançado se `a` for visitado antes de `alias`; em qualquer caso termina.
    assert!(!ign.is_empty());
    let _ = ok2;
    // O total de bytes não conta nada duas vezes.
    let s = estatisticas_raiz(&cat);
    assert_eq!(s, 30);
}

fn estatisticas_raiz(c: &Catalogo) -> i64 {
    n(
        c,
        "SELECT s.logical_bytes FROM folder_stats s JOIN folders f ON f.id = s.folder_id WHERE f.parent_folder_id IS NULL",
    )
}
