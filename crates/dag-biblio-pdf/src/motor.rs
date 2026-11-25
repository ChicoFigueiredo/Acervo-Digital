//! Amostragem progressiva e OCR seletivo, independentes do formato (PDF e DjVu implementam
//! [`Motor`]).
//!
//! # Política de amostragem
//!
//! 1. Fase `inicial`: páginas `1..=paginas_iniciais` (padrão 20), limitadas ao teto. Procura capa,
//!    folha de rosto, verso com ficha catalográfica (CIP), sumário/índice e ISBN.
//! 2. Se faltar identificação (nem ISBN válido nem título e autor com confiança), ou faltar sumário,
//!    expande para a fase `final`: as últimas `paginas_finais` páginas (índice tardio, colofão).
//! 3. Se ainda faltar identificação (não apenas o sumário), fase `meio`: `amostras_meio` páginas
//!    espaçadas pelo miolo.
//! 4. Nunca passa de `teto_paginas` páginas amostradas, `max_paginas_ocr` páginas com OCR, nem do
//!    tempo total. Estourar tempo devolve o que já existe com `LimiteExcedido`.
//!
//! # Texto nativo versus OCR (por página)
//!
//! Texto nativo é confiável com pelo menos `min_caracteres_pagina` caracteres úteis e qualidade
//! acima de `qualidade_minima_texto`. Fora disso a página entra em OCR se tiver imagem (ou se não se
//! souber), ou se o texto nativo for ilegível. Página sem texto e sem imagem é `Vazia` (sem OCR).
//! Em páginas híbridas fica o texto com mais caracteres alfanuméricos.
//!
//! O texto extraído é sempre DADO: é guardado como veio, nunca interpretado; trechos que parecem
//! instruções a um modelo são marcados em `alertas_conteudo` e em `avisos`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use dag_biblio::{
    Campo, EstadoLeitura, Evidencia, ItemSumario, OrigemEvidencia, PaginaTexto, ValorSugerido, isbn,
};

use crate::config::ConfigExtracaoPdf;
use crate::ferramentas::Ferramenta;
use crate::metadados::CONFIANCA_METADADO;
use crate::ocr;
use crate::proc::ErroProc;
use crate::relatorio::{ClassePagina, MedicaoPagina};
use crate::texto::{
    AlertaConteudo, avaliar_texto, contar_alfanumericos, detectar_idioma,
    detectar_instrucoes_suspeitas, detectar_sumario, extrair_da_ficha, itens_de_sumario,
    parece_ficha,
};

/// Falha de uma chamada ao formato.
#[derive(Debug)]
pub enum ErroMotor {
    Timeout,
    Saida,
    Falha(String),
}

impl From<ErroProc> for ErroMotor {
    fn from(e: ErroProc) -> Self {
        match e {
            ErroProc::Timeout(_) => Self::Timeout,
            ErroProc::SaidaExcedida(_) => Self::Saida,
            outro => Self::Falha(outro.to_string()),
        }
    }
}

/// OCR de uma página: saída e milissegundos gastos.
type ResultadoOcr = Result<(ocr::SaidaOcr, f64), ErroMotor>;

/// Acesso às páginas de um documento. `Sync`: o OCR roda várias páginas em paralelo.
pub trait Motor: Sync {
    /// Texto nativo das páginas `de..=ate` (1-based), uma `String` por página, nessa ordem.
    fn texto_nativo(&self, de: u32, ate: u32, timeout: Duration) -> Result<Vec<String>, ErroMotor>;
    /// Páginas (dentro do intervalo) com imagem grande. `None` se não for possível saber.
    fn paginas_com_imagem(&self, de: u32, ate: u32, timeout: Duration) -> Option<BTreeSet<u32>>;
    /// Imagem da página para o OCR (PNG ou PNM), em memória.
    fn imagem_para_ocr(
        &self,
        pagina: u32,
        dpi: u32,
        timeout: Duration,
    ) -> Result<Vec<u8>, ErroMotor>;
}

