//! Extração de PDF pelas ferramentas do poppler.
//!
//! Por que processos do poppler e não uma biblioteca Rust (`lopdf`, `pdf-extract`): o poppler já é
//! a referência para texto em PDFs reais (fontes sem mapeamento, PDFs reparados, criptografia),
//! entrega o sumário (`pdftohtml -xml`), renderiza a página para o OCR e roda isolado: um PDF
//! malformado que derruba ou trava a ferramenta só perde aquele processo (timeout e kill), nunca o
//! agente. O custo é depender de binários; `Ferramentas::detectar` dá erro claro quando faltam.
//!
//! Entrada em memória (Membros de Contêiner): os bytes viram um `memfd` e as ferramentas leem
//! `/dev/stdin` (ver `proc`). Nada vai para o disco.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::time::{Duration, Instant};

use dag_biblio::{
    Campo, EstadoLeitura, Evidencia, FormatoLivro, ItemSumario, OrigemEvidencia, ValorSugerido,
    isbn,
};

use crate::config::LimitesExecucao;
use crate::extrator::{Extrator, consolidar, resultado_base};
use crate::ferramentas::Ferramentas;
use crate::fonte::{Fonte, Local, Trechos, abrir, contem, ler_trechos};
use crate::metadados::{decodificar_entidades, parse_pdfinfo, sugerir_de_metadados, xmp_valores};
use crate::motor::{Analise, ErroMotor, Motor};
use crate::proc::{ErroProc, Saida, executar};
use crate::relatorio::RelatorioExtracao;

/// Marcas de criptografia por DRM (não por senha). Nada disso é contornado.
const MARCAS_DRM: [(&[u8], &str); 5] = [
    (b"/EBX_HANDLER", "Adobe ADEPT (EBX_HANDLER)"),
    (
        b"/Adobe.PubSec",
        "Adobe PubSec (criptografia por certificado)",
    ),
    (b"/Adobe.APS", "Adobe Policy Server"),
    (b"/FOPN_", "FileOpen"),
    (b"/Vitrium", "Vitrium"),
];

fn detectar_drm(t: &Trechos) -> Option<&'static str> {
    MARCAS_DRM
        .iter()
        .find(|(m, _)| contem(&t.inicio, m) || contem(&t.fim, m))
        .map(|(_, nome)| *nome)
}

struct MotorPdf<'a> {
    ferr: &'a Ferramentas,
    local: &'a Local,
    lim: &'a LimitesExecucao,
}

impl MotorPdf<'_> {
    fn rodar(
        &self,
        programa: &std::path::Path,
        mut args: Vec<OsString>,
        timeout: Duration,
    ) -> Result<Saida, ErroProc> {
        args.push(self.local.arg());
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

impl Motor for MotorPdf<'_> {
    fn texto_nativo(&self, de: u32, ate: u32, timeout: Duration) -> Result<Vec<String>, ErroMotor> {
        // Saída `-` = stdout; o arquivo de entrada vai antes dela.
        let mut args = vec![
            arg("-f"),
            arg(de),
            arg("-l"),
            arg(ate),
            arg("-layout"),
            arg("-enc"),
            arg("UTF-8"),
        ];
        args.push(self.local.arg());
        args.push(arg("-"));
        let s = executar(
            &self.ferr.pdftotext.caminho,
            &args,
            self.local.entrada(),
            &[],
            timeout,
            self.lim.max_bytes_saida,
        )?;
        if !s.sucesso {
            return Err(ErroMotor::Falha(primeira_linha(&s.stderr)));
        }
        let n = (ate - de + 1) as usize;
        let texto = String::from_utf8_lossy(&s.stdout);
        let mut paginas: Vec<String> = texto.split('\u{c}').map(str::to_string).collect();
        paginas.truncate(n);
        paginas.resize(n, String::new());
        Ok(paginas)
    }

    fn paginas_com_imagem(&self, de: u32, ate: u32, timeout: Duration) -> Option<BTreeSet<u32>> {
        let t = self.ferr.pdfimages.as_ref()?;
        let s = self
            .rodar(
                &t.caminho,
                vec![arg("-list"), arg("-f"), arg(de), arg("-l"), arg(ate)],
                timeout,
            )
            .ok()?;
        if !s.sucesso {
            return None;
        }
        let texto = String::from_utf8_lossy(&s.stdout);
        let mut set = BTreeSet::new();
        for l in texto.lines().skip(2) {
            let c: Vec<&str> = l.split_whitespace().collect();
            if c.len() < 5 || c[2] != "image" {
                continue;
            }
            let (Ok(p), Ok(w), Ok(h)) = (
                c[0].parse::<u32>(),
                c[3].parse::<u64>(),
                c[4].parse::<u64>(),
            ) else {
                continue;
            };
            if w * h >= 100_000 {
                set.insert(p);
            }
        }
        Some(set)
    }

    fn imagem_para_ocr(
        &self,
        pagina: u32,
        dpi: u32,
        timeout: Duration,
    ) -> Result<Vec<u8>, ErroMotor> {
        let args = vec![
            arg("-f"),
            arg(pagina),
            arg("-l"),
            arg(pagina),
            arg("-r"),
            arg(dpi),
            arg("-gray"),
            arg("-png"),
        ];
        // Com um só argumento posicional (o arquivo), o pdftoppm escreve o PNG no stdout.
        let s = self.rodar(&self.ferr.pdftoppm.caminho, args, timeout)?;
        if !s.sucesso || s.stdout.is_empty() {
            return Err(ErroMotor::Falha(primeira_linha(&s.stderr)));
        }
        Ok(s.stdout)
    }
}

