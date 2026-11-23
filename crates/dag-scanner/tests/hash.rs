//! Etapa de Hash Completo e Verificação de Integridade (A-23, A-24, A-33, CA-A18, CA-A19,
//! CA-A25, CA-A26, CA-A29).
#![allow(clippy::unwrap_used)]

mod comum;

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use comum::*;
use dag_core::ExecucaoId;
use dag_scanner::{ConfigHash, ControleScan, Desfecho, EtapaHash, VerificacaoIntegridade};
use dag_store::Catalogo;

fn conteudo(semente: u64, tamanho: usize) -> Vec<u8> {
    let mut x = semente.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..tamanho)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 32) as u8
        })
        .collect()
}

fn gravar(p: &Path, dados: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, dados).unwrap();
}

fn varrer(raiz: &Path, cat: &mut Catalogo) -> i64 {
    scan(raiz).executar(cat).unwrap().execucao_id
}

fn hashes_por_caminho(cat: &Catalogo) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let con = cat.armazem().conexao();
    let mut st = con
        .prepare("SELECT f.path_raw, h.hash FROM hashes h JOIN files f ON f.id = h.file_id")
        .unwrap();
    st.query_map([], |l| Ok((l.get(0)?, l.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn bytes(p: &Path) -> Vec<u8> {
    p.as_os_str().to_str().unwrap().as_bytes().to_vec()
}

fn arvore(raiz: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut v = Vec::new();
    for (i, tam) in [
        0usize,
        1,
        100,
        4095,
        4096,
        65_537,
        1 << 20,
        (1 << 20) + 13,
        3_000_000,
    ]
    .into_iter()
    .enumerate()
    {
        let p = raiz.join(format!("d{}/arq{i}.bin", i % 3));
        let c = conteudo(i as u64 + 1, tam);
        gravar(&p, &c);
        v.push((p, c));
    }
    v
}

#[test]
fn hashes_batem_com_blake3_e_com_b3sum() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    let arqs = arvore(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = varrer(&raiz, &mut cat);
    let r = EtapaHash::nova(ExecucaoId(run)).executar(&mut cat).unwrap();
    assert_eq!(r.desfecho, Desfecho::Concluida);
    assert_eq!(r.hasheados, arqs.len() as u64);
    assert_eq!(r.erros, 0);
    let hs = hashes_por_caminho(&cat);
    assert_eq!(hs.len(), arqs.len());
    for (p, c) in &arqs {
        assert_eq!(hs[&bytes(p)], blake3::hash(c).as_bytes().to_vec(), "{p:?}");
    }
    // Referência externa, se existir.
    if let Ok(out) = std::process::Command::new("b3sum")
        .arg("--no-names")
        .arg(&arqs[7].0)
        .output()
    {
        let hex: String = hs[&bytes(&arqs[7].0)]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), hex);
    }
    // Estado do banco: fase restaurada, contadores, fila concluída, hashed_at e evidências.
    let fase: String = cat
        .armazem()
        .conexao()
        .query_row("SELECT phase FROM scan_runs WHERE id = ?1", [run], |l| {
            l.get(0)
        })
        .unwrap();
    assert_eq!(fase, "concluida");
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_runs WHERE json_extract(counters,'$.hash.hasheados') = 9"
        ),
        1
    );
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM fila_hash WHERE state <> 'concluido'"
        ),
        0
    );
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM hashes h JOIN files f ON f.id = h.file_id WHERE h.size_bytes = f.logical_bytes AND h.mtime_ns = f.mtime_ns AND h.ctime_ns = f.ctime_ns"
        ),
        9
    );
}

#[test]
fn hard_links_sao_lidos_uma_vez() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    let grande = conteudo(7, 3 << 20);
    gravar(&raiz.join("a/orig.bin"), &grande);
    fs::hard_link(raiz.join("a/orig.bin"), raiz.join("b/link1.bin")).unwrap_or_else(|_| {
        fs::create_dir_all(raiz.join("b")).unwrap();
        fs::hard_link(raiz.join("a/orig.bin"), raiz.join("b/link1.bin")).unwrap();
    });
    fs::hard_link(raiz.join("a/orig.bin"), raiz.join("a/link2.bin")).unwrap();
    let solto = conteudo(8, 1 << 20);
    gravar(&raiz.join("a/solto.bin"), &solto);
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = varrer(&raiz, &mut cat);
    let r = EtapaHash::nova(ExecucaoId(run)).executar(&mut cat).unwrap();
    assert_eq!(
        r.bytes_lidos,
        (3 << 20) + (1 << 20),
        "o arquivo físico é lido uma vez"
    );
    assert_eq!(r.hasheados, 4, "as quatro linhas recebem hash");
    let hs = hashes_por_caminho(&cat);
    let esperado = blake3::hash(&grande).as_bytes().to_vec();
    for p in ["a/orig.bin", "b/link1.bin", "a/link2.bin"] {
        assert_eq!(hs[&bytes(&raiz.join(p))], esperado, "{p}");
    }
    // Segunda passada: nada a ler.
    let r2 = EtapaHash::nova(ExecucaoId(run)).executar(&mut cat).unwrap();
    assert_eq!((r2.bytes_lidos, r2.hasheados, r2.reaproveitados), (0, 0, 4));
}

