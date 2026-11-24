//! Lógica pura da seleção de volumes (sem egui): filtro de discos reais, classe de cada linha,
//! raízes escolhidas a partir da seleção e avisos. Testável sem display.

use std::collections::{BTreeSet, HashMap};

use dag_core::{ClasseVolume, VolumeInfo};
use dag_store::consulta_gui::LinhaVolume;

/// Classe e selo "sistema" dos volumes, aprendidos da última resposta de `ListarVolumes`
/// (a classificação vem do sistema, não fica gravada no Catálogo).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Classificacao {
    por_id: HashMap<i64, (ClasseVolume, bool)>,
    /// Volumes de imagem/virtuais detectados que não estão no Catálogo (sem id).
    ocultos_fora_do_catalogo: Vec<VolumeInfo>,
    /// Classe por ponto de montagem, vinda da descoberta direta do sistema (sem passar pelo
    /// worker): vale enquanto a resposta de `ListarVolumes` não chega.
    por_montagem: HashMap<String, (ClasseVolume, bool)>,
    /// `true` depois da primeira resposta; antes, vale a classe presumida pelo filesystem.
    pub recebida: bool,
}

impl Classificacao {
    /// A partir de `Resposta::Volumes`.
    #[must_use]
    pub fn de_resposta(volumes: &[VolumeInfo], ocultos: &[VolumeInfo]) -> Self {
        let mut c = Self {
            recebida: true,
            ..Self::default()
        };
        for v in volumes.iter().chain(ocultos) {
            if v.id.0 != 0 {
                c.por_id.insert(v.id.0, (v.classe, v.sistema));
            }
        }
        c.ocultos_fora_do_catalogo = ocultos.iter().filter(|v| v.id.0 == 0).cloned().collect();
        c
    }

    /// A partir da descoberta direta de volumes do sistema (somente leitura, sem Catálogo).
    #[must_use]
    pub fn de_descobertos(volumes: &[dag_core::VolumeDescoberto]) -> Self {
        let mut c = Self::default();
        for v in volumes {
            for p in &v.pontos_de_montagem {
                let ponto = String::from_utf8_lossy(p).into_owned();
                c.por_montagem.insert(ponto, (v.classe, v.sistema));
            }
        }
        c
    }

    /// Junta uma descoberta direta à classificação (o que já veio do worker prevalece).
    pub fn aprender_descobertos(&mut self, volumes: &[dag_core::VolumeDescoberto]) {
        let nova = Self::de_descobertos(volumes);
        for (k, v) in nova.por_montagem {
            self.por_montagem.entry(k).or_insert(v);
        }
    }

    fn de(&self, v: &LinhaVolume) -> (ClasseVolume, bool) {
        if let Some(c) = self.por_id.get(&v.id) {
            return *c;
        }
        if let Some(c) = v.montagem.as_ref().and_then(|m| self.por_montagem.get(m)) {
            return *c;
        }
        (
            classe_presumida(v.fs_type.as_deref(), v.montagem.as_deref()),
            false,
        )
    }
}

/// Sem classificação do sistema: presume pelo filesystem e pelo ponto de montagem.
///
/// * rede: `nfs*`, `cifs`, `smb*`, `sshfs`, `fuse.rclone`, `ceph`, `9p`...;
/// * imagem: `squashfs`, `erofs`, `iso9660`, `udf`, AppImage (`/tmp/.mount_*`);
/// * virtual: `overlay`, `tmpfs`, `fuse.*` locais, e tudo sob `/var/lib/docker`,
///   `/var/lib/containers` ou `/run`;
/// * o resto conta como `Fisico` (melhor mostrar a mais do que esconder um disco real).
#[must_use]
pub fn classe_presumida(fs_type: Option<&str>, montagem: Option<&str>) -> ClasseVolume {
    let fs = fs_type.unwrap_or("").to_ascii_lowercase();
    let ponto = montagem.unwrap_or("");
    let sob = |p: &str| ponto == p || ponto.starts_with(&format!("{p}/"));
    if matches!(
        fs.as_str(),
        "nfs"
            | "nfs4"
            | "cifs"
            | "smb3"
            | "smbfs"
            | "sshfs"
            | "ceph"
            | "9p"
            | "afs"
            | "glusterfs"
            | "fuse.sshfs"
            | "fuse.rclone"
            | "fuse.glusterfs"
    ) {
        return ClasseVolume::Rede;
    }
    if matches!(fs.as_str(), "squashfs" | "erofs" | "iso9660" | "udf")
        || fs.contains("appimage")
        || ponto.starts_with("/tmp/.mount_")
    {
        return ClasseVolume::Imagem;
    }
    if matches!(
        fs.as_str(),
        "overlay" | "tmpfs" | "devtmpfs" | "ramfs" | "proc" | "sysfs" | "cgroup2" | "efivarfs"
    ) || fs.starts_with("fuse.")
        || fs == "fuse"
        || sob("/var/lib/docker")
        || sob("/var/lib/containers")
        || (sob("/run") && !sob("/run/media") && !sob("/run/mount"))
    {
        return ClasseVolume::Virtual;
    }
    ClasseVolume::Fisico
}

