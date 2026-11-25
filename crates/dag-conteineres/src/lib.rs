//! Leitura de Contêineres (ADR 0006): compactados (zip, 7z, rar, tar e tar comprimido) lidos
//! **em fluxo e na memória**. Nada é extraído, montado ou gravado em disco, nem em temporário.
//!
//! # Desenho
//!
//! * [`LeitorConteiner`] é o contrato de um formato: percorre os Membros em ordem e entrega a
//!   cada um o [`MembroInfo`] e um `&mut dyn Read` com o conteúdo. Os leitores de Imagem de
//!   Disco (crate própria) plugam aqui implementando [`FabricaLeitor`] e registrando-se num
//!   [`Registro`]; o resto do DAG não muda.
//! * O formato é decidido por **bytes mágicos** ([`FabricaLeitor::reconhece`]), nunca pela
//!   extensão.
//! * [`ler_conteiner`] é a travessia completa: calcula o BLAKE3 de cada Membro **na mesma
//!   leitura** e desce em Contêineres aninhados (limite de profundidade e de memória).

mod aninhado;
mod erro;
mod formatos;
mod imagens;
mod registro;
mod tempo;

pub use aninhado::{
    EstadoMembro, MembroLido, Opcoes, PontoDeRetomada, ResultadoLeitura, ler_conteiner,
    ler_conteiner_com_pontos,
};
pub use erro::ErroConteiner;
pub use imagens::FabricaImagens;
pub use registro::{FabricaLeitor, Registro};

use std::io::{Read, Seek};

/// Fonte de bytes de um Contêiner: leitura com posicionamento, em memória ou num arquivo.
pub trait Fonte: Read + Seek + Send {}
impl<T: Read + Seek + Send> Fonte for T {}

/// Fonte com dono, como os leitores a recebem.
pub type FonteBox = Box<dyn Fonte>;

/// Natureza de um Membro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipoMembro {
    Arquivo,
    Pasta,
    /// Link simbólico ou físico: tem destino, não conteúdo próprio.
    Link,
    Outro,
}

impl TipoMembro {
    /// Nome estável gravado no Catálogo.
    #[must_use]
    pub fn nome(self) -> &'static str {
        match self {
            Self::Arquivo => "arquivo",
            Self::Pasta => "pasta",
            Self::Link => "link",
            Self::Outro => "outro",
        }
    }
}

/// Metadados de um Membro, como o formato os guarda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembroInfo {
    /// Caminho interno, bytes exatos do formato.
    pub caminho: Vec<u8>,
    /// Tamanho declarado (nem todo formato sabe antes de ler).
    pub tamanho: Option<u64>,
    /// Nanossegundos Unix.
    pub mtime_ns: Option<i64>,
    pub tipo: TipoMembro,
    /// O conteúdo do Membro é cifrado.
    pub cifrado: bool,
}

/// O que a visita decide depois de cada Membro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Controle {
    Continuar,
    Parar,
}

/// Visita a um Membro. O leitor entrega o conteúdo em fluxo; ler menos do que o total é
/// permitido (o leitor descarta o resto quando o formato exige).
///
/// Erros de leitura do conteúdo: `ErrorKind::Unsupported` = método de compressão sem suporte
/// só neste Membro; `ErrorKind::PermissionDenied` = senha incorreta; demais = Contêiner
/// corrompido.
pub type Visita<'a> = dyn FnMut(MembroInfo, &mut dyn Read) -> Result<Controle, ErroConteiner> + 'a;

/// Totais de uma travessia.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Resumo {
    pub membros: u64,
    pub bytes: u64,
}

/// Contrato de um formato de Contêiner.
pub trait LeitorConteiner {
    /// Nome estável do formato (`zip`, `7z`, `rar`, `tar`, `tar.gz`...), gravado no Catálogo.
    fn formato(&self) -> &'static str;

    /// Percorre todos os Membros em ordem, chamando `visita` para cada um.
    ///
    /// # Errors
    /// [`ErroConteiner::SenhaNecessaria`] / [`ErroConteiner::SenhaIncorreta`] (antes de visitar
    /// qualquer Membro, quando o formato permite saber), corrupção, E/S ou o erro da visita.
    fn percorrer(&mut self, visita: &mut Visita<'_>) -> Result<Resumo, ErroConteiner>;

    /// A ordem dos Membros é estável e pular os `n` primeiros sem ler o conteúdo é barato
    /// (imagens de disco: só percorre diretórios). Só leitores assim podem retomar uma leitura
    /// interrompida do último Membro concluído, em vez de recomeçar do zero.
    fn retomavel(&self) -> bool {
        false
    }

    /// Problemas toleráveis achados no percurso (partição não lida, diretório corrompido...).
    /// Consultado depois de `percorrer`.
    fn avisos(&self) -> Vec<String> {
        Vec::new()
    }
}