/// Entrada do OCR já resolvida.
#[derive(Debug)]
pub struct ConfigOcr<'a> {
    pub tesseract: &'a Ferramenta,
    pub idiomas: String,
}

/// Valores derivados do texto amostrado.
#[derive(Debug, Default)]
pub struct Evidencias {
    pub isbns: Vec<(String, Evidencia)>,
    pub ficha: Option<(u32, String)>,
    pub valores_ficha: Vec<ValorSugerido>,
    pub sumario_texto: Vec<ItemSumario>,
    pub idioma: Option<&'static str>,
}

/// Estado de uma amostragem em andamento.
pub struct Analise<'a> {
    motor: &'a dyn Motor,
    cfg: &'a ConfigExtracaoPdf,
    ocr: Option<ConfigOcr<'a>>,
    inicio: Instant,
    pub paginas: BTreeMap<u32, PaginaTexto>,
    pub medicoes: BTreeMap<u32, MedicaoPagina>,
    pub paginas_ocr: Vec<u32>,
    pub avisos: Vec<String>,
    pub alertas: Vec<AlertaConteudo>,
    pub fases: Vec<String>,
    /// Primeiro limite estourado, se algum.
    pub limite: Option<String>,
    tentativas_texto: u32,
    falhas_texto: u32,
    avisou_sem_ocr: bool,
    visitadas: BTreeSet<u32>,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn faixas(paginas: &[u32]) -> Vec<(u32, u32)> {
    let mut fx: Vec<(u32, u32)> = Vec::new();
    for &p in paginas {
        match fx.last_mut() {
            Some((_, fim)) if *fim + 1 == p => *fim = p,
            _ => fx.push((p, p)),
        }
    }
    fx
}

impl std::fmt::Debug for Analise<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analise")
            .field("paginas", &self.paginas.len())
            .field("paginas_ocr", &self.paginas_ocr)
            .field("fases", &self.fases)
            .field("limite", &self.limite)
            .finish_non_exhaustive()
    }
}

impl<'a> Analise<'a> {
    pub fn nova(
        motor: &'a dyn Motor,
        cfg: &'a ConfigExtracaoPdf,
        ocr: Option<ConfigOcr<'a>>,
        inicio: Instant,
    ) -> Self {
        Self {
            motor,
            cfg,
            ocr,
            inicio,
            paginas: BTreeMap::new(),
            medicoes: BTreeMap::new(),
            paginas_ocr: Vec::new(),
            avisos: Vec::new(),
            alertas: Vec::new(),
            fases: Vec::new(),
            limite: None,
            tentativas_texto: 0,
            falhas_texto: 0,
            avisou_sem_ocr: false,
            visitadas: BTreeSet::new(),
        }
    }

    /// Todas as chamadas de texto falharam (e houve ao menos uma): o corpo do arquivo não é legível.
    #[must_use]
    pub fn texto_ilegivel(&self) -> bool {
        self.tentativas_texto > 0 && self.falhas_texto == self.tentativas_texto
    }

    fn restante(&self) -> Duration {
        self.cfg
            .limites
            .timeout_total
            .saturating_sub(self.inicio.elapsed())
    }

    fn marcar_limite(&mut self, qual: &str) {
        if self.limite.is_none() {
            self.limite = Some(qual.to_string());
        }
    }

    /// Roda a amostragem progressiva e devolve as evidências finais.
    pub fn amostrar(
        &mut self,
        total: u32,
        tem_sumario_nativo: bool,
        ja_conhecidos: &[ValorSugerido],
    ) -> Evidencias {
        let teto = self.cfg.teto_paginas.max(1);
        let n_ini = self.cfg.paginas_iniciais.max(1).min(teto).min(total);
        self.fases.push("inicial".into());
        self.processar(&(1..=n_ini).collect::<Vec<_>>());

        for fase in ["final", "meio"] {
            if self.limite.is_some() || self.texto_ilegivel() {
                break;
            }
            let ev = self.avaliar(ja_conhecidos);
            let identificado = identificacao_ok(&ev, ja_conhecidos);
            let tem_sumario = tem_sumario_nativo || !ev.sumario_texto.is_empty();
            if identificado && tem_sumario {
                break;
            }
            if fase == "meio" && identificado {
                break;
            }
            let pags = self.plano(fase, total, teto);
            if pags.is_empty() {
                continue;
            }
            self.fases.push(fase.into());
            self.processar(&pags);
        }
        self.avaliar(ja_conhecidos)
    }