/// Uma linha da tabela de Discos, já classificada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinhaDisco {
    pub volume: LinhaVolume,
    pub classe: ClasseVolume,
    pub sistema: bool,
}

/// Linhas a mostrar e quantas foram ocultadas. Sem `todos`, só físicos, removíveis e de rede.
/// Volumes ocultos fora do Catálogo ganham ids negativos (`-1`, `-2`...) só para a tabela.
#[must_use]
pub fn montar_linhas(
    volumes: &[LinhaVolume],
    c: &Classificacao,
    todos: bool,
) -> (Vec<LinhaDisco>, usize) {
    let mut todas: Vec<LinhaDisco> = volumes
        .iter()
        .map(|v| {
            let (classe, sistema) = c.de(v);
            LinhaDisco {
                volume: v.clone(),
                classe,
                sistema,
            }
        })
        .collect();
    for (i, o) in c.ocultos_fora_do_catalogo.iter().enumerate() {
        todas.push(LinhaDisco {
            volume: LinhaVolume {
                id: -(i64::try_from(i).unwrap_or(0) + 1),
                rotulo: o.rotulo.clone(),
                montagem: o.ponto_montagem.clone(),
                montagens: 1,
                fs_type: o.fs_type.clone(),
                capacidade: o.capacidade_bytes,
                livre: o.livre_bytes,
                dispositivo: None,
                tipo_dispositivo: o.tipo_dispositivo.map(|t| format!("{t:?}").to_lowercase()),
                conectado: o.conectado,
                bytes_catalogados: None,
                visto_em: None,
            },
            classe: o.classe,
            sistema: o.sistema,
        });
    }
    let total = todas.len();
    if !todos {
        todas.retain(|l| l.classe.eh_real());
    }
    let ocultos = total - todas.len();
    (todas, ocultos)
}

/// "3 volume(s) físicos, 60 de imagem/virtuais ocultos".
#[must_use]
pub fn resumo(linhas: &[LinhaDisco], ocultos: usize) -> String {
    let fisicos = linhas
        .iter()
        .filter(|l| matches!(l.classe, ClasseVolume::Fisico | ClasseVolume::Removivel))
        .count();
    let rede = linhas
        .iter()
        .filter(|l| l.classe == ClasseVolume::Rede)
        .count();
    let mut s = format!("{fisicos} volume(s) físicos");
    if rede > 0 {
        s.push_str(&format!(", {rede} de rede"));
    }
    if ocultos > 0 {
        s.push_str(&format!(", {ocultos} de imagem/virtuais ocultos"));
    }
    s
}

/// Remove espaços e barras finais (mantém `/`).
#[must_use]
pub fn normalizar_raiz(caminho: &str) -> String {
    let mut c = caminho.trim().to_string();
    while c.len() > 1 && c.ends_with('/') {
        c.pop();
    }
    c
}

/// Raízes dos volumes selecionados: o ponto de montagem principal de cada um, sem repetição,
/// na ordem da tabela. Volumes sem montagem (offline) são ignorados.
#[must_use]
pub fn raizes_dos_selecionados(linhas: &[LinhaDisco], sel: &BTreeSet<i64>) -> Vec<String> {
    let mut raizes: Vec<String> = Vec::new();
    for l in linhas.iter().filter(|l| sel.contains(&l.volume.id)) {
        if let Some(m) = l.volume.montagem.as_deref().map(normalizar_raiz)
            && !m.is_empty()
            && !raizes.contains(&m)
        {
            raizes.push(m);
        }
    }
    raizes
}

