//! Cliente HTTP síncrono para servidores de IA locais.
//!
//! Dois protocolos: OpenAI-compatível (`llama-server`, Ollama `/v1`) e API nativa do Ollama
//! (`/api/chat`). Temperatura 0, seed fixa, timeout, retries limitados e limite de tamanho da
//! resposta. Só HTTP puro em loopback: o texto dos livros nunca sai da máquina por acidente.
//!
//! Por que `ureq` (sem TLS)? É Rust puro, pequeno, síncrono (o agente já usa threads) e maduro;
//! com `default-features = false` não puxa rustls/ring/gzip. Não há TLS porque o servidor é local.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::erro::{ErroIa, Resultado, indica_falta_de_memoria};
use crate::modelo::{Backend, ModoSaida, PerfilModelo, quantizacao_do_nome};

/// Parâmetros de transporte.
#[derive(Debug, Clone)]
pub struct ConfigCliente {
    /// Tempo total por tentativa.
    pub timeout: Duration,
    pub timeout_conexao: Duration,
    /// Total de tentativas (1 = sem retry).
    pub tentativas: u32,
    /// Espera inicial entre tentativas (dobra a cada uma).
    pub espera_inicial: Duration,
    /// Máximo de bytes aceitos na resposta.
    pub limite_resposta: usize,
    /// Permite endpoint fora do loopback (o conteúdo dos livros sairia da máquina).
    pub permitir_nao_local: bool,
    pub chave_api: Option<String>,
}

impl Default for ConfigCliente {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(300),
            timeout_conexao: Duration::from_secs(5),
            tentativas: 3,
            espera_inicial: Duration::from_millis(500),
            limite_resposta: 4 * 1024 * 1024,
            permitir_nao_local: false,
            chave_api: None,
        }
    }
}

/// Uma mensagem de chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mensagem {
    pub papel: &'static str,
    pub conteudo: String,
}

impl Mensagem {
    #[must_use]
    pub fn sistema(c: impl Into<String>) -> Self {
        Self {
            papel: "system",
            conteudo: c.into(),
        }
    }
    #[must_use]
    pub fn usuario(c: impl Into<String>) -> Self {
        Self {
            papel: "user",
            conteudo: c.into(),
        }
    }
}

/// Métricas normalizadas dos dois protocolos.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Metricas {
    pub tokens_entrada: Option<u64>,
    pub tokens_saida: Option<u64>,
    pub ms_prompt: Option<f64>,
    pub ms_geracao: Option<f64>,
    pub tokens_por_segundo: Option<f64>,
}

/// Resposta de um chat.
#[derive(Debug, Clone)]
pub struct RespostaChat {
    pub conteudo: String,
    /// O modelo parou por limite de tokens.
    pub truncada: bool,
    pub metricas: Metricas,
    /// Tempo de parede da chamada bem-sucedida.
    pub tempo: Duration,
    pub modelo_reportado: Option<String>,
}

/// O que o servidor diz sobre si e o modelo (B-23).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InfoServidor {
    pub modelo_id: Option<String>,
    pub caminho_modelo: Option<String>,
    pub quantizacao: Option<String>,
    pub n_ctx: Option<u64>,
    pub n_ctx_treino: Option<u64>,
    pub parametros: Option<String>,
    pub multimodal: Option<bool>,
    pub versao_servidor: Option<String>,
    /// Ollama `/api/ps`: bytes do modelo na VRAM.
    pub vram_modelo_bytes: Option<u64>,
    pub avisos: Vec<String>,
}

/// Cliente de um modelo (um endpoint + um perfil).
#[derive(Debug)]
pub struct ClienteLlm {
    perfil: PerfilModelo,
    cfg: ConfigCliente,
    base: String,
    agente: ureq::Agent,
}

