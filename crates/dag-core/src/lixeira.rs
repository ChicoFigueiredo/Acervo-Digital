//! Tipos da Lixeira em dois níveis (ADR 0005): resultado de mover e restaurar, itens Na Lixeira.
//!
//! A única escrita que o DAG faz em arquivos catalogados é o `rename(2)` para a `DAG-Lixeira` do
//! próprio Volume e de volta; nunca apaga, sobrescreve nem cruza Volumes.

use crate::{ArquivoId, VolumeId};
use serde::{Deserialize, Serialize};

/// Nome da pasta da Lixeira do Volume, na raiz do sistema de arquivos.
pub const NOME_LIXEIRA: &str = "DAG-Lixeira";

/// Texto que o usuário confirma (digitando ou marcando) no segundo diálogo da GUI.
pub const FRASE_ENTENDIMENTO: &str = "Entendo que os arquivos serão movidos para DAG-Lixeira";

/// Move ou restaura.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcaoLixeira {
    Mover,
    Restaurar,
}

/// Um arquivo movido (ou, em simulação, que seria movido).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemMovido {
    pub arquivo: ArquivoId,
    pub volume: VolumeId,
    /// Caminho antes do movimento.
    pub de: String,
    /// Caminho depois (em simulação, o destino previsto).
    pub para: String,
    pub bytes: u64,
    /// Única Cópia no Catálogo (nenhuma outra cópia fora da Lixeira).
    #[serde(default)]
    pub unica_copia: bool,
}

/// Um alvo (ou arquivo) recusado, com o motivo e, quando útil, os arquivos que o causaram.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRecusado {
    pub alvo: String,
    /// Código estável: `sem_marcacao`, `unica_copia`, `mudou_no_disco`, `outro_volume`,
    /// `hard_link`, `permissao_negada`, `nao_encontrado`...
    pub codigo: String,
    pub motivo: String,
    /// Amostra dos arquivos responsáveis (por exemplo, os sem Marcação numa pasta).
    #[serde(default)]
    pub arquivos: Vec<String>,
}

/// Resultado de `MoverParaLixeira` e `RestaurarDaLixeira`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultadoLixeira {
    pub acao: AcaoLixeira,
    /// Só planejou: nada foi tocado no disco.
    pub simulado: bool,
    pub movidos: Vec<ItemMovido>,
    pub recusados: Vec<ItemRecusado>,
    /// Totais exatos (`movidos` pode ser uma amostra).
    pub total_arquivos: u64,
    pub total_bytes: u64,
    pub unicas_copias: u64,
    /// Marcações para Deleção criadas pelo passo "marcar tudo e mover".
    #[serde(default)]
    pub marcados_agora: u64,
    /// Lixeiras do Volume envolvidas (`<ponto de montagem>/DAG-Lixeira`).
    pub destinos: Vec<String>,
    /// Volumes envolvidos (rótulo ou ponto de montagem).
    #[serde(default)]
    pub volumes: Vec<String>,
}

/// Um item Na Lixeira.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemLixeira {
    pub arquivo: ArquivoId,
    pub volume: VolumeId,
    /// Onde está agora (dentro da `DAG-Lixeira`).
    pub caminho: String,
    /// De onde veio (para onde `RestaurarDaLixeira` o devolve).
    pub origem: String,
    pub bytes: u64,
    /// Segundos Unix.
    pub movido_em: Option<i64>,
}

/// Registro Descartado: estava Na Lixeira e sumiu (o usuário a esvaziou).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDescartado {
    pub caminho: String,
    pub origem: Option<String>,
    /// Hash Completo em hexadecimal, se havia.
    pub hash: Option<String>,
    pub bytes: u64,
    pub movido_em: Option<i64>,
    pub descartado_em: i64,
}

// ---------------------------------------------------------------------- caminhos (bytes brutos)

fn sem_barra_final(b: &[u8]) -> &[u8] {
    if b.len() > 1 && b.ends_with(b"/") {
        &b[..b.len() - 1]
    } else {
        b
    }
}