/// Ponto de montagem principal de um volume real e conectado, se houver.
#[must_use]
pub fn raiz_do_volume(l: &LinhaDisco) -> Option<String> {
    (l.classe.eh_real() && l.volume.conectado)
        .then(|| l.volume.montagem.as_deref().map(normalizar_raiz))
        .flatten()
        .filter(|m| !m.is_empty())
}

/// Raízes de "todos os discos físicos": volumes `Fisico` conectados, menos partições de sistema
/// (`/boot/efi`...). Removíveis e de rede ficam de fora: o usuário escolhe.
#[must_use]
pub fn raizes_dos_discos_fisicos(linhas: &[LinhaDisco]) -> Vec<String> {
    let mut raizes: Vec<String> = Vec::new();
    for l in linhas
        .iter()
        .filter(|l| l.classe == ClasseVolume::Fisico && !l.sistema)
    {
        if let Some(m) = raiz_do_volume(l)
            && !raizes.contains(&m)
        {
            raizes.push(m);
        }
    }
    raizes
}

/// Aviso (não bloqueante) quando `/` está entre as raízes.
#[must_use]
pub fn aviso_raiz_sistema(raizes: &[String]) -> Option<&'static str> {
    raizes.iter().any(|r| normalizar_raiz(r) == "/").then_some(
        "Selecionar \"/\" inclui todo o disco do sistema. Os volumes montados abaixo dele \
         (ex.: /mnt/d, /boot/efi) são outros volumes e não são atravessados: selecione-os também.",
    )
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_core::VolumeId;

    fn linha(id: i64, montagem: &str, fs: &str, conectado: bool) -> LinhaVolume {
        LinhaVolume {
            id,
            rotulo: None,
            montagem: Some(montagem.into()),
            montagens: 1,
            fs_type: Some(fs.into()),
            capacidade: Some(100),
            livre: Some(50),
            dispositivo: None,
            tipo_dispositivo: None,
            conectado,
            bytes_catalogados: None,
            visto_em: None,
        }
    }

    fn info(id: i64, classe: ClasseVolume, sistema: bool, mont: &str) -> VolumeInfo {
        VolumeInfo {
            id: VolumeId(id),
            dispositivo: None,
            tipo_dispositivo: None,
            fs_type: Some("x".into()),
            rotulo: None,
            ponto_montagem: Some(mont.into()),
            conectado: true,
            capacidade_bytes: None,
            livre_bytes: None,
            classe,
            sistema,
        }
    }

    fn cenario() -> (Vec<LinhaVolume>, Classificacao) {
        let db = vec![
            linha(1, "/", "ext4", true),
            linha(2, "/boot/efi", "vfat", true),
            linha(3, "/mnt/d", "ext4", true),
            linha(4, "/snap/core/1", "squashfs", true),
            linha(5, "/mnt/usb", "vfat", false),
        ];
        let c = Classificacao::de_resposta(
            &[
                info(1, ClasseVolume::Fisico, false, "/"),
                info(2, ClasseVolume::Fisico, true, "/boot/efi"),
                info(3, ClasseVolume::Fisico, false, "/mnt/d"),
                info(5, ClasseVolume::Removivel, false, "/mnt/usb"),
            ],
            &[
                info(4, ClasseVolume::Imagem, false, "/snap/core/1"),
                info(0, ClasseVolume::Imagem, false, "/snap/bare/5"),
            ],
        );
        (db, c)
    }

    #[test]
    fn padrao_esconde_imagem_e_virtual_e_conta_ocultos() {
        let (db, c) = cenario();
        let (linhas, ocultos) = montar_linhas(&db, &c, false);
        assert_eq!(linhas.len(), 4);
        assert_eq!(ocultos, 2);
        assert_eq!(
            resumo(&linhas, ocultos),
            "4 volume(s) físicos, 2 de imagem/virtuais ocultos"
        );
        let (todas, ocultos) = montar_linhas(&db, &c, true);
        assert_eq!((todas.len(), ocultos), (6, 0));
        assert!(todas.iter().any(|l| l.volume.id < 0), "oculto sem id");
    }

    #[test]
    fn presuncao_sem_classificacao_do_sistema() {
        use ClasseVolume::{Fisico, Imagem, Rede, Virtual};
        let c = |fs: &str, p: &str| classe_presumida(Some(fs), Some(p));
        assert_eq!(
            c("overlay", "/var/lib/docker/rootfs/overlayfs/ab12"),
            Virtual
        );
        assert_eq!(c("ext4", "/var/lib/docker/volumes/x"), Virtual);
        assert_eq!(c("fuse.rclone", "/mnt/googledrive"), Rede);
        assert_eq!(c("fuse.rclone", "/mnt/onedrive"), Rede);
        assert_eq!(c("nfs4", "/mnt/nas"), Rede);
        assert_eq!(c("cifs", "/mnt/smb"), Rede);
        assert_eq!(c("fuse.sshfs", "/mnt/s"), Rede);
        assert_eq!(c("fuse.AppImage-x", "/tmp/.mount_appXYZ"), Imagem);
        assert_eq!(c("fuse", "/tmp/.mount_abc"), Imagem);
        assert_eq!(c("squashfs", "/snap/x/1"), Imagem);
        assert_eq!(c("tmpfs", "/run/user/1000"), Virtual);
        assert_eq!(c("fuse.portal", "/run/user/1000/doc"), Virtual);
        assert_eq!(c("ext4", "/run/media/u/PEN"), Fisico);
        assert_eq!(c("ext4", "/mnt/dados"), Fisico);
        assert_eq!(c("btrfs", "/"), Fisico);
    }

    #[test]
    fn descoberta_direta_classifica_sem_o_worker() {
        let linhas = vec![
            linha(1, "/mnt/googledrive", "fuse.rclone", true),
            linha(2, "/mnt/dados", "ext4", true),
        ];
        let mut d = dag_core::VolumeDescoberto {
            platform_id: "a".into(),
            st_dev: 1,
            rotulo: None,
            fs_type: "ext4".into(),
            capacidade_bytes: None,
            livre_bytes: None,
            pontos_de_montagem: vec![b"/mnt/dados".to_vec()],
            pseudo: false,
            classe: ClasseVolume::Removivel,
            sistema: false,
            dispositivo: None,
        };
        let c = Classificacao::de_descobertos(std::slice::from_ref(&d));
        let (l, ocultos) = montar_linhas(&linhas, &c, false);
        assert_eq!(ocultos, 0);
        assert_eq!(l[1].classe, ClasseVolume::Removivel);
        assert_eq!(l[0].classe, ClasseVolume::Rede);
        d.classe = ClasseVolume::Virtual;
        let c = Classificacao::de_descobertos(&[d]);
        assert_eq!(montar_linhas(&linhas, &c, false).1, 1);
    }

    #[test]
    fn sem_classificacao_usa_fs_de_imagem() {
        let (db, _) = cenario();
        let (linhas, ocultos) = montar_linhas(&db, &Classificacao::default(), false);
        assert_eq!((linhas.len(), ocultos), (4, 1));
    }

    #[test]
    fn selecao_vira_raizes_sem_repetir_e_ignora_offline() {
        let (db, c) = cenario();
        let (linhas, _) = montar_linhas(&db, &c, true);
        let sel: BTreeSet<i64> = [3, 1, 3, 4].into_iter().collect();
        assert_eq!(
            raizes_dos_selecionados(&linhas, &sel),
            vec!["/", "/mnt/d", "/snap/core/1"]
        );
        let mut dup = linhas.clone();
        dup.push(LinhaDisco {
            volume: linha(9, "/mnt/d/", "ext4", true),
            classe: ClasseVolume::Fisico,
            sistema: false,
        });
        let sel: BTreeSet<i64> = [3, 9].into_iter().collect();
        assert_eq!(raizes_dos_selecionados(&dup, &sel), vec!["/mnt/d"]);
    }

    #[test]
    fn discos_fisicos_excluem_sistema_removivel_offline_e_imagem() {
        let (db, c) = cenario();
        let (linhas, _) = montar_linhas(&db, &c, true);
        assert_eq!(raizes_dos_discos_fisicos(&linhas), vec!["/", "/mnt/d"]);
    }

    #[test]
    fn aviso_so_para_a_raiz() {
        assert!(aviso_raiz_sistema(&["/mnt/d".into()]).is_none());
        assert!(aviso_raiz_sistema(&["/mnt/d".into(), "/".into()]).is_some());
        assert_eq!(normalizar_raiz(" /mnt/d// "), "/mnt/d");
        assert_eq!(normalizar_raiz("/"), "/");
    }
}
