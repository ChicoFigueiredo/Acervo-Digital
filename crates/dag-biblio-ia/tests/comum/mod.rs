//! Servidor HTTP falso (127.0.0.1:0) com respostas roteirizadas e fixtures de entrada.
#![allow(dead_code, clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dag_biblio::{EstadoLeitura, FormatoLivro, PaginaTexto, ResultadoExtracao};
use dag_biblio_ia::{EntradaClassificacao, PerfilModelo, VocabularioHomologado};
use serde_json::{Value, json};

pub enum Resp {
    Json(u16, String),
    /// Espera antes de responder (para testar timeout).
    Atraso(Duration, Box<Resp>),
    /// Fecha a conexão sem responder.
    Fechar,
}

pub type Handler = dyn Fn(&str, &Value) -> Option<Resp> + Send + Sync;

pub struct Falso {
    pub endpoint: String,
    /// (caminho, corpo JSON) de cada requisição recebida.
    pub requisicoes: Arc<Mutex<Vec<(String, Value)>>>,
    parar: Arc<AtomicBool>,
    addr: std::net::SocketAddr,
}

impl Falso {
    pub fn novo(handler: impl Fn(&str, &Value) -> Option<Resp> + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let parar = Arc::new(AtomicBool::new(false));
        let requisicoes = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let (p, r) = (parar.clone(), requisicoes.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if p.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(conn) = conn else { continue };
                let (h, r) = (handler.clone(), r.clone());
                std::thread::spawn(move || atender(conn, &*h, &r));
            }
        });
        Self {
            endpoint: format!("http://{addr}"),
            requisicoes,
            parar,
            addr,
        }
    }

    pub fn contagem(&self, caminho: &str) -> usize {
        self.requisicoes
            .lock()
            .unwrap()
            .iter()
            .filter(|(c, _)| c == caminho)
            .count()
    }

    pub fn ultimo_corpo(&self, caminho: &str) -> Value {
        self.requisicoes
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(c, _)| c == caminho)
            .map(|(_, b)| b.clone())
            .unwrap()
    }
}

impl Drop for Falso {
    fn drop(&mut self) {
        self.parar.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.addr);
    }
}

fn atender(mut s: TcpStream, h: &Handler, reg: &Mutex<Vec<(String, Value)>>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let (cabecalho_fim, tamanho) = loop {
        let Ok(n) = s.read(&mut tmp) else { return };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let cab = String::from_utf8_lossy(&buf[..i]).to_lowercase();
            let tam = cab
                .lines()
                .find_map(|l| {
                    l.strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            break (i + 4, tam);
        }
    };
    while buf.len() < cabecalho_fim + tamanho {
        let Ok(n) = s.read(&mut tmp) else { return };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let cab = String::from_utf8_lossy(&buf[..cabecalho_fim]).into_owned();
    let caminho = cab.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let corpo: Value = serde_json::from_slice(&buf[cabecalho_fim..]).unwrap_or(Value::Null);
    reg.lock().unwrap().push((caminho.clone(), corpo.clone()));
    let resp = h(&caminho, &corpo).unwrap_or_else(|| padrao(&caminho));
    enviar(&mut s, resp);
}

fn enviar(s: &mut TcpStream, r: Resp) {
    match r {
        Resp::Fechar => {}
        Resp::Atraso(d, r) => {
            std::thread::sleep(d);
            enviar(s, *r);
        }
        Resp::Json(status, corpo) => {
            let msg = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corpo}",
                corpo.len()
            );
            let _ = s.write_all(msg.as_bytes());
            let _ = s.flush();
        }
    }
}

/// Rotas auxiliares de um llama-server/Ollama genérico.
pub fn padrao(caminho: &str) -> Resp {
    let v = match caminho {
        "/health" => json!({"status": "ok"}),
        "/v1/models" => {
            json!({"data": [{"id": "qwen-fake-Q4_K_M.gguf", "meta": {"n_ctx_train": 32768, "n_params": 7_000_000_000u64}}]})
        }
        "/props" => json!({
            "default_generation_settings": {"n_ctx": 8192},
            "model_path": "/modelos/Qwen2.5-7B-Instruct-Q4_K_M.gguf",
            "build_info": "b9999-fake",
            "modalities": {"vision": false}
        }),
        "/api/version" => json!({"version": "0.33.3"}),
        "/api/tags" => json!({"models": [{"name": "gemma3:12b", "model": "gemma3:12b"}]}),
        "/api/show" => json!({
            "details": {"quantization_level": "Q4_K_M", "parameter_size": "12.2B"},
            "model_info": {"gemma3.context_length": 131072},
            "capabilities": ["completion", "vision"]
        }),
        "/api/ps" => json!({"models": [{"name": "gemma3:12b", "size_vram": 9_437_184_000u64}]}),
        _ => return Resp::Json(404, "{\"error\":\"nao existe\"}".into()),
    };
    Resp::Json(200, v.to_string())
}