#[test]
fn segunda_etapa_em_arvore_inalterada_reaproveita_tudo() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    let arqs = arvore(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    let run1 = varrer(&raiz, &mut cat);
    let r1 = EtapaHash::nova(ExecucaoId(run1))
        .executar(&mut cat)
        .unwrap();
    assert!(r1.bytes_lidos > 0);
    let antes = hashes_por_caminho(&cat);
    // Nova varredura (outra execução) sem mudanças.
    let run2 = varrer(&raiz, &mut cat);
    assert_ne!(run1, run2);
    let ultimo = Arc::new(AtomicU64::new(0));
    let u = Arc::clone(&ultimo);
    let r2 = EtapaHash::nova(ExecucaoId(run2))
        .com_progresso(move |p| u.store(p.reaproveitados, Ordering::SeqCst))
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r2.bytes_lidos, 0);
    assert_eq!(r2.hasheados, 0);
    assert_eq!(r2.reaproveitados, arqs.len() as u64);
    assert_eq!(ultimo.load(Ordering::SeqCst), arqs.len() as u64);
    assert_eq!(hashes_por_caminho(&cat), antes);
}

#[test]
fn arquivo_modificado_e_varrido_de_novo_e_relido() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    let arqs = arvore(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    let run1 = varrer(&raiz, &mut cat);
    EtapaHash::nova(ExecucaoId(run1))
        .executar(&mut cat)
        .unwrap();
    // Mesmo tamanho, conteúdo diferente, mtime diferente.
    let alvo = &arqs[5].0;
    let novo = conteudo(999, arqs[5].1.len());
    std::thread::sleep(Duration::from_millis(20));
    fs::write(alvo, &novo).unwrap();
    let run2 = varrer(&raiz, &mut cat);
    let r = EtapaHash::nova(ExecucaoId(run2))
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r.hasheados, 1);
    assert_eq!(r.reaproveitados, arqs.len() as u64 - 1);
    assert_eq!(r.bytes_lidos, novo.len() as u64);
    let hs = hashes_por_caminho(&cat);
    assert_eq!(hs[&bytes(alvo)], blake3::hash(&novo).as_bytes().to_vec());
}

#[test]
fn cancelar_e_retomar_nao_rele_arquivos_concluidos() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    let total = 300usize;
    let tam = 256 << 10;
    let mut esperados = Vec::new();
    for i in 0..total {
        let c = conteudo(i as u64 + 100, tam);
        let p = raiz.join(format!("p{}/f{i}.bin", i % 10));
        gravar(&p, &c);
        esperados.push((p, blake3::hash(&c).as_bytes().to_vec()));
    }
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = varrer(&raiz, &mut cat);
    let cfg = ConfigHash {
        concorrencia: 2,
        tamanho_buffer: 64 << 10,
        intervalo_de_progresso: Duration::ZERO,
        ..ConfigHash::default()
    };
    let ctl = ControleScan::novo();
    let c2 = ctl.clone();
    let r1 = EtapaHash::nova(ExecucaoId(run))
        .com_config(cfg.clone())
        .com_controle(ctl)
        .com_progresso(move |p| {
            if p.arquivos_feitos >= 60 {
                c2.cancelar();
            }
        })
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r1.desfecho, Desfecho::Cancelada);
    let feitos = hashes_por_caminho(&cat).len() as u64;
    assert!((60..total as u64).contains(&feitos), "feitos = {feitos}");
    assert_eq!(r1.hasheados, feitos, "nenhum hash parcial gravado");
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM fila_hash WHERE state = 'em_andamento'"
        ),
        0
    );
    let fase: String = cat
        .armazem()
        .conexao()
        .query_row("SELECT phase FROM scan_runs WHERE id = ?1", [run], |l| {
            l.get(0)
        })
        .unwrap();
    assert_eq!(fase, "hash", "cancelada fica na fase hash");
    // Retoma.
    let r2 = EtapaHash::nova(ExecucaoId(run))
        .com_config(cfg)
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r2.desfecho, Desfecho::Concluida);
    assert_eq!(r2.reaproveitados, feitos, "concluídos não são relidos");
    assert_eq!(r2.hasheados, total as u64 - feitos);
    assert_eq!(r2.bytes_lidos, (total as u64 - feitos) * tam as u64);
    // Leituras da 1ª rodada além dos arquivos concluídos: só as em voo (leitores).
    let extra = r1.bytes_lidos - feitos * tam as u64;
    assert!(extra <= 2 * tam as u64, "leituras descartadas: {extra}");
    let hs = hashes_por_caminho(&cat);
    assert_eq!(hs.len(), total);
    for (p, h) in &esperados {
        assert_eq!(&hs[&bytes(p)], h);
    }
}