fn base_do_endpoint(endpoint: &str) -> Resultado<String> {
    let mut e = endpoint.trim().trim_end_matches('/').to_owned();
    if e.starts_with("https://") {
        return Err(ErroIa::Endpoint(
            "https não é suportado (sem TLS: o servidor deve ser local, em http://127.0.0.1)"
                .into(),
        ));
    }
    if !e.starts_with("http://") {
        return Err(ErroIa::Endpoint(format!(
            "esperado http://host:porta, veio `{endpoint}`"
        )));
    }
    for sufixo in ["/chat/completions", "/api/chat", "/v1", "/api"] {
        if let Some(s) = e.strip_suffix(sufixo) {
            e = s.trim_end_matches('/').to_owned();
        }
    }
    Ok(e)
}

/// O endpoint (`http://host[:porta][/caminho]`) é, de fato, a própria máquina? Analisa a
/// autoridade de forma estrita: recusa `usuario@host` (o host real seria o que vem depois do
/// `@`), nomes que só começam com `127.` ou `localhost` (`127.evil.com`, `localhost.evil`),
/// barra invertida e portas inválidas. Só `localhost` e IPs de loopback (v4 e v6) passam.
#[must_use]
pub fn endpoint_e_loopback(url: &str) -> bool {
    let url = url.trim();
    let resto = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"));
    let Some(resto) = resto else {
        return false;
    };
    let autoridade = resto.split(['/', '?', '#']).next().unwrap_or("");
    if autoridade.is_empty()
        || autoridade
            .chars()
            .any(|c| c == '@' || c == '\\' || c.is_whitespace() || c.is_control())
    {
        return false;
    }
    let (host, porta) = if let Some(v6) = autoridade.strip_prefix('[') {
        let Some((h, r)) = v6.split_once(']') else {
            return false;
        };
        (
            h,
            r.strip_prefix(':')
                .or(if r.is_empty() { Some("") } else { None }),
        )
    } else {
        match autoridade.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (autoridade, Some("")),
        }
    };
    match porta {
        Some(p) if p.is_empty() || (p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit())) => {}
        _ => return false,
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.to_canonical().is_loopback())
}

fn e_loopback(base: &str) -> bool {
    endpoint_e_loopback(base)
}

impl ClienteLlm {
    /// Cria o cliente a partir de um perfil.
    ///
    /// # Errors
    /// `Endpoint` se o endpoint não for `http://` ou não for loopback (sem `permitir_nao_local`).
    pub fn novo(perfil: PerfilModelo, cfg: ConfigCliente) -> Resultado<Self> {
        let base = base_do_endpoint(&perfil.endpoint)?;
        if !cfg.permitir_nao_local && !e_loopback(&base) {
            return Err(ErroIa::Endpoint(format!(
                "`{base}` não é local; o conteúdo dos livros não pode sair da máquina sem permissão explícita"
            )));
        }
        let config = ureq::config::Config::builder()
            .timeout_global(Some(cfg.timeout))
            .timeout_connect(Some(cfg.timeout_conexao))
            // Sem redirecionamento: um 30x do servidor local não pode levar o texto dos livros
            // para fora do loopback.
            .max_redirects(0)
            .http_status_as_error(false)
            .build();
        Ok(Self {
            perfil,
            cfg,
            base,
            agente: ureq::Agent::new_with_config(config),
        })
    }

    #[must_use]
    pub fn perfil(&self) -> &PerfilModelo {
        &self.perfil
    }

    #[must_use]
    pub fn config(&self) -> &ConfigCliente {
        &self.cfg
    }

    /// Uma tentativa HTTP. Devolve (status, corpo).
    fn http(&self, caminho: &str, corpo: Option<&str>) -> Resultado<(u16, String)> {
        let url = format!("{}{caminho}", self.base);
        let limite = self.cfg.limite_resposta;
        let resultado = if let Some(c) = corpo {
            let mut req = self
                .agente
                .post(&url)
                .header("Content-Type", "application/json");
            if let Some(k) = &self.cfg.chave_api {
                req = req.header("Authorization", &format!("Bearer {k}"));
            }
            req.send(c)
        } else {
            let mut req = self.agente.get(&url);
            if let Some(k) = &self.cfg.chave_api {
                req = req.header("Authorization", &format!("Bearer {k}"));
            }
            req.call()
        };
        let mut resp = resultado.map_err(|e| self.mapear(&e))?;
        let status = resp.status().as_u16();
        let texto = resp
            .body_mut()
            .with_config()
            .limit(limite as u64)
            .read_to_string()
            .map_err(|e| self.mapear(&e))?;
        Ok((status, texto))
    }

