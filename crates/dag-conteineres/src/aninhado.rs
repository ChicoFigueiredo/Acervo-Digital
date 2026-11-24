//! Travessia completa: hash BLAKE3 de cada Membro na mesma leitura e descida em Contêineres
//! aninhados, tudo na memória.

use std::io::{self, Read};

use crate::formatos::ler_ate_encher;
use crate::{Controle, ErroConteiner, FonteBox, LeitorConteiner, MembroInfo, Registro, TipoMembro};

/// Parâmetros de uma leitura.
#[derive(Debug, Clone)]
pub struct Opcoes {
    /// Níveis de aninhamento abertos além do Contêiner raiz (padrão 3).
    pub profundidade_max: u32,
    /// Maior Membro aninhado que se aceita carregar na memória para abrir por dentro
    /// (padrão 256 MiB). Acima disso o Membro é hasheado em fluxo e fica "não lido (aninhado
    /// grande)".
    pub limite_aninhado: u64,
    /// Senha a tentar neste Contêiner (e nos aninhados cifrados).
    pub senha: Option<String>,
}

impl Default for Opcoes {
    fn default() -> Self {
        Self {
            profundidade_max: 3,
            limite_aninhado: 256 << 20,
            senha: None,
        }
    }
}

/// O que se sabe do conteúdo de um Membro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EstadoMembro {
    /// Lido (e, se arquivo, com hash).
    Lido,
    /// Conteúdo em Contêiner aninhado cifrado que a senha não abre.
    Cifrado,
    /// Método de compressão sem suporte neste Membro.
    NaoSuportado(String),
    /// Contêiner aninhado grande demais para abrir na memória.
    AninhadoGrande,
    /// Contêiner aninhado ilegível.
    Erro(String),
}

/// Um Membro lido, na ordem de descoberta (pai antes dos filhos).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembroLido {
    /// Sequência dentro do Contêiner raiz, a partir de 0.
    pub ordem: u64,
    /// `ordem` do Membro pai (Contêiner aninhado), se houver.
    pub pai: Option<u64>,
    pub info: MembroInfo,
    /// BLAKE3 do conteúdo completo, para arquivos lidos.
    pub hash: Option<[u8; 32]>,
    pub estado: EstadoMembro,
    /// Formato do Contêiner aninhado, se o Membro foi aberto por dentro.
    pub formato_interno: Option<&'static str>,
}

/// Totais da leitura do Contêiner raiz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultadoLeitura {
    /// `None` quando o conteúdo não é um Contêiner.
    pub formato: Option<&'static str>,
    pub membros: u64,
    pub bytes: u64,
}

struct Ctx<'a> {
    registro: &'a Registro,
    opcoes: &'a Opcoes,
    cancelado: &'a dyn Fn() -> bool,
    progresso: &'a mut dyn FnMut(u64),
    saida: &'a mut dyn FnMut(MembroLido) -> Result<(), ErroConteiner>,
    proxima: u64,
    bytes: u64,
    /// Filhos de aninhados em abertura: só valem se o aninhado inteiro for lido.
    pilha: Vec<Vec<MembroLido>>,
}

fn emitir(ctx: &mut Ctx<'_>, m: MembroLido) -> Result<(), ErroConteiner> {
    match ctx.pilha.last_mut() {
        Some(v) => {
            v.push(m);
            Ok(())
        }
        None => (ctx.saida)(m),
    }
}

/// Lê o Contêiner `fonte` inteiro (com os aninhados), chamando `saida` para cada Membro.
///
/// * `cancelado`: consultado a cada Membro e a cada bloco lido.
/// * `progresso`: bytes de conteúdo lidos a cada bloco.
///
/// Se `saida` ou a leitura falham, o chamador deve descartar tudo o que recebeu deste
/// Contêiner (leitura tudo-ou-nada).
///
/// # Errors
/// [`ErroConteiner::SenhaNecessaria`] e [`ErroConteiner::SenhaIncorreta`] (Contêiner raiz),
/// corrupção, E/S, cancelamento ou o erro de `saida`.
pub fn ler_conteiner(
    registro: &Registro,
    fonte: FonteBox,
    opcoes: &Opcoes,
    cancelado: &dyn Fn() -> bool,
    progresso: &mut dyn FnMut(u64),
    saida: &mut dyn FnMut(MembroLido) -> Result<(), ErroConteiner>,
) -> Result<ResultadoLeitura, ErroConteiner> {
    let Some(mut leitor) = registro.abrir(fonte, opcoes.senha.as_deref())? else {
        return Ok(ResultadoLeitura {
            formato: None,
            membros: 0,
            bytes: 0,
        });
    };
    let formato = leitor.formato();
    let mut ctx = Ctx {
        registro,
        opcoes,
        cancelado,
        progresso,
        saida,
        proxima: 0,
        bytes: 0,
        pilha: Vec::new(),
    };
    nivel(&mut ctx, &mut *leitor, None, 0)?;
    Ok(ResultadoLeitura {
        formato: Some(formato),
        membros: ctx.proxima,
        bytes: ctx.bytes,
    })
}

fn nivel(
    ctx: &mut Ctx<'_>,
    leitor: &mut dyn LeitorConteiner,
    pai: Option<u64>,
    prof: u32,
) -> Result<(), ErroConteiner> {
    leitor
        .percorrer(&mut |info, r| membro(ctx, info, r, pai, prof))
        .map(|_| ())
}

const BLOCO: usize = 256 << 10;
const PREFIXO: usize = 512;

