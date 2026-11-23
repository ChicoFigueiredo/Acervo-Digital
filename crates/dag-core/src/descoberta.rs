//! Contrato entre a descoberta de plataforma (`dag-platform-*`) e o scanner.
//!
//! A plataforma descreve o que existe agora; o scanner e o Catálogo decidem o que
//! persistir. Valores indisponíveis ficam `None`, nunca zero inventado.

use serde::{Deserialize, Serialize};

/// Tipo de mídia do Dispositivo Físico, quando o sistema informa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoDispositivo {
    Desconhecido,
    Hdd,
    Ssd,
    Nvme,
    Usb,
    Rede,
    Virtual,
    Outro,
}

/// Disco ou mídia física que hospeda um ou mais Volumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispositivoDescoberto {
    /// Identificador estável: WWN, serial ou, em último caso, nome do kernel prefixado com `kname:`.
    pub identificador: String,
    pub modelo: Option<String>,
    pub tipo: TipoDispositivo,
    pub removivel: bool,
}

/// Sistema de arquivos montado, visto agora.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeDescoberto {
    /// Identificador estável do filesystem (UUID); sem UUID, `devnum:<maj>:<min>` (instável entre boots).
    pub platform_id: String,
    /// `st_dev` do filesystem, para associar entradas varridas ao Volume.
    pub st_dev: u64,
    pub rotulo: Option<String>,
    pub fs_type: String,
    pub capacidade_bytes: Option<u64>,
    pub livre_bytes: Option<u64>,
    /// Todos os pontos de montagem (bind mounts incluídos), em bytes brutos.
    pub pontos_de_montagem: Vec<Vec<u8>>,
    /// Pseudo-filesystem (proc, sysfs, devtmpfs, cgroup...) que não é varrido por padrão.
    pub pseudo: bool,
    /// Dispositivo físico que hospeda o volume, quando determinável.
    pub dispositivo: Option<DispositivoDescoberto>,
}
