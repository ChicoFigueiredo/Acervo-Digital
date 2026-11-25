//! Adaptador das Imagens de Disco (`dag-imagens`) para o contrato de Contêiner.
//!
//! * Detecção por bytes mágicos: VHDX, qcow2, ISO/UDF e raw **só com tabela de partições
//!   (MBR/GPT) ou assinatura de sistema de arquivos conhecida** (nunca um arquivo qualquer).
//! * O caminho interno leva o prefixo da partição (`p1/etc/passwd`); o índice é a ordem de
//!   descoberta mais um.
//! * Hard links (`repetido`) são pulados: o conteúdo já foi visto no mesmo `id`.
//! * Link simbólico é [`TipoMembro::Link`] (sem conteúdo próprio, nunca lido).
//! * Avisos e partições não lidas vão para [`LeitorConteiner::avisos`] (viram uma Falha de
//!   Leitura agregada por imagem na Fila de Serviço).
//! * É **retomável**: a ordem de percurso (partição, diretório) é determinística e pular um
//!   Membro não lê o conteúdo dele, só caminha pelos diretórios.

use std::io;

use dag_imagens::{Controle as CImg, Erro, Imagem, MembroImagem, TipoMembro as TImg};

use crate::{
    Controle, ErroConteiner, FabricaLeitor, Fonte, FonteBox, LeitorConteiner, MembroInfo, Resumo,
    TipoMembro, Visita,
};

/// Fábrica do leitor de Imagens de Disco.
#[derive(Debug, Default, Clone, Copy)]
pub struct FabricaImagens;

impl FabricaLeitor for FabricaImagens {
    fn nome(&self) -> &'static str {
        "imagem"
    }

    fn reconhece(&self, fonte: &mut dyn Fonte) -> io::Result<bool> {
        Ok(dag_imagens::detectar(&mut Adaptador(fonte)).is_some())
    }

    /// Dentro de outro Contêiner só se reconhece pelo começo: VHDX, qcow2 ou setor de boot
    /// com assinatura `55 AA` (MBR, ou o MBR protetor de um disco GPT).
    fn prefixo_promissor(&self, p: &[u8]) -> bool {
        p.starts_with(b"vhdxfile")
            || p.starts_with(b"QFI\xfb")
            || (p.len() >= 512 && p[510] == 0x55 && p[511] == 0xAA)
    }

    fn abrir(
        &self,
        fonte: FonteBox,
        _senha: Option<&str>,
    ) -> Result<Box<dyn LeitorConteiner>, ErroConteiner> {
        let img = dag_imagens::abrir(fonte).map_err(erro)?;
        Ok(Box::new(LeitorImagem {
            img,
            avisos: Vec::new(),
        }))
    }
}

/// `&mut dyn Fonte` como `Read + Seek` sized.
struct Adaptador<'a>(&'a mut dyn Fonte);

impl io::Read for Adaptador<'_> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.0.read(b)
    }
}

impl io::Seek for Adaptador<'_> {
    fn seek(&mut self, p: io::SeekFrom) -> io::Result<u64> {
        self.0.seek(p)
    }
}

fn erro(e: Erro) -> ErroConteiner {
    match e {
        Erro::Io(e) => ErroConteiner::de_leitura(e),
        Erro::Desconhecido => ErroConteiner::NaoSuportado("formato de imagem desconhecido".into()),
        Erro::Corrompido(m) => ErroConteiner::Corrompido(m),
        Erro::SemSuporte(m) => ErroConteiner::NaoSuportado(m),
        Erro::Visita(b) => match b.downcast::<ErroConteiner>() {
            Ok(e) => *e,
            Err(b) => ErroConteiner::Corrompido(b.to_string()),
        },
    }
}

struct LeitorImagem {
    img: Imagem,
    avisos: Vec<String>,
}

fn converter(m: &MembroImagem) -> MembroInfo {
    let mut caminho = format!("p{}/", m.particao + 1).into_bytes();
    caminho.extend_from_slice(&m.caminho);
    MembroInfo {
        caminho,
        tamanho: (m.tipo == TImg::Arquivo).then_some(m.tamanho),
        mtime_ns: m.mtime.and_then(|s| s.checked_mul(1_000_000_000)),
        tipo: match m.tipo {
            TImg::Arquivo => TipoMembro::Arquivo,
            TImg::Pasta => TipoMembro::Pasta,
            TImg::Link => TipoMembro::Link,
        },
        cifrado: false,
    }
}

impl LeitorConteiner for LeitorImagem {
    fn formato(&self) -> &'static str {
        "imagem"
    }

    fn retomavel(&self) -> bool {
        true
    }

    fn avisos(&self) -> Vec<String> {
        self.avisos.clone()
    }

    fn percorrer(&mut self, visita: &mut Visita<'_>) -> Result<Resumo, ErroConteiner> {
        let mut bytes = 0u64;
        let mut membros = 0u64;
        let r = self
            .img
            .percorrer(&mut |m: MembroImagem,
                             conteudo: &mut dyn io::Read|
             -> Result<CImg, ErroConteiner> {
                if m.repetido {
                    return Ok(CImg::Continuar);
                }
                let info = converter(&m);
                membros += 1;
                if info.tipo == TipoMembro::Arquivo {
                    bytes += m.tamanho;
                }
                Ok(match visita(info, conteudo)? {
                    Controle::Continuar => CImg::Continuar,
                    Controle::Parar => CImg::Parar,
                })
            })
            .map_err(erro)?;
        for a in &r.avisos {
            self.avisos.push(a.clone());
        }
        for (i, motivo) in &r.nao_lidas {
            self.avisos
                .push(format!("partição {} não lida: {motivo}", i + 1));
        }
        Ok(Resumo { membros, bytes })
    }
}