pub fn chat_openai(conteudo: &str) -> Resp {
    Resp::Json(
        200,
        json!({
            "model": "fake",
            "choices": [{"message": {"role": "assistant", "content": conteudo}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1200, "completion_tokens": 200},
            "timings": {"prompt_n": 1200, "prompt_ms": 300.0, "predicted_n": 200, "predicted_ms": 4000.0, "predicted_per_second": 50.0}
        })
        .to_string(),
    )
}

pub fn chat_ollama(conteudo: &str) -> Resp {
    Resp::Json(
        200,
        json!({
            "model": "gemma3:12b",
            "message": {"role": "assistant", "content": conteudo},
            "done": true, "done_reason": "stop",
            "prompt_eval_count": 1500, "prompt_eval_duration": 400_000_000u64,
            "eval_count": 250, "eval_duration": 5_000_000_000u64
        })
        .to_string(),
    )
}

// ---------- fixtures ----------

pub fn campo(valor: Value, conf: f64, pag: u32, trecho: &str) -> Value {
    json!({"valor": valor, "confianca": conf, "evidencias": [{"pagina": pag, "trecho": trecho}]})
}

/// Campo com várias evidências (todas na mesma página).
pub fn campo2(valor: Value, pag: u32, trechos: &[&str]) -> Value {
    let evs: Vec<Value> = trechos
        .iter()
        .map(|t| json!({"pagina": pag, "trecho": t}))
        .collect();
    json!({"valor": valor, "evidencias": evs})
}

/// JSON de saída do modelo com os campos informados.
pub fn saida(campos: Value) -> String {
    json!({"campos": campos, "tags_novas_propostas": []}).to_string()
}

pub fn pagina(n: u32, t: &str) -> PaginaTexto {
    PaginaTexto {
        numero: n,
        texto: t.into(),
        via_ocr: false,
        qualidade: 1.0,
        idioma_ocr: None,
    }
}

pub fn entrada(paginas: Vec<PaginaTexto>) -> EntradaClassificacao {
    EntradaClassificacao {
        id: "caso".into(),
        nome_arquivo: "stewart-calculo.pdf".into(),
        extracao: ResultadoExtracao {
            formato: FormatoLivro::Pdf,
            estado: EstadoLeitura::Lido,
            metadados: vec![],
            isbns: vec![],
            sumario: vec![],
            paginas,
            total_paginas: Some(800),
            ficha_catalografica: None,
            capa: None,
            capa_mime: None,
            paginas_ocr: vec![],
            avisos: vec![],
        },
        vocabulario: VocabularioHomologado {
            tags: vec!["Cálculo".into(), "Álgebra".into()],
            estantes: vec!["Matemática".into(), "Literatura".into()],
            autores_parecidos: vec!["Stewart, James".into()],
        },
        exemplos: vec![],
        referencias: vec![],
        imagem_pagina1: None,
    }
}

pub fn entrada_calculo() -> EntradaClassificacao {
    entrada(vec![
        pagina(
            3,
            "CÁLCULO\nVolume 1\nJames Stewart\nCengage Learning\nISBN 978-85-359-0277-8\n2013\nUm livro de cálculo diferencial e integral.",
        ),
        pagina(
            4,
            "Prefácio. Este livro trata de limites, derivadas e integrais.",
        ),
    ])
}

pub fn perfil_openai(
    endpoint: &str,
    nome: &str,
    papel: dag_biblio_ia::PapelModelo,
) -> PerfilModelo {
    let mut p = PerfilModelo::ollama(nome, false, papel);
    p.backend = dag_biblio_ia::Backend::OpenAi;
    p.endpoint = endpoint.to_owned();
    p
}
