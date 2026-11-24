//! zip (inclui zip64, deflate, deflate64, bzip2, lzma e AES/ZipCrypto).
//! Zstd dentro de zip exigiria a `zstd` em C: esse Membro fica "não suportado".

use std::io::{self, Read};

use zip::ZipArchive;
use zip::result::ZipError;

use crate::formatos::LeitorComErro;
use crate::tempo::civil_para_ns;
use crate::{
    Controle, ErroConteiner, FabricaLeitor, Fonte, FonteBox, LeitorConteiner, MembroInfo, Resumo,
    TipoMembro, Visita,
};

#[derive(Debug)]
pub(crate) struct FabricaZip;

impl FabricaLeitor for FabricaZip {
    fn nome(&self) -> &'static str {
        "zip"
    }

    fn reconhece(&self, fonte: &mut dyn Fonte) -> io::Result<bool> {
        let mut m = [0u8; 4];
        let n = super::ler_ate_encher(fonte, &mut m)?;
        Ok(n == 4 && (m == *b"PK\x03\x04" || m == *b"PK\x05\x06"))
    }

    fn prefixo_promissor(&self, p: &[u8]) -> bool {
        p.starts_with(b"PK\x03\x04") || p.starts_with(b"PK\x05\x06")
    }

    fn abrir(
        &self,
        fonte: FonteBox,
        senha: Option<&str>,
    ) -> Result<Box<dyn LeitorConteiner>, ErroConteiner> {
        let arq = ZipArchive::new(fonte).map_err(erro_zip)?;
        Ok(Box::new(LeitorZip {
            arq,
            senha: senha.map(str::to_owned),
        }))
    }
}

struct LeitorZip {
    arq: ZipArchive<FonteBox>,
    senha: Option<String>,
}

fn erro_zip(e: ZipError) -> ErroConteiner {
    match e {
        ZipError::InvalidPassword => ErroConteiner::SenhaIncorreta,
        ZipError::Io(e) => ErroConteiner::de_leitura(e),
        ZipError::UnsupportedArchive(m) => ErroConteiner::NaoSuportado(m.to_string()),
        outro => ErroConteiner::Corrompido(outro.to_string()),
    }
}

/// Converte um erro de leitura do conteúdo cifrado: checksum/HMAC ruim = senha errada.
struct ConteudoCifrado<R>(R);

impl<R: Read> Read for ConteudoCifrado<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf).map_err(|e| {
            if e.kind() == io::ErrorKind::InvalidData {
                io::Error::new(io::ErrorKind::PermissionDenied, "senha incorreta")
            } else {
                e
            }
        })
    }
}

impl LeitorConteiner for LeitorZip {
    fn formato(&self) -> &'static str {
        "zip"
    }

    fn percorrer(&mut self, visita: &mut Visita<'_>) -> Result<Resumo, ErroConteiner> {
        let n = self.arq.len();
        // Antes de visitar qualquer Membro: se há conteúdo cifrado, a senha tem de abrir o
        // primeiro Membro cifrado (ZipCrypto só confere 1 byte; o resto se vê ao ler).
        let mut primeiro_cifrado = None;
        for i in 0..n {
            let e = self.arq.by_index_data(i).map_err(erro_zip)?;
            if e.encrypted() {
                primeiro_cifrado = Some(i);
                break;
            }
        }
        if let Some(i) = primeiro_cifrado {
            let senha = self
                .senha
                .as_deref()
                .ok_or(ErroConteiner::SenhaNecessaria)?;
            match self.arq.by_index_decrypt(i, senha.as_bytes()) {
                Ok(_) => {}
                Err(ZipError::InvalidPassword) => return Err(ErroConteiner::SenhaIncorreta),
                // Método sem suporte no Membro de teste: a senha só se confirma lendo.
                Err(ZipError::UnsupportedArchive(_)) => {}
                Err(e) => return Err(erro_zip(e)),
            }
        }
        let mut resumo = Resumo::default();
        for i in 0..n {
            let (info, sem_conteudo) = {
                let e = self.arq.by_index_data(i).map_err(erro_zip)?;
                let tipo = if e.is_dir() {
                    TipoMembro::Pasta
                } else if e.is_symlink() {
                    TipoMembro::Link
                } else {
                    TipoMembro::Arquivo
                };
                let mtime_ns = e.last_modified().map(|d| {
                    civil_para_ns(
                        i64::from(d.year()),
                        i64::from(d.month()),
                        i64::from(d.day()),
                        i64::from(d.hour()),
                        i64::from(d.minute()),
                        i64::from(d.second()),
                    )
                });
                (
                    MembroInfo {
                        caminho: e.name_raw().to_vec(),
                        tamanho: Some(e.size()),
                        mtime_ns,
                        tipo,
                        cifrado: e.encrypted(),
                    },
                    tipo != TipoMembro::Arquivo,
                )
            };
            resumo.membros += 1;
            let controle = if sem_conteudo {
                visita(info, &mut io::empty())?
            } else {
                let cifrado = info.cifrado;
                let tamanho = info.tamanho.unwrap_or(0);
                let aberto = if cifrado {
                    let s = self.senha.as_deref().unwrap_or_default();
                    self.arq.by_index_decrypt(i, s.as_bytes())
                } else {
                    self.arq.by_index(i)
                };
                match aberto {
                    Ok(mut f) => {
                        resumo.bytes += tamanho;
                        if cifrado {
                            visita(info, &mut ConteudoCifrado(&mut f))?
                        } else {
                            visita(info, &mut f)?
                        }
                    }
                    Err(ZipError::InvalidPassword) => return Err(ErroConteiner::SenhaIncorreta),
                    Err(ZipError::UnsupportedArchive(m)) => visita(
                        info,
                        &mut LeitorComErro(io::ErrorKind::Unsupported, m.to_string()),
                    )?,
                    Err(e) => return Err(erro_zip(e)),
                }
            };
            if controle == Controle::Parar {
                break;
            }
        }
        Ok(resumo)
    }
}