/// `caminho` relativo ao ponto de montagem `montagem` (sem barra inicial); `None` se está fora
/// ou é o próprio ponto de montagem.
#[must_use]
pub fn relativo_a(montagem: &[u8], caminho: &[u8]) -> Option<Vec<u8>> {
    let m = sem_barra_final(montagem);
    if m == b"/" {
        return caminho
            .strip_prefix(b"/")
            .filter(|r| !r.is_empty())
            .map(<[u8]>::to_vec);
    }
    let resto = caminho.strip_prefix(m)?;
    let resto = resto.strip_prefix(b"/")?;
    (!resto.is_empty()).then(|| resto.to_vec())
}

/// `<montagem>/DAG-Lixeira`.
#[must_use]
pub fn raiz_da_lixeira(montagem: &[u8]) -> Vec<u8> {
    let m = sem_barra_final(montagem);
    let mut v = if m == b"/" { Vec::new() } else { m.to_vec() };
    v.push(b'/');
    v.extend_from_slice(NOME_LIXEIRA.as_bytes());
    v
}

/// Destino previsto de `caminho` na Lixeira do Volume montado em `montagem`:
/// `<montagem>/DAG-Lixeira/<caminho relativo>`. `None` se o caminho não está sob `montagem` ou
/// já está na Lixeira.
#[must_use]
pub fn destino_na_lixeira(montagem: &[u8], caminho: &[u8]) -> Option<Vec<u8>> {
    let rel = relativo_a(montagem, caminho)?;
    if esta_na_lixeira(montagem, caminho) {
        return None;
    }
    let mut v = raiz_da_lixeira(montagem);
    v.push(b'/');
    v.extend_from_slice(&rel);
    Some(v)
}

/// `caminho` está dentro da `DAG-Lixeira` de `montagem` (a pasta em si não conta).
#[must_use]
pub fn esta_na_lixeira(montagem: &[u8], caminho: &[u8]) -> bool {
    let raiz = raiz_da_lixeira(montagem);
    caminho
        .strip_prefix(raiz.as_slice())
        .is_some_and(|r| r.starts_with(b"/") && r.len() > 1)
}

/// Caminho de origem a partir do caminho dentro da Lixeira (remove `<montagem>/DAG-Lixeira/`).
#[must_use]
pub fn origem_a_partir_da_lixeira(montagem: &[u8], caminho: &[u8]) -> Option<Vec<u8>> {
    let raiz = raiz_da_lixeira(montagem);
    let rel = caminho.strip_prefix(raiz.as_slice())?.strip_prefix(b"/")?;
    if rel.is_empty() {
        return None;
    }
    let m = sem_barra_final(montagem);
    let mut v = if m == b"/" { Vec::new() } else { m.to_vec() };
    v.push(b'/');
    v.extend_from_slice(rel);
    Some(v)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn caminhos_da_lixeira() {
        assert_eq!(
            destino_na_lixeira(b"/mnt/d", b"/mnt/d/a/b.txt").unwrap(),
            b"/mnt/d/DAG-Lixeira/a/b.txt"
        );
        assert_eq!(
            destino_na_lixeira(b"/", b"/home/x").unwrap(),
            b"/DAG-Lixeira/home/x"
        );
        assert!(destino_na_lixeira(b"/mnt/d", b"/mnt/outro/x").is_none());
        assert!(destino_na_lixeira(b"/mnt/d", b"/mnt/d/DAG-Lixeira/a").is_none());
        assert!(esta_na_lixeira(b"/mnt/d", b"/mnt/d/DAG-Lixeira/a"));
        assert!(!esta_na_lixeira(b"/mnt/d", b"/mnt/d/DAG-Lixeira"));
        assert!(!esta_na_lixeira(b"/mnt/d", b"/mnt/d/DAG-LixeiraX/a"));
        assert_eq!(
            origem_a_partir_da_lixeira(b"/mnt/d", b"/mnt/d/DAG-Lixeira/a/b").unwrap(),
            b"/mnt/d/a/b"
        );
        assert_eq!(
            origem_a_partir_da_lixeira(b"/", b"/DAG-Lixeira/a").unwrap(),
            b"/a"
        );
    }
}
