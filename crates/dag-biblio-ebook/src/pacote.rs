//! Travessia de um Contêiner (zip, rar, 7z...) pela `dag-conteineres`, em fluxo e na memória,
//! aplicando os limites: só carrega os membros que o chamador pede.

use std::io::{self, Read};

use dag_biblio::EstadoLeitura;
use dag_conteineres::{Controle, ErroConteiner, Registro, TipoMembro};

use crate::limites::Medidor;
use crate::origem::Origem;
use crate::util;

/// Por que uma extração parou.
#[derive(Debug)]
pub(crate) enum Falha {
    /// Um limite de CA-B14 foi excedido (texto = qual).
    Limite(String),
    /// Senha, corrupção ou formato sem suporte.
    Estado(EstadoLeitura),
}

impl Falha {
    pub(crate) fn estado(self) -> EstadoLeitura {
        match self {
            Self::Limite(limite) => EstadoLeitura::LimiteExcedido { limite },
            Self::Estado(e) => e,
        }
    }

    pub(crate) fn corrompido(motivo: impl Into<String>) -> Self {
        Self::Estado(EstadoLeitura::Corrompido {
            motivo: motivo.into(),
        })
    }

    pub(crate) fn nao_suportado(motivo: impl Into<String>) -> Self {
        Self::Estado(EstadoLeitura::NaoSuportado {
            motivo: motivo.into(),
        })
    }
}

pub(crate) fn mapear_erro(e: ErroConteiner) -> Falha {
    match e {
        ErroConteiner::SenhaNecessaria | ErroConteiner::SenhaIncorreta => {
            Falha::Estado(EstadoLeitura::Senha)
        }
        ErroConteiner::NaoSuportado(m) => Falha::nao_suportado(m),
        outro => Falha::corrompido(outro.to_string()),
    }
}

/// Erro de leitura do conteúdo de um membro, no vocabulário da `dag-conteineres`.
pub(crate) fn erro_de_leitura(e: io::Error) -> ErroConteiner {
    match e.kind() {
        io::ErrorKind::PermissionDenied => ErroConteiner::SenhaIncorreta,
        io::ErrorKind::Unsupported => ErroConteiner::NaoSuportado(e.to_string()),
        io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => {
            ErroConteiner::Corrompido(e.to_string())
        }
        _ => ErroConteiner::Io(e),
    }
}

/// Um membro visto na travessia.
#[derive(Debug)]
pub(crate) struct Carregada {
    /// Caminho interno normalizado (barra `/`, sem `./`).
    pub nome: String,
    /// Conteúdo, se o filtro pediu (e coube no teto).
    pub dados: Option<Vec<u8>>,
}

/// O que fazer com um membro.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Decisao {
    /// Só registra o nome (nada é descompactado).
    Ignorar,
    /// Carrega até `teto` bytes (o excedente é descartado sem ler).
    Ler(u64),
}

#[derive(Debug)]
pub(crate) struct Varredura {
    pub entradas: Vec<Carregada>,
    pub avisos: Vec<String>,
}

/// Lê até `teto` bytes de `r`, medindo cada bloco (tempo, bytes, razão).
pub(crate) fn ler_limitado(
    r: &mut dyn Read,
    teto: u64,
    med: &Medidor,
) -> Result<Result<Vec<u8>, io::Error>, String> {
    let mut v: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 64 << 10];
    while (v.len() as u64) < teto {
        let falta = (teto - v.len() as u64).min(buf.len() as u64) as usize;
        let n = match r.read(&mut buf[..falta]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Ok(Err(e)),
        };
        med.ler(n as u64)?;
        v.extend_from_slice(&buf[..n]);
    }
    Ok(Ok(v))
}

/// Percorre o Contêiner de `origem`. O `filtro` decide, por nome e tamanho declarado, se o
/// membro é ignorado, lido (com teto) ou encerra a travessia. Membros com caminho perigoso
/// (`..`, absoluto, unidade) são descartados com aviso: nada é gravado de qualquer modo.
pub(crate) fn varrer(
    origem: &Origem,
    med: &Medidor,
    filtro: &mut dyn FnMut(&str, Option<u64>) -> Decisao,
) -> Result<Varredura, Falha> {
    let fonte = origem
        .abrir()
        .map_err(|e| Falha::corrompido(format!("falha ao abrir: {e}")))?;
    let registro = Registro::padrao();
    let mut leitor = match registro.abrir(fonte, None) {
        Ok(Some(l)) => l,
        Ok(None) => return Err(Falha::nao_suportado("não é um compactado reconhecido")),
        Err(e) => return Err(mapear_erro(e)),
    };
    let mut entradas: Vec<Carregada> = Vec::new();
    let mut avisos: Vec<String> = Vec::new();
    let mut falha: Option<Falha> = None;
    let res = leitor.percorrer(&mut |info, r| {
        if info.tipo == TipoMembro::Link {
            avisos.push(format!(
                "link ignorado: {}",
                util::normalizar_nome(&info.caminho)
            ));
        }
        if info.tipo != TipoMembro::Arquivo {
            return Ok(Controle::Continuar);
        }
        if let Err(l) = med.membro() {
            falha = Some(Falha::Limite(l));
            return Ok(Controle::Parar);
        }
        let nome = util::normalizar_nome(&info.caminho);
        if !util::caminho_seguro(&nome) {
            avisos.push(format!("membro com caminho suspeito ignorado: {nome:?}"));
            return Ok(Controle::Continuar);
        }
        if let Some(t) = info.tamanho {
            if let Err(l) = med.declarar(t) {
                falha = Some(Falha::Limite(l));
                return Ok(Controle::Parar);
            }
        }
        match filtro(&nome, info.tamanho) {
            Decisao::Ignorar => {
                entradas.push(Carregada { nome, dados: None });
                Ok(Controle::Continuar)
            }
            Decisao::Ler(teto) => match ler_limitado(r, teto, med) {
                Err(l) => {
                    falha = Some(Falha::Limite(l));
                    Ok(Controle::Parar)
                }
                Ok(Ok(dados)) => {
                    entradas.push(Carregada {
                        nome,
                        dados: Some(dados),
                    });
                    Ok(Controle::Continuar)
                }
                Ok(Err(e)) if e.kind() == io::ErrorKind::Unsupported => {
                    avisos.push(format!("membro {nome:?} com compressão sem suporte: {e}"));
                    entradas.push(Carregada { nome, dados: None });
                    Ok(Controle::Continuar)
                }
                Ok(Err(e)) => Err(erro_de_leitura(e)),
            },
        }
    });
    if let Some(f) = falha {
        return Err(f);
    }
    if let Err(e) = res {
        return Err(mapear_erro(e));
    }
    avisos.extend(leitor.avisos());
    Ok(Varredura { entradas, avisos })
}

/// Lê um único membro (por nome exato) até `teto` bytes.
pub(crate) fn ler_membro(
    origem: &Origem,
    med: &Medidor,
    nome: &str,
    teto: u64,
) -> Result<Option<Vec<u8>>, Falha> {
    let v = varrer(origem, med, &mut |n, _| {
        if n == nome {
            Decisao::Ler(teto)
        } else {
            Decisao::Ignorar
        }
    })?;
    Ok(v.entradas
        .into_iter()
        .find(|e| e.nome == nome)
        .and_then(|e| e.dados))
}
