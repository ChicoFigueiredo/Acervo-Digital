//! Pipeline completo no agente, com arquivos reais: Full Scan → hash → duplicados → marcações →
//! relatório. Verifica também que o acervo nunca é tocado (CA-A23).
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use dag_agent::Agente;
use dag_core::{
    Alvo, Autor, Comando, DadosRelatorio, DispositivoDescoberto, EstadoMarcacao, ModoVarredura,
    Pedido, Resposta, ResultadoMarcacao, TipoDispositivo, TipoRelatorio, VolumeDescoberto,
};
use tempfile::TempDir;

fn volume(raiz: &Path, rotulo: &str, disco: &str) -> VolumeDescoberto {
    let dev = fs::metadata(raiz).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("{rotulo}-{dev}"),
        st_dev: dev,
        rotulo: Some(rotulo.into()),
        fs_type: "teste".into(),
        capacidade_bytes: Some(1_000_000),
        livre_bytes: Some(400_000),
        pontos_de_montagem: vec![raiz.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: Some(DispositivoDescoberto {
            identificador: disco.into(),
            modelo: None,
            tipo: TipoDispositivo::Ssd,
            removivel: false,
        }),
    }
}

fn escrever(p: &Path, conteudo: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, conteudo).unwrap();
}

fn conteudo(letra: u8, n: usize) -> Vec<u8> {
    vec![letra; n]
}

fn scan(raizes: &[&Path], sem_hash: bool) -> Comando {
    Comando::IniciarScan {
        modo: ModoVarredura::FullScan,
        raizes: raizes
            .iter()
            .map(|r| r.to_str().unwrap().to_string())
            .collect(),
        sem_hash,
        elevar: false,
        releitura_total: false,
        threads_por_disco: None,
        max_discos: None,
    }
}

/// Estado observável de cada arquivo e pasta: tamanho, mtime, inode, permissão e conteúdo.
fn instantaneo(raiz: &Path) -> BTreeMap<PathBuf, (u64, i64, u64, u32, Vec<u8>)> {
    fn rec(d: &Path, m: &mut BTreeMap<PathBuf, (u64, i64, u64, u32, Vec<u8>)>) {
        for e in fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            let md = fs::symlink_metadata(&p).unwrap();
            let dados = if md.is_file() {
                fs::read(&p).unwrap()
            } else {
                Vec::new()
            };
            m.insert(
                p.clone(),
                (
                    md.len(),
                    md.mtime_nsec() + md.mtime() * 1_000_000_000,
                    md.ino(),
                    md.mode(),
                    dados,
                ),
            );
            if md.is_dir() {
                rec(&p, m);
            }
        }
    }
    let mut m = BTreeMap::new();
    rec(raiz, &mut m);
    m
}

struct Arvore {
    _tmp: TempDir,
    raiz: PathBuf,
    catalogo: PathBuf,
}

/// raiz/{a/dup1, b/dup2 (iguais), c/unico, h/{l1,l2,l3} (hard links), v/{vazio1,vazio2}}
fn arvore() -> Arvore {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("acervo");
    escrever(&raiz.join("a/dup1.bin"), &conteudo(b'a', 10_000));
    escrever(&raiz.join("b/dup2.bin"), &conteudo(b'a', 10_000));
    escrever(&raiz.join("c/unico.bin"), &conteudo(b'u', 7_000));
    escrever(&raiz.join("h/l1.bin"), &conteudo(b'h', 2_000_000));
    fs::hard_link(raiz.join("h/l1.bin"), raiz.join("h/l2.bin")).unwrap();
    fs::hard_link(raiz.join("h/l1.bin"), raiz.join("h/l3.bin")).unwrap();
    escrever(&raiz.join("v/vazio1"), b"");
    escrever(&raiz.join("v/vazio2"), b"");
    let catalogo = tmp.path().join("dados/catalogo.sqlite");
    Arvore {
        _tmp: tmp,
        raiz,
        catalogo,
    }
}

fn agente(a: &Arvore) -> Agente {
    Agente::abrir(&a.catalogo)
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![volume(&a.raiz, "SSD", "disco-ssd")])
}

fn p(raiz: &Path, rel: &str) -> String {
    raiz.join(rel).to_str().unwrap().to_string()
}

fn marcacao(r: Resposta) -> ResultadoMarcacao {
    let Resposta::Marcacao { resultado } = r else {
        panic!("{r:?}")
    };
    resultado
}

