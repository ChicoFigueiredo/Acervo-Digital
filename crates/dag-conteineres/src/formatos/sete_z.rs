//! 7z (LZMA, LZMA2, BZIP2, COPY, BCJ... e AES-256 com cabeçalho cifrado ou não).

use std::io::{self, Read};

use sevenz_rust2::{ArchiveReader, EntryDecision, Error as Erro7z, Password};

use crate::{
    Controle, ErroConteiner, FabricaLeitor, Fonte, FonteBox, LeitorConteiner, MembroInfo, Resumo,
    TipoMembro, Visita,
};

const MAGICA: [u8; 6] = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];
/// 100 ns entre 1601-01-01 e 1970-01-01.
const NT_PARA_UNIX_100NS: u64 = 134_774 * 86_400 * 10_000_000;

#[derive(Debug)]
pub(crate) struct FabricaSeteZ;

impl FabricaLeitor for FabricaSeteZ {
    fn nome(&self) -> &'static str {
        "7z"
    }

    fn reconhece(&self, fonte: &mut dyn Fonte) -> io::Result<bool> {
        let mut m = [0u8; 6];
        let n = super::ler_ate_encher(fonte, &mut m)?;
        Ok(n == 6 && m == MAGICA)
    }

    fn prefixo_promissor(&self, p: &[u8]) -> bool {
        p.starts_with(&MAGICA)
    }

    fn abrir(
        &self,
        fonte: FonteBox,
        senha: Option<&str>,
    ) -> Result<Box<dyn LeitorConteiner>, ErroConteiner> {
        let pw = senha.map_or_else(Password::empty, Password::new);
        let leitor = ArchiveReader::new(fonte, pw).map_err(|e| match e {
            Erro7z::PasswordRequired => ErroConteiner::SenhaNecessaria,
            Erro7z::MaybeBadPassword(_) if senha.is_some() => ErroConteiner::SenhaIncorreta,
            Erro7z::MaybeBadPassword(_) => ErroConteiner::SenhaNecessaria,
            outro => erro_7z(outro),
        })?;
        Ok(Box::new(LeitorSeteZ {
            leitor,
            tem_senha: senha.is_some(),
        }))
    }
}

fn erro_7z(e: Erro7z) -> ErroConteiner {
    match e {
        Erro7z::PasswordRequired => ErroConteiner::SenhaNecessaria,
        Erro7z::MaybeBadPassword(_) => ErroConteiner::SenhaIncorreta,
        Erro7z::UnsupportedCompressionMethod(m) => ErroConteiner::NaoSuportado(m),
        Erro7z::Unsupported(m) => ErroConteiner::NaoSuportado(m.to_string()),
        Erro7z::Io(e, _) | Erro7z::FileOpen(e, _) => ErroConteiner::de_leitura(e),
        outro => ErroConteiner::Corrompido(outro.to_string()),
    }
}

struct LeitorSeteZ {
    leitor: ArchiveReader<FonteBox>,
    tem_senha: bool,
}

/// Num arquivo cifrado, qualquer falha ao ler o conteúdo é, em primeiro lugar, senha errada.
struct ConteudoCifrado<'a>(&'a mut dyn Read);

impl Read for ConteudoCifrado<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf).map_err(|e| {
            if e.kind() == io::ErrorKind::Unsupported {
                e
            } else {
                io::Error::new(io::ErrorKind::PermissionDenied, "senha incorreta")
            }
        })
    }
}

impl LeitorConteiner for LeitorSeteZ {
    fn formato(&self) -> &'static str {
        "7z"
    }

    fn percorrer(&mut self, visita: &mut Visita<'_>) -> Result<Resumo, ErroConteiner> {
        let arq = self.leitor.archive();
        let cifrado_no_conteudo = (0..arq.files.len()).any(|i| arq.is_file_encrypted(i));
        if cifrado_no_conteudo && !self.tem_senha {
            return Err(ErroConteiner::SenhaNecessaria);
        }
        let mut resumo = Resumo::default();
        let mut falha: Option<ErroConteiner> = None;
        let r = self.leitor.for_each_entries_with_decision(|e, leitor| {
            if e.is_anti_item {
                return Ok(EntryDecision::Continue);
            }
            let tipo = if e.is_directory {
                TipoMembro::Pasta
            } else {
                TipoMembro::Arquivo
            };
            let ft: u64 = e.last_modified_date.into();
            let mtime_ns = (e.has_last_modified_date && ft >= NT_PARA_UNIX_100NS)
                .then(|| i64::try_from((ft - NT_PARA_UNIX_100NS).saturating_mul(100)).ok())
                .flatten();
            let info = MembroInfo {
                caminho: e.name.as_bytes().to_vec(),
                tamanho: Some(e.size),
                mtime_ns,
                tipo,
                cifrado: cifrado_no_conteudo && e.has_stream,
            };
            resumo.membros += 1;
            resumo.bytes += e.size;
            let c = if cifrado_no_conteudo {
                visita(info, &mut ConteudoCifrado(leitor))
            } else {
                visita(info, leitor)
            };
            match c {
                Ok(Controle::Continuar) => Ok(EntryDecision::Continue),
                Ok(Controle::Parar) => Ok(EntryDecision::Stop),
                Err(e) => {
                    falha = Some(e);
                    Err(Erro7z::Other("visita interrompida".into()))
                }
            }
        });
        if let Some(e) = falha {
            return Err(e);
        }
        match r {
            Ok(()) => Ok(resumo),
            Err(Erro7z::ChecksumVerificationFailed) if cifrado_no_conteudo => {
                Err(ErroConteiner::SenhaIncorreta)
            }
            Err(e) => Err(erro_7z(e)),
        }
    }
}
