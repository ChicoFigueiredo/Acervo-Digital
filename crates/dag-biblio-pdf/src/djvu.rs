//! Extração de DjVu pelo djvulibre: `djvused` (páginas, meta, marcadores), `djvutxt` (texto),
//! `ddjvu` (renderização em PNM para o OCR e a capa).
//!
//! Entrada em memória: o djvulibre não aceita `-` como stdin; o `memfd` lido por `/dev/stdin`
//! resolve (ver `proc`).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::time::{Duration, Instant};

use dag_biblio::{
    Campo, EstadoLeitura, Evidencia, FormatoLivro, ItemSumario, OrigemEvidencia, ValorSugerido,
    isbn,
};

use crate::config::LimitesExecucao;
use crate::extrator::{Extrator, consolidar, resultado_base};
use crate::ferramentas::FerramentasDjvu;
use crate::fonte::{Fonte, Local, abrir, contem, ler_trechos};
use crate::metadados::{CONFIANCA_METADADO, parece_lixo, sugerir_de_metadados};
use crate::motor::{Analise, ErroMotor, Motor};
use crate::png::pnm_para_png;
use crate::proc::executar;
use crate::relatorio::RelatorioExtracao;

fn arg(s: impl ToString) -> OsString {
    OsString::from(s.to_string())
}

fn primeira_linha(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("sem mensagem")
        .chars()
        .take(200)
        .collect()
}

/// Símbolos e strings de uma s-expression do `djvused`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Abre,
    Fecha,
    Texto(String),
    Simbolo(String),
}

/// Decodifica as escapes do djvused: `\"`, `\\`, `\n`, `\t` e `\NNN` (octal, bytes UTF-8).
fn tokenizar(s: &str) -> Vec<Token> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut tokens = Vec::new();
    while i < b.len() {
        match b[i] {
            b'(' => {
                tokens.push(Token::Abre);
                i += 1;
            }
            b')' => {
                tokens.push(Token::Fecha);
                i += 1;
            }
            b'"' => {
                i += 1;
                let mut bytes = Vec::new();
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        i += 1;
                        match b[i] {
                            b'n' => bytes.push(b'\n'),
                            b't' => bytes.push(b'\t'),
                            b'0'..=b'7' => {
                                let mut v = 0u32;
                                let mut n = 0;
                                while n < 3 && i < b.len() && (b'0'..=b'7').contains(&b[i]) {
                                    v = v * 8 + u32::from(b[i] - b'0');
                                    i += 1;
                                    n += 1;
                                }
                                bytes.push((v & 0xFF) as u8);
                                continue;
                            }
                            outro => bytes.push(outro),
                        }
                    } else {
                        bytes.push(b[i]);
                    }
                    i += 1;
                }
                i += 1;
                tokens.push(Token::Texto(String::from_utf8_lossy(&bytes).into_owned()));
            }
            c if c.is_ascii_whitespace() => i += 1,
            _ => {
                let ini = i;
                while i < b.len()
                    && !b[i].is_ascii_whitespace()
                    && b[i] != b'('
                    && b[i] != b')'
                    && b[i] != b'"'
                {
                    i += 1;
                }
                tokens.push(Token::Simbolo(s[ini..i].to_string()));
            }
        }
    }
    tokens
}

/// Marcadores do `print-outline`: `(bookmarks ("Título" "#5" ("Filho" "#6")) ...)`.
#[must_use]
pub fn parse_outline_djvu(saida: &str) -> Vec<ItemSumario> {
    let tokens = tokenizar(saida);
    let mut itens = Vec::new();
    let mut pilha = 0i32; // profundidade de parênteses
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Abre => {
                pilha += 1;
                // Um item: "(" Texto Texto ...
                if let (Some(Token::Texto(titulo)), Some(Token::Texto(link))) =
                    (tokens.get(i + 1), tokens.get(i + 2))
                {
                    let pagina = link
                        .trim_start_matches('#')
                        .chars()
                        .filter(char::is_ascii_digit)
                        .collect::<String>()
                        .parse::<u32>()
                        .ok();
                    if itens.len() < 5000 && !titulo.trim().is_empty() {
                        itens.push(ItemSumario {
                            titulo: titulo.trim().chars().take(300).collect(),
                            nivel: u8::try_from((pilha - 2).max(0)).unwrap_or(0),
                            pagina,
                        });
                    }
                    i += 3;
                    continue;
                }
            }
            Token::Fecha => pilha -= 1,
            _ => {}
        }
        i += 1;
    }
    itens
}

