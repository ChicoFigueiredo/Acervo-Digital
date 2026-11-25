//! Extração de e-books e quadrinhos (Fase B, frente B2), **tudo em memória**: EPUB 2/3,
//! MOBI/AZW/AZW3, FB2 (e `.fb2.zip`), quadrinhos CBZ/CBR/CB7 e livros dentro de ZIP/RAR/7Z.
//!
//! * Nada é extraído, montado ou gravado em disco (ADR 0006); compactados são lidos em fluxo
//!   pela `dag-conteineres`, com limites de profundidade, bytes, quantidade de membros, tempo e
//!   razão de expansão ([`LimitesConteiner`]).
//! * Texto dos livros é **dado**: nada é interpretado nem executado (XHTML só é varrido; XML
//!   sem DTD, então sem entidades definidas pelo documento).
//! * DRM (ADEPT, Readium LCP, FairPlay, MOBI cifrado) é detectado e **não** contornado:
//!   `EstadoLeitura::Drm`, com os metadados que estiverem em claro.
//! * O formato é decidido pela **assinatura** (bytes mágicos), a extensão só desempata.
//!
//! Entrada principal: [`extrair_arquivo`], [`extrair_bytes`] ou [`extrair_leitor`]; a saída é
//! [`Extraido`] (um livro, [`ResultadoExtracao`], ou um compactado com vários livros).

#![cfg_attr(test, allow(clippy::unwrap_used))]
// `if let` aninhado fica mais legível que let-chains nos percursos de XML/contêiner.
#![allow(clippy::collapsible_if)]

mod compactado;
mod epub;
mod fb2;
mod finalizar;
mod limites;
mod mobi;
#[cfg(test)]
mod mobi_testes;
mod origem;
mod pacote;
mod quadrinho;
mod util;
mod xml;

use std::io::Read;
use std::path::Path;

pub use compactado::{MembroLivro, ResultadoCompactado};
use dag_biblio::{EstadoLeitura, FormatoLivro, ResultadoExtracao};
pub use limites::LimitesConteiner;

use limites::Medidor;
use origem::Origem;
use pacote::{Decisao, Falha, varrer};

/// Parâmetros de uma extração.
#[derive(Debug, Clone)]
pub struct OpcoesExtracao {
    pub limites: LimitesConteiner,
    /// Quantas páginas de texto amostrar (padrão 20, como B-07).
    pub paginas_texto: u32,
    /// Tamanho aproximado de uma "página" de texto em livros sem paginação (padrão 2000).
    pub caracteres_por_pagina: usize,
    /// Extrair os bytes da capa (padrão sim).
    pub extrair_capa: bool,
    /// Guardar os bytes dos livros que esta crate não extrai (PDF, DjVu...) em compactados.
    pub reter_dados_nao_tratados: bool,
}

impl Default for OpcoesExtracao {
    fn default() -> Self {
        Self {
            limites: LimitesConteiner::default(),
            paginas_texto: 20,
            caracteres_por_pagina: 2000,
            extrair_capa: true,
            reter_dados_nao_tratados: false,
        }
    }
}

/// O que uma extração devolve.
#[derive(Debug, Clone, PartialEq)]
pub enum Extraido {
    /// Um livro (EPUB, MOBI, FB2, quadrinho...), ou um formato sem suporte (`NaoSuportado`).
    Livro(ResultadoExtracao),
    /// Um compactado genérico: um resultado por livro dentro dele.
    Compactado(ResultadoCompactado),
}

impl Extraido {
    /// Estado de leitura (do livro, ou do compactado como um todo).
    #[must_use]
    pub fn estado(&self) -> &EstadoLeitura {
        match self {
            Self::Livro(r) => &r.estado,
            Self::Compactado(c) => &c.estado,
        }
    }

    /// O resultado, se é um livro único.
    #[must_use]
    pub fn livro(&self) -> Option<&ResultadoExtracao> {
        match self {
            Self::Livro(r) => Some(r),
            Self::Compactado(_) => None,
        }
    }
}

/// Falhas de E/S ao ler a entrada (o conteúdo ruim vira `EstadoLeitura`, não erro).
#[derive(Debug, thiserror::Error)]
pub enum ErroEbook {
    #[error("falha de E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Falha(String),
}

/// Extrai de um arquivo (aberto só para leitura). `formato` é uma dica (por exemplo, da
/// extensão); a assinatura prevalece.
///
/// # Errors
/// Arquivo inexistente ou ilegível.
pub fn extrair_arquivo(
    caminho: &Path,
    formato: Option<FormatoLivro>,
    opc: &OpcoesExtracao,
) -> Result<Extraido, ErroEbook> {
    let meta = std::fs::metadata(caminho)?;
    let nome = caminho.file_name().and_then(|n| n.to_str());
    let dica = formato.or_else(|| {
        caminho
            .extension()
            .and_then(|e| e.to_str())
            .and_then(FormatoLivro::da_extensao)
    });
    let origem = Origem::Arquivo(caminho.to_path_buf(), meta.len());
    Ok(extrair_origem(&origem, nome, dica, opc))
}

