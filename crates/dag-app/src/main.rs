//! DAG — Acervo Digital (GUI). Abre a casca Bancada; neste incremento não lê nem grava nada.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    dag_app::app::montar()?.executar()?;
    Ok(())
}
