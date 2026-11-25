//! Exportação de relatórios para CSV ou JSON, em caminho dado pelo usuário.
//!
//! O arquivo é criado novo (nunca sobrescreve) com permissão 0600, porque lista caminhos do
//! acervo. O relatório inteiro é gravado, não só uma página.

use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use dag_core::{
    DadosRelatorio, FormatoExportacao, LIMITACOES_ESPACO, MarcacaoInfo, NivelEspaco, TipoRelatorio,
};
use rusqlite::Connection;
use serde_json::{Value, json};

use super::espaco::relatorio_espaco;
use super::marcacoes::listar_marcacoes;
use super::relatorios::{relatorio_cobertura, relatorio_sem_cobertura};
use super::{ErroDuplicados, Resultado, tipo_str};

const PAGINA: u32 = 5000;

/// Formato pela extensão do caminho (`.csv` ou `.json`).
///
/// # Errors
/// [`ErroDuplicados::Invalido`] para outra extensão.
pub fn formato_do_caminho(p: &Path) -> Resultado<FormatoExportacao> {
    match p
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("csv") => Ok(FormatoExportacao::Csv),
        Some("json") => Ok(FormatoExportacao::Json),
        _ => Err(ErroDuplicados::Invalido(format!(
            "o arquivo de saída precisa terminar em .csv ou .json: {}",
            p.display()
        ))),
    }
}

fn csv_campo(s: &str) -> String {
    // Aspas duplas em RFC 4180; também protege contra fórmulas em planilhas.
    let perigoso = s.starts_with(['=', '+', '-', '@']) && s.parse::<i64>().is_err();
    let s = if perigoso {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

struct Linha {
    json: Value,
    csv: Vec<String>,
}

fn opt<T: ToString>(x: Option<T>) -> String {
    x.map(|v| v.to_string()).unwrap_or_default()
}

fn linha_marcacao(m: &MarcacaoInfo) -> Linha {
    Linha {
        json: serde_json::to_value(m).unwrap_or(Value::Null),
        csv: vec![
            m.id.to_string(),
            tipo_str(m.tipo).into(),
            match m.estado {
                dag_core::EstadoMarcacao::Ativa => "ativa",
                dag_core::EstadoMarcacao::Pendente => "pendente",
            }
            .into(),
            m.escopo.como_str().into(),
            m.alvo.clone(),
            opt(m.volume.map(|x| x.0)),
            opt(m.pasta.map(|x| x.0)),
            opt(m.arquivo.map(|x| x.0)),
            opt(m.grupo.map(|x| x.0)),
            opt(m.hash_na_marcacao),
            match m.autor {
                dag_core::Autor::Gui => "gui",
                dag_core::Autor::Cli => "cli",
                dag_core::Autor::Mcp => "mcp",
            }
            .into(),
            m.criada_em.to_string(),
            m.atualizada_em.to_string(),
        ],
    }
}

fn cabecalho(tipo: TipoRelatorio) -> &'static [&'static str] {
    match tipo {
        TipoRelatorio::Marcacoes => &[
            "id",
            "tipo",
            "estado",
            "escopo",
            "alvo",
            "volume_id",
            "pasta_id",
            "arquivo_id",
            "grupo_id",
            "hash",
            "autor",
            "criada_em",
            "atualizada_em",
        ],
        TipoRelatorio::SemCobertura => &[
            "arquivo_id",
            "volume_id",
            "volume",
            "caminho",
            "tamanho_bytes",
            "hash",
            "unica_copia",
        ],
        TipoRelatorio::Espaco => &[
            "nivel",
            "id",
            "nome",
            "conectado",
            "bytes_marcados_delecao",
            "bytes_redundantes_estimados",
            "bytes_total",
            "copias",
        ],
        TipoRelatorio::Cobertura => &[
            "nivel",
            "id",
            "nome",
            "conectado",
            "arquivos",
            "com_cobertura",
            "cobertura_nao_verificada",
            "sem_cobertura",
            "bytes_sem_cobertura",
        ],
    }
}

fn nome_nivel(n: NivelEspaco) -> &'static str {
    match n {
        NivelEspaco::Computador => "computador",
        NivelEspaco::Volume => "volume",
        NivelEspaco::Pasta => "pasta",
        NivelEspaco::Grupo => "grupo",
    }
}