/// Marcadores (bookmarks) do PDF a partir de `pdftohtml -xml` (`<outline><item page="N">...`).
#[must_use]
pub fn parse_outline_xml(xml: &str) -> Vec<ItemSumario> {
    let mut itens = Vec::new();
    let mut nivel: i32 = 0;
    let mut resto = xml;
    while let Some(i) = resto.find('<') {
        let depois = &resto[i + 1..];
        let Some(fim) = depois.find('>') else { break };
        let tag = &depois[..fim];
        resto = &depois[fim + 1..];
        if tag == "outline" {
            nivel += 1;
        } else if tag == "/outline" {
            nivel -= 1;
        } else if let Some(attrs) = tag.strip_prefix("item") {
            let pagina = attrs
                .split("page=\"")
                .nth(1)
                .and_then(|p| p.split('"').next())
                .and_then(|p| p.parse::<u32>().ok());
            if let Some(z) = resto.find("</item>") {
                let titulo = decodificar_entidades(resto[..z].trim());
                if !titulo.is_empty() && itens.len() < 5000 {
                    itens.push(ItemSumario {
                        titulo: titulo.chars().take(300).collect(),
                        nivel: u8::try_from((nivel - 1).max(0)).unwrap_or(0),
                        pagina,
                    });
                }
                resto = &resto[z + 7..];
            }
        }
    }
    itens
}