/// Resultado de ler o conteúdo de um Membro.
enum Leitura {
    Ok,
    NaoSuportado(String),
}

fn membro(
    ctx: &mut Ctx<'_>,
    mut info: MembroInfo,
    r: &mut dyn Read,
    pai: Option<u64>,
    prof: u32,
) -> Result<Controle, ErroConteiner> {
    if (ctx.cancelado)() {
        return Err(ErroConteiner::Cancelado);
    }
    let ordem = ctx.proxima;
    ctx.proxima += 1;
    let mut lido = MembroLido {
        ordem,
        pai,
        info: info.clone(),
        hash: None,
        estado: EstadoMembro::Lido,
        formato_interno: None,
    };
    if info.tipo != TipoMembro::Arquivo {
        emitir(ctx, lido)?;
        return Ok(Controle::Continuar);
    }
    let mut hasher = blake3::Hasher::new();
    let mut total: u64 = 0;
    let mut prefixo = vec![0u8; PREFIXO];
    let n = ler_conteudo(r, &mut prefixo)?;
    let mut leitura = Leitura::Ok;
    let (n, falha_prefixo) = n;
    prefixo.truncate(n);
    if let Some(m) = falha_prefixo {
        leitura = Leitura::NaoSuportado(m);
    }
    hasher.update(&prefixo);
    total += n as u64;
    (ctx.progresso)(n as u64);

    let candidato = matches!(leitura, Leitura::Ok)
        && prof < ctx.opcoes.profundidade_max
        && ctx.registro.candidato(&prefixo);
    let mut memoria: Option<Vec<u8>> = None;
    if candidato {
        let grande = info.tamanho.is_some_and(|t| t > ctx.opcoes.limite_aninhado);
        if grande {
            lido.estado = EstadoMembro::AninhadoGrande;
        } else {
            memoria = Some(prefixo);
        }
    }
    let mut buf = vec![0u8; BLOCO];
    while matches!(leitura, Leitura::Ok) {
        if (ctx.cancelado)() {
            return Err(ErroConteiner::Cancelado);
        }
        let k = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                leitura = Leitura::NaoSuportado(e.to_string());
                break;
            }
            Err(e) => return Err(ErroConteiner::de_leitura(e)),
        };
        hasher.update(&buf[..k]);
        total += k as u64;
        (ctx.progresso)(k as u64);
        if let Some(v) = memoria.as_mut() {
            if total > ctx.opcoes.limite_aninhado {
                // Tamanho declarado mentia (ou era desconhecido): segue em fluxo.
                memoria = None;
                lido.estado = EstadoMembro::AninhadoGrande;
            } else {
                v.extend_from_slice(&buf[..k]);
            }
        }
    }
    ctx.bytes += total;
    info.tamanho = Some(total);
    lido.info = info;
    match leitura {
        Leitura::NaoSuportado(m) => {
            lido.estado = EstadoMembro::NaoSuportado(m);
            emitir(ctx, lido)?;
            return Ok(Controle::Continuar);
        }
        Leitura::Ok => lido.hash = Some(*hasher.finalize().as_bytes()),
    }
    let Some(dados) = memoria else {
        emitir(ctx, lido)?;
        return Ok(Controle::Continuar);
    };
    // Candidato carregado inteiro: tenta abrir por dentro.
    let fonte: FonteBox = Box::new(io::Cursor::new(dados));
    match ctx.registro.abrir(fonte, ctx.opcoes.senha.as_deref()) {
        Ok(None) => {
            emitir(ctx, lido)?;
        }
        Ok(Some(mut interno)) => {
            lido.formato_interno = Some(interno.formato());
            ctx.pilha.push(Vec::new());
            let r = nivel(ctx, &mut *interno, Some(ordem), prof + 1);
            let filhos = ctx.pilha.pop().unwrap_or_default();
            match r {
                Ok(()) => {
                    emitir(ctx, lido)?;
                    for f in filhos {
                        emitir(ctx, f)?;
                    }
                }
                Err(ErroConteiner::SenhaNecessaria | ErroConteiner::SenhaIncorreta) => {
                    lido.formato_interno = None;
                    lido.estado = EstadoMembro::Cifrado;
                    emitir(ctx, lido)?;
                }
                Err(ErroConteiner::Corrompido(m) | ErroConteiner::NaoSuportado(m)) => {
                    lido.formato_interno = None;
                    lido.estado = EstadoMembro::Erro(m);
                    emitir(ctx, lido)?;
                }
                Err(e) => return Err(e),
            }
        }
        Err(ErroConteiner::SenhaNecessaria | ErroConteiner::SenhaIncorreta) => {
            lido.estado = EstadoMembro::Cifrado;
            emitir(ctx, lido)?;
        }
        Err(ErroConteiner::Corrompido(m) | ErroConteiner::NaoSuportado(m)) => {
            lido.estado = EstadoMembro::Erro(m);
            emitir(ctx, lido)?;
        }
        Err(e) => return Err(e),
    }
    Ok(Controle::Continuar)
}

/// Lê o prefixo; um erro "sem suporte" vira mensagem, os demais sobem.
fn ler_conteudo(
    r: &mut dyn Read,
    buf: &mut [u8],
) -> Result<(usize, Option<String>), ErroConteiner> {
    match ler_ate_encher(r, buf) {
        Ok(n) => Ok((n, None)),
        Err(e) if e.kind() == io::ErrorKind::Unsupported => Ok((0, Some(e.to_string()))),
        Err(e) => Err(ErroConteiner::de_leitura(e)),
    }
}
