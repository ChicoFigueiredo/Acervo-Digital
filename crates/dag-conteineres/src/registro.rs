//! Registro de formatos: a detecção é por bytes mágicos, em ordem de registro.

use std::io::{self, Seek, SeekFrom};
use std::sync::Arc;

use crate::{ErroConteiner, Fonte, FonteBox, LeitorConteiner};

/// Fábrica de um formato. Os leitores de Imagem de Disco implementam isto e se registram.
pub trait FabricaLeitor: Send + Sync {
    /// Nome estável do formato.
    fn nome(&self) -> &'static str;

    /// Decide pelos bytes da fonte (a posição inicial é 0; pode ler e andar à vontade) se este
    /// formato é o do conteúdo. Nunca olha a extensão.
    ///
    /// # Errors
    /// Falha de E/S ao ler a fonte.
    fn reconhece(&self, fonte: &mut dyn Fonte) -> io::Result<bool>;

    /// Triagem barata pelo começo do Membro (até 512 bytes): `false` só se **com certeza** não
    /// é deste formato. Evita carregar na memória todo Membro aninhado. Padrão: `true`.
    fn prefixo_promissor(&self, _prefixo: &[u8]) -> bool {
        true
    }

    /// Abre o leitor. A fonte está na posição 0. `senha` é a tentativa corrente, se houver.
    ///
    /// # Errors
    /// Cabeçalho inválido, senha necessária ou incorreta.
    fn abrir(
        &self,
        fonte: FonteBox,
        senha: Option<&str>,
    ) -> Result<Box<dyn LeitorConteiner>, ErroConteiner>;
}

/// Conjunto de fábricas consultadas em ordem.
#[derive(Clone)]
pub struct Registro {
    fabricas: Vec<Arc<dyn FabricaLeitor>>,
}

impl std::fmt::Debug for Registro {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.fabricas.iter().map(|x| x.nome()))
            .finish()
    }
}

impl Default for Registro {
    fn default() -> Self {
        Self::padrao()
    }
}

impl Registro {
    /// Sem nenhum formato (para testes e para montar um conjunto próprio).
    #[must_use]
    pub fn vazio() -> Self {
        Self {
            fabricas: Vec::new(),
        }
    }

    /// Os compactados desta crate: zip, 7z, rar, tar, tar.gz, tar.bz2, tar.xz e tar.zst.
    #[must_use]
    pub fn padrao() -> Self {
        let mut r = Self::vazio();
        for f in crate::formatos::fabricas_padrao() {
            r.registrar(f);
        }
        r
    }

    /// Acrescenta um formato (por exemplo, o leitor de Imagens de Disco).
    pub fn registrar(&mut self, fabrica: Arc<dyn FabricaLeitor>) {
        self.fabricas.push(fabrica);
    }

    /// Nomes dos formatos conhecidos, em ordem.
    #[must_use]
    pub fn formatos(&self) -> Vec<&'static str> {
        self.fabricas.iter().map(|f| f.nome()).collect()
    }

    /// Abre `fonte` com o primeiro formato que a reconhecer; `None` se não é um Contêiner.
    ///
    /// # Errors
    /// E/S, senha necessária/incorreta ou cabeçalho corrompido de um formato reconhecido.
    pub fn abrir(
        &self,
        mut fonte: FonteBox,
        senha: Option<&str>,
    ) -> Result<Option<Box<dyn LeitorConteiner>>, ErroConteiner> {
        for f in &self.fabricas {
            fonte.seek(SeekFrom::Start(0))?;
            if reconhece(f.as_ref(), &mut *fonte)? {
                fonte.seek(SeekFrom::Start(0))?;
                return f.abrir(fonte, senha).map(Some);
            }
        }
        Ok(None)
    }

    /// `true` se alguma fábrica acha o começo `prefixo` promissor.
    #[must_use]
    pub fn candidato(&self, prefixo: &[u8]) -> bool {
        self.fabricas.iter().any(|f| f.prefixo_promissor(prefixo))
    }

    /// Só detecta (nome do formato), sem abrir.
    ///
    /// # Errors
    /// Falha de E/S.
    pub fn detectar(&self, fonte: &mut dyn Fonte) -> io::Result<Option<&'static str>> {
        for f in &self.fabricas {
            fonte.seek(SeekFrom::Start(0))?;
            if reconhece(f.as_ref(), fonte)? {
                return Ok(Some(f.nome()));
            }
        }
        Ok(None)
    }
}

/// Fonte curta demais para o formato (fim inesperado) = "não é este formato".
fn reconhece(f: &dyn FabricaLeitor, fonte: &mut dyn Fonte) -> io::Result<bool> {
    match f.reconhece(fonte) {
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        r => r,
    }
}