fn linhas_de(d: &DadosRelatorio, com_total: bool) -> Vec<Linha> {
    match d {
        DadosRelatorio::Marcacoes { marcacoes } => marcacoes.iter().map(linha_marcacao).collect(),
        DadosRelatorio::SemCobertura { arquivos } => arquivos
            .iter()
            .map(|a| Linha {
                json: serde_json::to_value(a).unwrap_or(Value::Null),
                csv: vec![
                    a.arquivo.to_string(),
                    a.volume.to_string(),
                    a.volume_rotulo.clone().unwrap_or_default(),
                    a.caminho.clone(),
                    a.tamanho.to_string(),
                    opt(a.hash),
                    a.unica_copia.to_string(),
                ],
            })
            .collect(),
        DadosRelatorio::Espaco { total, itens, .. } => com_total
            .then_some(total)
            .into_iter()
            .chain(itens.iter())
            .map(|i| Linha {
                json: serde_json::to_value(i).unwrap_or(Value::Null),
                csv: vec![
                    nome_nivel(i.nivel).into(),
                    opt(i.id),
                    i.nome.clone(),
                    opt(i.conectado),
                    i.bytes_marcados_delecao.to_string(),
                    i.bytes_redundantes_estimados.to_string(),
                    i.bytes_total.to_string(),
                    i.copias.to_string(),
                ],
            })
            .collect(),
        DadosRelatorio::Cobertura { volumes, pastas } => {
            let mut v: Vec<Linha> = volumes
                .iter()
                .map(|x| Linha {
                    json: json!({"nivel": "volume", "dados": x}),
                    csv: vec![
                        "volume".into(),
                        x.volume.to_string(),
                        x.rotulo.clone().unwrap_or_default(),
                        x.conectado.to_string(),
                        x.arquivos.to_string(),
                        x.com_cobertura.to_string(),
                        x.cobertura_nao_verificada.to_string(),
                        x.sem_cobertura.to_string(),
                        x.bytes_sem_cobertura.to_string(),
                    ],
                })
                .collect();
            v.extend(pastas.iter().map(|x| Linha {
                json: json!({"nivel": "pasta", "dados": x}),
                csv: vec![
                    "pasta".into(),
                    x.pasta.to_string(),
                    x.caminho.clone(),
                    String::new(),
                    x.arquivos.to_string(),
                    x.com_cobertura.to_string(),
                    String::new(),
                    x.sem_cobertura.to_string(),
                    x.bytes_sem_cobertura.to_string(),
                ],
            }));
            v
        }
    }
}

/// Páginas do relatório inteiro, entregues a `f`.
fn paginas(
    conn: &Connection,
    tipo: TipoRelatorio,
    mut f: impl FnMut(&DadosRelatorio, bool) -> Resultado<()>,
) -> Resultado<()> {
    match tipo {
        TipoRelatorio::Marcacoes => {
            let mut depois = None;
            loop {
                let (v, prox) = listar_marcacoes(conn, depois, PAGINA)?;
                f(&DadosRelatorio::Marcacoes { marcacoes: v }, true)?;
                match prox {
                    Some(p) => depois = Some(p),
                    None => break,
                }
            }
        }
        TipoRelatorio::SemCobertura => {
            let mut cursor: Option<String> = None;
            loop {
                let (d, prox) = relatorio_sem_cobertura(conn, None, PAGINA, cursor.as_deref())?;
                f(&d, true)?;
                match prox {
                    Some(p) => cursor = Some(p),
                    None => break,
                }
            }
        }
        TipoRelatorio::Cobertura => {
            let (d, _) = relatorio_cobertura(conn, None, u32::MAX, None)?;
            f(&d, true)?;
        }
        TipoRelatorio::Espaco => {
            // Computador + todos os níveis, cada um inteiro.
            let (d, _) = relatorio_espaco(conn, NivelEspaco::Volume, u32::MAX, None)?;
            f(&d, true)?;
            for n in [NivelEspaco::Pasta, NivelEspaco::Grupo] {
                let (d, _) = relatorio_espaco(conn, n, u32::MAX, None)?;
                f(&d, false)?;
            }
        }
    }
    Ok(())
}

/// Grava o relatório inteiro em `destino` (`.csv` ou `.json`) e devolve as linhas gravadas.
/// Recusa sobrescrever arquivo existente.
///
/// # Errors
/// [`ErroDuplicados::Invalido`] (extensão), [`ErroDuplicados::Es`] (destino existe, sem
/// permissão...), falha do SQLite.
pub fn exportar_relatorio(
    conn: &Connection,
    tipo: TipoRelatorio,
    destino: &Path,
    agora: i64,
) -> Resultado<(FormatoExportacao, u64)> {
    let formato = formato_do_caminho(destino)?;
    let es = |e: std::io::Error| ErroDuplicados::Es {
        contexto: format!("gravar relatório em {}", destino.display()),
        fonte: e,
    };
    let arq = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destino)
        .map_err(es)?;
    let mut w = BufWriter::new(arq);
    let mut n: u64 = 0;
    let resultado = (|| -> Resultado<()> {
        match formato {
            FormatoExportacao::Csv => {
                let cab: Vec<String> = cabecalho(tipo).iter().map(|c| csv_campo(c)).collect();
                writeln!(w, "{}", cab.join(",")).map_err(es)?;
            }
            FormatoExportacao::Json => {
                let nome = serde_json::to_string(&tipo).unwrap_or_default();
                write!(
                    w,
                    "{{\"relatorio\":{nome},\"gerado_em\":{agora},\"linhas\":["
                )
                .map_err(es)?;
            }
        }
        paginas(conn, tipo, |d, com_total| {
            for l in linhas_de(d, com_total) {
                match formato {
                    FormatoExportacao::Csv => {
                        let campos: Vec<String> = l.csv.iter().map(|c| csv_campo(c)).collect();
                        writeln!(w, "{}", campos.join(",")).map_err(es)?;
                    }
                    FormatoExportacao::Json => {
                        if n > 0 {
                            write!(w, ",").map_err(es)?;
                        }
                        write!(w, "{}", l.json).map_err(es)?;
                    }
                }
                n += 1;
            }
            Ok(())
        })?;
        if formato == FormatoExportacao::Json {
            let extra = if tipo == TipoRelatorio::Espaco {
                format!(
                    ",\"limitacoes\":{}",
                    serde_json::to_string(&LIMITACOES_ESPACO).unwrap_or_default()
                )
            } else {
                String::new()
            };
            write!(w, "]{extra}}}").map_err(es)?;
        }
        w.flush().map_err(es)
    })();
    if let Err(e) = resultado {
        drop(w);
        // Não deixa relatório pela metade.
        let _ = std::fs::remove_file(destino);
        return Err(e);
    }
    Ok((formato, n))
}
