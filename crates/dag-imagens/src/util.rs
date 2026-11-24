//! Utilitários de leitura segura de campos (sem pânico em limites).

pub(crate) fn campo<const N: usize>(b: &[u8], o: usize) -> [u8; N] {
    let mut r = [0u8; N];
    if let Some(fim) = o.checked_add(N)
        && let Some(s) = b.get(o..fim)
    {
        r.copy_from_slice(s);
    }
    r
}
pub(crate) fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(campo(b, o))
}
pub(crate) fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(campo(b, o))
}
pub(crate) fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(campo(b, o))
}
pub(crate) fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes(campo(b, o))
}
pub(crate) fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(campo(b, o))
}
pub(crate) fn be64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(campo(b, o))
}
pub(crate) fn byte(b: &[u8], o: usize) -> u8 {
    b.get(o).copied().unwrap_or(0)
}
/// Sub-fatia segura (vazia se fora dos limites).
pub(crate) fn fatia(b: &[u8], o: usize, n: usize) -> &[u8] {
    o.checked_add(n)
        .and_then(|f| b.get(o..f))
        .unwrap_or_default()
}

/// Dias desde 1970-01-01 para uma data civil.
pub(crate) fn dias_civil(a: i64, m: i64, d: i64) -> i64 {
    let a = if m <= 2 { a - 1 } else { a };
    let era = a.div_euclid(400);
    let yoe = a - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub(crate) fn data_para_unix(ano: i64, mes: i64, dia: i64, h: i64, mi: i64, s: i64) -> Option<i64> {
    if !(1..=12).contains(&mes) || !(1..=31).contains(&dia) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some(dias_civil(ano, mes, dia) * 86_400 + h * 3600 + mi * 60 + s)
}

/// Data/hora DOS (FAT/exFAT) para segundos Unix.
pub(crate) fn dos_para_unix(data: u16, hora: u16) -> Option<i64> {
    data_para_unix(
        1980 + i64::from(data >> 9),
        i64::from((data >> 5) & 0xF),
        i64::from(data & 0x1F),
        i64::from(hora >> 11),
        i64::from((hora >> 5) & 0x3F),
        i64::from(hora & 0x1F) * 2,
    )
}

pub(crate) fn utf16le_para_string(b: &[u8]) -> String {
    let u: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&u)
}

pub(crate) fn utf16be_para_string(b: &[u8]) -> String {
    let u: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&u)
}

/// Nome seguro como componente de caminho: troca `/` e NUL.
pub(crate) fn limpar_nome(n: Vec<u8>) -> Vec<u8> {
    n.into_iter()
        .map(|c| if c == b'/' || c == 0 { b'_' } else { c })
        .collect()
}

pub(crate) fn juntar(pai: &[u8], nome: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(pai.len() + nome.len() + 1);
    v.extend_from_slice(pai);
    if !pai.is_empty() {
        v.push(b'/');
    }
    v.extend_from_slice(nome);
    v
}
