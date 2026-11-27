//! Limites contra bombas de descompressão e o medidor que os aplica (CA-B14).

use std::cell::Cell;
use std::time::{Duration, Instant};

/// Limites de leitura de um Contêiner (e dos aninhados nele). Tudo em memória: nada é gravado.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitesConteiner {
    /// Níveis de Contêiner aninhado abertos além do raiz (padrão 3).
    pub profundidade_max: u32,
    /// Soma dos tamanhos **declarados** dos membros (padrão 16 GiB). Checada à medida que os
    /// membros aparecem, antes de descompactar qualquer um.
    pub bytes_total_max: u64,
    /// Bytes **efetivamente descompactados** pela extração (padrão 2 GiB).
    pub bytes_lidos_max: u64,
    /// Maior membro carregado inteiro na memória (padrão 512 MiB).
    pub bytes_membro_max: u64,
    /// Razão máxima descompactado/compactado (padrão 150), cobrada acima de `piso_razao`.
    pub razao_expansao_max: f64,
    /// Abaixo deste total a razão não é cobrada (arquivos pequenos expandem muito à toa).
    pub piso_razao: u64,
    /// Quantidade máxima de membros percorridos (padrão 50 000).
    pub membros_max: u64,
    /// Tempo total de uma extração (padrão 120 s).
    pub tempo_max: Duration,
}

impl Default for LimitesConteiner {
    fn default() -> Self {
        Self {
            profundidade_max: 3,
            bytes_total_max: 16 << 30,
            bytes_lidos_max: 2 << 30,
            bytes_membro_max: 512 << 20,
            razao_expansao_max: 150.0,
            piso_razao: 64 << 20,
            membros_max: 50_000,
            tempo_max: Duration::from_secs(120),
        }
    }
}

/// Contabilidade de uma extração (compartilhada com os aninhados). Single-thread.
#[derive(Debug)]
pub(crate) struct Medidor {
    limites: LimitesConteiner,
    inicio: Instant,
    entrada: u64,
    lidos: Cell<u64>,
    declarados: Cell<u64>,
    membros: Cell<u64>,
}

impl Medidor {
    pub(crate) fn novo(limites: &LimitesConteiner, tamanho_entrada: u64) -> Self {
        Self {
            limites: limites.clone(),
            inicio: Instant::now(),
            entrada: tamanho_entrada.max(1),
            lidos: Cell::new(0),
            declarados: Cell::new(0),
            membros: Cell::new(0),
        }
    }

    /// Medidor para uma passada de reconhecimento sobre a mesma entrada: mesmo relógio, contagens
    /// zeradas. A classificação e a extração percorrem os mesmos Membros; sem isto cada Membro
    /// contaria duas vezes (um zip de 30 mil Membros estourava o limite de 50 mil).
    pub(crate) fn para_reconhecer(&self) -> Self {
        Self {
            limites: self.limites.clone(),
            inicio: self.inicio,
            entrada: self.entrada,
            lidos: Cell::new(0),
            declarados: Cell::new(0),
            membros: Cell::new(0),
        }
    }

    pub(crate) fn limites(&self) -> &LimitesConteiner {
        &self.limites
    }

    pub(crate) fn bytes_lidos(&self) -> u64 {
        self.lidos.get()
    }

    pub(crate) fn membros_vistos(&self) -> u64 {
        self.membros.get()
    }

    pub(crate) fn tempo(&self) -> Result<(), String> {
        if self.inicio.elapsed() > self.limites.tempo_max {
            return Err(format!("tempo (> {:?})", self.limites.tempo_max));
        }
        Ok(())
    }

    fn razao(&self, total: u64) -> Result<(), String> {
        let teto = (self.limites.razao_expansao_max * self.entrada as f64) as u64;
        if total > self.limites.piso_razao && total > teto {
            return Err(format!(
                "razão de expansão ({}x > {}x)",
                total / self.entrada,
                self.limites.razao_expansao_max
            ));
        }
        Ok(())
    }

    /// Um membro a mais.
    pub(crate) fn membro(&self) -> Result<(), String> {
        self.tempo()?;
        let n = self.membros.get() + 1;
        self.membros.set(n);
        if n > self.limites.membros_max {
            return Err(format!(
                "quantidade de membros (> {})",
                self.limites.membros_max
            ));
        }
        Ok(())
    }

    /// Tamanho declarado de um membro, antes de descompactá-lo.
    pub(crate) fn declarar(&self, tamanho: u64) -> Result<(), String> {
        let total = self.declarados.get().saturating_add(tamanho);
        self.declarados.set(total);
        if total > self.limites.bytes_total_max {
            return Err(format!(
                "bytes descompactados declarados (> {})",
                self.limites.bytes_total_max
            ));
        }
        self.razao(total)
    }

    /// `n` bytes acabaram de ser descompactados.
    pub(crate) fn ler(&self, n: u64) -> Result<(), String> {
        self.tempo()?;
        let total = self.lidos.get().saturating_add(n);
        self.lidos.set(total);
        if total > self.limites.bytes_lidos_max {
            return Err(format!(
                "bytes descompactados (> {})",
                self.limites.bytes_lidos_max
            ));
        }
        self.razao(total)
    }
}