fn grupos(a: &mut Agente) -> Vec<dag_core::GrupoDuplicados> {
    let r = a.executar(Comando::ListarDuplicados {
        volume: None,
        tamanho_minimo: None,
        ordem: None,
        limite: None,
        cursor: None,
    });
    let Resposta::Duplicados { grupos, .. } = r else {
        panic!("{r:?}")
    };
    grupos
}

#[test]
fn full_scan_faz_hash_forma_grupos_e_hard_links_ficam_de_fora() {
    let t = arvore();
    let mut a = agente(&t);
    let r = a.executar(scan(&[&t.raiz], false));
    let Resposta::ScanConcluido {
        desfecho,
        hash,
        duplicados,
        erros,
        ..
    } = r
    else {
        panic!("{r:?}")
    };
    assert_eq!((desfecho.as_str(), erros), ("concluida", 0));
    let h = hash.unwrap();
    // 3 caminhos de hard link + dup1 + dup2 + unico + 2 vazios: todo arquivo recebe hash,
    // mas vazios ficam fora dos grupos.
    assert_eq!(h.hasheados, 3 + 2 + 1 + 2);
    // CA-A19: o arquivo de 2 MB foi lido uma vez.
    assert_eq!(h.bytes_lidos, 2_000_000 + 10_000 + 10_000 + 7_000);
    let d = duplicados.unwrap();
    assert_eq!((d.grupos, d.copias), (1, 2));
    assert_eq!(d.espaco_recuperavel_bytes, 10_000);
    let gs = grupos(&mut a);
    assert_eq!(gs.len(), 1);
    assert_eq!(gs[0].tamanho, 10_000);
    // Sem Prioridade: a Cópia Principal é determinística (menor id).
    assert!(gs[0].copia_principal.is_some());
}

#[test]
fn sem_hash_faz_so_o_inventario() {
    let t = arvore();
    let mut a = agente(&t);
    let r = a.executar(scan(&[&t.raiz], true));
    let Resposta::ScanConcluido {
        hash,
        duplicados,
        arquivos,
        ..
    } = r
    else {
        panic!("{r:?}")
    };
    assert!(hash.is_none() && duplicados.is_none());
    assert_eq!(arquivos, 8);
    assert!(grupos(&mut a).is_empty());
    // Marcação manual exige Hash Completo: erro claro, nada gravado.
    let r = a.executar(Comando::MarcarDelecao {
        alvo: Alvo::Caminho(p(&t.raiz, "a/dup1.bin")),
    });
    let Resposta::Erro { codigo, mensagem } = r else {
        panic!()
    };
    assert_eq!(codigo, "sem_hash");
    assert!(mensagem.contains("Hash Completo"), "{mensagem}");
    // Um Full Scan completo depois preenche tudo (hash + grupos).
    let r = a.executar(scan(&[&t.raiz], false));
    let Resposta::ScanConcluido { duplicados, .. } = r else {
        panic!()
    };
    assert_eq!(duplicados.unwrap().grupos, 1);
}

