//! Livros dentro de compactados genéricos (zip, rar, 7z, tar...): lista os membros que são
//! livros (por extensão **e** assinatura), extrai cada um em memória e devolve um resultado por
//! membro com o caminho interno. Aninhados (compactado dentro de compactado) descem até
//! `profundidade_max`. Nada é gravado em disco; nomes com `..` ou absolutos são descartados
//! com aviso.

use std::sync::Arc;

use dag_biblio::{EstadoLeitura, FormatoLivro, ResultadoExtracao};
use dag_conteineres::{Controle, ErroConteiner, Registro, TipoMembro};

use crate::limites::Medidor;
use crate::origem::Origem;
use crate::pacote::{self, Falha, erro_de_leitura, ler_limitado, mapear_erro};
use crate::util;
use crate::{Detectado, OpcoesExtracao, detectar, extrair_livro};

/// Um livro achado dentro de um compactado.
#[derive(Debug, Clone, PartialEq)]
pub struct MembroLivro {
    /// Caminho interno; aninhados usam `!/` (`a.zip!/b.rar!/livro.epub`).
    pub caminho_interno: String,
    pub formato: FormatoLivro,
    pub tamanho: u64,
    pub resultado: ResultadoExtracao,
    /// Bytes do membro, só para formatos que esta crate não extrai (PDF, DjVu...) e se
    /// `OpcoesExtracao::reter_dados_nao_tratados`.
    pub dados: Option<Vec<u8>>,
}

/// Resultado da leitura de um compactado genérico.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultadoCompactado {
    /// Formato do Contêiner raiz (`zip`, `rar`, `7z`, `tar.gz`...).
    pub conteiner: String,
    /// `Lido`, ou o motivo de a leitura ter parado (limite, senha, corrupção). Os membros já
    /// extraídos antes da parada ficam em `membros`.
    pub estado: EstadoLeitura,
    pub membros: Vec<MembroLivro>,
    /// Membros que são (ou podem ser) livros mas foram rejeitados, com o motivo.
    pub rejeitados: Vec<(String, String)>,
    pub membros_vistos: u64,
    pub bytes_descompactados: u64,
    pub avisos: Vec<String>,
}

const EXT_LIVRO: [&str; 14] = [
    "epub", "kepub", "mobi", "azw", "azw3", "prc", "fb2", "cbz", "cbr", "cb7", "pdf", "djvu",
    "djv", "kfx",
];

/// Extensões de zips que nunca são coleções de livros.
const EXT_IGNORAR: [&str; 12] = [
    "docx", "xlsx", "pptx", "odt", "ods", "odp", "jar", "apk", "xpi", "whl", "htmlz", "docm",
];

fn assinatura_livro(p: &[u8]) -> bool {
    p.starts_with(b"%PDF-")
        || p.starts_with(b"AT&TFORM")
        || crate::mobi::assinatura(p)
        || crate::mobi::e_kfx(p)
        || crate::fb2::assinatura(p)
}

pub(crate) fn extrair(
    origem: &Origem,
    conteiner: &str,
    opc: &OpcoesExtracao,
    med: &Medidor,
) -> ResultadoCompactado {
    let mut saida = ResultadoCompactado {
        conteiner: conteiner.to_owned(),
        estado: EstadoLeitura::Lido,
        membros: Vec::new(),
        rejeitados: Vec::new(),
        membros_vistos: 0,
        bytes_descompactados: 0,
        avisos: Vec::new(),
    };
    if let Err(f) = nivel(origem, opc, med, 0, "", &mut saida) {
        saida.estado = f.estado();
    }
    saida.membros_vistos = med.membros_vistos();
    saida.bytes_descompactados = med.bytes_lidos();
    saida
}

