//! OCR por processo: `tesseract stdin stdout -l <idiomas> tsv`. A imagem entra por memória e o
//! TSV volta por stdout, de onde saem o texto e a confiança média da página.

use std::ffi::OsString;
use std::time::Duration;

use crate::ferramentas::Ferramenta;
use crate::proc::{Entrada, ErroProc, executar};

/// Resultado do OCR de uma página.
#[derive(Debug, Clone, PartialEq)]
pub struct SaidaOcr {
    pub texto: String,
    /// Confiança média das palavras, 0..=1 (0 se não houve palavras).
    pub confianca: f32,
    pub palavras: usize,
}

/// Interpreta o TSV do tesseract (nível 5 = palavra; `conf` -1 = estrutura).
#[must_use]
pub fn interpretar_tsv(tsv: &str) -> SaidaOcr {
    let mut texto = String::new();
    let mut soma = 0.0f64;
    let mut palavras = 0usize;
    let mut linha_atual: Option<(u32, u32, u32, u32)> = None;
    for l in tsv.lines().skip(1) {
        let c: Vec<&str> = l.splitn(12, '\t').collect();
        if c.len() < 12 || c[0] != "5" {
            continue;
        }
        let palavra = c[11].trim();
        let conf: f64 = c[10].trim().parse().unwrap_or(-1.0);
        if palavra.is_empty() || conf < 0.0 {
            continue;
        }
        let n = |i: usize| c[i].trim().parse::<u32>().unwrap_or(0);
        let chave = (n(1), n(2), n(3), n(4));
        match linha_atual {
            None => {}
            Some(ant) if ant == chave => texto.push(' '),
            Some(ant) if (ant.0, ant.1, ant.2) == (chave.0, chave.1, chave.2) => texto.push('\n'),
            Some(_) => texto.push_str("\n\n"),
        }
        linha_atual = Some(chave);
        texto.push_str(palavra);
        soma += conf;
        palavras += 1;
    }
    let confianca = if palavras == 0 {
        0.0
    } else {
        (soma / palavras as f64 / 100.0) as f32
    };
    SaidaOcr {
        texto,
        confianca: confianca.clamp(0.0, 1.0),
        palavras,
    }
}

/// Roda o tesseract sobre uma imagem (PNG/PNM/JPEG) em memória.
pub fn reconhecer(
    tesseract: &Ferramenta,
    imagem: &[u8],
    idiomas: &str,
    timeout: Duration,
    limite_saida: usize,
) -> Result<SaidaOcr, ErroProc> {
    let args: Vec<OsString> = ["stdin", "stdout", "-l", idiomas, "--psm", "3", "tsv"]
        .iter()
        .map(OsString::from)
        .collect();
    let saida = executar(
        &tesseract.caminho,
        &args,
        Entrada::Bytes(imagem),
        // Uma thread por processo: várias páginas e várias frentes disputam a CPU.
        &[("OMP_THREAD_LIMIT", "1")],
        timeout,
        limite_saida,
    )?;
    if !saida.sucesso {
        let primeira = saida.stderr.lines().next().unwrap_or("falha sem mensagem");
        return Err(ErroProc::Io(format!("tesseract: {primeira}")));
    }
    Ok(interpretar_tsv(&String::from_utf8_lossy(&saida.stdout)))
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn tsv_vira_texto_e_confianca_media() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
1\t1\t0\t0\t0\t0\t0\t0\t100\t100\t-1\t\n\
5\t1\t1\t1\t1\t1\t0\t0\t10\t10\t90.5\tOlá\n\
5\t1\t1\t1\t1\t2\t0\t0\t10\t10\t80.0\tmundo\n\
5\t1\t1\t1\t2\t1\t0\t0\t10\t10\t70.0\tsegunda\n\
5\t1\t2\t1\t1\t1\t0\t0\t10\t10\t60.0\tbloco\n";
        let o = interpretar_tsv(tsv);
        assert_eq!(o.texto, "Olá mundo\nsegunda\n\nbloco");
        assert_eq!(o.palavras, 4);
        assert!((o.confianca - 0.75125).abs() < 0.001, "{}", o.confianca);
        assert_eq!(interpretar_tsv("").palavras, 0);
    }
}