    fn mapear(&self, e: &ureq::Error) -> ErroIa {
        match e {
            ureq::Error::Timeout(_) => ErroIa::Timeout(self.cfg.timeout),
            ureq::Error::BodyExceedsLimit(_) => ErroIa::RespostaGrande(self.cfg.limite_resposta),
            outro => ErroIa::Transporte(outro.to_string()),
        }
    }

    /// HTTP com retries limitados e tratamento de status.
    fn http_com_retries(&self, caminho: &str, corpo: Option<&str>) -> Resultado<String> {
        let total = self.cfg.tentativas.max(1);
        let mut espera = self.cfg.espera_inicial;
        let mut ultimo: Option<ErroIa> = None;
        for tentativa in 1..=total {
            let r = self.http(caminho, corpo).and_then(|(status, texto)| {
                if (200..300).contains(&status) {
                    Ok(texto)
                } else if indica_falta_de_memoria(&texto) {
                    Err(ErroIa::MemoriaInsuficiente(recortar(&texto, 300)))
                } else {
                    Err(ErroIa::Http {
                        status,
                        corpo: recortar(&texto, 300),
                    })
                }
            });
            match r {
                Ok(t) => return Ok(t),
                Err(e) if e.e_transitorio() && tentativa < total => {
                    ultimo = Some(e);
                    std::thread::sleep(espera);
                    espera = espera.saturating_mul(2);
                }
                Err(e) => return Err(e),
            }
        }
        Err(ultimo.unwrap_or_else(|| ErroIa::Transporte("sem tentativas".into())))
    }

