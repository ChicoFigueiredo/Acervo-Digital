//! Detecção das ferramentas externas (versão e presença).

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::proc::{Entrada, executar};

/// Uma ferramenta encontrada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ferramenta {
    pub nome: String,
    pub caminho: PathBuf,
    /// Primeira linha de versão que a ferramenta imprime (texto livre).
    pub versao: String,
}

/// Ferramentas do djvulibre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FerramentasDjvu {
    pub djvused: Ferramenta,
    pub djvutxt: Ferramenta,
    pub ddjvu: Ferramenta,
}

/// Conjunto de ferramentas disponíveis nesta máquina.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ferramentas {
    pub pdfinfo: Ferramenta,
    pub pdftotext: Ferramenta,
    pub pdftoppm: Ferramenta,
    /// Sem ela, a detecção de páginas-imagem fica às cegas (o OCR ainda roda).
    pub pdfimages: Option<Ferramenta>,
    /// Sem ela, não há sumário por marcadores (bookmarks) do PDF.
    pub pdftohtml: Option<Ferramenta>,
    /// Sem ela, não há OCR: páginas sem texto ficam sem texto, com aviso.
    pub tesseract: Option<Ferramenta>,
    /// Idiomas instalados no tesseract (sem `osd`).
    pub idiomas_tesseract: Vec<String>,
    /// Sem elas, DjVu é `NaoSuportado`.
    pub djvu: Option<FerramentasDjvu>,
}

#[derive(Debug, thiserror::Error)]
pub enum ErroFerramentas {
    #[error("ferramenta obrigatória ausente: `{nome}`. {dica}")]
    Ausente { nome: String, dica: String },
}

fn procurar(nome: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let extra: Vec<PathBuf> = std::env::split_paths(&path).collect();
    dirs.iter().chain(extra.iter()).find_map(|d| {
        let c = d.join(nome);
        let meta = std::fs::metadata(&c).ok()?;
        (meta.is_file() && meta.permissions().mode() & 0o111 != 0).then_some(c)
    })
}

fn primeira_linha_de_versao(texto: &str) -> String {
    texto
        .lines()
        .map(str::trim)
        .find(|l| l.chars().any(|c| c.is_ascii_digit()))
        .unwrap_or("versão desconhecida")
        .chars()
        .take(100)
        .collect()
}

fn sondar(nome: &str, dirs: &[PathBuf], arg_versao: &str) -> Option<Ferramenta> {
    let caminho = procurar(nome, dirs)?;
    let versao = executar(
        &caminho,
        &[OsString::from(arg_versao)],
        Entrada::Nenhuma,
        &[],
        Duration::from_secs(10),
        64 * 1024,
    )
    .map(|s| {
        primeira_linha_de_versao(&format!(
            "{}\n{}",
            String::from_utf8_lossy(&s.stdout),
            s.stderr
        ))
    })
    .unwrap_or_else(|_| "versão desconhecida".into());
    Some(Ferramenta {
        nome: nome.into(),
        caminho,
        versao,
    })
}

impl Ferramentas {
    /// Procura no `PATH`.
    pub fn detectar() -> Result<Self, ErroFerramentas> {
        Self::detectar_em(&[])
    }

    /// Procura primeiro em `dirs` (útil em testes e instalações em pastas próprias), depois no `PATH`.
    pub fn detectar_em(dirs: &[PathBuf]) -> Result<Self, ErroFerramentas> {
        let obrigatoria = |nome: &str| {
            sondar(nome, dirs, "-v").ok_or_else(|| ErroFerramentas::Ausente {
                nome: nome.into(),
                dica: "instale o poppler-utils (Ubuntu: `sudo apt install poppler-utils`)".into(),
            })
        };
        let pdfinfo = obrigatoria("pdfinfo")?;
        let pdftotext = obrigatoria("pdftotext")?;
        let pdftoppm = obrigatoria("pdftoppm")?;
        let pdfimages = sondar("pdfimages", dirs, "-v");
        let pdftohtml = sondar("pdftohtml", dirs, "-v");
        let tesseract = sondar("tesseract", dirs, "--version");
        let idiomas_tesseract = tesseract
            .as_ref()
            .map(|t| listar_idiomas(&t.caminho))
            .unwrap_or_default();
        let djvu = match (
            sondar("djvused", dirs, "-h"),
            sondar("djvutxt", dirs, "--help"),
            sondar("ddjvu", dirs, "--help"),
        ) {
            (Some(djvused), Some(djvutxt), Some(ddjvu)) => Some(FerramentasDjvu {
                djvused,
                djvutxt,
                ddjvu,
            }),
            _ => None,
        };
        Ok(Self {
            pdfinfo,
            pdftotext,
            pdftoppm,
            pdfimages,
            pdftohtml,
            tesseract,
            idiomas_tesseract,
            djvu,
        })
    }

    /// Idiomas do OCR que existem de fato, na ordem pedida, unidos por `+` (formato do tesseract).
    #[must_use]
    pub fn idiomas_ocr_efetivos(&self, pedidos: &[String]) -> Option<String> {
        let ok: Vec<&str> = pedidos
            .iter()
            .filter(|p| self.idiomas_tesseract.iter().any(|i| i == *p))
            .map(String::as_str)
            .collect();
        (!ok.is_empty()).then(|| ok.join("+"))
    }
}

fn listar_idiomas(tesseract: &Path) -> Vec<String> {
    let Ok(s) = executar(
        tesseract,
        &[OsString::from("--list-langs")],
        Entrada::Nenhuma,
        &[],
        Duration::from_secs(10),
        64 * 1024,
    ) else {
        return Vec::new();
    };
    let texto = format!("{}\n{}", String::from_utf8_lossy(&s.stdout), s.stderr);
    texto
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "osd" && !l.contains(' '))
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn versao_pega_primeira_linha_numerada() {
        assert_eq!(
            primeira_linha_de_versao("\npdfinfo version 24.02.0\nCopyright"),
            "pdfinfo version 24.02.0"
        );
        assert_eq!(primeira_linha_de_versao(""), "versão desconhecida");
    }

    #[test]
    fn ausencia_de_ferramenta_obrigatoria_e_erro_claro() {
        let vazio = tempfile::tempdir().unwrap();
        assert!(procurar("pdfinfo-que-nao-existe", &[vazio.path().to_path_buf()]).is_none());
    }

    #[test]
    fn idiomas_efetivos_ignoram_ausentes() {
        let f = Ferramentas {
            pdfinfo: dummy(),
            pdftotext: dummy(),
            pdftoppm: dummy(),
            pdfimages: None,
            pdftohtml: None,
            tesseract: None,
            idiomas_tesseract: vec!["eng".into(), "por".into()],
            djvu: None,
        };
        assert_eq!(
            f.idiomas_ocr_efetivos(&["por".into(), "deu".into(), "eng".into()]),
            Some("por+eng".into())
        );
        assert_eq!(f.idiomas_ocr_efetivos(&["deu".into()]), None);
    }

    fn dummy() -> Ferramenta {
        Ferramenta {
            nome: "x".into(),
            caminho: "/x".into(),
            versao: "1".into(),
        }
    }
}
