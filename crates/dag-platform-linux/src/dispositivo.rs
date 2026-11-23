//! Resolução de `major:minor` até o Dispositivo Físico via sysfs.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use dag_core::{DispositivoDescoberto, TipoDispositivo};

use crate::fonte::FonteSistema;

const PROFUNDIDADE_MAXIMA: u8 = 8;

/// Normaliza `.` e `..` lexicalmente (suficiente: os links de `/sys/dev/block` e
/// `/sys/class/block` são relativos a diretórios reais).
fn normalizar(base: &Path, alvo: &Path) -> PathBuf {
    let bruto = if alvo.is_absolute() {
        alvo.to_path_buf()
    } else {
        base.join(alvo)
    };
    let mut saida = PathBuf::new();
    for c in bruto.components() {
        match c {
            Component::ParentDir => {
                saida.pop();
            }
            Component::CurDir => {}
            outro => saida.push(outro.as_os_str()),
        }
    }
    saida
}

fn ler_texto(fonte: &dyn FonteSistema, caminho: &Path) -> Option<String> {
    let bytes = fonte.ler_arquivo(caminho).ok()?;
    let texto = String::from_utf8_lossy(&bytes).trim().to_string();
    (!texto.is_empty()).then_some(texto)
}

/// Caminho real em `/sys/devices/...` do dispositivo de bloco `major:minor`.
pub(crate) fn caminho_sysfs(fonte: &dyn FonteSistema, major: u32, minor: u32) -> Option<PathBuf> {
    let link = PathBuf::from(format!("/sys/dev/block/{major}:{minor}"));
    let alvo = fonte.ler_link(&link).ok()?;
    Some(normalizar(Path::new("/sys/dev/block"), &alvo))
}

/// `major:minor` de um dispositivo pelo nome do kernel (`sda1`, `dm-0`).
pub(crate) fn majmin_por_nome(fonte: &dyn FonteSistema, nome: &OsStr) -> Option<(u32, u32)> {
    let caminho = Path::new("/sys/class/block").join(nome).join("dev");
    let texto = ler_texto(fonte, &caminho)?;
    let (a, b) = texto.split_once(':')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Conjunto de discos inteiros (caminhos sysfs) que sustentam o dispositivo.
/// `None` se a árvore for profunda demais (ciclo suspeito).
fn discos_de(fonte: &dyn FonteSistema, sysfs: &Path, nivel: u8) -> Option<BTreeSet<PathBuf>> {
    if nivel > PROFUNDIDADE_MAXIMA {
        return None;
    }
    // device-mapper e md listam seus componentes em `slaves/`.
    if let Ok(escravos) = fonte.listar_dir(&sysfs.join("slaves"))
        && !escravos.is_empty()
    {
        let mut todos = BTreeSet::new();
        for nome in escravos {
            let link = Path::new("/sys/class/block").join(&nome);
            let alvo = fonte.ler_link(&link).ok()?;
            let filho = normalizar(Path::new("/sys/class/block"), &alvo);
            todos.extend(discos_de(fonte, &filho, nivel + 1)?);
        }
        return Some(todos);
    }
    // Partição: o disco é o diretório pai (que ainda pode ser dm/md particionado).
    if fonte.ler_arquivo(&sysfs.join("partition")).is_ok() {
        let pai = sysfs.parent()?;
        return discos_de(fonte, pai, nivel + 1);
    }
    Some(BTreeSet::from([sysfs.to_path_buf()]))
}

fn descrever_disco(fonte: &dyn FonteSistema, disco: &Path) -> Option<DispositivoDescoberto> {
    let nome = disco.file_name()?.to_string_lossy().into_owned();
    let primeiro = |candidatos: &[&str]| {
        candidatos
            .iter()
            .find_map(|c| ler_texto(fonte, &disco.join(c)))
    };
    let identificador = primeiro(&["wwid", "device/wwid", "device/serial"])
        .unwrap_or_else(|| format!("kname:{nome}"));
    let modelo = primeiro(&["device/model"]);
    let removivel = primeiro(&["removable"]).as_deref() == Some("1");
    let virtual_ = disco.starts_with("/sys/devices/virtual");
    let usb = disco
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with("usb"));
    let tipo = if virtual_ {
        TipoDispositivo::Virtual
    } else if nome.starts_with("nvme") {
        TipoDispositivo::Nvme
    } else if usb {
        TipoDispositivo::Usb
    } else {
        match primeiro(&["queue/rotational"]).as_deref() {
            Some("1") => TipoDispositivo::Hdd,
            Some("0") => TipoDispositivo::Ssd,
            _ => TipoDispositivo::Desconhecido,
        }
    };
    Some(DispositivoDescoberto {
        identificador,
        modelo,
        tipo,
        removivel,
    })
}

/// Dispositivo físico único que hospeda `major:minor`.
///
/// Devolve `None` quando não dá para afirmar um único disco: sysfs ausente, ciclo,
/// ou volume dm/md cujos componentes ficam em discos diferentes (LVM espalhado,
/// RAID). Atribuir um disco arbitrário seria inventar o dado; o Volume segue
/// catalogado, apenas sem Dispositivo Físico.
pub(crate) fn resolver_dispositivo(
    fonte: &dyn FonteSistema,
    major: u32,
    minor: u32,
) -> Option<DispositivoDescoberto> {
    let sysfs = caminho_sysfs(fonte, major, minor)?;
    let discos = discos_de(fonte, &sysfs, 0)?;
    let mut it = discos.iter();
    match (it.next(), it.next()) {
        (Some(unico), None) => descrever_disco(fonte, unico),
        _ => None,
    }
}
