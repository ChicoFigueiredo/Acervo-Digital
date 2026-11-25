//! Extração de PDF e DjVu para a Curadoria Bibliográfica (Fase B, frente B1).
//!
//! Entrada: um arquivo no disco (só leitura) ou bytes em memória (Membros de Contêiner, ADR 0006).
//! Saída: [`dag_biblio::ResultadoExtracao`] dentro de um [`RelatorioExtracao`], que acrescenta as
//! medições por página (benchmark), os alertas de conteúdo suspeito e os metadados brutos.
//!
//! ```no_run
//! use dag_biblio_pdf::{ConfigExtracaoPdf, Extrator, Fonte};
//! let extrator = Extrator::detectar(ConfigExtracaoPdf::default())?;
//! let rel = extrator.extrair_pdf(Fonte::Caminho("livro.pdf".as_ref()));
//! println!("{:?} {:?}", rel.resultado.estado, rel.resultado.isbns);
//! # Ok::<(), dag_biblio_pdf::ErroFerramentas>(())
//! ```
//!
//! # Garantias
//!
//! - O original nunca é alterado: as ferramentas só leem; nada é escrito em disco, nem temporário.
//!   Bytes em memória viram um `memfd` (arquivo anônimo na RAM); imagens para OCR e capas
//!   trafegam por stdout/stdin.
//! - Senha e DRM (Adobe ADEPT, FileOpen, restrição de cópia) são detectados e **não contornados**:
//!   `EstadoLeitura::Senha` ou `Drm`.
//! - Tamanho do arquivo, páginas do documento, páginas amostradas, tempo por chamada, tempo total e
//!   tamanho do stdout são limitados; o processo filho é morto ao estourar (`LimiteExcedido`).
//! - Todo texto extraído é DADO. É guardado como veio; instruções suspeitas ("ignore as instruções
//!   anteriores...") são só marcadas (`alertas_conteudo`, aviso `conteudo_suspeito`) e nunca
//!   alteram a extração. Quem entregar texto a um modelo deve usar [`como_dado_nao_confiavel`].
//!
//! # Fora do escopo (B-22, PDF pesquisável)
//!
//! O derivado "PDF pesquisável" não foi implementado: o tesseract gera PDF de uma imagem por vez
//! (`tesseract stdin stdout pdf`), e juntar páginas num PDF único exige um montador de PDF (não há
//! um instalado, e Rust puro maduro para isso é `lopdf`, dependência a ser decidida com o dono).
//! O desenho previsto: gerar PDF por página em memória, unir em memória e gravar como derivado
//! separado pelo agente escritor, com hash próprio, sem tocar no original.
//!
//! A política de amostragem está documentada em [`motor`].

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod config;
pub mod djvu;
pub mod extrator;
pub mod ferramentas;
pub mod fonte;
pub mod metadados;
pub mod motor;
pub mod ocr;
pub mod pdf;
pub mod png;
pub mod proc;
pub mod relatorio;
pub mod texto;

pub use config::{ConfigExtracaoPdf, LimitesExecucao};
pub use extrator::Extrator;
pub use ferramentas::{ErroFerramentas, Ferramenta, Ferramentas, FerramentasDjvu};
pub use fonte::Fonte;
pub use relatorio::{ClassePagina, MedicaoPagina, RelatorioExtracao};
pub use texto::{AlertaConteudo, como_dado_nao_confiavel};
