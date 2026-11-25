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

/// Natureza do Volume, deduzida de evidências do sistema (sysfs, tipo de filesystem).
///
/// Serve para separar o que é armazenamento real do ruído do sistema (snaps, AppImages,
/// zram, overlay...). A visão padrão mostra `Fisico`, `Removivel` e `Rede`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClasseVolume {
    /// Sustentado por um disco de verdade (SATA, NVMe, MMC, virtio, md/dm sobre discos reais).
    #[default]
    Fisico,
    /// Disco removível (`removable=1`) ou conectado por USB.
    Removivel,
    /// Filesystem de rede ou nuvem (NFS, CIFS, sshfs, rclone...).
    Rede,
    /// Imagem montada: dispositivo loop (snaps, AppImages, ISOs) ou filesystem somente
    /// leitura de imagem (squashfs, erofs, iso9660) sem disco real por baixo.
    Imagem,
    /// Sem disco real: zram, ram, device-mapper sem disco físico, overlay, fuse local...
    Virtual,
}

impl ClasseVolume {
    /// `true` para as classes mostradas por padrão (físico, removível e rede).
    pub fn eh_real(self) -> bool {
        matches!(self, Self::Fisico | Self::Removivel | Self::Rede)
    }

    /// Nome curto em português, para tabelas e listagens.
    pub fn rotulo(self) -> &'static str {
        match self {
            Self::Fisico => "físico",
            Self::Removivel => "removível",
            Self::Rede => "rede",
            Self::Imagem => "imagem",
            Self::Virtual => "virtual",
        }
    }
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
    /// Natureza do volume (físico, removível, rede, imagem, virtual).
    #[serde(default)]
    pub classe: ClasseVolume,
    /// Partição de sistema de um disco real (`/boot`, `/boot/efi`, `/efi`...): não é onde ficam
    /// os dados do usuário, mas é real e aparece na visão padrão com o selo "sistema".
    #[serde(default)]
    pub sistema: bool,
}
