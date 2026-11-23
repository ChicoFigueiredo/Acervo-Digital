//! Montagem do resultado final: agrupa mounts por `major:minor` em Volumes.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use dag_core::{DispositivoDescoberto, TipoDispositivo, VolumeDescoberto};

use crate::dispositivo::{caminho_sysfs, majmin_por_nome, resolver_dispositivo};
use crate::erro::Erro;
use crate::fonte::{FonteSistema, FonteSistemaReal};
use crate::mountinfo::{EntradaMontagem, parse_mountinfo};

const MOUNTINFO: &str = "/proc/self/mountinfo";

/// Filesystems virtuais do kernel ou em memória: não são varridos por padrão.
/// `overlay` e `squashfs` guardam arquivos reais e não entram aqui.
const PSEUDO: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "tmpfs",
    "ramfs",
    "rootfs",
    "cgroup",
    "cgroup2",
    "securityfs",
    "selinuxfs",
    "debugfs",
    "tracefs",
    "pstore",
    "bpf",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "autofs",
    "binfmt_misc",
    "nsfs",
    "efivarfs",
    "rpc_pipefs",
    "fuse.gvfsd-fuse",
    "fuse.portal",
];

/// Filesystems de rede: o "dispositivo" é o servidor/export.
const REDE: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "sshfs",
    "fuse.sshfs",
    "ceph",
    "9p",
    "afs",
    "fuse.glusterfs",
    // Montagem de nuvem (rclone mount): o "dispositivo" é o remoto.
    "fuse.rclone",
    "glusterfs",
];

fn eh_pseudo(fs_type: &str) -> bool {
    PSEUDO.contains(&fs_type)
}

fn eh_rede(fs_type: &str) -> bool {
    REDE.contains(&fs_type)
}

/// Decodifica escapes `\xNN` de udev nos nomes de `/dev/disk/by-label`.
fn decodificar_udev(nome: &[u8]) -> String {
    let mut saida = Vec::with_capacity(nome.len());
    let mut i = 0;
    while i < nome.len() {
        if nome[i] == b'\\'
            && nome.get(i + 1) == Some(&b'x')
            && let Some(hex) = nome.get(i + 2..i + 4)
            && let Some(b) = std::str::from_utf8(hex)
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            saida.push(b);
            i += 4;
            continue;
        }
        saida.push(nome[i]);
        i += 1;
    }
    String::from_utf8_lossy(&saida).into_owned()
}

/// Mapa `(major, minor)` -> nome, a partir dos links de `/dev/disk/by-*`.
fn mapa_ids(fonte: &dyn FonteSistema, diretorio: &str) -> HashMap<(u32, u32), String> {
    let mut mapa = HashMap::new();
    let Ok(nomes) = fonte.listar_dir(Path::new(diretorio)) else {
        return mapa;
    };
    for nome in nomes {
        let Ok(alvo) = fonte.ler_link(&Path::new(diretorio).join(&nome)) else {
            continue;
        };
        let Some(dispositivo) = alvo.file_name() else {
            continue;
        };
        if let Some(chave) = majmin_por_nome(fonte, dispositivo) {
            mapa.insert(chave, decodificar_udev(nome.as_bytes()));
        }
    }
    mapa
}

/// Dispositivo de bloco do grupo: o próprio `major:minor` quando existe em
/// `/sys/dev/block`; senão (ex.: subvolume btrfs, com st_dev anônimo) o `source`.
fn bloco_do_grupo(
    fonte: &dyn FonteSistema,
    major: u32,
    minor: u32,
    origem: &[u8],
) -> Option<(u32, u32)> {
    if caminho_sysfs(fonte, major, minor).is_some() {
        return Some((major, minor));
    }
    if !origem.starts_with(b"/dev/") {
        return None;
    }
    let caminho = Path::new(OsStr::from_bytes(origem));
    let nome = match fonte.ler_link(caminho) {
        Ok(alvo) => alvo.file_name()?.to_owned(),
        Err(_) => caminho.file_name()?.to_owned(),
    };
    majmin_por_nome(fonte, &nome)
}

struct Grupo {
    major: u32,
    minor: u32,
    entradas: Vec<EntradaMontagem>,
}

/// Descobre os volumes montados no sistema real.
pub fn descobrir_volumes() -> Result<Vec<VolumeDescoberto>, Erro> {
    descobrir_volumes_com(&FonteSistemaReal)
}