#[test]
fn alterado_durante_a_leitura_nao_e_gravado() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    let a = raiz.join("a.bin");
    let b = raiz.join("b.bin");
    let ok = raiz.join("ok.bin");
    gravar(&a, &conteudo(1, 100_000));
    gravar(&b, &conteudo(2, 100_000));
    gravar(&ok, &conteudo(3, 100_000));
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = varrer(&raiz, &mut cat);
    let (pa, pb) = (a.clone(), b.clone());
    let r = EtapaHash::nova(ExecucaoId(run))
        .com_gancho_apos_leitura(move |p| {
            if p == pa {
                // Acrescenta bytes: tamanho muda.
                let mut f = OpenOptions::new().append(true).open(p).unwrap();
                f.write_all(b"mais").unwrap();
            } else if p == pb {
                // Mesmo tamanho, outra data.
                let f = OpenOptions::new().write(true).open(p).unwrap();
                f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_500_000_000))
                    .unwrap();
            }
        })
        .executar(&mut cat)
        .unwrap();
    assert_eq!(r.desfecho, Desfecho::ConcluidaComErros);
    assert_eq!((r.hasheados, r.erros), (1, 2));
    let hs = hashes_por_caminho(&cat);
    assert_eq!(hs.len(), 1);
    assert!(hs.contains_key(&bytes(&ok)));
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_errors WHERE phase = 'hash' AND error_code = 'hash_alterado_durante_leitura' AND reason LIKE '%alterado durante leitura%'"
        ),
        2
    );
    // Uma nova varredura + etapa grava os dois arquivos (agora estáveis).
    let run2 = varrer(&raiz, &mut cat);
    let r2 = EtapaHash::nova(ExecucaoId(run2))
        .executar(&mut cat)
        .unwrap();
    assert_eq!((r2.hasheados, r2.erros, r2.reaproveitados), (2, 0, 1));
}

#[test]
fn arquivo_que_sumiu_vira_erro_e_o_resto_continua() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    for i in 0..5 {
        gravar(&raiz.join(format!("f{i}")), &conteudo(i, 5000));
    }
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = varrer(&raiz, &mut cat);
    fs::remove_file(raiz.join("f2")).unwrap();
    let r = EtapaHash::nova(ExecucaoId(run)).executar(&mut cat).unwrap();
    assert_eq!((r.hasheados, r.erros), (4, 1));
    assert_eq!(r.desfecho, Desfecho::ConcluidaComErros);
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_errors WHERE error_code = 'hash_ausente'"
        ),
        1
    );
}

#[test]
fn hash_nao_altera_tamanho_nem_datas() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    criar_arvore(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = varrer(&raiz, &mut cat);
    let antes = percorrer_fs(&raiz);
    let r = EtapaHash::nova(ExecucaoId(run)).executar(&mut cat).unwrap();
    assert!(r.hasheados > 2000);
    let depois = percorrer_fs(&raiz);
    assert_eq!(antes, depois, "tamanho, mtime, ctime e inode intactos");
    VerificacaoIntegridade::nova().executar(&mut cat).unwrap();
    assert_eq!(antes, percorrer_fs(&raiz));
}

#[test]
fn limite_de_bytes_por_segundo_e_respeitado() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    gravar(&raiz.join("x.bin"), &conteudo(1, 2 << 20));
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = varrer(&raiz, &mut cat);
    let t = Instant::now();
    EtapaHash::nova(ExecucaoId(run))
        .com_config(ConfigHash {
            limite_bytes_por_seg: Some(4 << 20),
            ..ConfigHash::default()
        })
        .executar(&mut cat)
        .unwrap();
    assert!(
        t.elapsed() >= Duration::from_millis(400),
        "{:?}",
        t.elapsed()
    );
}

// ------------------------------------------------------------------ verificação de integridade