    /// Corpo do pedido de chat (público para testes e para registrar o que foi enviado).
    #[must_use]
    pub fn montar_corpo(
        &self,
        mensagens: &[Mensagem],
        esquema: Option<&Value>,
        imagens_base64: &[String],
    ) -> Value {
        let p = &self.perfil;
        let ultimo_usuario = mensagens.iter().rposition(|m| m.papel == "user");
        let msgs: Vec<Value> = mensagens
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let com_imagem = Some(i) == ultimo_usuario && !imagens_base64.is_empty();
                match (p.backend, com_imagem) {
                    (Backend::Ollama, true) => {
                        json!({"role": m.papel, "content": m.conteudo, "images": imagens_base64})
                    }
                    (Backend::OpenAi, true) => {
                        let mut partes = vec![json!({"type": "text", "text": m.conteudo})];
                        for b in imagens_base64 {
                            let mime = if b.starts_with("iVBOR") {
                                "image/png"
                            } else {
                                "image/jpeg"
                            };
                            partes.push(json!({"type": "image_url",
                                "image_url": {"url": format!("data:{mime};base64,{b}")}}));
                        }
                        json!({"role": m.papel, "content": partes})
                    }
                    (_, false) => json!({"role": m.papel, "content": m.conteudo}),
                }
            })
            .collect();
        let mut corpo = match p.backend {
            Backend::Ollama => {
                let mut c = json!({
                    "model": p.nome,
                    "messages": msgs,
                    "stream": false,
                    "options": {
                        "temperature": 0,
                        "seed": p.seed,
                        "num_ctx": p.contexto,
                        "num_predict": p.max_tokens_saida,
                    },
                });
                if let Some(e) = esquema {
                    c["format"] = e.clone();
                }
                c
            }
            Backend::OpenAi => {
                let mut c = json!({
                    "model": p.nome,
                    "messages": msgs,
                    "stream": false,
                    "temperature": 0,
                    "seed": p.seed,
                    "max_tokens": p.max_tokens_saida,
                });
                if let Some(e) = esquema {
                    match &p.modo_saida {
                        ModoSaida::JsonSchema => {
                            c["response_format"] = json!({
                                "type": "json_schema",
                                "json_schema": {"name": "classificacao_bibliografica",
                                                "strict": true, "schema": e},
                            });
                        }
                        ModoSaida::JsonObjectComSchema => {
                            c["response_format"] = json!({"type": "json_object", "schema": e});
                        }
                        ModoSaida::JsonSchemaTopo => c["json_schema"] = e.clone(),
                        ModoSaida::Gramatica(g) => c["grammar"] = Value::String(g.clone()),
                        ModoSaida::SemRestricao => {}
                    }
                }
                c
            }
        };
        if let Some(obj) = corpo.as_object_mut() {
            for (k, v) in &p.extras {
                obj.insert(k.clone(), v.clone());
            }
        }
        corpo
    }

    /// Envia o chat e normaliza a resposta dos dois protocolos.
    ///
    /// # Errors
    /// Qualquer `ErroIa` de transporte/HTTP/resposta; `SaidaTruncada` se o modelo foi cortado.
    pub fn chat(
        &self,
        mensagens: &[Mensagem],
        esquema: Option<&Value>,
        imagens_base64: &[String],
    ) -> Resultado<RespostaChat> {
        let corpo = self
            .montar_corpo(mensagens, esquema, imagens_base64)
            .to_string();
        let caminho = match self.perfil.backend {
            Backend::Ollama => "/api/chat",
            Backend::OpenAi => "/v1/chat/completions",
        };
        let inicio = Instant::now();
        let texto = self.http_com_retries(caminho, Some(&corpo))?;
        let tempo = inicio.elapsed();
        let v: Value = serde_json::from_str(&texto)
            .map_err(|e| ErroIa::RespostaInvalida(format!("JSON: {e}")))?;
        let mut r = match self.perfil.backend {
            Backend::Ollama => interpretar_ollama(&v)?,
            Backend::OpenAi => interpretar_openai(&v)?,
        };
        r.tempo = tempo;
        if r.truncada {
            return Err(ErroIa::SaidaTruncada);
        }
        Ok(r)
    }

    fn get_json(&self, caminho: &str) -> Resultado<Value> {
        let t = self.http_com_retries(caminho, None)?;
        serde_json::from_str(&t).map_err(|e| ErroIa::RespostaInvalida(format!("{caminho}: {e}")))
    }

    /// O servidor responde? (`/health` do llama-server; `/api/version` do Ollama).
    ///
    /// # Errors
    /// Falha de transporte ou HTTP.
    pub fn saude(&self) -> Resultado<()> {
        match self.perfil.backend {
            Backend::Ollama => self.get_json("/api/version").map(|_| ()),
            Backend::OpenAi => {
                self.http_com_retries("/health", None)
                    .map(|_| ())
                    .or_else(|e| match e {
                        // Ollama `/v1` e outros não têm /health.
                        ErroIa::Http { status: 404, .. } => self.get_json("/v1/models").map(|_| ()),
                        outro => Err(outro),
                    })
            }
        }
    }

    /// Lê as informações do servidor para registrar modelo, quantização e contexto.
    ///
    /// # Errors
    /// Só falha se o servidor estiver inalcançável; detalhes ausentes viram `avisos`.
    pub fn info_servidor(&self) -> Resultado<InfoServidor> {
        match self.perfil.backend {
            Backend::OpenAi => self.info_openai(),
            Backend::Ollama => self.info_ollama(),
        }
    }

    fn info_openai(&self) -> Resultado<InfoServidor> {
        let mut info = InfoServidor::default();
        let mut falhas = Vec::new();
        match self.get_json("/v1/models") {
            Ok(v) => {
                let m = v.pointer("/data/0");
                info.modelo_id = m.and_then(|m| m["id"].as_str()).map(str::to_owned);
                info.n_ctx_treino = m
                    .and_then(|m| m.pointer("/meta/n_ctx_train"))
                    .and_then(Value::as_u64);
                info.parametros = m
                    .and_then(|m| m.pointer("/meta/n_params"))
                    .and_then(Value::as_u64)
                    .map(|n| n.to_string());
            }
            Err(e) => falhas.push(format!("/v1/models: {e}")),
        }
        match self.get_json("/props") {
            Ok(v) => {
                info.n_ctx = v
                    .pointer("/default_generation_settings/n_ctx")
                    .or_else(|| v.get("n_ctx"))
                    .and_then(Value::as_u64);
                info.caminho_modelo = v["model_path"].as_str().map(str::to_owned);
                info.versao_servidor = v["build_info"].as_str().map(str::to_owned);
                info.multimodal = v.pointer("/modalities/vision").and_then(Value::as_bool);
            }
            Err(e) => falhas.push(format!("/props: {e}")),
        }
        if falhas.len() == 2 {
            return Err(ErroIa::Transporte(falhas.join("; ")));
        }
        info.avisos = falhas;
        let fonte = info
            .caminho_modelo
            .clone()
            .or_else(|| info.modelo_id.clone());
        info.quantizacao = fonte.as_deref().and_then(quantizacao_do_nome);
        Ok(info)
    }

    fn info_ollama(&self) -> Resultado<InfoServidor> {
        let mut info = InfoServidor::default();
        let versao = self.get_json("/api/version")?;
        info.versao_servidor = versao["version"].as_str().map(str::to_owned);
        // Presença do modelo: nunca baixamos nada sozinhos.
        let tags = self.get_json("/api/tags")?;
        let presente = tags["models"].as_array().is_some_and(|ms| {
            ms.iter().any(|m| {
                [m["name"].as_str(), m["model"].as_str()]
                    .into_iter()
                    .flatten()
                    .any(|n| nome_ollama_igual(n, &self.perfil.nome))
            })
        });
        if !presente {
            return Err(ErroIa::ModeloAusente(format!(
                "`{}` não está instalado no Ollama; peça ao usuário para rodar `ollama pull {}`",
                self.perfil.nome, self.perfil.nome
            )));
        }
        info.modelo_id = Some(self.perfil.nome.clone());
        let corpo = json!({"model": self.perfil.nome}).to_string();
        match self.http_com_retries("/api/show", Some(&corpo)) {
            Ok(t) => {
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    info.quantizacao = v
                        .pointer("/details/quantization_level")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    info.parametros = v
                        .pointer("/details/parameter_size")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    if let Some(mi) = v["model_info"].as_object() {
                        for (k, val) in mi {
                            if k.ends_with(".context_length") {
                                info.n_ctx_treino = val.as_u64();
                            }
                        }
                    }
                    info.multimodal = v["capabilities"]
                        .as_array()
                        .map(|c| c.iter().any(|x| x.as_str() == Some("vision")));
                }
            }
            Err(e) => info.avisos.push(format!("/api/show: {e}")),
        }
        info.n_ctx = Some(u64::from(self.perfil.contexto));
        info.vram_modelo_bytes = self.vram_ollama();
        Ok(info)
    }

    /// Ollama `/api/ps`: bytes do modelo atual carregados na VRAM (`None` se não carregado/sem dado).
    #[must_use]
    pub fn vram_ollama(&self) -> Option<u64> {
        if self.perfil.backend != Backend::Ollama {
            return None;
        }
        let v = self.get_json("/api/ps").ok()?;
        v["models"].as_array()?.iter().find_map(|m| {
            let nome = m["name"].as_str().or_else(|| m["model"].as_str())?;
            if nome_ollama_igual(nome, &self.perfil.nome) {
                m["size_vram"].as_u64()
            } else {
                None
            }
        })
    }
}

