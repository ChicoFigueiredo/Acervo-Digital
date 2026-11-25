//! Instantâneos BLAKE3 para provar antes/depois que nada mudou (CA-B01).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::erro::{ErroCalibre, Resultado};

/// Arquivo registrado no instantâneo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArquivoRegistrado {
    pub tamanho: u64,
    /// Modificação em nanossegundos desde a época Unix.
    pub mtime_ns: i128,
    /// BLAKE3 hexadecimal; `None` no escopo `Listagem`.
    pub blake3: Option<String>,
}

/// Quanto do conteúdo entra no instantâneo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Escopo {
    /// Só `metadata.db` e `metadata_db_prefs_backup.json` (com hash).
    Minimo,
    /// Todos os arquivos, só tamanho e mtime (barato; detecta criação/remoção/alteração).
    Listagem,
    /// Todos os arquivos com hash.
    Completo,
}

/// Fotografia de uma biblioteca.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instantaneo {
    pub raiz: PathBuf,
    pub escopo: Escopo,
    pub criado_em_unix: u64,
    /// Caminho relativo -> registro. Sempre contém os arquivos de metadados se existirem.
    pub arquivos: BTreeMap<String, ArquivoRegistrado>,
}

/// Uma diferença entre dois instantâneos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", rename_all = "snake_case")]
pub enum Diferenca {
    Adicionado {
        caminho: String,
    },
    Removido {
        caminho: String,
    },
    Alterado {
        caminho: String,
        campos: Vec<String>,
    },
}

impl Diferenca {
    #[must_use]
    pub fn caminho(&self) -> &str {
        match self {
            Self::Adicionado { caminho }
            | Self::Removido { caminho }
            | Self::Alterado { caminho, .. } => caminho,
        }
    }
}

/// Arquivos de metadados sempre incluídos.
pub const ARQUIVOS_METADADOS: [&str; 2] = ["metadata.db", "metadata_db_prefs_backup.json"];

/// BLAKE3 de um arquivo, em fluxo.
///
/// # Errors
/// Falha de E/S.
pub fn hash_arquivo(caminho: &Path) -> Resultado<String> {
    let mut f = File::open(caminho)
        .map_err(|e| ErroCalibre::io(format!("abrir {}", caminho.display()), e))?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| ErroCalibre::io(format!("ler {}", caminho.display()), e))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().to_hex().to_string())
}

fn registrar(caminho: &Path, com_hash: bool) -> Resultado<ArquivoRegistrado> {
    let md = std::fs::metadata(caminho)
        .map_err(|e| ErroCalibre::io(format!("stat {}", caminho.display()), e))?;
    let mtime_ns = md
        .modified()
        .ok()
        .map_or(0, |t| match t.duration_since(UNIX_EPOCH) {
            Ok(d) => i128::try_from(d.as_nanos()).unwrap_or(i128::MAX),
            Err(e) => -i128::try_from(e.duration().as_nanos()).unwrap_or(i128::MAX),
        });
    Ok(ArquivoRegistrado {
        tamanho: md.len(),
        mtime_ns,
        blake3: if com_hash {
            Some(hash_arquivo(caminho)?)
        } else {
            None
        },
    })
}

fn percorrer(base: &Path, atual: &Path, saida: &mut Vec<(String, PathBuf)>) -> Resultado<()> {
    let rd = std::fs::read_dir(atual)
        .map_err(|e| ErroCalibre::io(format!("listar {}", atual.display()), e))?;
    for ent in rd {
        let ent = ent.map_err(|e| ErroCalibre::io(format!("listar {}", atual.display()), e))?;
        let ft = ent
            .file_type()
            .map_err(|e| ErroCalibre::io(format!("tipo de {}", ent.path().display()), e))?;
        let p = ent.path();
        if ft.is_dir() {
            percorrer(base, &p, saida)?;
        } else if ft.is_file() {
            let rel = p
                .strip_prefix(base)
                .map_or_else(|_| p.clone(), Path::to_path_buf);
            saida.push((rel.to_string_lossy().into_owned(), p));
        }
        // Links simbólicos não são seguidos nem registrados.
    }
    Ok(())
}

/// Tira um instantâneo. `anterior` permite reaproveitar hashes de arquivos cujo tamanho e
/// mtime não mudaram (modo `Completo`); `metadata.db` e o backup de preferências são sempre
/// rehasheados.
///
/// # Errors
/// Falha de E/S.
pub fn tirar(
    raiz: &Path,
    escopo: Escopo,
    anterior: Option<&Instantaneo>,
) -> Resultado<Instantaneo> {
    let agora = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut arquivos = BTreeMap::new();
    for nome in ARQUIVOS_METADADOS {
        let p = raiz.join(nome);
        if p.is_file() {
            arquivos.insert(nome.to_owned(), registrar(&p, true)?);
        }
    }
    if escopo != Escopo::Minimo {
        let mut todos = Vec::new();
        percorrer(raiz, raiz, &mut todos)?;
        for (rel, p) in todos {
            if arquivos.contains_key(&rel) {
                continue;
            }
            let mut reg = registrar(&p, false)?;
            if escopo == Escopo::Completo {
                let reuso = anterior
                    .and_then(|a| a.arquivos.get(&rel))
                    .filter(|o| o.tamanho == reg.tamanho && o.mtime_ns == reg.mtime_ns)
                    .and_then(|o| o.blake3.clone());
                reg.blake3 = Some(match reuso {
                    Some(h) => h,
                    None => hash_arquivo(&p)?,
                });
            }
            arquivos.insert(rel, reg);
        }
    }
    Ok(Instantaneo {
        raiz: raiz.to_path_buf(),
        escopo,
        criado_em_unix: agora,
        arquivos,
    })
}

/// Compara dois instantâneos (antes, depois) e lista as diferenças. Lista vazia = nada mudou.
/// Hash só é comparado quando os dois lados o têm.
#[must_use]
pub fn comparar(antes: &Instantaneo, depois: &Instantaneo) -> Vec<Diferenca> {
    let mut out = Vec::new();
    for (c, a) in &antes.arquivos {
        match depois.arquivos.get(c) {
            None => out.push(Diferenca::Removido { caminho: c.clone() }),
            Some(d) => {
                let mut campos = Vec::new();
                if a.tamanho != d.tamanho {
                    campos.push("tamanho".to_owned());
                }
                if a.mtime_ns != d.mtime_ns {
                    campos.push("mtime".to_owned());
                }
                if let (Some(x), Some(y)) = (&a.blake3, &d.blake3) {
                    if x != y {
                        campos.push("blake3".to_owned());
                    }
                }
                if !campos.is_empty() {
                    out.push(Diferenca::Alterado {
                        caminho: c.clone(),
                        campos,
                    });
                }
            }
        }
    }
    for c in depois.arquivos.keys() {
        if !antes.arquivos.contains_key(c) {
            out.push(Diferenca::Adicionado { caminho: c.clone() });
        }
    }
    out
}
