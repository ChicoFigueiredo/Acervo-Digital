//! Parser de `/proc/self/mountinfo` (formato de `proc(5)`).

use crate::erro::Erro;

/// Uma linha de mountinfo. Caminhos ficam em bytes brutos, já sem escapes octais.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntradaMontagem {
    pub id: u32,
    pub id_pai: u32,
    pub major: u32,
    pub minor: u32,
    /// Raiz do mount dentro do filesystem (`/` ou subdiretório, no caso de bind mount).
    pub raiz: Vec<u8>,
    pub ponto_de_montagem: Vec<u8>,
    pub opcoes: String,
    pub fs_type: String,
    /// Fonte do mount: `/dev/sda1`, `server:/export`, `proc`, `none`...
    pub fonte: Vec<u8>,
    pub opcoes_super: String,
}

/// Desfaz escapes `\NNN` (octal, ex.: `\040` espaço, `\134` barra invertida).
/// Sequências inválidas ficam literais; nunca falha e não assume UTF-8.
fn desescapar(entrada: &[u8]) -> Vec<u8> {
    let mut saida = Vec::with_capacity(entrada.len());
    let mut i = 0;
    while i < entrada.len() {
        if entrada[i] == b'\\'
            && i + 3 < entrada.len()
            && entrada[i + 1..i + 4]
                .iter()
                .all(|d| (b'0'..=b'7').contains(d))
        {
            let v = u32::from(entrada[i + 1] - b'0') * 64
                + u32::from(entrada[i + 2] - b'0') * 8
                + u32::from(entrada[i + 3] - b'0');
            if let Ok(b) = u8::try_from(v) {
                saida.push(b);
                i += 4;
                continue;
            }
        }
        saida.push(entrada[i]);
        i += 1;
    }
    saida
}

fn numero(campo: &[u8], linha: &[u8]) -> Result<u32, Erro> {
    std::str::from_utf8(campo)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalida(linha))
}

fn invalida(linha: &[u8]) -> Erro {
    Erro::MountinfoInvalido(String::from_utf8_lossy(linha).into_owned())
}

/// Interpreta uma linha (sem o `\n`).
pub fn parse_linha_mountinfo(linha: &[u8]) -> Result<EntradaMontagem, Erro> {
    let campos: Vec<&[u8]> = linha
        .split(|b| *b == b' ')
        .filter(|c| !c.is_empty())
        .collect();
    // Campos opcionais (ex.: `shared:1`) ficam entre o índice 6 e o separador `-`.
    let sep = campos
        .iter()
        .skip(6)
        .position(|c| *c == b"-")
        .map(|p| p + 6)
        .ok_or_else(|| invalida(linha))?;
    if campos.len() < sep + 4 {
        return Err(invalida(linha));
    }
    let (major, minor) = {
        let mut it = campos[2].splitn(2, |b| *b == b':');
        let (a, b) = (it.next(), it.next());
        match (a, b) {
            (Some(a), Some(b)) => (numero(a, linha)?, numero(b, linha)?),
            _ => return Err(invalida(linha)),
        }
    };
    let texto = |c: &[u8]| String::from_utf8_lossy(c).into_owned();
    Ok(EntradaMontagem {
        id: numero(campos[0], linha)?,
        id_pai: numero(campos[1], linha)?,
        major,
        minor,
        raiz: desescapar(campos[3]),
        ponto_de_montagem: desescapar(campos[4]),
        opcoes: texto(campos[5]),
        fs_type: texto(campos[sep + 1]),
        fonte: desescapar(campos[sep + 2]),
        opcoes_super: texto(campos[sep + 3]),
    })
}

/// Interpreta o arquivo inteiro. Linhas malformadas são ignoradas (uma linha ruim
/// não deve impedir a descoberta dos demais volumes); linhas vazias também.
pub fn parse_mountinfo(conteudo: &[u8]) -> Vec<EntradaMontagem> {
    conteudo
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| parse_linha_mountinfo(l).ok())
        .collect()
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn linha_simples_com_campos_opcionais() {
        let e = parse_linha_mountinfo(
            b"36 35 98:0 /mnt1 /mnt2 rw,noatime master:1 shared:2 - ext3 /dev/root rw,errors=continue",
        )
        .unwrap();
        assert_eq!((e.id, e.id_pai, e.major, e.minor), (36, 35, 98, 0));
        assert_eq!(e.raiz, b"/mnt1");
        assert_eq!(e.ponto_de_montagem, b"/mnt2");
        assert_eq!(e.fs_type, "ext3");
        assert_eq!(e.fonte, b"/dev/root");
        assert_eq!(e.opcoes_super, "rw,errors=continue");
    }

    #[test]
    fn escapes_octais_viram_bytes() {
        let e = parse_linha_mountinfo(
            b"40 27 8:17 / /media/user/Meu\\040Disco\\134X rw - vfat /dev/sdb1 rw",
        )
        .unwrap();
        assert_eq!(e.ponto_de_montagem, b"/media/user/Meu Disco\\X");
    }

    #[test]
    fn bytes_nao_utf8_sao_preservados() {
        let e =
            parse_linha_mountinfo(b"40 27 8:17 / /mnt/\xff\\012x rw - ext4 /dev/sdb1 rw").unwrap();
        assert_eq!(e.ponto_de_montagem, b"/mnt/\xff\nx");
    }

    #[test]
    fn linha_invalida_e_ignorada_no_arquivo() {
        let r = parse_mountinfo(b"lixo\n\n22 27 0:21 / /proc rw - proc proc rw\n");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].fs_type, "proc");
        assert!(parse_linha_mountinfo(b"1 2 3 / / rw").is_err());
    }
}