#[test]
fn verificacao_detecta_byte_corrompido_com_mtime_restaurado() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    let arqs = arvore(&raiz);
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = varrer(&raiz, &mut cat);
    EtapaHash::nova(ExecucaoId(run)).executar(&mut cat).unwrap();
    let guardados = hashes_por_caminho(&cat);

    // Rodada limpa: tudo ok.
    let v0 = VerificacaoIntegridade::nova().executar(&mut cat).unwrap();
    assert_eq!(v0.desfecho, Desfecho::Concluida);
    assert_eq!(v0.verificados, arqs.len() as u64);
    assert!(v0.divergentes.is_empty());
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM hashes WHERE verification_state = 'ok' AND verified_at IS NOT NULL"
        ),
        arqs.len() as i64
    );

    // Corrompe um byte e restaura o mtime.
    let alvo = &arqs[6].0;
    let mtime = fs::metadata(alvo).unwrap().modified().unwrap();
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(alvo)
        .unwrap();
    f.seek(SeekFrom::Start(1000)).unwrap();
    let mut um = [0u8; 1];
    f.read_exact(&mut um).unwrap();
    f.seek(SeekFrom::Start(1000)).unwrap();
    f.write_all(&[um[0] ^ 0x01]).unwrap();
    f.set_modified(mtime).unwrap();
    drop(f);
    let corrompido = fs::read(alvo).unwrap();
    let meta_antes = fs::metadata(alvo).unwrap();

    // Um arquivo legitimamente alterado (mtime novo) não é "divergente".
    let legit = &arqs[4].0;
    std::thread::sleep(Duration::from_millis(20));
    fs::write(legit, conteudo(4242, arqs[4].1.len())).unwrap();

    let v = VerificacaoIntegridade::nova().executar(&mut cat).unwrap();
    assert_eq!(v.desfecho, Desfecho::ConcluidaComErros);
    assert_eq!(v.divergentes.len(), 1);
    assert_eq!(v.alterados, 1);
    assert_eq!(v.verificados, arqs.len() as u64 - 1);
    let id: i64 = cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT id FROM files WHERE path_raw = ?1",
            [bytes(alvo)],
            |l| l.get(0),
        )
        .unwrap();
    assert_eq!(v.divergentes[0].0, id);
    let estado: String = cat
        .armazem()
        .conexao()
        .query_row(
            "SELECT verification_state FROM hashes WHERE file_id = ?1",
            [id],
            |l| l.get(0),
        )
        .unwrap();
    assert_eq!(estado, "divergente");
    // O hash guardado não foi sobrescrito e o arquivo continua como estava.
    assert_eq!(
        hashes_por_caminho(&cat)[&bytes(alvo)],
        guardados[&bytes(alvo)]
    );
    assert_ne!(
        guardados[&bytes(alvo)],
        blake3::hash(&corrompido).as_bytes().to_vec()
    );
    assert_eq!(fs::read(alvo).unwrap(), corrompido);
    let meta_depois = fs::metadata(alvo).unwrap();
    assert_eq!(meta_antes.len(), meta_depois.len());
    assert_eq!(
        meta_antes.modified().unwrap(),
        meta_depois.modified().unwrap()
    );
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_errors WHERE error_code = 'verificacao_divergente'"
        ),
        1
    );
    // Execução própria, modo verificacao, concluída.
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM scan_runs WHERE mode = 'verificacao' AND phase = 'concluida'"
        ),
        2
    );
}

#[test]
fn verificacao_por_raiz_e_hard_link_uma_leitura() {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = tmp.path().join("acervo");
    gravar(&raiz.join("a/x.bin"), &conteudo(1, 1 << 20));
    fs::create_dir_all(raiz.join("b")).unwrap();
    fs::hard_link(raiz.join("a/x.bin"), raiz.join("b/y.bin")).unwrap();
    gravar(&raiz.join("c/z.bin"), &conteudo(2, 1 << 20));
    let mut cat = Catalogo::em_memoria().unwrap();
    let run = varrer(&raiz, &mut cat);
    EtapaHash::nova(ExecucaoId(run)).executar(&mut cat).unwrap();
    let v = VerificacaoIntegridade::nova()
        .raizes([raiz.join("a"), raiz.join("b")])
        .executar(&mut cat)
        .unwrap();
    assert_eq!(v.bytes_lidos, 1 << 20, "hard link lido uma vez");
    assert_eq!(v.verificados, 2);
    assert_eq!(
        n(
            &cat,
            "SELECT COUNT(*) FROM hashes WHERE verification_state IS NOT NULL"
        ),
        2
    );
    // Prefixo não casa com pasta irmã de nome parecido.
    let v2 = VerificacaoIntegridade::nova()
        .raizes([raiz.join("c/z.bin")])
        .executar(&mut cat)
        .unwrap();
    assert_eq!(v2.verificados, 1);
}