/// `gemma3:12b` == `gemma3:12b`; `gemma3` == `gemma3:latest`.
fn nome_ollama_igual(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        if s.contains(':') {
            s.to_owned()
        } else {
            format!("{s}:latest")
        }
    };
    norm(a) == norm(b)
}

fn recortar(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn interpretar_openai(v: &Value) -> Resultado<RespostaChat> {
    let escolha = v
        .pointer("/choices/0")
        .ok_or_else(|| ErroIa::RespostaInvalida("sem choices[0]".into()))?;
    let conteudo = escolha
        .pointer("/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| ErroIa::RespostaInvalida("sem message.content".into()))?
        .to_owned();
    let truncada = escolha["finish_reason"].as_str() == Some("length");
    let mut m = Metricas {
        tokens_entrada: v.pointer("/usage/prompt_tokens").and_then(Value::as_u64),
        tokens_saida: v
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64),
        ..Metricas::default()
    };
    if let Some(t) = v.get("timings") {
        m.tokens_entrada = t["prompt_n"].as_u64().or(m.tokens_entrada);
        m.tokens_saida = t["predicted_n"].as_u64().or(m.tokens_saida);
        m.ms_prompt = t["prompt_ms"].as_f64();
        m.ms_geracao = t["predicted_ms"].as_f64();
        m.tokens_por_segundo = t["predicted_per_second"].as_f64();
    }
    Ok(RespostaChat {
        conteudo,
        truncada,
        metricas: m,
        tempo: Duration::ZERO,
        modelo_reportado: v["model"].as_str().map(str::to_owned),
    })
}

#[allow(clippy::cast_precision_loss)]
fn interpretar_ollama(v: &Value) -> Resultado<RespostaChat> {
    if let Some(e) = v["error"].as_str() {
        return Err(if indica_falta_de_memoria(e) {
            ErroIa::MemoriaInsuficiente(recortar(e, 300))
        } else {
            ErroIa::RespostaInvalida(recortar(e, 300))
        });
    }
    let conteudo = v
        .pointer("/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| ErroIa::RespostaInvalida("sem message.content".into()))?
        .to_owned();
    let truncada = v["done_reason"].as_str() == Some("length");
    let ns = |k: &str| v[k].as_u64().map(|n| n as f64 / 1e6);
    let tokens_saida = v["eval_count"].as_u64();
    let ms_geracao = ns("eval_duration");
    let tps = match (tokens_saida, ms_geracao) {
        (Some(n), Some(ms)) if ms > 0.0 => Some(n as f64 / (ms / 1000.0)),
        _ => None,
    };
    Ok(RespostaChat {
        conteudo,
        truncada,
        metricas: Metricas {
            tokens_entrada: v["prompt_eval_count"].as_u64(),
            tokens_saida,
            ms_prompt: ns("prompt_eval_duration"),
            ms_geracao,
            tokens_por_segundo: tps,
        },
        tempo: Duration::ZERO,
        modelo_reportado: v["model"].as_str().map(str::to_owned),
    })
}

#[cfg(test)]
mod testes {
    #[test]
    fn endpoint_loopback_estrito() {
        use super::endpoint_e_loopback as l;
        for ok in [
            "http://127.0.0.1:11434",
            "http://127.1.2.3/v1",
            "http://localhost:8080",
            "http://LOCALHOST",
            "http://[::1]:11434/api",
            "http://[::1]",
        ] {
            assert!(l(ok), "{ok}");
        }
        for ruim in [
            "http://127.evil.com:11434",
            "http://127.0.0.1@evil.com",
            "http://127.0.0.1:80@evil.com/",
            "http://localhost@evil.com",
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com",
            "http://[::1]@evil.com",
            "http://[::1]evil",
            "http://2130706433",
            "http://0.0.0.0",
            "http://10.0.0.1",
            "http://localhost\\@evil.com",
            "http://localhost:80a",
            "ftp://localhost",
            "http://",
        ] {
            assert!(!l(ruim), "{ruim}");
        }
    }

    use super::*;
    use crate::modelo::PapelModelo;

    fn perfil_openai() -> PerfilModelo {
        let mut p = PerfilModelo::ollama("m", false, PapelModelo::Primario);
        p.backend = Backend::OpenAi;
        p.endpoint = "http://127.0.0.1:8080/v1".into();
        p
    }

    #[test]
    fn endpoint_normalizado_e_validado() {
        assert_eq!(
            base_do_endpoint("http://127.0.0.1:8080/v1/chat/completions").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            base_do_endpoint("http://localhost:11434/").unwrap(),
            "http://localhost:11434"
        );
        assert!(base_do_endpoint("https://x").is_err());
        assert!(base_do_endpoint("127.0.0.1").is_err());
        let mut p = perfil_openai();
        p.endpoint = "http://10.0.0.5:8080".into();
        assert!(ClienteLlm::novo(p, ConfigCliente::default()).is_err());
    }

    #[test]
    fn corpo_openai_cobre_os_modos_de_saida() {
        let esquema = json!({"type": "object"});
        let msgs = [Mensagem::sistema("s"), Mensagem::usuario("u")];
        let mut p = perfil_openai();
        let c = ClienteLlm::novo(p.clone(), ConfigCliente::default()).unwrap();
        let b = c.montar_corpo(&msgs, Some(&esquema), &[]);
        assert_eq!(b["temperature"], 0);
        assert_eq!(b["seed"], 42);
        assert_eq!(b["response_format"]["type"], "json_schema");
        assert_eq!(b["response_format"]["json_schema"]["schema"], esquema);

        p.modo_saida = ModoSaida::JsonObjectComSchema;
        let b = ClienteLlm::novo(p.clone(), ConfigCliente::default())
            .unwrap()
            .montar_corpo(&msgs, Some(&esquema), &[]);
        assert_eq!(b["response_format"]["schema"], esquema);

        p.modo_saida = ModoSaida::JsonSchemaTopo;
        let b = ClienteLlm::novo(p.clone(), ConfigCliente::default())
            .unwrap()
            .montar_corpo(&msgs, Some(&esquema), &[]);
        assert_eq!(b["json_schema"], esquema);

        p.modo_saida = ModoSaida::Gramatica("root ::= \"a\"".into());
        let b = ClienteLlm::novo(p, ConfigCliente::default())
            .unwrap()
            .montar_corpo(&msgs, Some(&esquema), &[]);
        assert_eq!(b["grammar"], "root ::= \"a\"");
    }

    #[test]
    fn corpo_ollama_usa_format_options_e_imagens() {
        let mut p = PerfilModelo::ollama("gemma3:12b", true, PapelModelo::Primario);
        p.extras.insert("think".into(), json!(false));
        let c = ClienteLlm::novo(p, ConfigCliente::default()).unwrap();
        let esquema = json!({"type": "object"});
        let b = c.montar_corpo(
            &[Mensagem::usuario("u")],
            Some(&esquema),
            &["QUJD".to_owned()],
        );
        assert_eq!(b["format"], esquema);
        assert_eq!(b["options"]["temperature"], 0);
        assert_eq!(b["options"]["seed"], 42);
        assert_eq!(b["options"]["num_ctx"], 8192);
        assert_eq!(b["messages"][0]["images"][0], "QUJD");
        assert_eq!(b["think"], false);
        assert_eq!(b["stream"], false);
    }

    #[test]
    fn interpreta_metricas_dos_dois_protocolos() {
        let o = interpretar_ollama(&json!({
            "message": {"content": "{}"}, "done_reason": "stop", "prompt_eval_count": 100,
            "eval_count": 50, "eval_duration": 2_000_000_000u64, "prompt_eval_duration": 500_000_000u64
        }))
        .unwrap();
        assert_eq!(o.metricas.tokens_saida, Some(50));
        assert!((o.metricas.tokens_por_segundo.unwrap() - 25.0).abs() < 1e-9);
        let l = interpretar_openai(&json!({
            "choices": [{"message": {"content": "{}"}, "finish_reason": "length"}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3},
            "timings": {"predicted_per_second": 40.5, "predicted_n": 3}
        }))
        .unwrap();
        assert!(l.truncada);
        assert_eq!(l.metricas.tokens_entrada, Some(7));
        assert_eq!(l.metricas.tokens_por_segundo, Some(40.5));
    }

    #[test]
    fn nomes_ollama() {
        assert!(nome_ollama_igual("gemma3", "gemma3:latest"));
        assert!(!nome_ollama_igual("gemma3:12b", "gemma3:4b"));
    }
}