    fn plano(&self, fase: &str, total: u32, teto: u32) -> Vec<u32> {
        let sobra = (teto as usize).saturating_sub(self.visitadas.len());
        if sobra == 0 || total == 0 {
            return Vec::new();
        }
        let mut pags: Vec<u32> = if fase == "final" {
            let k = self.cfg.paginas_finais.min(total);
            (total - k + 1..=total)
                .filter(|p| !self.visitadas.contains(p))
                .collect()
        } else {
            let k = self.cfg.amostras_meio;
            let mut v = Vec::new();
            if k > 0 {
                for i in 1..=k {
                    let p = 1 + (u64::from(total - 1) * u64::from(i) / u64::from(k + 1)) as u32;
                    if !self.visitadas.contains(&p) && !v.contains(&p) {
                        v.push(p);
                    }
                }
            }
            v
        };
        pags.truncate(sobra);
        pags
    }

    /// Lê (texto nativo, imagem, OCR) as páginas pedidas que ainda não foram vistas.
    fn processar(&mut self, pedidas: &[u32]) {
        let novas: Vec<u32> = pedidas
            .iter()
            .copied()
            .filter(|p| !self.visitadas.contains(p))
            .collect();
        for fx in faixas(&novas) {
            if self.limite.is_some() {
                return;
            }
            self.ler_faixa(fx.0, fx.1);
        }
    }

    fn ler_faixa(&mut self, de: u32, ate: u32) {
        if self.restante().is_zero() {
            self.marcar_limite("tempo total");
            return;
        }
        let n = ate - de + 1;
        let timeout = (self.cfg.limites.timeout_pagina * n).min(self.restante());
        self.tentativas_texto += 1;
        let t0 = Instant::now();
        let textos = match self.motor.texto_nativo(de, ate, timeout) {
            Ok(t) => t,
            Err(ErroMotor::Timeout) => {
                self.marcar_limite("tempo por página");
                self.falhas_texto += 1;
                return;
            }
            Err(ErroMotor::Saida) => {
                self.marcar_limite("tamanho da saída");
                self.falhas_texto += 1;
                return;
            }
            Err(ErroMotor::Falha(m)) if n > 1 => {
                // Isola a página ruim: tenta uma a uma.
                self.avisos.push(format!(
                    "falha ao ler páginas {de}-{ate} em lote ({m}); tentando uma a uma"
                ));
                // O lote não conta; cada página conta por si.
                self.tentativas_texto = self.tentativas_texto.saturating_sub(1);
                for p in de..=ate {
                    self.ler_faixa(p, p);
                    if self.limite.is_some() {
                        return;
                    }
                }
                return;
            }
            Err(ErroMotor::Falha(m)) => {
                self.avisos
                    .push(format!("falha ao ler o texto da página {de}: {m}"));
                self.falhas_texto += 1;
                self.visitadas.insert(de);
                return;
            }
        };
        let por_pagina = ms(t0.elapsed()) / f64::from(n);
        let imagens = self.motor.paginas_com_imagem(
            de,
            ate,
            self.cfg.limites.timeout_metadados.min(self.restante()),
        );

        let mut para_ocr: Vec<u32> = Vec::new();
        for (i, p) in (de..=ate).enumerate() {
            self.visitadas.insert(p);
            let mut bruto = textos.get(i).cloned().unwrap_or_default();
            bruto.retain(|c| c != '\0' && c != '\u{c}');
            if bruto.chars().count() > self.cfg.limites.max_caracteres_pagina {
                bruto = bruto
                    .chars()
                    .take(self.cfg.limites.max_caracteres_pagina)
                    .collect();
                self.avisos.push(format!(
                    "página {p}: texto cortado em {} caracteres",
                    self.cfg.limites.max_caracteres_pagina
                ));
            }
            let av = avaliar_texto(&bruto);
            let tem_img = imagens.as_ref().map(|s| s.contains(&p));
            let confiavel = av.uteis >= self.cfg.min_caracteres_pagina
                && av.qualidade >= self.cfg.qualidade_minima_texto;
            let precisa_ocr = if confiavel {
                false
            } else if av.uteis == 0 {
                tem_img != Some(false)
            } else if av.qualidade < self.cfg.qualidade_minima_texto {
                true
            } else {
                tem_img == Some(true)
            };
            let classe = if confiavel {
                ClassePagina::Nativa
            } else if precisa_ocr && av.uteis == 0 {
                ClassePagina::Imagem
            } else if precisa_ocr {
                ClassePagina::Hibrida
            } else if av.uteis == 0 {
                ClassePagina::Vazia
            } else {
                ClassePagina::Nativa
            };
            self.medicoes.insert(
                p,
                MedicaoPagina {
                    pagina: p,
                    classe,
                    caracteres_uteis: av.uteis,
                    qualidade_nativa: av.qualidade,
                    tempo_texto_ms: por_pagina,
                    tempo_ocr_ms: None,
                    qualidade_ocr: None,
                },
            );
            if !bruto.trim().is_empty() {
                self.registrar_texto(p, bruto, false, av.qualidade, None);
            }
            if precisa_ocr {
                para_ocr.push(p);
            }
        }
        self.ocr_em_paralelo(&para_ocr);
    }