/// Linhas `chave<TAB>"valor"` do `print-meta`.
#[must_use]
pub fn parse_meta_djvu(saida: &str) -> BTreeMap<String, String> {
    saida
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(char::is_whitespace)?;
            let valor = match tokenizar(v.trim()).into_iter().next()? {
                Token::Texto(t) => t,
                _ => return None,
            };
            Some((k.trim().to_lowercase(), valor))
        })
        .collect()
}

struct MotorDjvu<'a> {
    f: &'a FerramentasDjvu,
    local: &'a Local,
    lim: &'a LimitesExecucao,
}

impl MotorDjvu<'_> {
    fn rodar_em(
        &self,
        programa: &std::path::Path,
        args: Vec<OsString>,
        timeout: Duration,
    ) -> Result<crate::proc::Saida, crate::proc::ErroProc> {
        executar(
            programa,
            &args,
            self.local.entrada(),
            &[],
            timeout,
            self.lim.max_bytes_saida,
        )
    }
}

impl Motor for MotorDjvu<'_> {
    fn texto_nativo(&self, de: u32, ate: u32, timeout: Duration) -> Result<Vec<String>, ErroMotor> {
        let mut paginas = Vec::new();
        let inicio = Instant::now();
        for p in de..=ate {
            let restante = timeout.saturating_sub(inicio.elapsed());
            if restante.is_zero() {
                return Err(ErroMotor::Timeout);
            }
            let s = self.rodar_em(
                &self.f.djvutxt.caminho,
                vec![arg(format!("--page={p}")), self.local.arg()],
                restante,
            )?;
            if !s.sucesso {
                return Err(ErroMotor::Falha(primeira_linha(&s.stderr)));
            }
            let texto: String = String::from_utf8_lossy(&s.stdout)
                .chars()
                .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
                .collect();
            paginas.push(texto);
        }
        Ok(paginas)
    }

    fn paginas_com_imagem(&self, _de: u32, _ate: u32, _timeout: Duration) -> Option<BTreeSet<u32>> {
        // Toda página DjVu é imagem em camadas; sem texto, o OCR vale.
        None
    }

    fn imagem_para_ocr(
        &self,
        pagina: u32,
        _dpi: u32,
        timeout: Duration,
    ) -> Result<Vec<u8>, ErroMotor> {
        // O ddjvu renderiza na resolução nativa da página (em geral 300 dpi), sem `-dpi`.
        let s = self.rodar_em(
            &self.f.ddjvu.caminho,
            vec![
                arg("-format=pnm"),
                arg(format!("-page={pagina}")),
                self.local.arg(),
            ],
            timeout,
        )?;
        if !s.sucesso || s.stdout.is_empty() {
            return Err(ErroMotor::Falha(primeira_linha(&s.stderr)));
        }
        Ok(s.stdout)
    }
}

