//! Imprime os volumes descobertos no sistema atual.
fn main() {
    match dag_platform_linux::descobrir_volumes() {
        Ok(vs) => {
            for v in vs {
                let pontos: Vec<String> = v
                    .pontos_de_montagem
                    .iter()
                    .map(|p| String::from_utf8_lossy(p).into_owned())
                    .collect();
                println!(
                    "{} {} {:?} pseudo={} cap={:?} livre={:?} rotulo={:?} disp={:?}",
                    v.platform_id,
                    v.fs_type,
                    pontos,
                    v.pseudo,
                    v.capacidade_bytes,
                    v.livre_bytes,
                    v.rotulo,
                    v.dispositivo
                );
            }
        }
        Err(e) => eprintln!("erro: {e}"),
    }
}