    /// OCR de várias páginas ao mesmo tempo (`paralelo_ocr` processos), aplicando os resultados
    /// na ordem das páginas. Os limites (páginas de OCR, tempo total) valem por lote.
    fn ocr_em_paralelo(&mut self, paginas: &[u32]) {
        let lote = self.cfg.paralelo_ocr.max(1) as usize;
        let mut resto = paginas;
        while !resto.is_empty() {
            if self.limite.is_some() {
                break;
            }
            let Some(ocr_cfg) = &self.ocr else {
                // Sem OCR: o caminho sequencial só registra o aviso uma vez.
                self.fazer_ocr(resto[0]);
                return;
            };
            let livres = self
                .cfg
                .max_paginas_ocr
                .saturating_sub(self.paginas_ocr.len() as u32) as usize;
            if livres == 0 || self.restante().is_zero() {
                // Deixa o caminho sequencial registrar o aviso/limite certo.
                self.fazer_ocr(resto[0]);
                return;
            }
            let n = lote.min(livres).min(resto.len());
            let (agora, depois) = resto.split_at(n);
            resto = depois;
            if n == 1 {
                self.fazer_ocr(agora[0]);
                continue;
            }
            let motor = self.motor;
            let cfg = self.cfg;
            let tesseract = ocr_cfg.tesseract;
            let idiomas = ocr_cfg.idiomas.clone();
            let timeout = cfg.limites.timeout_pagina.min(self.restante());
            let resultados: Vec<(u32, ResultadoOcr)> = std::thread::scope(|s| {
                let alcas: Vec<_> = agora
                    .iter()
                    .map(|&p| {
                        let idiomas = idiomas.as_str();
                        s.spawn(move || {
                            let t0 = Instant::now();
                            let r = motor
                                .imagem_para_ocr(p, cfg.dpi_ocr, timeout)
                                .and_then(|img| {
                                    ocr::reconhecer(
                                        tesseract,
                                        &img,
                                        idiomas,
                                        timeout,
                                        cfg.limites.max_bytes_saida,
                                    )
                                    .map_err(ErroMotor::from)
                                })
                                .map(|s| (s, ms(t0.elapsed())));
                            (p, r)
                        })
                    })
                    .collect();
                alcas
                    .into_iter()
                    .map(|a| {
                        a.join()
                            .unwrap_or_else(|_| (0, Err(ErroMotor::Falha("OCR em pânico".into()))))
                    })
                    .collect()
            });
            for (p, r) in resultados {
                match r {
                    Ok((saida, gasto)) => self.aplicar_ocr(p, saida, gasto, &idiomas),
                    Err(e) => self.falha_ocr(p, &e),
                }
            }
        }
    }