#[test]
fn ca_a23_marcar_so_altera_o_catalogo_e_relatorio_lista_exatamente_as_marcacoes() {
    let t = arvore();
    let mut a = agente(&t);
    a.executar(scan(&[&t.raiz], false));
    let antes = instantaneo(&t.raiz);

    // Deleção da cópia duplicada: sem alerta de Única Cópia.
    let r = marcacao(a.executar(Comando::MarcarDelecao {
        alvo: Alvo::Caminho(p(&t.raiz, "b/dup2.bin")),
    }));
    assert_eq!(r.unicas_copias_total, 0);
    assert_eq!(r.marcacoes.len(), 1);
    // Deleção do arquivo sem cópia: alerta de Única Cópia.
    let r = marcacao(a.executar(Comando::MarcarDelecao {
        alvo: Alvo::Caminho(p(&t.raiz, "c/unico.bin")),
    }));
    assert_eq!(r.unicas_copias_total, 1);
    assert!(r.unicas_copias[0].caminho.ends_with("c/unico.bin"));
    // Backup por regra de pasta.
    let r = marcacao(a.executar(Comando::MarcarBackup {
        alvo: Alvo::Pasta(p(&t.raiz, "a")),
    }));
    assert_eq!(r.marcacoes.len(), 1);
    // Conflito: o arquivo já tem Marcação.
    let r = a.executar(Comando::MarcarBackup {
        alvo: Alvo::Caminho(p(&t.raiz, "c/unico.bin")),
    });
    let Resposta::Erro { codigo, .. } = r else {
        panic!()
    };
    assert_eq!(codigo, "conflito_marcacao");

    // O acervo ficou idêntico: conteúdo, mtime, tamanho, inode, permissões e a árvore toda.
    assert_eq!(instantaneo(&t.raiz), antes);

    // Relatório exportado lista exatamente as marcações do Catálogo.
    let saida = t.catalogo.parent().unwrap().join("marcacoes.json");
    let r = a.executar(Comando::Relatorio {
        tipo: TipoRelatorio::Marcacoes,
        nivel: None,
        limite: None,
        cursor: None,
        saida: Some(saida.to_str().unwrap().into()),
    });
    let Resposta::RelatorioExportado { linhas, .. } = r else {
        panic!("{r:?}")
    };
    assert_eq!(linhas, 3);
    let v: serde_json::Value = serde_json::from_slice(&fs::read(&saida).unwrap()).unwrap();
    let alvos: Vec<&str> = v["linhas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["alvo"].as_str().unwrap())
        .collect();
    assert_eq!(
        alvos,
        [
            p(&t.raiz, "b/dup2.bin"),
            p(&t.raiz, "c/unico.bin"),
            p(&t.raiz, "a")
        ]
    );
    // A exportação não mexeu no acervo e não sobrescreve.
    assert_eq!(instantaneo(&t.raiz), antes);
    let r = a.executar(Comando::Relatorio {
        tipo: TipoRelatorio::Marcacoes,
        nivel: None,
        limite: None,
        cursor: None,
        saida: Some(saida.to_str().unwrap().into()),
    });
    assert!(matches!(r, Resposta::Erro { .. }));
}

#[test]
fn ca_a24_conteudo_muda_rescan_torna_marcacao_pendente() {
    let t = arvore();
    let mut a = agente(&t);
    a.executar(scan(&[&t.raiz], false));
    let r = marcacao(a.executar(Comando::MarcarDelecao {
        alvo: Alvo::Caminho(p(&t.raiz, "b/dup2.bin")),
    }));
    let id = r.marcacoes[0].id;
    let Resposta::Relatorio {
        dados: DadosRelatorio::Espaco { total, .. },
        ..
    } = a.executar(Comando::Relatorio {
        tipo: TipoRelatorio::Espaco,
        nivel: None,
        limite: None,
        cursor: None,
        saida: None,
    })
    else {
        panic!()
    };
    assert_eq!(total.bytes_marcados_delecao, 10_000);

    // O conteúdo muda (mesmo tamanho); mtime avança.
    let alvo = t.raiz.join("b/dup2.bin");
    fs::write(&alvo, conteudo(b'Z', 10_000)).unwrap();
    File::options()
        .write(true)
        .open(&alvo)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();

    let r = a.executar(scan(&[&t.raiz], false));
    let Resposta::ScanConcluido {
        hash, duplicados, ..
    } = r
    else {
        panic!("{r:?}")
    };
    // Só o arquivo alterado foi relido (CA-A25).
    let h = hash.unwrap();
    assert_eq!((h.hasheados, h.bytes_lidos), (1, 10_000));
    let d = duplicados.unwrap();
    assert_eq!((d.grupos, d.marcacoes_pendentes), (0, 1));
    // A marcação segue existindo, pendente, fora do espaço.
    let Resposta::Relatorio {
        dados: DadosRelatorio::Marcacoes { marcacoes },
        ..
    } = a.executar(Comando::Relatorio {
        tipo: TipoRelatorio::Marcacoes,
        nivel: None,
        limite: None,
        cursor: None,
        saida: None,
    })
    else {
        panic!()
    };
    assert_eq!(marcacoes.len(), 1);
    assert_eq!(marcacoes[0].estado, EstadoMarcacao::Pendente);
    let Resposta::Relatorio {
        dados: DadosRelatorio::Espaco { total, .. },
        ..
    } = a.executar(Comando::Relatorio {
        tipo: TipoRelatorio::Espaco,
        nivel: None,
        limite: None,
        cursor: None,
        saida: None,
    })
    else {
        panic!()
    };
    assert_eq!(total.bytes_total, 0);
    // Confirmar a reativa com o hash novo.
    let r = marcacao(a.executar(Comando::ConfirmarMarcacao { marcacao: id }));
    assert_eq!(r.marcacoes[0].estado, EstadoMarcacao::Ativa);
    // O histórico guarda criada, pendente e confirmada.
    let eventos: Vec<String> = a
        .catalogo()
        .armazem()
        .conexao()
        .prepare("SELECT evento FROM marcacoes_historico ORDER BY id")
        .unwrap()
        .query_map([], |l| l.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(eventos, ["criada", "pendente", "confirmada"]);
}

#[test]
fn autor_do_pedido_vai_para_o_historico() {
    let t = arvore();
    let mut a = agente(&t);
    a.executar(scan(&[&t.raiz], false));
    let r = a.executar_pedido(Pedido {
        autor: Autor::Mcp,
        comando: Comando::MarcarBackup {
            alvo: Alvo::Caminho(p(&t.raiz, "a/dup1.bin")),
        },
    });
    assert!(matches!(r, Resposta::Marcacao { .. }));
    let grupo = grupos(&mut a)[0].id;
    let r = a.executar_pedido(Pedido {
        autor: Autor::Gui,
        comando: Comando::DetalharGrupo { grupo },
    });
    let Resposta::Grupo { detalhe } = r else {
        panic!("{r:?}")
    };
    assert_eq!(detalhe.copias.len(), 2);
    let autores: Vec<String> = a
        .catalogo()
        .armazem()
        .conexao()
        .prepare("SELECT autor FROM marcacoes_historico")
        .unwrap()
        .query_map([], |l| l.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(autores, ["mcp"]);
}

#[test]
fn verificar_integridade_pelo_agente() {
    let t = arvore();
    let mut a = agente(&t);
    a.executar(scan(&[&t.raiz], false));
    let r = a.executar(Comando::VerificarIntegridade {
        raizes: vec![],
        threads_por_disco: None,
        max_discos: None,
    });
    let Resposta::Verificacao { resumo } = r else {
        panic!("{r:?}")
    };
    assert_eq!(resumo.desfecho, "concluida");
    assert!(resumo.verificados >= 5, "{resumo:?}");
    assert_eq!(resumo.divergentes, 0);
    // Corrompe um byte sem mudar tamanho nem mtime: a verificação aponta e nada é alterado.
    let alvo = t.raiz.join("c/unico.bin");
    let mtime = fs::metadata(&alvo).unwrap().modified().unwrap();
    let mut bytes = fs::read(&alvo).unwrap();
    bytes[3] ^= 0xff;
    fs::write(&alvo, &bytes).unwrap();
    File::options()
        .write(true)
        .open(&alvo)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    let r = a.executar(Comando::VerificarIntegridade {
        raizes: vec![t.raiz.to_str().unwrap().into()],
        threads_por_disco: None,
        max_discos: None,
    });
    let Resposta::Verificacao { resumo } = r else {
        panic!("{r:?}")
    };
    assert_eq!(resumo.divergentes + resumo.alterados, 1, "{resumo:?}");
    assert_eq!(
        fs::read(&alvo).unwrap(),
        bytes,
        "o arquivo não pode ser alterado"
    );
}

/// Dois volumes reais: a árvore em `/tmp` e outra em `/dev/shm` (st_dev diferente). Se o sistema
/// não tem dois filesystems distintos, o teste não tem o que provar e retorna.
#[test]
fn dois_volumes_prioridade_backup_cobertura_e_offline() {
    let t = arvore();
    let shm = Path::new("/dev/shm");
    if !shm.is_dir() || fs::metadata(shm).unwrap().dev() == fs::metadata(&t.raiz).unwrap().dev() {
        eprintln!("sem segundo filesystem; teste de dois volumes ignorado");
        return;
    }
    let hd = tempfile::Builder::new()
        .prefix("dag-hd-")
        .tempdir_in(shm)
        .unwrap();
    let hd_raiz = fs::canonicalize(hd.path()).unwrap();
    escrever(&hd_raiz.join("copia/dup1.bin"), &conteudo(b'a', 10_000));
    escrever(&hd_raiz.join("copia/so_no_hd.bin"), &conteudo(b'q', 3_000));
    let ssd = volume(&t.raiz, "SSD", "disco-ssd");
    let hdv = volume(&hd_raiz, "HD-Externo", "disco-hd");
    let mut a = Agente::abrir(&t.catalogo)
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![ssd.clone(), hdv]);
    let r = a.executar(scan(&[&t.raiz, &hd_raiz], false));
    let Resposta::ScanConcluido { duplicados, .. } = r else {
        panic!("{r:?}")
    };
    let d = duplicados.unwrap();
    // dup1 + dup2 (SSD) + dup1 (HD) = um grupo de 3 cópias em 2 volumes.
    assert_eq!((d.grupos, d.copias), (1, 3));

    // Prioridade SSD > HD-Externo: a Cópia Principal está no SSD.
    let r = a.executar(Comando::DefinirPrioridadeVolume {
        ordem: vec!["SSD".into(), "HD-Externo".into()],
    });
    assert!(matches!(r, Resposta::Ok { .. }), "{r:?}");
    let g = grupos(&mut a)[0].clone();
    let Resposta::Grupo { detalhe } = a.executar(Comando::DetalharGrupo { grupo: g.id }) else {
        panic!()
    };
    let principal = detalhe.copias.iter().find(|c| c.principal).unwrap();
    assert_eq!(principal.volume_rotulo.as_deref(), Some("SSD"));

    // Regra de Backup no HD-Externo: as cópias do SSD ganham Cobertura.
    marcacao(a.executar(Comando::MarcarBackup {
        alvo: Alvo::Volume("HD-Externo".into()),
    }));
    let Resposta::Grupo { detalhe } = a.executar(Comando::DetalharGrupo { grupo: g.id }) else {
        panic!()
    };
    for c in &detalhe.copias {
        let no_hd = c.volume_rotulo.as_deref() == Some("HD-Externo");
        assert_eq!(c.cobertura.coberto, !no_hd, "{c:?}");
    }

    // O HD é desconectado: a Cobertura continua, com o selo de data, e o membro fica offline.
    drop(a);
    let mut so_ssd = Agente::abrir(&t.catalogo)
        .unwrap()
        .silencioso()
        .com_volumes_fixos(vec![ssd]);
    let r = so_ssd.executar(Comando::ListarVolumes);
    assert!(matches!(r, Resposta::Volumes { .. }));
    let Resposta::Grupo { detalhe } = so_ssd.executar(Comando::DetalharGrupo { grupo: g.id })
    else {
        panic!()
    };
    let off = detalhe
        .copias
        .iter()
        .find(|c| c.volume_rotulo.as_deref() == Some("HD-Externo"))
        .unwrap();
    assert!(!off.conectado && off.visto_em.is_some());
    let ssd_copia = detalhe
        .copias
        .iter()
        .find(|c| c.volume_rotulo.as_deref() == Some("SSD"))
        .unwrap();
    assert!(ssd_copia.cobertura.coberto);
    assert!(ssd_copia.cobertura.nao_verificada_desde.is_some());
    // O arquivo só do HD segue sem Cobertura nem grupo; o grupo mantém as 3 cópias.
    assert_eq!(grupos(&mut so_ssd)[0].copias, 3);
    assert_eq!(grupos(&mut so_ssd)[0].copias_offline, 1);
}

/// CA-A18: a árvore e os tamanhos ficam navegáveis (leitor em outro "processo") antes de a etapa
/// de hash começar, e o hash completa depois.
#[test]
fn ca_a18_arvore_navegavel_antes_do_fim_do_hash() {
    use std::sync::{Arc, Mutex};
    let t = arvore();
    let visto: Arc<Mutex<Option<(usize, i64)>>> = Arc::new(Mutex::new(None));
    let (v2, caminho, raiz) = (
        Arc::clone(&visto),
        t.catalogo.clone(),
        t.raiz.to_str().unwrap().to_string(),
    );
    let mut a = agente(&t).com_progresso(move |p| {
        let dag_core::Progresso::Mensagem { texto } = p else {
            return;
        };
        let mut v = v2.lock().unwrap();
        if texto.contains("classificando") && v.is_none() {
            let mut leitor = Agente::abrir_leitura(&caminho).unwrap();
            let Resposta::Explorar { entradas, .. } = leitor.executar(Comando::Explorar {
                pasta: Some(raiz.clone()),
                limite: None,
                cursor: None,
            }) else {
                panic!("explorar falhou")
            };
            let hashes: i64 = leitor
                .catalogo()
                .armazem()
                .conexao()
                .query_row("SELECT COUNT(*) FROM hashes", [], |l| l.get(0))
                .unwrap();
            *v = Some((entradas.len(), hashes));
        }
    });
    let r = a.executar(scan(&[&t.raiz], false));
    assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    let (entradas, hashes_na_hora) = visto.lock().unwrap().expect("etapa de classificação vista");
    assert!(entradas >= 4, "árvore navegável: {entradas} entradas");
    assert_eq!(hashes_na_hora, 0, "o hash ainda não havia começado");
    let depois: i64 = a
        .catalogo()
        .armazem()
        .conexao()
        .query_row("SELECT COUNT(*) FROM hashes", [], |l| l.get(0))
        .unwrap();
    assert_eq!(depois, 8, "todo arquivo regular recebeu Hash Completo");
}