impl Extrator {
    /// Extrai um DjVu (do disco ou da memória). Nunca falha: problemas viram `EstadoLeitura`.
    #[must_use]
    pub fn extrair_djvu(&self, fonte: Fonte<'_>) -> RelatorioExtracao {
        let inicio = Instant::now();
        let lim = &self.config.limites;
        let mut rel =
            RelatorioExtracao::vazio(resultado_base(FormatoLivro::Djvu, EstadoLeitura::Lido));
        let fim = |mut r: RelatorioExtracao, estado: EstadoLeitura| {
            r.resultado.estado = estado;
            r.duracao_total_ms = inicio.elapsed().as_secs_f64() * 1000.0;
            r
        };
        let Some(djvu) = &self.ferramentas.djvu else {
            return fim(
                rel,
                EstadoLeitura::NaoSuportado {
                    motivo: "djvulibre ausente (djvused, djvutxt, ddjvu); instale `djvulibre-bin`"
                        .into(),
                },
            );
        };
        let trechos = match ler_trechos(&fonte) {
            Ok(t) => t,
            Err(e) => {
                return fim(
                    rel,
                    EstadoLeitura::NaoSuportado {
                        motivo: format!("não foi possível ler o arquivo: {e}"),
                    },
                );
            }
        };
        if trechos.tamanho > lim.max_bytes_arquivo {
            return fim(
                rel,
                EstadoLeitura::LimiteExcedido {
                    limite: format!(
                        "tamanho do arquivo ({} bytes > {})",
                        trechos.tamanho, lim.max_bytes_arquivo
                    ),
                },
            );
        }
        if !contem(&trechos.inicio[..trechos.inicio.len().min(16)], b"AT&TFORM") {
            return fim(
                rel,
                EstadoLeitura::Corrompido {
                    motivo: "cabeçalho DjVu (AT&TFORM) ausente".into(),
                },
            );
        }
        let local = match abrir(&fonte) {
            Ok(l) => l,
            Err(e) => {
                return fim(
                    rel,
                    EstadoLeitura::NaoSuportado {
                        motivo: format!("não foi possível abrir: {e}"),
                    },
                );
            }
        };
        let motor = MotorDjvu {
            f: djvu,
            local: &local,
            lim,
        };

        let sed = |script: &str| {
            motor.rodar_em(
                &djvu.djvused.caminho,
                vec![local.arg(), arg("-e"), arg(script)],
                lim.timeout_metadados,
            )
        };
        let total = match sed("n") {
            Ok(s) if s.sucesso => String::from_utf8_lossy(&s.stdout)
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| *n > 0),
            Ok(s) => {
                return fim(
                    rel,
                    EstadoLeitura::Corrompido {
                        motivo: primeira_linha(&s.stderr),
                    },
                );
            }
            Err(crate::proc::ErroProc::Timeout(_)) => {
                return fim(
                    rel,
                    EstadoLeitura::LimiteExcedido {
                        limite: "tempo (djvused)".into(),
                    },
                );
            }
            Err(e) => {
                return fim(
                    rel,
                    EstadoLeitura::Corrompido {
                        motivo: e.to_string(),
                    },
                );
            }
        };
        let Some(total) = total else {
            return fim(
                rel,
                EstadoLeitura::Corrompido {
                    motivo: "o DjVu não declara páginas".into(),
                },
            );
        };
        rel.resultado.total_paginas = Some(total);
        if total > lim.max_paginas_documento {
            return fim(
                rel,
                EstadoLeitura::LimiteExcedido {
                    limite: format!(
                        "páginas do documento ({total} > {})",
                        lim.max_paginas_documento
                    ),
                },
            );
        }