    fn registrar_texto(
        &mut self,
        p: u32,
        texto: String,
        via_ocr: bool,
        qualidade: f32,
        idioma: Option<String>,
    ) {
        let alertas = detectar_instrucoes_suspeitas(p, &texto);
        if !alertas.is_empty() {
            self.avisos.push(format!(
                "conteudo_suspeito: página {p} contém texto que parece instrução a um modelo ({}); tratado apenas como dado",
                alertas.iter().map(|a| a.padrao.as_str()).collect::<Vec<_>>().join(", ")
            ));
            self.alertas.extend(alertas);
        }
        self.paginas.insert(
            p,
            PaginaTexto {
                numero: p,
                texto,
                via_ocr,
                qualidade,
                idioma_ocr: idioma,
            },
        );
    }

    fn fazer_ocr(&mut self, p: u32) {
        let Some(ocr_cfg) = &self.ocr else {
            if !self.avisou_sem_ocr {
                self.avisou_sem_ocr = true;
                self.avisos.push("OCR indisponível (tesseract ausente ou sem idioma configurado): páginas sem texto ficam sem texto".into());
            }
            return;
        };
        if self.paginas_ocr.len() as u32 >= self.cfg.max_paginas_ocr {
            if !self.avisos.iter().any(|a| a.starts_with("limite de OCR")) {
                self.avisos.push(format!(
                    "limite de OCR: só as primeiras {} páginas sem texto foram lidas",
                    self.cfg.max_paginas_ocr
                ));
            }
            return;
        }
        if self.restante().is_zero() {
            self.marcar_limite("tempo total");
            return;
        }
        let idiomas = ocr_cfg.idiomas.clone();
        let tesseract = ocr_cfg.tesseract;
        let t0 = Instant::now();
        let timeout = self.cfg.limites.timeout_pagina.min(self.restante());
        let imagem = match self.motor.imagem_para_ocr(p, self.cfg.dpi_ocr, timeout) {
            Ok(i) => i,
            Err(e) => return self.falha_ocr(p, &e),
        };
        let timeout = self.cfg.limites.timeout_pagina.min(self.restante());
        let saida = match ocr::reconhecer(
            tesseract,
            &imagem,
            &idiomas,
            timeout,
            self.cfg.limites.max_bytes_saida,
        ) {
            Ok(s) => s,
            Err(e) => return self.falha_ocr(p, &ErroMotor::from(e)),
        };
        let gasto = ms(t0.elapsed());
        self.aplicar_ocr(p, saida, gasto, &idiomas);
    }

    fn aplicar_ocr(&mut self, p: u32, saida: ocr::SaidaOcr, gasto: f64, idiomas: &str) {
        self.paginas_ocr.push(p);
        if let Some(m) = self.medicoes.get_mut(&p) {
            m.tempo_ocr_ms = Some(gasto);
            m.qualidade_ocr = Some(saida.confianca);
        }
        if saida.confianca < 0.5 && saida.palavras > 0 {
            self.avisos.push(format!(
                "página {p}: OCR de baixa qualidade ({:.0}%)",
                saida.confianca * 100.0
            ));
        }
        let nativo = self
            .paginas
            .get(&p)
            .map_or(0, |pt| contar_alfanumericos(&pt.texto));
        if contar_alfanumericos(&saida.texto) > nativo {
            self.paginas.remove(&p);
            self.registrar_texto(
                p,
                saida.texto,
                true,
                saida.confianca,
                Some(idiomas.to_owned()),
            );
        }
    }

    fn falha_ocr(&mut self, p: u32, e: &ErroMotor) {
        match e {
            ErroMotor::Timeout => {
                self.marcar_limite("tempo por página");
                self.avisos.push(format!(
                    "OCR da página {p} excedeu o tempo; processo encerrado"
                ));
            }
            ErroMotor::Saida => {
                self.marcar_limite("tamanho da saída");
                self.avisos
                    .push(format!("OCR da página {p} excedeu o limite de saída"));
            }
            ErroMotor::Falha(m) => self.avisos.push(format!("OCR da página {p} falhou: {m}")),
        }
    }

