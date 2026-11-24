//! Cofre de Senhas (ADR 0006): cifra as senhas dos Contêineres antes de irem ao Catálogo.
//!
//! O Catálogo guarda só `nonce (24 B) + texto cifrado` (XChaCha20-Poly1305). A chave de 256 bits
//! fica no keyring do sistema (ou no arquivo cifrado por senha-mestra da `bancada-segredos`), sob
//! o rótulo [`ROTULO_CHAVE`]; nunca no Catálogo, nunca em log. Sem keyring utilizável, o Cofre
//! fica indisponível e os Contêineres cifrados esperam ("aguardando o chaveiro").

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use bancada_segredos::{
    ChaveiroDoSistema, ConfiguracaoDaGuarda, ErroDeSegredo, GuardaDeSegredos, Segredo, Situacao,
};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use dag_core::SituacaoCofre;
use dag_scanner::Cofre;

/// Rótulo da chave de cifra no keyring.
pub const ROTULO_CHAVE: &str = "dag:chave-catalogo";
const SERVICO: &str = "Acervo Digital (DAG)";
const NONCE: usize = 24;

/// Cofre de Senhas sobre a guarda de segredos do sistema.
pub struct CofreSistema {
    guarda: GuardaDeSegredos,
    chave: Mutex<Option<[u8; 32]>>,
}

impl fmt::Debug for CofreSistema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CofreSistema")
            .field("situacao", &self.situacao())
            .finish_non_exhaustive()
    }
}

impl CofreSistema {
    /// Abre a guarda. `diretorio` guarda o arquivo cifrado de reserva (se o usuário criar um).
    /// `chaveiro`: `None` usa o keyring nativo; os testes passam um falso.
    ///
    /// # Errors
    /// Falha ao abrir o índice da guarda.
    pub fn abrir(
        diretorio: &Path,
        chaveiro: Option<Arc<dyn ChaveiroDoSistema>>,
    ) -> Result<Self, ErroDeSegredo> {
        let mut cfg = ConfiguracaoDaGuarda::nova(SERVICO, diretorio);
        if let Some(c) = chaveiro {
            cfg = cfg.chaveiro(c);
        }
        Ok(Self {
            guarda: GuardaDeSegredos::abrir(cfg)?,
            chave: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn situacao(&self) -> SituacaoCofre {
        match self.guarda.situacao() {
            Situacao::ChaveiroDoSistema => SituacaoCofre::Chaveiro,
            Situacao::ArquivoTrancado => SituacaoCofre::Trancado,
            Situacao::ArquivoAberto => SituacaoCofre::Aberto,
            Situacao::Indisponivel => SituacaoCofre::Indisponivel,
        }
    }

    /// Chave de cifra; `criar` gera uma nova se ainda não existir.
    fn chave(&self, criar: bool) -> Result<[u8; 32], ErroDeSegredo> {
        let mut cache = self
            .chave
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(c) = *cache {
            return Ok(c);
        }
        let chave = match self.guarda.ler(ROTULO_CHAVE) {
            Ok(s) => decodificar_chave(s.expor()).ok_or(ErroDeSegredo::IndiceCorrompido)?,
            Err(ErroDeSegredo::NaoEncontrado) if criar => {
                let mut c = [0u8; 32];
                getrandom::fill(&mut c).map_err(|_| ErroDeSegredo::Indisponivel)?;
                self.guarda
                    .gravar(ROTULO_CHAVE, Segredo::novo(codificar_chave(&c)))?;
                c
            }
            Err(e) => return Err(e),
        };
        *cache = Some(chave);
        Ok(chave)
    }
}

impl Cofre for CofreSistema {
    fn disponivel(&self) -> bool {
        match self.guarda.situacao() {
            Situacao::ChaveiroDoSistema | Situacao::ArquivoAberto => {}
            Situacao::ArquivoTrancado | Situacao::Indisponivel => return false,
        }
        // Sem chave ainda é utilizável (ela nasce na primeira senha); keyring recusando, não.
        matches!(self.chave(false), Ok(_) | Err(ErroDeSegredo::NaoEncontrado))
    }

    fn cifrar(&self, senha: &str) -> Option<Vec<u8>> {
        let chave = self.chave(true).ok()?;
        let cifra = XChaCha20Poly1305::new_from_slice(&chave).ok()?;
        let mut nonce = [0u8; NONCE];
        getrandom::fill(&mut nonce).ok()?;
        let texto = cifra.encrypt(&XNonce::from(nonce), senha.as_bytes()).ok()?;
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&texto);
        Some(blob)
    }

    fn decifrar(&self, blob: &[u8]) -> Option<String> {
        if blob.len() <= NONCE {
            return None;
        }
        let chave = self.chave(false).ok()?;
        let cifra = XChaCha20Poly1305::new_from_slice(&chave).ok()?;
        let (nonce, texto) = blob.split_at(NONCE);
        let nonce: [u8; NONCE] = nonce.try_into().ok()?;
        let claro = cifra.decrypt(&XNonce::from(nonce), texto).ok()?;
        String::from_utf8(claro).ok()
    }
}

/// Cofre que só abre a guarda (e o keyring) na primeira vez que alguém o usa: um agente que nunca
/// encontra Contêiner cifrado não toca o keyring.
pub struct CofreLazy {
    diretorio: PathBuf,
    chaveiro: Option<Arc<dyn ChaveiroDoSistema>>,
    interno: OnceLock<Option<CofreSistema>>,
}

impl fmt::Debug for CofreLazy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CofreLazy").finish_non_exhaustive()
    }
}