/// Como [`descobrir_volumes`], sobre qualquer [`FonteSistema`].
///
/// - Mounts com o mesmo `major:minor` (bind mounts, montagens múltiplas) viram um
///   só Volume com vários `pontos_de_montagem`.
/// - Volumes pseudo não sofrem `statvfs` (evita acionar automount) nem recebem
///   dispositivo.
/// - `statvfs` em NFS/CIFS sem resposta pode bloquear no kernel; a descoberta não
///   impõe prazo (limite do `std` sem threads auxiliares).
/// - Subvolumes btrfs compartilham UUID mas têm `st_dev` distintos: aparecem como
///   Volumes separados com o mesmo `platform_id`.
pub fn descobrir_volumes_com(fonte: &dyn FonteSistema) -> Result<Vec<VolumeDescoberto>, Erro> {
    let bruto = fonte
        .ler_arquivo(Path::new(MOUNTINFO))
        .map_err(|e| Erro::Io {
            caminho: MOUNTINFO.into(),
            fonte: e,
        })?;

    let mut indice: HashMap<(u32, u32), usize> = HashMap::new();
    let mut grupos: Vec<Grupo> = Vec::new();
    for e in parse_mountinfo(&bruto) {
        let chave = (e.major, e.minor);
        let pos = *indice.entry(chave).or_insert_with(|| {
            grupos.push(Grupo {
                major: e.major,
                minor: e.minor,
                entradas: Vec::new(),
            });
            grupos.len() - 1
        });
        grupos[pos].entradas.push(e);
    }

    let uuids = mapa_ids(fonte, "/dev/disk/by-uuid");
    let rotulos = mapa_ids(fonte, "/dev/disk/by-label");

    let mut volumes = Vec::with_capacity(grupos.len());
    for g in grupos {
        let primeira = &g.entradas[0];
        let fs_type = primeira.fs_type.clone();
        let pseudo = eh_pseudo(&fs_type);
        let bloco = bloco_do_grupo(fonte, g.major, g.minor, &primeira.fonte);
        let uuid = bloco.and_then(|b| uuids.get(&b));
        let platform_id = uuid
            .cloned()
            .unwrap_or_else(|| format!("devnum:{}:{}", g.major, g.minor));
        let rotulo = bloco.and_then(|b| rotulos.get(&b).cloned());
        let estat = if pseudo {
            None
        } else {
            let ponto = Path::new(OsStr::from_bytes(&primeira.ponto_de_montagem));
            fonte.statvfs(ponto).ok()
        };
        let dispositivo = if pseudo {
            None
        } else if eh_rede(&fs_type) {
            Some(DispositivoDescoberto {
                identificador: String::from_utf8_lossy(&primeira.fonte).into_owned(),
                modelo: None,
                tipo: TipoDispositivo::Rede,
                removivel: false,
            })
        } else {
            bloco.and_then(|(a, b)| resolver_dispositivo(fonte, a, b))
        };
        volumes.push(VolumeDescoberto {
            platform_id,
            st_dev: rustix::fs::makedev(g.major, g.minor),
            rotulo,
            fs_type,
            capacidade_bytes: estat.map(|s| s.capacidade_bytes),
            livre_bytes: estat.map(|s| s.livre_bytes),
            pontos_de_montagem: g
                .entradas
                .into_iter()
                .map(|e| e.ponto_de_montagem)
                .collect(),
            pseudo,
            dispositivo,
        });
    }
    Ok(volumes)
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::fonte::falsa::FonteFalsa;

    const SATA: &str = "devices/pci0000:00/0000:00:1f.2/ata1/host0/target0:0:0/0:0:0:0/block/sda";
    const NVME: &str = "devices/pci0000:00/0000:00:1d.0/0000:3d:00.0/nvme/nvme0/nvme0n1";
    const USB: &str =
        "devices/pci0000:00/0000:00:14.0/usb2/2-1/2-1:1.0/host6/target6:0:0/6:0:0:0/block/sdb";

    /// Registra o dispositivo de bloco `nome` (`major:minor`) em `/sys`.
    fn bloco(f: &mut FonteFalsa, maj: u32, min: u32, nome: &str, sysfs: &str) {
        let alvo = format!("../../{sysfs}");
        f.link(&format!("/sys/dev/block/{maj}:{min}"), &alvo);
        f.link(&format!("/sys/class/block/{nome}"), &alvo);
        f.arquivo(
            &format!("/sys/class/block/{nome}/dev"),
            format!("{maj}:{min}\n"),
        );
    }

    fn disco(
        f: &mut FonteFalsa,
        sysfs: &str,
        rot: &str,
        rem: &str,
        modelo: &str,
        wwid: Option<&str>,
    ) {
        f.arquivo(&format!("/sys/{sysfs}/queue/rotational"), rot);
        f.arquivo(&format!("/sys/{sysfs}/removable"), rem);
        f.arquivo(
            &format!("/sys/{sysfs}/device/model"),
            format!("{modelo}   \n"),
        );
        if let Some(w) = wwid {
            f.arquivo(&format!("/sys/{sysfs}/wwid"), format!("{w}\n"));
        }
    }

    fn particao(f: &mut FonteFalsa, maj: u32, min: u32, nome: &str, sysfs_disco: &str, n: u32) {
        let caminho = format!("{sysfs_disco}/{nome}");
        bloco(f, maj, min, nome, &caminho);
        f.arquivo(&format!("/sys/{caminho}/partition"), n.to_string());
    }

    fn por_uuid(f: &mut FonteFalsa, uuid: &str, nome: &str) {
        f.link(
            &format!("/dev/disk/by-uuid/{uuid}"),
            &format!("../../{nome}"),
        );
    }

    fn proc_e_sys(mi: &mut String) {
        mi.push_str("22 27 0:21 / /proc rw,relatime - proc proc rw\n");
        mi.push_str("23 27 0:22 / /sys rw,relatime - sysfs sysfs rw\n");
    }

    fn descobrir(f: &FonteFalsa) -> Vec<VolumeDescoberto> {
        descobrir_volumes_com(f).unwrap()
    }

    #[test]
    fn ext4_em_sda1_com_bind_mount() {
        let mut f = FonteFalsa::default();
        disco(
            &mut f,
            SATA,
            "1",
            "0",
            "ST1000DM010",
            Some("naa.5000c500a1b2c3d4"),
        );
        particao(&mut f, 8, 1, "sda1", SATA, 1);
        por_uuid(&mut f, "1111-aaaa", "sda1");
        f.link("/dev/disk/by-label/Dados\\x20Fotos", "../../sda1");
        f.stat("/", 1000, 400);
        let mut mi = String::from("27 1 8:1 / / rw,relatime - ext4 /dev/sda1 rw\n");
        mi.push_str("50 27 8:1 /home/u/fotos /srv/fotos rw - ext4 /dev/sda1 rw\n");
        proc_e_sys(&mut mi);
        f.arquivo("/proc/self/mountinfo", mi);

        let v = descobrir(&f);
        assert_eq!(v.len(), 3);
        let raiz = &v[0];
        assert_eq!(raiz.platform_id, "1111-aaaa");
        assert_eq!(raiz.rotulo.as_deref(), Some("Dados Fotos"));
        assert_eq!(raiz.st_dev, rustix::fs::makedev(8, 1));
        assert_eq!(raiz.fs_type, "ext4");
        assert_eq!(
            raiz.pontos_de_montagem,
            vec![b"/".to_vec(), b"/srv/fotos".to_vec()]
        );
        assert_eq!(
            (raiz.capacidade_bytes, raiz.livre_bytes),
            (Some(1000), Some(400))
        );
        assert!(!raiz.pseudo);
        let d = raiz.dispositivo.as_ref().unwrap();
        assert_eq!(d.identificador, "naa.5000c500a1b2c3d4");
        assert_eq!(d.modelo.as_deref(), Some("ST1000DM010"));
        assert_eq!(d.tipo, TipoDispositivo::Hdd);
        assert!(!d.removivel);
    }

    #[test]
    fn particao_nvme() {
        let mut f = FonteFalsa::default();
        disco(
            &mut f,
            NVME,
            "0",
            "0",
            "Samsung SSD 980",
            Some("eui.0025388b01234567"),
        );
        particao(&mut f, 259, 1, "nvme0n1p2", NVME, 2);
        por_uuid(&mut f, "uuid-nvme", "nvme0n1p2");
        f.arquivo(
            "/proc/self/mountinfo",
            "27 1 259:1 / / rw - btrfs /dev/nvme0n1p2 rw\n",
        );
        let v = descobrir(&f);
        let d = v[0].dispositivo.as_ref().unwrap();
        assert_eq!(d.tipo, TipoDispositivo::Nvme);
        assert_eq!(d.identificador, "eui.0025388b01234567");
        assert_eq!(v[0].platform_id, "uuid-nvme");
        // statvfs indisponível => None, nunca zero inventado.
        assert_eq!(v[0].capacidade_bytes, None);
        assert_eq!(v[0].livre_bytes, None);
    }

    #[test]
    fn luks_sobre_nvme() {
        let mut f = FonteFalsa::default();
        disco(&mut f, NVME, "0", "0", "WD Black", Some("eui.abc"));
        particao(&mut f, 259, 3, "nvme0n1p3", NVME, 3);
        let dm = "devices/virtual/block/dm-0";
        bloco(&mut f, 254, 0, "dm-0", dm);
        f.link(&format!("/sys/{dm}/slaves/nvme0n1p3"), "../../../../pci/x");
        por_uuid(&mut f, "uuid-interno", "dm-0");
        f.link("/dev/mapper/cripto", "../dm-0");
        f.arquivo(
            "/proc/self/mountinfo",
            "27 1 254:0 / / rw - ext4 /dev/mapper/cripto rw\n",
        );
        let v = descobrir(&f);
        assert_eq!(v[0].platform_id, "uuid-interno");
        let d = v[0].dispositivo.as_ref().unwrap();
        assert_eq!(d.identificador, "eui.abc");
        assert_eq!(d.tipo, TipoDispositivo::Nvme);
    }

    #[test]
    fn dm_com_discos_diferentes_nao_tem_dispositivo() {
        let mut f = FonteFalsa::default();
        disco(&mut f, SATA, "1", "0", "A", Some("wwn-a"));
        particao(&mut f, 8, 1, "sda1", SATA, 1);
        disco(&mut f, NVME, "0", "0", "B", Some("wwn-b"));
        particao(&mut f, 259, 1, "nvme0n1p1", NVME, 1);
        let dm = "devices/virtual/block/dm-1";
        bloco(&mut f, 254, 1, "dm-1", dm);
        f.link(&format!("/sys/{dm}/slaves/sda1"), "x");
        f.link(&format!("/sys/{dm}/slaves/nvme0n1p1"), "x");
        f.arquivo(
            "/proc/self/mountinfo",
            "27 1 254:1 / /dados rw - xfs /dev/mapper/vg-lv rw\n",
        );
        let v = descobrir(&f);
        assert_eq!(v.len(), 1);
        assert!(v[0].dispositivo.is_none());
        assert_eq!(v[0].platform_id, "devnum:254:1");
    }

    #[test]
    fn dm_com_duas_particoes_do_mesmo_disco_resolve_o_disco() {
        let mut f = FonteFalsa::default();
        disco(&mut f, SATA, "0", "0", "A", Some("wwn-a"));
        particao(&mut f, 8, 1, "sda1", SATA, 1);
        particao(&mut f, 8, 2, "sda2", SATA, 2);
        let dm = "devices/virtual/block/dm-1";
        bloco(&mut f, 254, 1, "dm-1", dm);
        f.link(&format!("/sys/{dm}/slaves/sda1"), "x");
        f.link(&format!("/sys/{dm}/slaves/sda2"), "x");
        f.arquivo(
            "/proc/self/mountinfo",
            "27 1 254:1 / / rw - xfs /dev/mapper/vg-lv rw\n",
        );
        let d = descobrir(&f)[0].dispositivo.clone().unwrap();
        assert_eq!(d.identificador, "wwn-a");
        assert_eq!(d.tipo, TipoDispositivo::Ssd);
    }

    #[test]
    fn usb_removivel_sem_wwid_usa_serial() {
        let mut f = FonteFalsa::default();
        disco(&mut f, USB, "1", "1", "Flash Disk", None);
        f.arquivo(&format!("/sys/{USB}/device/serial"), "SER123\n");
        particao(&mut f, 8, 17, "sdb1", USB, 1);
        por_uuid(&mut f, "ABCD-1234", "sdb1");
        f.stat("/media/u/PEN", 8_000, 7_000);
        f.arquivo(
            "/proc/self/mountinfo",
            "40 27 8:17 / /media/u/PEN rw - vfat /dev/sdb1 rw\n",
        );
        let v = descobrir(&f);
        let d = v[0].dispositivo.as_ref().unwrap();
        assert_eq!(d.tipo, TipoDispositivo::Usb);
        assert!(d.removivel);
        assert_eq!(d.identificador, "SER123");
        assert_eq!(v[0].platform_id, "ABCD-1234");
    }

    #[test]
    fn sem_identificadores_cai_para_kname() {
        let mut f = FonteFalsa::default();
        disco(&mut f, SATA, "1", "0", "A", None);
        particao(&mut f, 8, 1, "sda1", SATA, 1);
        f.arquivo(
            "/proc/self/mountinfo",
            "27 1 8:1 / / rw - ext4 /dev/sda1 rw\n",
        );
        let v = descobrir(&f);
        assert_eq!(
            v[0].dispositivo.as_ref().unwrap().identificador,
            "kname:sda"
        );
        assert_eq!(v[0].platform_id, "devnum:8:1");
    }

    #[test]
    fn nfs_vira_dispositivo_de_rede() {
        let mut f = FonteFalsa::default();
        f.stat("/mnt/nas", 5000, 2500);
        f.arquivo(
            "/proc/self/mountinfo",
            "60 27 0:50 / /mnt/nas rw - nfs4 nas.local:/export/fotos rw,vers=4.2\n",
        );
        let v = descobrir(&f);
        let d = v[0].dispositivo.as_ref().unwrap();
        assert_eq!(d.tipo, TipoDispositivo::Rede);
        assert_eq!(d.identificador, "nas.local:/export/fotos");
        assert_eq!(v[0].capacidade_bytes, Some(5000));
        assert!(!v[0].pseudo);
    }

    #[test]
    fn rclone_e_nuvem_tratado_como_rede() {
        let mut f = FonteFalsa::default();
        f.stat("/mnt/nuvem", 5000, 2500);
        f.arquivo(
            "/proc/self/mountinfo",
            "61 27 0:51 / /mnt/nuvem rw - fuse.rclone gdrive: rw,user_id=1000\n",
        );
        let v = descobrir(&f);
        let d = v[0].dispositivo.as_ref().unwrap();
        assert_eq!(d.tipo, TipoDispositivo::Rede);
        assert!(!v[0].pseudo);
    }

    #[test]
    fn caminho_com_espaco_octal() {
        let mut f = FonteFalsa::default();
        f.stat("/media/u/Meu Disco", 10, 5);
        f.arquivo(
            "/proc/self/mountinfo",
            "40 27 0:60 / /media/u/Meu\\040Disco rw - exfat /dev/sdz1 rw\n",
        );
        let v = descobrir(&f);
        assert_eq!(
            v[0].pontos_de_montagem,
            vec![b"/media/u/Meu Disco".to_vec()]
        );
        assert_eq!(v[0].capacidade_bytes, Some(10));
    }

    #[test]
    fn pseudo_filesystems() {
        let mut f = FonteFalsa::default();
        let mut mi = String::new();
        proc_e_sys(&mut mi);
        mi.push_str("25 24 0:23 / /dev/shm rw - tmpfs tmpfs rw\n");
        mi.push_str("37 36 0:29 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n");
        mi.push_str("38 27 0:70 / /var/lib/docker/overlay2/x/merged rw - overlay overlay rw\n");
        mi.push_str("39 27 7:0 / /snap/core rw - squashfs /dev/loop0 ro\n");
        f.arquivo("/proc/self/mountinfo", mi);
        // statvfs configurado: pseudo nunca deve consultá-lo.
        f.stat("/proc", 1, 1);
        let v = descobrir(&f);
        let pseudo: Vec<bool> = v.iter().map(|x| x.pseudo).collect();
        assert_eq!(pseudo, vec![true, true, true, true, false, false]);
        assert!(v[0].capacidade_bytes.is_none());
        assert!(v[0].dispositivo.is_none());
        assert_eq!(v[0].platform_id, "devnum:0:21");
    }

    #[test]
    fn mountinfo_ilegivel_e_erro() {
        assert!(matches!(
            descobrir_volumes_com(&FonteFalsa::default()),
            Err(Erro::Io { .. })
        ));
    }

    #[test]
    fn subvolume_btrfs_usa_source_para_achar_uuid_e_disco() {
        let mut f = FonteFalsa::default();
        disco(&mut f, NVME, "0", "0", "X", Some("eui.z"));
        particao(&mut f, 259, 2, "nvme0n1p2", NVME, 2);
        por_uuid(&mut f, "uuid-btrfs", "nvme0n1p2");
        f.arquivo(
            "/proc/self/mountinfo",
            "27 1 0:44 /@home /home rw - btrfs /dev/nvme0n1p2 rw,subvol=/@home\n",
        );
        let v = descobrir(&f);
        assert_eq!(v[0].platform_id, "uuid-btrfs");
        assert_eq!(v[0].dispositivo.as_ref().unwrap().identificador, "eui.z");
    }
}
