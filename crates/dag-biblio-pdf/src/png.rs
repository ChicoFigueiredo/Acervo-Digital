//! Codificador PNG mínimo (cinza ou RGB de 8 bits) para a capa de DjVu: o `ddjvu` entrega PNM e a
//! capa tem de sair em PNG/JPEG, em memória.

use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

fn crc32(dados: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in dados {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn bloco(saida: &mut Vec<u8>, tipo: &[u8; 4], dados: &[u8]) {
    saida.extend_from_slice(&(dados.len() as u32).to_be_bytes());
    let mut corpo = tipo.to_vec();
    corpo.extend_from_slice(dados);
    saida.extend_from_slice(&corpo);
    saida.extend_from_slice(&crc32(&corpo).to_be_bytes());
}

/// Converte um PNM binário (P5 cinza ou P6 RGB, maxval 255) em PNG. `None` se o PNM for inválido.
#[must_use]
pub fn pnm_para_png(pnm: &[u8]) -> Option<Vec<u8>> {
    // Cabeçalho: magic, largura, altura, maxval, separados por espaço/comentário; um espaço e os dados.
    let mut pos = 0usize;
    let mut campos: Vec<String> = Vec::new();
    while campos.len() < 4 {
        while pos < pnm.len() && pnm[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pnm.get(pos) == Some(&b'#') {
            while pos < pnm.len() && pnm[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        let ini = pos;
        while pos < pnm.len() && !pnm[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if ini == pos {
            return None;
        }
        campos.push(String::from_utf8_lossy(&pnm[ini..pos]).into_owned());
    }
    pos += 1;
    let canais: usize = match campos[0].as_str() {
        "P5" => 1,
        "P6" => 3,
        _ => return None,
    };
    let largura: usize = campos[1].parse().ok()?;
    let altura: usize = campos[2].parse().ok()?;
    if campos[3] != "255" || largura == 0 || altura == 0 || largura > 20_000 || altura > 20_000 {
        return None;
    }
    let linha = largura.checked_mul(canais)?;
    let dados = pnm.get(pos..pos.checked_add(linha.checked_mul(altura)?)?)?;
    let mut cru = Vec::with_capacity((linha + 1) * altura);
    for l in dados.chunks(linha) {
        cru.push(0);
        cru.extend_from_slice(l);
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(&cru).ok()?;
    let comprimido = z.finish().ok()?;

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(largura as u32).to_be_bytes());
    ihdr.extend_from_slice(&(altura as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, if canais == 1 { 0 } else { 2 }, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    bloco(&mut png, b"IHDR", &ihdr);
    bloco(&mut png, b"IDAT", &comprimido);
    bloco(&mut png, b"IEND", &[]);
    Some(png)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn converte_pgm_pequeno() {
        let mut pnm = b"P5\n2 2\n255\n".to_vec();
        pnm.extend_from_slice(&[0, 64, 128, 255]);
        let png = pnm_para_png(&pnm).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    #[test]
    fn rejeita_pnm_truncado_ou_invalido() {
        assert!(pnm_para_png(b"P5\n10 10\n255\nabc").is_none());
        assert!(pnm_para_png(b"nada").is_none());
        assert!(pnm_para_png(b"P3\n1 1\n255\n0 0 0").is_none());
    }
}