/// Extrai de bytes em memória. `nome` (opcional) só serve de dica e para o número da edição
/// de quadrinhos.
#[must_use]
pub fn extrair_bytes(
    dados: &[u8],
    nome: Option<&str>,
    formato: Option<FormatoLivro>,
    opc: &OpcoesExtracao,
) -> Extraido {
    let dica = formato.or_else(|| nome.and_then(|n| FormatoLivro::da_extensao(&util::extensao(n))));
    extrair_origem(&Origem::de_vec(dados.to_vec()), nome, dica, opc)
}

/// Extrai de um leitor, que é lido até `bytes_membro_max`.
///
/// # Errors
/// Falha de leitura, ou entrada maior que o limite.
pub fn extrair_leitor<R: Read>(
    leitor: R,
    nome: Option<&str>,
    formato: Option<FormatoLivro>,
    opc: &OpcoesExtracao,
) -> Result<Extraido, ErroEbook> {
    let teto = opc.limites.bytes_membro_max;
    let mut v = Vec::new();
    leitor.take(teto.saturating_add(1)).read_to_end(&mut v)?;
    if v.len() as u64 > teto {
        return Ok(Extraido::Livro(util::vazio(
            formato.unwrap_or(FormatoLivro::Outro),
            EstadoLeitura::LimiteExcedido {
                limite: format!("tamanho da entrada (> {teto} bytes)"),
            },
        )));
    }
    let dica = formato.or_else(|| nome.and_then(|n| FormatoLivro::da_extensao(&util::extensao(n))));
    Ok(extrair_origem(&Origem::de_vec(v), nome, dica, opc))
}

/// Lê, na memória, os bytes de um membro de um compactado (caminho interno exato), sob os
/// mesmos limites. Serve para entregar PDFs/DjVus de dentro de compactados a outros extratores.
///
/// # Errors
/// Arquivo ilegível, membro ausente ou limite excedido (`ErroEbook::Falha`).
pub fn ler_membro_de_compactado(
    caminho: &Path,
    interno: &str,
    opc: &OpcoesExtracao,
) -> Result<Vec<u8>, ErroEbook> {
    let meta = std::fs::metadata(caminho)?;
    let origem = Origem::Arquivo(caminho.to_path_buf(), meta.len());
    let med = Medidor::novo(&opc.limites, meta.len());
    compactado::ler_um(&origem, interno, &med)
        .map_err(|f| ErroEbook::Falha(format!("{:?}", f.estado())))
}

