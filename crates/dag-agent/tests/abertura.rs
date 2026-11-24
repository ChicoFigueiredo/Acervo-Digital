//! Recuperação ao abrir (ADR 0003): execução interrompida, retomada automática, passadas de
//! alcance, verificação ao abrir e Ponto de Confiança. Determinístico: a "queda" é reproduzida
//! deixando o Catálogo como um processo morto o deixaria.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use dag_agent::{Agente, PoliticaAbertura, VerificarAoAbrir};
use dag_core::{
    Comando, EstadoExecucao, ModoVarredura, Resposta, StatusConfianca, StatusExecucao,
    VolumeDescoberto,
};
use dag_store::Catalogo;
use dag_store::inventario;
use tempfile::TempDir;

fn volume(raiz: &Path) -> VolumeDescoberto {
    let dev = fs::metadata(raiz).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("abertura-{dev}"),
        st_dev: dev,
        rotulo: Some("ABERTURA".into()),
        fs_type: "teste".into(),
        capacidade_bytes: Some(1_000_000),
        livre_bytes: Some(400_000),
        pontos_de_montagem: vec![raiz.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: None,
    }
}

fn escrever(p: &Path, c: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, c).unwrap();
}

struct T {
    _tmp: TempDir,
    raiz: PathBuf,
    catalogo: PathBuf,
}

/// raiz/{a/um.bin, a/dois.bin, b/tres.bin, b/quatro.bin} com conteúdos distintos.
fn arvore() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("acervo");
    escrever(&raiz.join("a/um.bin"), &[1; 5_000]);
    escrever(&raiz.join("a/dois.bin"), &[2; 6_000]);
    escrever(&raiz.join("b/tres.bin"), &[3; 7_000]);
    escrever(&raiz.join("b/quatro.bin"), &[4; 8_000]);
    let catalogo = tmp.path().join("dados/catalogo.sqlite");
    T {
        _tmp: tmp,
        raiz,
        catalogo,
    }
}

fn agente(t: &T) -> Agente {
    Agente::abrir(&t.catalogo)
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![volume(&t.raiz)])
}

fn scan_full(t: &T) -> Comando {
    Comando::IniciarScan {
        modo: ModoVarredura::FullScan,
        raizes: vec![t.raiz.to_str().unwrap().into()],
        sem_hash: false,
        elevar: false,
        releitura_total: false,
        threads_por_disco: None,
        max_discos: None,
    }
}

fn politica(monitor: bool) -> PoliticaAbertura {
    PoliticaAbertura {
        monitor_ativo: monitor,
        ..PoliticaAbertura::default()
    }
}

fn contagem(t: &T, sql: &str) -> i64 {
    let cat = Catalogo::abrir(&t.catalogo).unwrap();
    cat.armazem()
        .conexao()
        .query_row(sql, [], |l| l.get(0))
        .unwrap()
}

fn sql(t: &T, comando: &str) {
    let cat = Catalogo::abrir(&t.catalogo).unwrap();
    cat.armazem().conexao().execute_batch(comando).unwrap();
}

fn status(a: &mut Agente) -> (Vec<StatusExecucao>, Vec<StatusConfianca>) {
    match a.executar(Comando::Status { execucao: None }) {
        Resposta::Status {
            execucoes,
            confianca,
            ..
        } => (execucoes, confianca),
        r => panic!("{r:?}"),
    }
}

fn bytes_lidos_do_hash(r: &Resposta) -> (u64, u64) {
    match r {
        Resposta::ScanConcluido { hash: Some(h), .. } => (h.hasheados, h.bytes_lidos),
        r => panic!("{r:?}"),
    }
}

/// O que um processo morto no meio do inventário deixa: execução `em_andamento` com a fila de
/// diretórios ainda por visitar e nenhum arquivo gravado.
fn queda_no_inventario(t: &T) -> i64 {
    let id = {
        let mut a = agente(t);
        a.controle().cancelar(); // para o inventário antes de visitar qualquer pasta
        let r = a.executar(scan_full(t));
        let Resposta::ScanConcluido {
            execucao,
            desfecho,
            arquivos,
            ..
        } = r
        else {
            panic!("{r:?}")
        };
        assert_eq!((desfecho.as_str(), arquivos), ("cancelada", 0));
        execucao.0
    };
    sql(
        t,
        &format!("UPDATE scan_runs SET status = 'em_andamento', ended_at = NULL WHERE id = {id}"),
    );
    id
}