    /// Deriva as evidências do texto já amostrado.
    #[must_use]
    pub fn avaliar(&self, ja_conhecidos: &[ValorSugerido]) -> Evidencias {
        let mut ev = Evidencias::default();
        let pagina_ficha = self
            .paginas
            .values()
            .find(|p| parece_ficha(&p.texto))
            .map(|p| p.numero);
        if let Some(n) = pagina_ficha {
            let texto: String = self.paginas[&n].texto.trim().chars().take(4000).collect();
            ev.valores_ficha = extrair_da_ficha(&texto, n);
            ev.ficha = Some((n, texto));
        }

        let mut melhores: BTreeMap<String, Evidencia> = BTreeMap::new();
        for pt in self.paginas.values() {
            for i in isbn::encontrar(&pt.texto) {
                let (origem, conf) = if Some(pt.numero) == pagina_ficha {
                    (OrigemEvidencia::FichaCatalografica, 0.9)
                } else if pt.via_ocr {
                    (OrigemEvidencia::Ocr, 0.6)
                } else {
                    (OrigemEvidencia::Conteudo, 0.75)
                };
                let trecho = pt
                    .texto
                    .lines()
                    .map(str::trim)
                    .find(|l| isbn::encontrar(l).contains(&i))
                    .map(|l| l.chars().take(200).collect::<String>());
                let nova = Evidencia {
                    origem,
                    pagina: Some(pt.numero),
                    trecho,
                    detalhe: None,
                    confianca: conf,
                };
                match melhores.get(&i) {
                    Some(atual) if atual.confianca >= conf => {}
                    _ => {
                        melhores.insert(i, nova);
                    }
                }
            }
        }
        let mut isbns: Vec<(String, Evidencia)> = melhores.into_iter().collect();
        isbns.sort_by(|a, b| {
            b.1.confianca
                .total_cmp(&a.1.confianca)
                .then(a.1.pagina.cmp(&b.1.pagina))
        });
        ev.isbns = isbns;

        // Sumário pelo texto: primeira página com cabeçalho + continuação imediata.
        let mut ult: Option<u32> = None;
        for pt in self.paginas.values() {
            if ev.sumario_texto.is_empty() {
                if let Some(itens) = detectar_sumario(&pt.texto) {
                    ev.sumario_texto = itens;
                    ult = Some(pt.numero);
                }
            } else if let Some(u) = ult
                && pt.numero == u + 1
                && itens_de_sumario(&pt.texto).len() >= 6
            {
                ev.sumario_texto.extend(itens_de_sumario(&pt.texto));
                ult = Some(pt.numero);
            }
        }

        let amostra: String = self
            .paginas
            .values()
            .map(|p| p.texto.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(200_000)
            .collect();
        ev.idioma = detectar_idioma(&amostra);
        let _ = ja_conhecidos;
        ev
    }
}

/// Há identificação suficiente para parar de expandir?
#[must_use]
pub fn identificacao_ok(ev: &Evidencias, conhecidos: &[ValorSugerido]) -> bool {
    if !ev.isbns.is_empty() {
        return true;
    }
    let bom = |c: Campo| {
        conhecidos
            .iter()
            .chain(ev.valores_ficha.iter())
            .any(|v| v.campo == c && v.valor.is_some() && v.confianca >= CONFIANCA_METADADO)
    };
    bom(Campo::Titulo) && bom(Campo::Autores)
}

/// Estado final a partir do limite estourado (se houver).
#[must_use]
pub fn estado_final(limite: Option<String>) -> EstadoLeitura {
    limite.map_or(EstadoLeitura::Lido, |l| EstadoLeitura::LimiteExcedido {
        limite: l,
    })
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn faixas_agrupam_paginas_consecutivas() {
        assert_eq!(faixas(&[1, 2, 3, 7, 9, 10]), vec![(1, 3), (7, 7), (9, 10)]);
        assert!(faixas(&[]).is_empty());
    }
}