impl Extrator {
    /// Extrai um PDF (do disco ou da memória). Nunca falha: problemas viram `EstadoLeitura` e avisos.
    #[must_use]
    pub fn extrair_pdf(&self, fonte: Fonte<'_>) -> RelatorioExtracao {
        let inicio = Instant::now();
        let lim = &self.config.limites;
        let mut rel =
            RelatorioExtracao::vazio(resultado_base(FormatoLivro::Pdf, EstadoLeitura::Lido));
        let fim = |mut r: RelatorioExtracao, estado: EstadoLeitura| {
            r.resultado.estado = estado;
            r.duracao_total_ms = inicio.elapsed().as_secs_f64() * 1000.0;
            r
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
        if !contem(&trechos.inicio[..trechos.inicio.len().min(1024)], b"%PDF-") {
            return fim(
                rel,
                EstadoLeitura::Corrompido {
                    motivo: "cabeçalho %PDF- ausente".into(),
                },
            );
        }
        if let Some(nome) = detectar_drm(&trechos) {
            rel.resultado
                .avisos
                .push(format!("DRM detectado: {nome}; não é contornado"));
            return fim(rel, EstadoLeitura::Drm);
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
        let motor = MotorPdf {
            ferr: &self.ferramentas,
            local: &local,
            lim,
        };

        // pdfinfo: páginas, criptografia, Info.
        let info_saida = match motor.rodar(
            &self.ferramentas.pdfinfo.caminho,
            vec![arg("-isodates")],
            lim.timeout_metadados,
        ) {
            Ok(s) => s,
            Err(ErroProc::Timeout(_)) => {
                return fim(
                    rel,
                    EstadoLeitura::LimiteExcedido {
                        limite: "tempo (pdfinfo)".into(),
                    },
                );
            }
            Err(ErroProc::SaidaExcedida(_)) => {
                return fim(
                    rel,
                    EstadoLeitura::LimiteExcedido {
                        limite: "tamanho da saída (pdfinfo)".into(),
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
        if !info_saida.sucesso {
            let msg = primeira_linha(&info_saida.stderr);
            let estado = if msg.to_lowercase().contains("password") {
                EstadoLeitura::Senha
            } else {
                EstadoLeitura::Corrompido { motivo: msg }
            };
            return fim(rel, estado);
        }
        let info = parse_pdfinfo(&String::from_utf8_lossy(&info_saida.stdout));
        let Some(total) = info
            .get("Pages")
            .and_then(|p| p.parse::<u32>().ok())
            .filter(|p| *p > 0)
        else {
            return fim(
                rel,
                EstadoLeitura::Corrompido {
                    motivo: "o PDF não declara páginas".into(),
                },
            );
        };
        rel.resultado.total_paginas = Some(total);
        rel.metadados_brutos = info.clone();
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

        // XMP (opcional) e metadados nativos.
        let xmp = motor
            .rodar(
                &self.ferramentas.pdfinfo.caminho,
                vec![arg("-meta")],
                lim.timeout_metadados,
            )
            .ok()
            .filter(|s| s.sucesso)
            .map(|s| String::from_utf8_lossy(&s.stdout).into_owned())
            .unwrap_or_default();
        let mut metadados = sugerir_de_metadados(&info, &xmp);
        metadados.push(ValorSugerido {
            campo: Campo::Paginas,
            valor: Some(total.to_string()),
            lista: Vec::new(),
            confianca: 0.95,
            evidencias: vec![Evidencia {
                origem: OrigemEvidencia::MetadadoInterno,
                pagina: None,
                trecho: None,
                detalhe: Some("pdfinfo: Pages".into()),
                confianca: 0.95,
            }],
        });
        let mut isbns_nativos: Vec<(String, Evidencia)> = Vec::new();
        let mut candidatos: Vec<String> = ["Subject", "Keywords", "Title"]
            .iter()
            .filter_map(|k| info.get(*k).cloned())
            .collect();
        candidatos.extend(xmp_valores(&xmp, "dc:identifier"));
        candidatos.extend(xmp_valores(&xmp, "dc:description"));
        for c in &candidatos {
            for i in isbn::encontrar(c) {
                isbns_nativos.push((
                    i,
                    Evidencia {
                        origem: OrigemEvidencia::MetadadoInterno,
                        pagina: None,
                        trecho: Some(c.chars().take(200).collect()),
                        detalhe: Some("metadado do arquivo".into()),
                        confianca: 0.7,
                    },
                ));
            }
        }

        // Criptografia: senha já saiu acima; aqui, restrição de cópia (não contornada).
        if let Some(enc) = info.get("Encrypted").filter(|e| e.starts_with("yes")) {
            if enc.contains("copy:no") {
                rel.resultado.metadados = metadados;
                rel.resultado.avisos.push(format!(
                    "PDF com restrição de cópia ({enc}); respeitada, o texto não é extraído"
                ));
                return fim(rel, EstadoLeitura::Drm);
            }
            rel.resultado
                .avisos
                .push(format!("PDF criptografado, legível sem senha ({enc})"));
        }

        // Marcadores (sumário nativo).
        let sumario_nativo = self
            .ferramentas
            .pdftohtml
            .as_ref()
            .map_or_else(Vec::new, |t| {
                motor
                    .rodar(
                        &t.caminho,
                        ["-xml", "-i", "-stdout", "-q", "-f", "1", "-l", "1"]
                            .iter()
                            .map(OsString::from)
                            .collect(),
                        lim.timeout_metadados,
                    )
                    .ok()
                    .filter(|s| s.sucesso)
                    .map(|s| parse_outline_xml(&String::from_utf8_lossy(&s.stdout)))
                    .unwrap_or_default()
            });

        // Capa: página 1 em baixa resolução, JPEG em memória.
        let capa = motor
            .rodar(
                &self.ferramentas.pdftoppm.caminho,
                [
                    "-f",
                    "1",
                    "-l",
                    "1",
                    "-scale-to",
                    &self.config.tamanho_capa.to_string(),
                    "-jpeg",
                ]
                .iter()
                .map(OsString::from)
                .collect(),
                lim.timeout_pagina.min(lim.timeout_total),
            )
            .ok()
            .filter(|s| s.sucesso && s.stdout.starts_with(&[0xFF, 0xD8]))
            .map(|s| s.stdout);
        if capa.is_some() {
            rel.resultado.capa = capa;
            rel.resultado.capa_mime = Some("image/jpeg".into());
        } else {
            rel.resultado
                .avisos
                .push("não foi possível renderizar a capa".into());
        }

        // Amostragem progressiva.
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
    fn outline_aninhado_do_pdftohtml() {
        let xml = r#"<pdf2xml><outline><item page="3">Cap&#237;tulo &amp; Um</item>
            <outline><item page="4">Se&#231;&#227;o 1.1</item></outline>
            <item page="9">Dois</item></outline></pdf2xml>"#;
        let itens = parse_outline_xml(xml);
        assert_eq!(itens.len(), 3);
        assert_eq!((itens[0].nivel, itens[0].pagina), (0, Some(3)));
        assert_eq!((itens[1].nivel, itens[1].pagina), (1, Some(4)));
        assert_eq!(itens[2].titulo, "Dois");
        assert!(parse_outline_xml("lixo < sem tags >").is_empty());
    }

    #[test]
    fn marcas_de_drm_sao_detectadas() {
        let t = Trechos {
            tamanho: 40,
            inicio: b"%PDF-1.7 ... /Filter /EBX_HANDLER ...".to_vec(),
            fim: Vec::new(),
        };
        assert_eq!(detectar_drm(&t), Some("Adobe ADEPT (EBX_HANDLER)"));
        let limpo = Trechos {
            tamanho: 10,
            inicio: b"%PDF-1.4 /Standard".to_vec(),
            fim: Vec::new(),
        };
        assert_eq!(detectar_drm(&limpo), None);
    }
}