/// O que um processo morto no meio do hash deixa: execução `concluida` na fase `hash`, dois dos
/// quatro arquivos sem hash e com a fila pendente.
fn queda_no_hash(t: &T) -> i64 {
    {
        let mut a = agente(t);
        let r = a.executar(scan_full(t));
        assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    }
    let id = contagem(t, "SELECT MAX(id) FROM scan_runs");
    sql(
        t,
        "UPDATE scan_runs SET phase = 'hash';
         DELETE FROM hashes WHERE file_id IN
            (SELECT id FROM files WHERE kind = 'arquivo' ORDER BY id LIMIT 2);
         UPDATE fila_hash SET state = 'pendente'
            WHERE file_id IN (SELECT id FROM files WHERE kind = 'arquivo' ORDER BY id LIMIT 2);",
    );
    id
}

#[test]
fn queda_no_inventario_e_marcada_interrompida_e_retomada_sozinha() {
    let t = arvore();
    let id = queda_no_inventario(&t);
    // Enquanto ninguém abre: o Catálogo ainda diz `em_andamento` (processo morto).
    assert_eq!(
        contagem(
            &t,
            &format!("SELECT status = 'em_andamento' FROM scan_runs WHERE id = {id}")
        ),
        1
    );
    // Arquivo que chegou com o programa fora.
    escrever(&t.raiz.join("c/chegou.bin"), &[9; 3_000]);

    let mut a = agente(&t);
    let rel = a.recuperar_ao_abrir(&politica(true)).unwrap();
    assert!(rel.encerramento_nao_limpo);
    assert_eq!(rel.interrompidas, vec![id]);
    assert_eq!(rel.retomadas, vec![id]);
    assert!(!rel.cancelada, "{rel:?}");

    // O inventário retomado já viu os 5 arquivos (inclusive o novo); a passada de alcance
    // converge sem mudanças e a Raiz entra no Ponto de Confiança.
    assert_eq!(
        contagem(
            &t,
            "SELECT COUNT(*) FROM files WHERE kind = 'arquivo' AND state = 'ativo'"
        ),
        5
    );
    assert_eq!(contagem(&t, "SELECT COUNT(*) FROM hashes"), 5);
    let ultima = rel.passadas.last().unwrap();
    assert_eq!((ultima.mudancas, ultima.hasheados), (0, 0));
    assert_eq!(rel.raizes_confiaveis, vec![t.raiz.display().to_string()]);

    let (execs, conf) = status(&mut a);
    let e = execs.iter().find(|e| e.id.0 == id).unwrap();
    assert_eq!(e.estado, EstadoExecucao::Concluida);
    assert!(!e.retomando, "a flag zera quando a execução termina");
    assert_eq!(conf.len(), 1);
    assert!(conf[0].confiavel_em.is_some());
}

#[test]
fn queda_no_hash_retoma_sem_reler_o_que_ja_foi_hasheado() {
    let t = arvore();
    let id = queda_no_hash(&t);
    let mut a = agente(&t);
    let rel = a.recuperar_ao_abrir(&politica(false)).unwrap();
    assert_eq!(rel.interrompidas, vec![id]);
    assert_eq!(rel.retomadas, vec![id]);
    // A retomada leu só os 2 que faltavam; os outros 2 foram reaproveitados.
    let (hasheados, reaproveitados) = (
        contagem(
            &t,
            &format!(
                "SELECT json_extract(counters, '$.hash.hasheados') FROM scan_runs WHERE id = {id}"
            ),
        ),
        contagem(
            &t,
            &format!(
                "SELECT json_extract(counters, '$.hash.reaproveitados') FROM scan_runs WHERE id = {id}"
            ),
        ),
    );
    assert_eq!((hasheados, reaproveitados), (2, 2));
    assert_eq!(contagem(&t, "SELECT COUNT(*) FROM hashes"), 4);
    assert_eq!(
        contagem(
            &t,
            &format!("SELECT phase = 'concluida' FROM scan_runs WHERE id = {id}")
        ),
        1
    );
    // Sem monitor nenhum Ponto de Confiança é marcado.
    assert!(rel.raizes_confiaveis.is_empty());
}

#[test]
fn passada_de_alcance_acha_arquivo_que_chegou_durante_a_queda() {
    let t = arvore();
    queda_no_hash(&t);
    escrever(&t.raiz.join("novo/chegou.bin"), &[7; 4_000]);
    let mut a = agente(&t);
    let rel = a.recuperar_ao_abrir(&politica(true)).unwrap();
    // 1ª passada: acha a pasta e o arquivo novos e hasheia só ele; 2ª: nada mais mudou.
    assert_eq!(rel.passadas.len(), 2, "{rel:?}");
    assert!(rel.passadas[0].mudancas >= 2);
    assert_eq!(rel.passadas[0].hasheados, 1);
    assert_eq!(rel.passadas[1].mudancas, 0);
    assert_eq!(contagem(&t, "SELECT COUNT(*) FROM hashes"), 5);
    assert_eq!(rel.raizes_confiaveis.len(), 1);
}

