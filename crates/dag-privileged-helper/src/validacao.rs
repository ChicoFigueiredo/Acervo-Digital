//! Validação textual do pedido. A defesa contra corrida de symlink está em `listagem`
//! (percurso por componente com `openat`); aqui só se recusa o que nem deve começar.

use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::protocolo::Requisicao;

/// Limite do tamanho do JSON de entrada.
pub const MAX_REQUISICAO: u64 = 64 * 1024;
/// Máximo de raízes por pedido.
pub const MAX_RAIZES: usize = 64;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ErroValidacao {
    #[error("requisição não é um JSON válido no formato esperado: {0}")]
    Json(String),
    #[error("caminho vazio, relativo ou não canônico: {0}")]
    CaminhoInvalido(String),
    #[error("nenhuma raiz autorizada informada")]
    SemRaizes,
    #[error("raiz inválida: {0}")]
    RaizInvalida(String),
    #[error("caminho fora das raízes autorizadas: {0}")]
    ForaDasRaizes(String),
    #[error("caminho fora das raízes permitidas pelo administrador: {0}")]
    ForaDaPolitica(String),
}

impl ErroValidacao {
    #[must_use]
    pub fn codigo(&self) -> &'static str {
        match self {
            Self::Json(_) => "requisicao_invalida",
            Self::CaminhoInvalido(_) => "caminho_invalido",
            Self::SemRaizes | Self::RaizInvalida(_) => "raiz_invalida",
            Self::ForaDasRaizes(_) => "fora_das_raizes",
            Self::ForaDaPolitica(_) => "fora_da_politica",
        }
    }
}

/// Interpreta o JSON (uma linha), recusando campos desconhecidos.
///
/// # Errors
/// [`ErroValidacao::Json`] para qualquer desvio do formato.
pub fn interpretar(bytes: &[u8]) -> Result<Requisicao, ErroValidacao> {
    serde_json::from_slice::<Requisicao>(bytes).map_err(|e| ErroValidacao::Json(e.to_string()))
}

/// Absoluto, sem NUL, sem `.`/`..`/componentes vazios (`//`) e sem barra final: já canônico
/// textualmente. Devolve os componentes normais (a raiz `/` dá lista vazia).
fn componentes_canonicos(texto: &str) -> Option<Vec<&str>> {
    if texto.is_empty() || texto.contains('\0') || !texto.starts_with('/') {
        return None;
    }
    if texto == "/" {
        return Some(Vec::new());
    }
    let resto = &texto[1..];
    let mut v = Vec::new();
    for c in resto.split('/') {
        if c.is_empty() || c == "." || c == ".." {
            return None;
        }
        v.push(c);
    }
    Some(v)
}

/// Pedido validado: caminho decomposto em componentes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validado {
    pub caminho: PathBuf,
    pub componentes: Vec<String>,
}

/// Valida o pedido contra as raízes do chamante e, se houver, a política do administrador
/// (`politica`: raízes permitidas; `None` = sem política).
///
/// # Errors
/// Ver [`ErroValidacao`].
pub fn validar(req: &Requisicao, politica: Option<&[PathBuf]>) -> Result<Validado, ErroValidacao> {
    let comps = componentes_canonicos(&req.caminho)
        .ok_or_else(|| ErroValidacao::CaminhoInvalido(req.caminho.clone()))?;
    if req.raizes.is_empty() || req.raizes.len() > MAX_RAIZES {
        return Err(ErroValidacao::SemRaizes);
    }
    for r in &req.raizes {
        if componentes_canonicos(r).is_none() {
            return Err(ErroValidacao::RaizInvalida(r.clone()));
        }
    }
    let caminho = Path::new(&req.caminho);
    if !req.raizes.iter().any(|r| caminho.starts_with(r)) {
        return Err(ErroValidacao::ForaDasRaizes(req.caminho.clone()));
    }
    if let Some(p) = politica
        && !p.iter().any(|r| caminho.starts_with(r))
    {
        return Err(ErroValidacao::ForaDaPolitica(req.caminho.clone()));
    }
    debug_assert!(
        Path::new(&req.caminho)
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
    );
    Ok(Validado {
        caminho: caminho.to_path_buf(),
        componentes: comps.into_iter().map(str::to_string).collect(),
    })
}

/// Lê a política do administrador: um caminho absoluto por linha (`#` comenta). Arquivo
/// ausente = sem política (`None`); ilegível ou vazio = política que não permite nada.
#[must_use]
pub fn ler_politica(arquivo: &Path) -> Option<Vec<PathBuf>> {
    match std::fs::read_to_string(arquivo) {
        Ok(t) => Some(
            t.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .filter(|l| componentes_canonicos(l).is_some())
                .map(PathBuf::from)
                .collect(),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some(Vec::new()),
    }
}