fn nivel(
    origem: &Origem,
    opc: &OpcoesExtracao,
    med: &Medidor,
    prof: u32,
    prefixo: &str,
    saida: &mut ResultadoCompactado,
) -> Result<(), Falha> {
    let fonte = origem
        .abrir()
        .map_err(|e| Falha::corrompido(format!("falha ao abrir: {e}")))?;
    let registro = Registro::padrao();
    let mut leitor = match registro.abrir(fonte, None) {
        Ok(Some(l)) => l,
        Ok(None) => return Err(Falha::nao_suportado("não é um compactado reconhecido")),
        Err(e) => return Err(mapear_erro(e)),
    };
    let teto = med.limites().bytes_membro_max;
    let mut falha: Option<Falha> = None;
    let res = leitor.percorrer(&mut |info, r| {
        if info.tipo != TipoMembro::Arquivo {
            return Ok(Controle::Continuar);
        }
        if let Err(l) = med.membro() {
            falha = Some(Falha::Limite(l));
            return Ok(Controle::Parar);
        }
        let nome = util::normalizar_nome(&info.caminho);
        let completo = format!("{prefixo}{nome}");
        if !util::caminho_seguro(&nome) {
            saida.avisos.push(format!(
                "membro com caminho suspeito ignorado: {completo:?}"
            ));
            return Ok(Controle::Continuar);
        }
        if let Some(t) = info.tamanho {
            if let Err(l) = med.declarar(t) {
                falha = Some(Falha::Limite(l));
                return Ok(Controle::Parar);
            }
        }
        let ext = util::extensao(&nome);
        if EXT_IGNORAR.contains(&ext.as_str()) {
            return Ok(Controle::Continuar);
        }
        // Prefixo para a assinatura.
        let ini = match ler_limitado(r, 4096, med) {
            Err(l) => {
                falha = Some(Falha::Limite(l));
                return Ok(Controle::Parar);
            }
            Ok(Ok(v)) => v,
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::Unsupported => {
                saida
                    .rejeitados
                    .push((completo, format!("compressão sem suporte: {e}")));
                return Ok(Controle::Continuar);
            }
            Ok(Err(e)) => return Err(erro_de_leitura(e)),
        };
        let por_ext = EXT_LIVRO.contains(&ext.as_str());
        let candidato = por_ext
            || assinatura_livro(&ini)
            || ini.starts_with(b"PK\x03\x04")
            || ini.starts_with(b"Rar!")
            || ini.starts_with(b"7z\xBC\xAF")
            || registro.candidato(&ini);
        if !candidato {
            return Ok(Controle::Continuar);
        }
        if info.tamanho.is_some_and(|t| t > teto) {
            saida.rejeitados.push((
                completo,
                format!("membro maior que {teto} bytes não é carregado na memória"),
            ));
            return Ok(Controle::Continuar);
        }
        let resto_teto = teto.saturating_sub(ini.len() as u64);
        let mut dados = ini;
        match ler_limitado(r, resto_teto, med) {
            Err(l) => {
                falha = Some(Falha::Limite(l));
                return Ok(Controle::Parar);
            }
            Ok(Ok(v)) => dados.extend(v),
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::Unsupported => {
                saida
                    .rejeitados
                    .push((completo, format!("compressão sem suporte: {e}")));
                return Ok(Controle::Continuar);
            }
            Ok(Err(e)) => return Err(erro_de_leitura(e)),
        }
        if dados.len() as u64 >= teto {
            saida.rejeitados.push((
                completo,
                format!("membro maior que {teto} bytes não é carregado na memória"),
            ));
            return Ok(Controle::Continuar);
        }
        let tam = dados.len() as u64;
        let sub = Origem::Memoria(Arc::from(dados));
        match processar(&sub, &nome, &completo, tam, opc, med, prof, saida) {
            Ok(()) => Ok(Controle::Continuar),
            Err(f) => {
                falha = Some(f);
                Ok(Controle::Parar)
            }
        }
    });
    saida.avisos.extend(leitor.avisos());
    if let Some(f) = falha {
        return Err(f);
    }
    match res {
        Ok(_) => Ok(()),
        Err(ErroConteiner::Cancelado) => Err(Falha::corrompido("cancelado")),
        Err(e) => Err(mapear_erro(e)),
    }
}

#[allow(clippy::too_many_arguments)]
fn processar(
    sub: &Origem,
    nome: &str,
    completo: &str,
    tam: u64,
    opc: &OpcoesExtracao,
    med: &Medidor,
    prof: u32,
    saida: &mut ResultadoCompactado,
) -> Result<(), Falha> {
    let dica = FormatoLivro::da_extensao(&util::extensao(nome));
    match detectar(sub, Some(nome), dica, med)? {
        Detectado::Livro(formato) => {
            let resultado = extrair_livro(sub, formato, Some(nome), dica, opc, med);
            // Limite global estourado durante o membro: propaga para parar a travessia.
            if let EstadoLeitura::LimiteExcedido { limite } = &resultado.estado {
                if limite.starts_with("tempo")
                    || limite.starts_with("bytes descompactados")
                    || limite.starts_with("razão")
                    || limite.starts_with("quantidade")
                {
                    saida.membros.push(MembroLivro {
                        caminho_interno: completo.to_owned(),
                        formato,
                        tamanho: tam,
                        resultado: resultado.clone(),
                        dados: None,
                    });
                    return Err(Falha::Limite(limite.clone()));
                }
            }
            let tratado = matches!(
                formato,
                FormatoLivro::Epub
                    | FormatoLivro::Mobi
                    | FormatoLivro::Azw
                    | FormatoLivro::Azw3
                    | FormatoLivro::Fb2
                    | FormatoLivro::Cbz
                    | FormatoLivro::Cbr
                    | FormatoLivro::Cb7
            );
            let dados = if !tratado && opc.reter_dados_nao_tratados {
                sub.tudo(med.limites().bytes_membro_max)
                    .ok()
                    .map(|a| a.to_vec())
            } else {
                None
            };
            saida.membros.push(MembroLivro {
                caminho_interno: completo.to_owned(),
                formato,
                tamanho: tam,
                resultado,
                dados,
            });
        }
        Detectado::Conteiner(c) => {
            if prof + 1 > med.limites().profundidade_max {
                saida.rejeitados.push((
                    completo.to_owned(),
                    format!(
                        "compactado aninhado além da profundidade máxima ({})",
                        med.limites().profundidade_max
                    ),
                ));
                return Ok(());
            }
            let prefixo = format!("{completo}!/");
            match nivel(sub, opc, med, prof + 1, &prefixo, saida) {
                Ok(()) => {}
                Err(Falha::Limite(l)) => return Err(Falha::Limite(l)),
                Err(Falha::Estado(e)) => {
                    saida.rejeitados.push((
                        completo.to_owned(),
                        format!("compactado aninhado ({c}) não lido: {e:?}"),
                    ));
                }
            }
        }
        Detectado::Desconhecido => {}
    }
    Ok(())
}

/// Lê um membro (caminho interno exato, sem aninhamento) de um compactado, na memória.
pub(crate) fn ler_um(origem: &Origem, interno: &str, med: &Medidor) -> Result<Vec<u8>, Falha> {
    let teto = med.limites().bytes_membro_max;
    pacote::ler_membro(origem, med, interno, teto)?
        .ok_or_else(|| Falha::corrompido(format!("membro {interno:?} não encontrado")))
}