#[test]
fn retomada_automatica_desligada_so_marca_interrompida_com_a_flag_desligada() {
    let t = arvore();
    let id = queda_no_hash(&t);
    let mut a = agente(&t);
    let pol = PoliticaAbertura {
        retomar_automaticamente: false,
        verificar: VerificarAoAbrir::Nunca,
        ..PoliticaAbertura::default()
    };
    let rel = a.recuperar_ao_abrir(&pol).unwrap();
    assert_eq!(
        (rel.interrompidas.clone(), rel.nao_retomadas.clone()),
        (vec![id], vec![id])
    );
    assert!(rel.retomadas.is_empty());
    assert_eq!(
        contagem(&t, "SELECT COUNT(*) FROM hashes"),
        2,
        "nada foi relido"
    );
    let (execs, _) = status(&mut a);
    let e = execs.iter().find(|e| e.id.0 == id).unwrap();
    assert!(!e.retomando);
    // O usuário ainda retoma na mão.
    let r = a.executar(Comando::Retomar {
        execucao: Some(dag_core::ExecucaoId(id)),
        threads_por_disco: None,
        max_discos: None,
    });
    assert_eq!(bytes_lidos_do_hash(&r).0, 2);
}

#[test]
fn status_mostra_interrompida_retomando_com_a_flag() {
    let t = arvore();
    let id = queda_no_inventario(&t);
    {
        let cat = Catalogo::abrir(&t.catalogo).unwrap();
        let achadas =
            inventario::detectar_interrompidas(cat.armazem().conexao(), true, i64::MAX / 2)
                .unwrap();
        assert_eq!(achadas.len(), 1);
        assert_eq!(achadas[0].id, id);
    }
    let (execs, _) = status(&mut agente(&t));
    let e = execs.iter().find(|e| e.id.0 == id).unwrap();
    assert_eq!(e.estado_detalhe, "interrompida");
    assert!(e.retomando);
    let json = serde_json::to_value(e).unwrap();
    assert_eq!(json["retomando"], true);
}

#[test]
fn encerramento_do_agente_deixa_interrompida_e_a_proxima_abertura_continua() {
    let t = arvore();
    let id = {
        let mut a = agente(&t);
        a.controle().encerrar(); // SIGTERM: não é decisão do usuário
        let r = a.executar(scan_full(&t));
        let Resposta::ScanConcluido {
            execucao, desfecho, ..
        } = r
        else {
            panic!("{r:?}")
        };
        assert_eq!(desfecho, "cancelada");
        execucao.0
    };
    assert_eq!(
        contagem(
            &t,
            &format!(
                "SELECT status = 'interrompida' AND retomada_automatica = 1 FROM scan_runs WHERE id = {id}"
            )
        ),
        1
    );
    let mut a = agente(&t);
    let rel = a.recuperar_ao_abrir(&politica(false)).unwrap();
    assert_eq!(rel.retomadas, vec![id]);
    assert_eq!(contagem(&t, "SELECT COUNT(*) FROM hashes"), 4);
}

#[test]
fn cancelar_de_proposito_nao_retoma_sozinho() {
    let t = arvore();
    {
        let mut a = agente(&t);
        a.controle().cancelar();
        a.executar(scan_full(&t));
    }
    // Cancelada pelo usuário no inventário.
    let mut a = agente(&t);
    let rel = a.recuperar_ao_abrir(&politica(false)).unwrap();
    assert!(rel.interrompidas.is_empty(), "{rel:?}");
    // E no hash: `parada_deliberada` não conta como queda.
    let t2 = arvore();
    let id = queda_no_hash(&t2);
    sql(
        &t2,
        &format!("UPDATE scan_runs SET parada_deliberada = 1 WHERE id = {id}"),
    );
    let mut b = agente(&t2);
    let rel = b.recuperar_ao_abrir(&politica(false)).unwrap();
    assert!(rel.interrompidas.is_empty(), "{rel:?}");
}

#[test]
fn verificacao_ao_abrir_hasheia_so_arquivos_novos_ou_alterados() {
    let t = arvore();
    {
        let mut a = agente(&t);
        a.executar(scan_full(&t));
    }
    // Um alterado (conteúdo e mtime) e um novo; os outros 3 ficam como estavam.
    escrever(&t.raiz.join("a/um.bin"), &[8; 5_500]);
    let futuro = SystemTime::now() + Duration::from_secs(120);
    fs::File::options()
        .write(true)
        .open(t.raiz.join("a/um.bin"))
        .unwrap()
        .set_modified(futuro)
        .unwrap();
    escrever(&t.raiz.join("b/novo.bin"), &[5; 2_000]);

    let mut a = agente(&t);
    let rel = a.recuperar_ao_abrir(&politica(false)).unwrap();
    assert!(!rel.encerramento_nao_limpo);
    assert!(rel.interrompidas.is_empty());
    assert_eq!(rel.passadas.len(), 1, "sem monitor uma passada basta");
    assert_eq!(rel.passadas[0].mudancas, 2);
    assert_eq!(rel.passadas[0].hasheados, 2);
    assert_eq!(contagem(&t, "SELECT COUNT(*) FROM hashes"), 5);
    assert_eq!(
        contagem(&t, "SELECT SUM(size_bytes) FROM hashes"),
        5_500 + 6_000 + 7_000 + 8_000 + 2_000
    );
}