impl CofreLazy {
    #[must_use]
    pub fn novo(diretorio: PathBuf, chaveiro: Option<Arc<dyn ChaveiroDoSistema>>) -> Self {
        Self {
            diretorio,
            chaveiro,
            interno: OnceLock::new(),
        }
    }

    fn interno(&self) -> Option<&CofreSistema> {
        self.interno
            .get_or_init(|| CofreSistema::abrir(&self.diretorio, self.chaveiro.clone()).ok())
            .as_ref()
    }

    #[must_use]
    pub fn situacao(&self) -> SituacaoCofre {
        self.interno()
            .map_or(SituacaoCofre::Indisponivel, CofreSistema::situacao)
    }
}

impl Cofre for CofreLazy {
    fn disponivel(&self) -> bool {
        self.interno().is_some_and(Cofre::disponivel)
    }

    fn cifrar(&self, senha: &str) -> Option<Vec<u8>> {
        self.interno()?.cifrar(senha)
    }

    fn decifrar(&self, blob: &[u8]) -> Option<String> {
        self.interno()?.decifrar(blob)
    }
}

fn codificar_chave(c: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    c.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn decodificar_chave(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut c = [0u8; 32];
    for (i, b) in c.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(c)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use bancada_segredos::testes::ChaveiroEmMemoria;

    fn cofre(c: Arc<ChaveiroEmMemoria>) -> (tempfile::TempDir, CofreSistema) {
        let dir = tempfile::tempdir().unwrap();
        let cofre = CofreSistema::abrir(dir.path(), Some(c)).unwrap();
        (dir, cofre)
    }

    #[test]
    fn cifra_e_decifra_sem_deixar_a_senha_no_blob() {
        let (_d, c) = cofre(Arc::new(ChaveiroEmMemoria::novo()));
        assert!(c.disponivel());
        let blob = c.cifrar("segredo-123").unwrap();
        assert!(!blob.windows(11).any(|w| w == b"segredo-123"));
        assert_eq!(c.decifrar(&blob).as_deref(), Some("segredo-123"));
        // Duas cifras da mesma senha diferem (nonce aleatório).
        assert_ne!(blob, c.cifrar("segredo-123").unwrap());
    }

    #[test]
    fn chave_persiste_no_keyring_entre_aberturas() {
        let kr = Arc::new(ChaveiroEmMemoria::novo());
        let (_d, c) = cofre(Arc::clone(&kr));
        let blob = c.cifrar("abc").unwrap();
        let (_d2, c2) = cofre(Arc::clone(&kr));
        assert_eq!(c2.decifrar(&blob).as_deref(), Some("abc"));
        assert_eq!(kr.entradas().len(), 1);
    }

    #[test]
    fn sem_keyring_o_cofre_fica_indisponivel() {
        let (_d, c) = cofre(Arc::new(ChaveiroEmMemoria::indisponivel()));
        assert!(!c.disponivel());
        assert_eq!(c.situacao(), SituacaoCofre::Indisponivel);
        assert!(c.cifrar("x").is_none());
    }

    #[test]
    fn blob_adulterado_nao_decifra() {
        let (_d, c) = cofre(Arc::new(ChaveiroEmMemoria::novo()));
        let mut blob = c.cifrar("abc").unwrap();
        let n = blob.len();
        blob[n - 1] ^= 1;
        assert!(c.decifrar(&blob).is_none());
    }
}