fn extrair_origem(
    origem: &Origem,
    nome: Option<&str>,
    dica: Option<FormatoLivro>,
    opc: &OpcoesExtracao,
) -> Extraido {
    let med = Medidor::novo(&opc.limites, origem.tamanho());
    match detectar(origem, nome, dica, &med) {
        Err(f) => Extraido::Livro(util::vazio(dica.unwrap_or(FormatoLivro::Outro), f.estado())),
        Ok(Detectado::Livro(f)) => Extraido::Livro(extrair_livro(origem, f, nome, dica, opc, &med)),
        Ok(Detectado::Conteiner(c)) => {
            Extraido::Compactado(compactado::extrair(origem, c, opc, &med))
        }
        Ok(Detectado::Desconhecido) => Extraido::Livro(util::vazio(
            dica.unwrap_or(FormatoLivro::Outro),
            EstadoLeitura::NaoSuportado {
                motivo: "formato não reconhecido pela assinatura".to_owned(),
            },
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Detectado {
    Livro(FormatoLivro),
    Conteiner(&'static str),
    Desconhecido,
}

/// Decide o que é a entrada pelos bytes mágicos (e, em zips, pelos nomes dos membros).
pub(crate) fn detectar(
    origem: &Origem,
    nome: Option<&str>,
    dica: Option<FormatoLivro>,
    med: &Medidor,
) -> Result<Detectado, Falha> {
    let p = origem
        .prefixo(4096)
        .map_err(|e| Falha::corrompido(format!("falha de leitura: {e}")))?;
    let quadrinho = dica.is_some_and(FormatoLivro::e_quadrinho);
    if mobi::assinatura(&p) {
        return Ok(Detectado::Livro(
            dica.filter(|d| {
                matches!(
                    d,
                    FormatoLivro::Mobi | FormatoLivro::Azw | FormatoLivro::Azw3
                )
            })
            .unwrap_or(FormatoLivro::Mobi),
        ));
    }
    if mobi::e_kfx(&p) {
        return Ok(Detectado::Livro(FormatoLivro::Kfx));
    }
    if p.starts_with(b"%PDF-") {
        return Ok(Detectado::Livro(FormatoLivro::Pdf));
    }
    if p.starts_with(b"AT&TFORM") {
        return Ok(Detectado::Livro(FormatoLivro::Djvu));
    }
    if fb2::assinatura(&p) {
        return Ok(Detectado::Livro(FormatoLivro::Fb2));
    }
    if p.starts_with(b"PK\x03\x04") || p.starts_with(b"PK\x05\x06") {
        return classificar_zip(origem, dica, med);
    }
    if p.starts_with(b"Rar!\x1a\x07") {
        return Ok(if quadrinho || dica == Some(FormatoLivro::Cbr) {
            Detectado::Livro(FormatoLivro::Cbr)
        } else {
            Detectado::Conteiner("rar")
        });
    }
    if p.starts_with(b"7z\xBC\xAF\x27\x1C") {
        return Ok(if quadrinho {
            Detectado::Livro(FormatoLivro::Cb7)
        } else {
            Detectado::Conteiner("7z")
        });
    }
    // Outros compactados (tar, tar.gz...): só pela assinatura reconhecida.
    let mut fonte = origem
        .abrir()
        .map_err(|e| Falha::corrompido(format!("falha ao abrir: {e}")))?;
    if let Ok(Some(f)) = dag_conteineres::Registro::padrao().detectar(&mut *fonte) {
        return Ok(Detectado::Conteiner(f));
    }
    let _ = nome;
    Ok(match dica {
        Some(f) if !matches!(f, FormatoLivro::Outro) => Detectado::Livro(f),
        _ => Detectado::Desconhecido,
    })
}

fn classificar_zip(
    origem: &Origem,
    dica: Option<FormatoLivro>,
    med: &Medidor,
) -> Result<Detectado, Falha> {
    let v = varrer(origem, med, &mut |n, _| {
        if n.eq_ignore_ascii_case("mimetype") {
            Decisao::Ler(256)
        } else {
            Decisao::Ignorar
        }
    })?;
    let mimetype = v
        .entradas
        .iter()
        .find(|e| e.nome.eq_ignore_ascii_case("mimetype"))
        .and_then(|e| e.dados.as_ref())
        .map(|d| String::from_utf8_lossy(d).trim().to_owned());
    let tem = |n: &str| v.entradas.iter().any(|e| e.nome.eq_ignore_ascii_case(n));
    if mimetype.as_deref() == Some("application/epub+zip") || tem("META-INF/container.xml") {
        return Ok(Detectado::Livro(FormatoLivro::Epub));
    }
    let arquivos: Vec<&str> = v
        .entradas
        .iter()
        .map(|e| e.nome.as_str())
        .filter(|n| !n.starts_with("__MACOSX/"))
        .collect();
    if arquivos.iter().any(|n| util::extensao(n) == "fb2") && arquivos.len() <= 3 {
        return Ok(Detectado::Livro(FormatoLivro::Fb2));
    }
    let imagens = arquivos.iter().filter(|n| util::e_imagem(n)).count();
    if !arquivos.is_empty() && imagens * 10 >= arquivos.len() * 6 {
        return Ok(Detectado::Livro(FormatoLivro::Cbz));
    }
    if dica == Some(FormatoLivro::Cbz) {
        return Ok(Detectado::Livro(FormatoLivro::Cbz));
    }
    Ok(Detectado::Conteiner("zip"))
}

/// Chama o extrator do formato.
pub(crate) fn extrair_livro(
    origem: &Origem,
    formato: FormatoLivro,
    nome: Option<&str>,
    dica: Option<FormatoLivro>,
    opc: &OpcoesExtracao,
    med: &Medidor,
) -> ResultadoExtracao {
    match formato {
        FormatoLivro::Epub => epub::extrair(origem, opc, med),
        FormatoLivro::Mobi | FormatoLivro::Azw | FormatoLivro::Azw3 => {
            match origem.tudo(med.limites().bytes_membro_max) {
                Ok(d) => mobi::extrair(&d, dica, opc),
                Err(f) => util::vazio(formato, f.estado()),
            }
        }
        FormatoLivro::Fb2 => {
            let zip = origem.prefixo(4).is_ok_and(|p| p.starts_with(b"PK"));
            fb2::extrair(origem, opc, med, zip)
        }
        FormatoLivro::Cbz | FormatoLivro::Cbr | FormatoLivro::Cb7 => {
            quadrinho::extrair(origem, formato, nome, opc, med)
        }
        FormatoLivro::Kfx => util::vazio(
            formato,
            EstadoLeitura::NaoSuportado {
                motivo: "KFX (container proprietário da Amazon) não é suportado".to_owned(),
            },
        ),
        outro => util::vazio(
            outro,
            EstadoLeitura::NaoSuportado {
                motivo: format!(
                    "extração de {} não faz parte desta crate (outra frente)",
                    outro.nome_calibre()
                ),
            },
        ),
    }
}