#[test]
fn full_scan_explicito_com_hash_rele_tudo() {
    let t = arvore();
    let mut a = agente(&t);
    let primeira = a.executar(scan_full(&t));
    assert_eq!(bytes_lidos_do_hash(&primeira), (4, 26_000));
    // Verificação ao abrir não relê nada (nada mudou)...
    let rel = a.recuperar_ao_abrir(&politica(false)).unwrap();
    assert_eq!(rel.passadas[0].hasheados, 0);
    // Um Full Scan comum reaproveita o hash do que não mudou...
    let comum = a.executar(scan_full(&t));
    assert_eq!(bytes_lidos_do_hash(&comum), (0, 0));
    // ...mas o Full Scan com hash de tudo, pedido de propósito, lê todos os arquivos de novo.
    let Comando::IniciarScan {
        modo,
        raizes,
        sem_hash,
        elevar,
        ..
    } = scan_full(&t)
    else {
        unreachable!()
    };
    let total = a.executar(Comando::IniciarScan {
        modo,
        raizes,
        sem_hash,
        elevar,
        releitura_total: true,
        threads_por_disco: None,
        max_discos: None,
    });
    assert_eq!(bytes_lidos_do_hash(&total), (4, 26_000));
}

#[test]
fn politica_nao_confiaveis_pula_raiz_confiavel_e_sempre_nao() {
    let t = arvore();
    {
        let mut a = agente(&t);
        a.executar(scan_full(&t));
        let rel = a.recuperar_ao_abrir(&politica(true)).unwrap();
        assert_eq!(rel.raizes_confiaveis.len(), 1);
    }
    // Reabre sem queda (monitor `parado`): confiável.
    let pol = |v| PoliticaAbertura {
        verificar: v,
        ..politica(true)
    };
    let mut a = agente(&t);
    let rel = a
        .recuperar_ao_abrir(&pol(VerificarAoAbrir::NaoConfiaveis))
        .unwrap();
    assert!(rel.passadas.is_empty(), "{rel:?}");
    let rel = a
        .recuperar_ao_abrir(&pol(VerificarAoAbrir::Sempre))
        .unwrap();
    assert_eq!(rel.passadas.len(), 1);
    assert_eq!(rel.raizes_verificadas.len(), 1);
}

#[test]
fn encerramento_nao_limpo_do_monitor_tira_a_raiz_do_ponto_de_confianca() {
    let t = arvore();
    {
        let mut a = agente(&t);
        a.executar(scan_full(&t));
        a.recuperar_ao_abrir(&politica(true)).unwrap();
    }
    // Monitor que morreu sem escrever `parado`.
    sql(
        &t,
        "UPDATE monitor_saude SET estado = 'ativo', heartbeat = 1",
    );
    let mut a = agente(&t);
    let pol = PoliticaAbertura {
        verificar: VerificarAoAbrir::Nunca,
        ..politica(true)
    };
    let rel = a.recuperar_ao_abrir(&pol).unwrap();
    assert!(rel.encerramento_nao_limpo);
    let (_, conf) = status(&mut a);
    assert!(conf[0].confiavel_em.is_none());
    assert!(
        conf[0]
            .motivo_perda
            .as_deref()
            .unwrap()
            .contains("não limpo")
    );
    // A próxima abertura (verificando) a leva de volta ao Ponto de Confiança.
    let rel = a.recuperar_ao_abrir(&politica(true)).unwrap();
    assert_eq!(rel.raizes_confiaveis.len(), 1);
}

#[test]
fn raiz_de_execucao_retomada_e_da_politica_entra_uma_vez_so() {
    let t = arvore();
    queda_no_hash(&t);
    let mut a = agente(&t);
    let rel = a.recuperar_ao_abrir(&politica(true)).unwrap();
    assert_eq!(rel.raizes_verificadas, vec![t.raiz.display().to_string()]);
    // As passadas rodam em série: uma execução `fast` por passada, nenhuma ao mesmo tempo.
    let (execs, _) = status(&mut a);
    assert!(execs.iter().all(|e| e.estado == EstadoExecucao::Concluida));
}