        // Metadados (print-meta) e marcadores (print-outline).
        let meta = sed("print-meta")
            .ok()
            .filter(|s| s.sucesso)
            .map(|s| parse_meta_djvu(&String::from_utf8_lossy(&s.stdout)))
            .unwrap_or_default();
        let mut info = BTreeMap::new();
        for (de, para) in [
            ("title", "Title"),
            ("author", "Author"),
            ("subject", "Subject"),
            ("keywords", "Keywords"),
        ] {
            if let Some(v) = meta.get(de) {
                info.insert(para.to_string(), v.clone());
            }
        }
        rel.metadados_brutos = meta.clone();
        let mut metadados = sugerir_de_metadados(&info, "");
        for (chave, campo) in [("publisher", Campo::Editora), ("year", Campo::Data)] {
            if let Some(v) = meta.get(chave).filter(|v| !v.is_empty()) {
                let conf = if parece_lixo(v) {
                    0.1
                } else {
                    CONFIANCA_METADADO
                };
                metadados.push(ValorSugerido {
                    campo,
                    valor: Some(v.clone()),
                    lista: Vec::new(),
                    confianca: conf,
                    evidencias: vec![Evidencia {
                        origem: OrigemEvidencia::MetadadoInterno,
                        pagina: None,
                        trecho: Some(v.chars().take(200).collect()),
                        detalhe: Some(format!("djvused print-meta: {chave}")),
                        confianca: conf,
                    }],
                });
            }
        }
        metadados.push(ValorSugerido {
            campo: Campo::Paginas,
            valor: Some(total.to_string()),
            lista: Vec::new(),
            confianca: 0.95,
            evidencias: vec![Evidencia {
                origem: OrigemEvidencia::MetadadoInterno,
                pagina: None,
                trecho: None,
                detalhe: Some("djvused: n".into()),
                confianca: 0.95,
            }],
        });
        let isbns_nativos: Vec<(String, Evidencia)> = info
            .values()
            .flat_map(|v| isbn::encontrar(v).into_iter().map(move |i| (i, v.clone())))
            .map(|(i, v)| {
                (
                    i,
                    Evidencia {
                        origem: OrigemEvidencia::MetadadoInterno,
                        pagina: None,
                        trecho: Some(v.chars().take(200).collect()),
                        detalhe: Some("metadado do arquivo".into()),
                        confianca: 0.7,
                    },
                )
            })
            .collect();
        let sumario_nativo = sed("print-outline")
            .ok()
            .filter(|s| s.sucesso)
            .map(|s| parse_outline_djvu(&String::from_utf8_lossy(&s.stdout)))
            .unwrap_or_default();

        // Capa: página 1 reduzida, PNG em memória.
        let lado = self.config.tamanho_capa;
        let capa = motor
            .rodar_em(
                &djvu.ddjvu.caminho,
                vec![
                    arg("-format=pnm"),
                    arg("-page=1"),
                    arg(format!("-size={lado}x{lado}")),
                    local.arg(),
                ],
                lim.timeout_pagina,
            )
            .ok()
            .filter(|s| s.sucesso)
            .and_then(|s| pnm_para_png(&s.stdout));
        if capa.is_some() {
            rel.resultado.capa = capa;
            rel.resultado.capa_mime = Some("image/png".into());
        } else {
            rel.resultado
                .avisos
                .push("não foi possível renderizar a capa".into());
        }

        let mut avisos_ocr = Vec::new();
        let ocr = self.config_ocr(&mut avisos_ocr);
        rel.idiomas_ocr = ocr.as_ref().map(|o| o.idiomas.clone());
        rel.resultado.avisos.extend(avisos_ocr);
        let mut analise = Analise::nova(&motor, &self.config, ocr, inicio);
        let ev = analise.amostrar(total, !sumario_nativo.is_empty(), &metadados);
        rel.resultado.metadados = metadados;
        consolidar(&mut rel, analise, ev, isbns_nativos, sumario_nativo, inicio);
        rel
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn outline_com_escapes_octais_e_niveis() {
        let s = "(bookmarks\n (\"Cap\\303\\255tulo 1\" \"#3\"\n  (\"Se\\303\\247\\303\\243o\" \"#4\" ) )\n (\"Dois\" \"#p0009.djvu\" ) )\n";
        let itens = parse_outline_djvu(s);
        assert_eq!(itens.len(), 3);
        assert_eq!(itens[0].titulo, "Capítulo 1");
        assert_eq!((itens[0].nivel, itens[0].pagina), (0, Some(3)));
        assert_eq!(itens[1].titulo, "Seção");
        assert_eq!(itens[1].nivel, 1);
        assert_eq!(itens[2].pagina, Some(9));
    }

    #[test]
    fn meta_le_chaves_e_valores_citados() {
        let m = parse_meta_djvu("title\t\"Dom \\\"Casmurro\\\"\"\nauthor\t\"Machado de Assis\"\n");
        assert_eq!(m["title"], "Dom \"Casmurro\"");
        assert_eq!(m["author"], "Machado de Assis");
    }
}
