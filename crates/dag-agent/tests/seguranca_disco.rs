//! Teste estático de segurança (ADR 0005): o código de produção do workspace só muda o disco
//! por uma porta, `dag-agent/src/lixeira/fs.rs` (`renameat2` com `RENAME_NOREPLACE` e `mkdir`),
//! e as outras escritas são só em arquivos do próprio DAG (socket, trava, relatório, unidade
//! de serviço, atalho, auditoria), nunca em arquivos catalogados. Se alguém acrescentar uma
//! chamada que apaga, move, sobrescreve ou trunca, este teste falha e obriga a revisão.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};

/// Chamadas que alteram o disco (ou fogem das garantias de tipo do Rust).
const PROIBIDOS: &[&str] = &[
    "remove_file",
    "remove_dir",
    "unlinkat",
    "rmdir",
    "fs::rename",
    "renameat",
    "rename(",
    "set_len",
    "ftruncate",
    "fs::write",
    "fs::copy",
    "fs::hard_link",
    "fs::symlink(",
    "File::create",
    "create_new",
    ".create(true)",
    ".truncate(true)",
    ".write(true)",
    ".append(true)",
    "libc::",
    "unsafe ",
    "unsafe{",
    "create_dir",
    "DirBuilder",
    "persist(",
];

/// Escritas permitidas, por arquivo: só dados do próprio DAG.
const PERMITIDOS: &[(&str, &[&str])] = &[
    // Pasta do Catálogo e arquivo de trava do agente.
    (
        "dag-agent/src/caminho.rs",
        &[".create(true)", ".write(true)", "create_dir", "DirBuilder"],
    ),
    (
        "dag-agent/src/trava.rs",
        &[".create(true)", ".write(true)", "set_len"],
    ),
    // Log do agente destacado.
    (
        "dag-agent/src/lancar.rs",
        &[".create(true)", ".write(true)", ".append(true)"],
    ),
    // Socket do próprio agente.
    ("dag-agent/src/servidor.rs", &["remove_file", "DirBuilder"]),
    // Trilha de auditoria do helper (append).
    ("dag-privileged-helper/src/auditoria.rs", &[".append(true)"]),
    // Unidade de serviço e atalho de área de trabalho do DAG.
    (
        "dag-platform-linux/src/servico/gerenciador.rs",
        &[
            ".write(true)",
            "DirBuilder",
            "fs::rename",
            "rename(",
            "create_dir",
            "create_new",
            "remove_file",
            "fs::write",
        ],
    ),
    (
        "dag-platform-linux/src/atalho.rs",
        &["create_dir", "remove_file", "fs::write"],
    ),
    // Relatório CSV/JSON exportado pelo usuário: cria arquivo novo, nunca sobrescreve.
    (
        "dag-store/src/duplicados/exportar.rs",
        &[".write(true)", "create_new", "remove_file"],
    ),
    // Saídas pedidas pelo usuário na CLI (relatório de benchmark, casos do conjunto ouro):
    // arquivo novo, nunca sobrescreve.
    ("dag-cli/src/escrita.rs", &[".write(true)", "create_new"]),
    // Pasta nova e vazia da biblioteca A Classificar (ADR 0008): só cria quando não existe.
    ("dag-calibre/src/calibredb.rs", &["create_dir"]),
    // `fs.rs` da Lixeira é a única porta de mover; ver a verificação dedicada abaixo.
    ("dag-agent/src/lixeira/fs.rs", &["renameat", "create_dir"]),
];

/// Pasta `testes/` declarada como `#[cfg(test)] mod testes;` no módulo pai: só testes.
fn pasta_so_de_testes(p: &Path) -> bool {
    if p.file_name().is_none_or(|n| n != "testes") {
        return false;
    }
    let pai = p.parent().unwrap();
    ["lib.rs", "mod.rs", "main.rs"].iter().any(|m| {
        fs::read_to_string(pai.join(m)).is_ok_and(|t| t.contains("#[cfg(test)]\nmod testes;"))
    })
}

fn arquivos_rs(dir: &Path, saida: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            if pasta_so_de_testes(&p) {
                continue;
            }
            arquivos_rs(&p, saida);
        } else if p.extension().is_some_and(|x| x == "rs") {
            saida.push(p);
        }
    }
}

/// Código de produção do arquivo: sem comentários de linha e sem o módulo de testes.
fn producao(texto: &str) -> Vec<(usize, String)> {
    let mut v = Vec::new();
    for (i, l) in texto.lines().enumerate() {
        if l.trim() == "#[cfg(test)]" {
            break;
        }
        let t = l.trim_start();
        if t.starts_with("//") {
            continue;
        }
        v.push((i + 1, l.to_string()));
    }
    v
}

fn raiz_do_workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn so_a_porta_da_lixeira_move_arquivos_e_nada_apaga() {
    let raiz = raiz_do_workspace();
    let mut arquivos = Vec::new();
    for c in fs::read_dir(&raiz).unwrap() {
        let src = c.unwrap().path().join("src");
        if src.is_dir() {
            arquivos_rs(&src, &mut arquivos);
        }
    }
    assert!(arquivos.len() > 50, "varredura do workspace falhou");
    let mut violacoes = Vec::new();
    for a in &arquivos {
        let rel = a
            .strip_prefix(&raiz)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let permitidos: &[&str] = PERMITIDOS
            .iter()
            .find(|(f, _)| *f == rel)
            .map_or(&[], |(_, p)| *p);
        for (n, l) in producao(&fs::read_to_string(a).unwrap()) {
            for p in PROIBIDOS {
                if l.contains(p) && !permitidos.contains(p) {
                    violacoes.push(format!("{rel}:{n}: `{p}` em `{}`", l.trim()));
                }
            }
        }
    }
    assert!(
        violacoes.is_empty(),
        "chamadas que alteram o disco fora da lista revisada (ADR 0005):\n{}",
        violacoes.join("\n")
    );
}

#[test]
fn a_porta_da_lixeira_so_renomeia_sem_substituir() {
    let fonte =
        fs::read_to_string(raiz_do_workspace().join("dag-agent/src/lixeira/fs.rs")).unwrap();
    let prod = producao(&fonte);
    let corpo: String = prod.iter().map(|(_, l)| format!("{l}\n")).collect();
    assert!(corpo.contains("RenameFlags::NOREPLACE"));
    for (n, l) in &prod {
        if l.contains("renameat") && !l.contains("use rustix") {
            assert!(
                l.contains("renameat_with"),
                "fs.rs:{n}: só renameat_with, com flags"
            );
        }
    }
    // Nada que substitua, apague, escreva ou trunque.
    for proibido in [
        "RenameFlags::EXCHANGE",
        "RenameFlags::WHITEOUT",
        "remove_",
        "unlink",
        "set_len",
        "truncate",
        "fs::write",
        "File::create",
        "OpenOptions",
        "fs::copy",
        "fs::hard_link",
        "symlink(",
        "libc::",
        "unsafe",
        "create_dir_all",
        "fs::rename",
    ] {
        assert!(
            !corpo.contains(proibido),
            "fs.rs não pode conter `{proibido}`"
        );
    }
}

#[test]
fn o_workspace_proibe_unsafe() {
    let cargo =
        fs::read_to_string(raiz_do_workspace().parent().unwrap().join("Cargo.toml")).unwrap();
    assert!(
        cargo.contains("unsafe_code"),
        "lint unsafe_code ausente no Cargo.toml do workspace"
    );
}
