//! Testes unitários do descompressor PalmDOC e dos extras de registro.

use crate::mobi::{descomprimir_palmdoc, tamanho_extras};

#[test]
fn palmdoc_referencia_espaco_e_literal() {
    // "abc" + cópia (dist 3, len 3) + espaço+'a' (0xE1) + bloco literal de 2 bytes.
    let entrada = [b'a', b'b', b'c', 0x80, 0x18, 0xE1, 2, 0xC3, 0xA9];
    assert_eq!(
        descomprimir_palmdoc(&entrada, 100),
        "abcabc a\u{e9}".as_bytes()
    );
}

#[test]
fn palmdoc_hostil_nao_entra_em_panico() {
    // Distância maior que a saída, truncado no meio, bloco literal além do fim.
    assert_eq!(descomprimir_palmdoc(&[0x80, 0x08], 100), b"");
    assert_eq!(descomprimir_palmdoc(&[b'a', 0x80], 100), b"a");
    assert_eq!(descomprimir_palmdoc(&[8, b'a'], 100), b"a");
    assert!(descomprimir_palmdoc(&[b'a', 0xBF, 0xFF], 5).len() <= 5);
}

#[test]
fn extras_finais_sao_medidos_sem_estourar() {
    assert_eq!(tamanho_extras(&[], 0xFFFF), 0);
    assert_eq!(tamanho_extras(&[1, 2, 0], 1), 1);
    assert!(tamanho_extras(&[0xFF; 4], 0xFFFF) <= 4);
}
