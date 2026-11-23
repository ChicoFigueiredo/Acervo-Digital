#![allow(clippy::unwrap_used)]

use std::os::unix::fs::MetadataExt;

use dag_platform_linux::descobrir_volumes;

#[test]
fn descobre_o_volume_da_raiz_e_marca_proc_como_pseudo() {
    let volumes = descobrir_volumes().unwrap();
    assert!(!volumes.is_empty());

    let st_dev_raiz = std::fs::metadata("/").unwrap().dev();
    let raiz = volumes
        .iter()
        .find(|v| v.st_dev == st_dev_raiz)
        .expect("volume que contém /");
    assert!(!raiz.fs_type.is_empty());
    assert!(!raiz.pseudo);
    assert!(raiz.pontos_de_montagem.iter().any(|p| p == b"/"));
    assert!(raiz.platform_id.contains(':') || !raiz.platform_id.is_empty());
    if let (Some(cap), Some(livre)) = (raiz.capacidade_bytes, raiz.livre_bytes) {
        assert!(cap > 0 && livre <= cap);
    }

    let proc = volumes
        .iter()
        .find(|v| v.pontos_de_montagem.iter().any(|p| p == b"/proc"))
        .expect("/proc montado");
    assert!(proc.pseudo);
    assert_eq!(proc.fs_type, "proc");
    assert!(proc.dispositivo.is_none());
}
