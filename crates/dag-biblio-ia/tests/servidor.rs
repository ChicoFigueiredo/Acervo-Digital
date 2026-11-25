#![cfg(unix)]
#![allow(clippy::unwrap_used)]
mod comum;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use comum::{Falso, Resp};
use dag_biblio_ia::servidor::{ConfigLlamaServer, ModoExecucao, ServicoOllama, ServidorLlama};
use dag_biblio_ia::{ConfigCliente, ErroIa, PapelModelo, PerfilModelo};

/// "llama-server" falso: com `-ngl` != 0 morre por falta de memória; em CPU fica vivo.
fn binario_falso(dir: &std::path::Path) -> PathBuf {
    let p = dir.join("llama-server-falso");
    std::fs::write(
        &p,
        "#!/bin/sh\nngl=1\nwhile [ $# -gt 0 ]; do [ \"$1\" = \"-ngl\" ] && ngl=\"$2\"; shift; done\n\
         if [ \"$ngl\" != \"0\" ]; then echo 'ggml_cuda: CUDA error: out of memory' >&2; exit 1; fi\n\
         exec sleep 60\n",
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// `/health` fica 503 ("carregando") nas primeiras chamadas, depois 200.
fn health_lento() -> Falso {
    let n = Arc::new(AtomicUsize::new(0));
    Falso::novo(move |c, _| {
        (c == "/health").then(|| {
            if n.fetch_add(1, Ordering::SeqCst) < 3 {
                Resp::Json(503, "{\"error\":\"loading model\"}".into())
            } else {
                Resp::Json(200, "{\"status\":\"ok\"}".into())
            }
        })
    })
}

fn porta_de(f: &Falso) -> u16 {
    f.endpoint.rsplit(':').next().unwrap().parse().unwrap()
}

fn cfg(dir: &std::path::Path, f: &Falso) -> ConfigLlamaServer {
    let mut c = ConfigLlamaServer::novo(dir.join("modelo.gguf"), porta_de(f));
    c.binario = binario_falso(dir);
    c.tempo_limite_inicio = Duration::from_secs(20);
    c
}

#[test]
fn gpu_sem_memoria_cai_para_cpu_e_detecta_queda() {
    let d = tempfile::tempdir().unwrap();
    let f = health_lento();
    let c = cfg(d.path(), &f);
    let mut s = ServidorLlama::iniciar(&c).unwrap();
    assert_eq!(
        s.modo(),
        ModoExecucao::Cpu,
        "GPU falhou por OOM, fallback CPU"
    );
    s.verificar_vivo().unwrap();
    s.saude().unwrap();
    s.parar();
    let e = s.verificar_vivo().unwrap_err();
    assert!(matches!(e, ErroIa::Servidor(_)), "{e:?}");
}

#[test]
fn sem_fallback_o_oom_vira_erro_tipado() {
    let d = tempfile::tempdir().unwrap();
    let f = health_lento();
    let mut c = cfg(d.path(), &f);
    c.fallback_cpu = false;
    let e = ServidorLlama::iniciar(&c).unwrap_err();
    assert!(matches!(e, ErroIa::MemoriaInsuficiente(_)), "{e:?}");
}

#[test]
fn binario_inexistente_e_erro_claro() {
    let d = tempfile::tempdir().unwrap();
    let mut c = ConfigLlamaServer::novo(d.path().join("m.gguf"), 1);
    c.binario = d.path().join("nao-existe");
    let e = ServidorLlama::iniciar(&c).unwrap_err();
    assert!(e.to_string().contains("não foi possível executar"), "{e}");
}

#[test]
fn argumentos_levam_ngl_contexto_e_mmproj() {
    let mut c = ConfigLlamaServer::novo("/m/x.gguf".into(), 8081);
    c.mmproj = Some("/m/proj.gguf".into());
    c.n_ctx = 16384;
    let a = c.argumentos(99);
    let s = a.join(" ");
    assert!(s.contains("-ngl 99") && s.contains("--port 8081") && s.contains("-c 16384"));
    assert!(
        s.contains("--mmproj /m/proj.gguf")
            && s.contains("--host 127.0.0.1")
            && s.contains("-np 1")
    );
    assert!(c.argumentos(0).join(" ").contains("-ngl 0"));
}

#[test]
fn ollama_so_verifica_nunca_baixa() {
    let f = Falso::novo(|_, _| None);
    let mut p = PerfilModelo::ollama("gemma3:12b", true, PapelModelo::Primario);
    p.endpoint = f.endpoint.clone();
    let (s, info) = ServicoOllama::verificar(&p, ConfigCliente::default()).unwrap();
    assert_eq!(info.quantizacao.as_deref(), Some("Q4_K_M"));
    s.verificar_vivo().unwrap();
    // Nenhuma requisição de download/pull foi feita.
    assert_eq!(f.contagem("/api/pull"), 0);

    let mut ausente = PerfilModelo::ollama("qwen3:14b", false, PapelModelo::Secundario);
    ausente.endpoint = f.endpoint.clone();
    let e = ServicoOllama::verificar(&ausente, ConfigCliente::default()).unwrap_err();
    assert!(e.to_string().contains("ollama pull qwen3:14b"), "{e}");
    assert_eq!(f.contagem("/api/pull"), 0);
}
