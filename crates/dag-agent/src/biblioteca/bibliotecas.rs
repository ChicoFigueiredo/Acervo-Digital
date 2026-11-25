//! Registro, perfil e sincronização das bibliotecas Calibre. A Homologada é lida com
//! `mode=ro` (nunca `immutable`); nada aqui escreve nela.

use std::path::Path;
use std::time::Instant;

use dag_biblio::PapelBiblioteca;
use dag_calibre::instantaneo::{self, Escopo};
use dag_calibre::leitura::BibliotecaLeitura;
use dag_calibre::perfil::ler_perfil;
use dag_core::{
    BibliotecaInfo, CampoInfo, DadosBiblioteca, PapelRegistro, PerfilResumo, Resposta,
    ResumoSincronizacao,
};
use dag_store::biblioteca as store;

use super::{
    CHAVE_CONFIG, ConfigBiblioteca, dados, hash_de_hex, hex, homologada_ativa, info_biblioteca,
    resolver_biblioteca,
};
use crate::agente::Agente;
use crate::erro::ErroAgente;

const FATIA: usize = 500;

fn papel_store(p: PapelRegistro) -> PapelBiblioteca {
    match p {
        PapelRegistro::Homologada => PapelBiblioteca::Homologada,
        PapelRegistro::AClassificar => PapelBiblioteca::AClassificar,
    }
}

impl Agente {
    pub(super) fn registrar_biblioteca(
        &mut self,
        caminho: &str,
        papel: PapelRegistro,
        nome: Option<&str>,
    ) -> Result<Resposta, ErroAgente> {
        let cfg = self.config_biblioteca();
        let agora = self.agora_biblio();
        let bruto = Path::new(caminho);
        let mut avisos = Vec::new();
        let mut campos_perfil: Option<Vec<store::CampoCalibre>> = None;
        let canon = match papel {
            PapelRegistro::Homologada => {
                let c = std::fs::canonicalize(bruto).map_err(|e| {
                    ErroAgente::biblio("biblioteca_inacessivel", format!("{caminho}: {e}"))
                })?;
                // Abre só para leitura: valida que é uma biblioteca Calibre.
                let leitura = BibliotecaLeitura::abrir(&c)?;
                let uuid = leitura.library_id()?;
                let _ = uuid;
                c
            }
            PapelRegistro::AClassificar => {
                let homologada = homologada_ativa(self.conexao())?.ok_or_else(|| {
                    ErroAgente::biblio(
                        "sem_homologada",
                        "registre a Biblioteca Homologada antes da A Classificar",
                    )
                })?;
                let destino = if caminho.is_empty() {
                    Path::new(&cfg.destino_a_classificar)
                } else {
                    bruto
                };
                let h = Path::new(&homologada.caminho);
                let existe = destino.join("metadata.db").is_file();
                // Perfil da Homologada lido em `mode=ro`: nunca `calibredb clone`.
                let perfil = ler_perfil(h)?;
                let raiz = if existe {
                    let b = dag_calibre::calibredb::BibliotecaAClassificar::abrir(destino, h)?;
                    // Idempotente: só chama o calibredb se faltar coluna do perfil.
                    let tem = BibliotecaLeitura::abrir(b.raiz())?
                        .colunas_personalizadas()
                        .iter()
                        .map(|c| c.rotulo.clone())
                        .collect::<Vec<_>>();
                    let falta = perfil.colunas.iter().any(|c| !tem.contains(&c.rotulo));
                    if falta {
                        let cli = self.cliente_calibre(&cfg, Some(h))?;
                        cli.garantir_colunas_dag(&b)?;
                        let rel = cli.clonar_estrutura(&b, &perfil)?;
                        avisos.extend(avisos_do_relatorio(&rel));
                    }
                    b.raiz().to_path_buf()
                } else {
                    let cli = self.cliente_calibre(&cfg, Some(h))?;
                    let b = cli.criar_a_classificar(destino, h)?;
                    cli.garantir_colunas_dag(&b)?;
                    avisos.push("A Classificar criada com `calibredb`".into());
                    let rel = cli.clonar_estrutura(&b, &perfil)?;
                    avisos.extend(avisos_do_relatorio(&rel));
                    b.raiz().to_path_buf()
                };
                campos_perfil = Some(campos_do_perfil(&perfil));
                raiz
            }
        };
        let (uuid, versao) = match papel {
            PapelRegistro::Homologada => {
                let l = BibliotecaLeitura::abrir(&canon)?;
                (l.library_id()?, None)
            }
            PapelRegistro::AClassificar => (BibliotecaLeitura::abrir(&canon)?.library_id()?, None),
        };
        let caminho_txt = canon.to_string_lossy().into_owned();
        let nome_final = nome.map(str::to_owned).unwrap_or_else(|| {
            canon
                .file_name()
                .map_or_else(|| "biblioteca".into(), |n| n.to_string_lossy().into_owned())
        });
        let conn = self.conexao();
        let ja = store::biblioteca_por_caminho(conn, &caminho_txt)?.is_some();
        let id = store::registrar_biblioteca(
            conn,
            &store::NovaBiblioteca {
                caminho: &caminho_txt,
                papel: papel_store(papel),
                nome: &nome_final,
                library_uuid: uuid.as_deref(),
                versao_calibre: versao,
            },
            agora,
        )?;
        if let Some(campos) = &campos_perfil {
            store::gravar_campos(conn, id, campos)?;
        }
        let b = store::obter_biblioteca(conn, id)?
            .ok_or_else(|| ErroAgente::biblio("biblioteca_nao_registrada", "sumiu"))?;
        Ok(dados(DadosBiblioteca::Registrada {
            biblioteca: info_biblioteca(conn, &b)?,
            criada: !ja,
            avisos,
        }))
    }

    pub(super) fn listar_bibliotecas_cmd(&mut self) -> Result<Resposta, ErroAgente> {
        let conn = self.conexao();
        let itens = store::listar_bibliotecas(conn, None)?
            .iter()
            .map(|b| info_biblioteca(conn, b))
            .collect::<Result<Vec<BibliotecaInfo>, _>>()?;
        Ok(dados(DadosBiblioteca::Bibliotecas { itens }))
    }

    pub(super) fn perfil_biblioteca(&mut self, ref_: &str) -> Result<Resposta, ErroAgente> {
        let conn = self.conexao();
        let b = resolver_biblioteca(conn, ref_)?;
        let perfil = ler_perfil(Path::new(&b.caminho))?;
        let campos_gravados = store::listar_campos(conn, b.id)?;
        let campos: Vec<CampoInfo> = campos_gravados
            .into_iter()
            .map(|c| CampoInfo {
                rotulo: c.rotulo,
                nome: c.nome,
                datatype: c.datatype,
                multiplo: c.multiplo,
                clonavel: c.clonavel,
                motivo: c.motivo,
            })
            .collect();
        let nao_clonaveis: Vec<String> = perfil
            .compatibilidade
            .nao_clonaveis()
            .iter()
            .map(|i| i.elemento.clone())
            .collect();
        let resumo = PerfilResumo {
            versao_esquema: Some(perfil.versao_esquema),
            total_livros: perfil.total_livros,
            total_autores: perfil.total_autores,
            total_tags: perfil.total_tags,
            total_series: perfil.total_series,
            total_editoras: perfil.total_editoras,
            total_arquivos_formato: perfil.total_arquivos_formato,
            formatos: perfil
                .vocabulario
                .formatos
                .iter()
                .map(|f| (f.formato.clone(), f.contagem))
                .collect(),
            tags_principais: perfil
                .vocabulario
                .tags
                .iter()
                .take(30)
                .map(|t| (t.nome.clone(), t.contagem))
                .collect(),
            candidatos_estante: perfil
                .candidatos_estante
                .iter()
                .map(|c| (c.nome.clone(), c.pontuacao, c.razoes.clone()))
                .collect(),
            nao_clonaveis,
            perfil_bruto: serde_json::to_value(&perfil).unwrap_or_default(),
        };
        Ok(dados(DadosBiblioteca::Perfil {
            biblioteca: info_biblioteca(conn, &b)?,
            perfil: resumo,
            campos,
        }))
    }

    /// Lê a Homologada (só leitura), grava o espelho e liga/importa os hashes do Catálogo. O
    /// instantâneo antes/depois prova que a Homologada não mudou (CA-B01).
    pub(super) fn sincronizar_biblioteca(&mut self, ref_: &str) -> Result<Resposta, ErroAgente> {
        let ini = Instant::now();
        let agora = self.agora_biblio();
        let b = resolver_biblioteca(self.conexao(), ref_)?;
        if b.papel != PapelBiblioteca::Homologada {
            return Err(ErroAgente::biblio(
                "papel_invalido",
                "só a Biblioteca Homologada é sincronizada no espelho",
            ));
        }
        let raiz = Path::new(&b.caminho).to_path_buf();
        // Instantâneo antes: metadata.db e backup de preferências com hash, mais a listagem.
        let antes = instantaneo::tirar(&raiz, Escopo::Listagem, None)?;
        let leitura = BibliotecaLeitura::abrir(&raiz)?;
        let colunas = leitura.colunas_personalizadas().to_vec();
        let estante_col = colunas
            .iter()
            .find(|c| c.rotulo == "pasta" || c.nome.to_lowercase().contains("pasta"))
            .map(|c| c.rotulo.clone());
        self.mensagem("biblioteca: lendo a Homologada (somente leitura)...");
        let conn = self.conexao();
        let tx = conn.unchecked_transaction()?;
        let geracao = store::iniciar_sincronizacao(&tx, b.id)?;
        let mut lote = Vec::with_capacity(FATIA);
        let (mut total, mut formatos) = (0_u64, 0_u64);
        for livro in leitura.livros(FATIA) {
            let l = livro?;
            let estante = estante_col.as_ref().and_then(|r| {
                l.personalizados
                    .iter()
                    .find(|(k, _)| k == r)
                    .and_then(|(_, v)| v.as_str().map(str::to_owned))
            });
            let mut e = store::LivroEspelho::de_calibre(&l, estante);
            // Caminho absoluto dos formatos (a Homologada fica onde está registrada).
            for f in &mut e.formatos {
                if !f.caminho.starts_with('/') {
                    f.caminho = raiz.join(&f.caminho).to_string_lossy().into_owned();
                }
                if let Ok(m) = std::fs::metadata(&f.caminho) {
                    use std::os::unix::fs::MetadataExt;
                    f.mtime_ns = Some(m.mtime() * 1_000_000_000 + m.mtime_nsec());
                }
            }
            formatos += e.formatos.len() as u64;
            total += 1;
            lote.push(e);
            if lote.len() == FATIA {
                store::gravar_lote(&tx, b.id, geracao, &lote, agora)?;
                lote.clear();
            }
        }
        if !lote.is_empty() {
            store::gravar_lote(&tx, b.id, geracao, &lote, agora)?;
        }
        let removidos = store::remover_ausentes(&tx, b.id, geracao)?;
        tx.commit()?;
        // Liga formatos ao Catálogo e importa hashes (índices, em fatias).
        let conn = self.conexao();
        let mut ligados = 0_u64;
        let volume = store::volume_do_caminho(conn, b.caminho.as_bytes())?;
        if let Some(v) = volume {
            let mut apos = 0;
            loop {
                let (n, ult) = store::ligar_formatos_por_caminho(conn, b.id, v, "", apos, FATIA)?;
                ligados += n;
                match ult {
                    Some(u) => apos = u,
                    None => break,
                }
            }
        }
        let mut importados = 0_u64;
        let mut apos = 0;
        loop {
            let (n, ult) = store::importar_hashes_do_catalogo(conn, b.id, apos, FATIA, agora)?;
            importados += n;
            match ult {
                Some(u) => apos = u,
                None => break,
            }
        }
        // O que ainda não tem hash: o DAG lê (só leitura) e calcula.
        let mut calculados = 0_u64;
        let mut apos = 0;
        loop {
            let fatia = store::formatos_sem_hash(conn, b.id, apos, FATIA)?;
            if fatia.is_empty() {
                break;
            }
            for f in &fatia {
                apos = f.id;
                if !self.seguir_biblioteca() {
                    break;
                }
                let conn = self.conexao();
                if let Ok(h) = super::blake3_arquivo(Path::new(&f.caminho))
                    && store::gravar_hash_formato(conn, f.id, &h, f.tamanho, agora)?
                {
                    calculados += 1;
                }
            }
            if !self.seguir_biblioteca() {
                break;
            }
        }
        let conn = self.conexao();
        let sem_hash = store::formatos_sem_hash(conn, b.id, 0, 1_000_000)?.len() as u64;
        let sem_arquivo: i64 = conn.query_row(
            "SELECT COUNT(*) FROM formatos_homologados WHERE biblioteca_id = ?1 AND file_id IS NULL",
            [b.id],
            |l| l.get(0),
        )?;
        // Instantâneo depois e comparação com o antes.
        let depois = instantaneo::tirar(&raiz, Escopo::Listagem, Some(&antes))?;
        let difs = instantaneo::comparar(&antes, &depois);
        let intacta = difs.is_empty();
        let hash_db = depois
            .arquivos
            .get("metadata.db")
            .and_then(|a| a.blake3.as_deref())
            .and_then(hash_de_hex);
        let snap = store::gravar_instantaneo(
            conn,
            b.id,
            &store::NovoInstantaneo {
                quando: agora,
                hash_metadata_db: hash_db,
                tamanho_metadata_db: depois.arquivos.get("metadata.db").map(|a| a.tamanho),
                livros: total,
                formatos,
                manifesto_json: None,
                resultado: None,
                comparacao_json: serde_json::to_string(&difs).ok(),
            },
        )?;
        if snap.resultado == store::ResultadoComparacao::Alterado {
            let _ = store::emitir_alerta(
                conn,
                &store::NovoAlerta {
                    chave: Some(format!(
                        "biblioteca_alterada:{}:{}",
                        b.id,
                        hash_db.map(|h| hex(&h)).unwrap_or_default()
                    )),
                    tipo: store::TipoAlerta::BibliotecaAlterada,
                    file_id: None,
                    dados_json: serde_json::json!({"biblioteca": b.id}).to_string(),
                },
                agora,
            );
        }
        // O vigia começa daqui (se ainda não começou).
        if dag_store::configuracao::ler(conn, super::watcher::CHAVE_MARCA)
            .ok()
            .flatten()
            .is_none()
        {
            let ult = store::ultimo_evento_id(conn)?;
            let _ = dag_store::configuracao::gravar(
                conn,
                super::watcher::CHAVE_MARCA,
                &ult.to_string(),
            );
        }
        Ok(dados(DadosBiblioteca::Sincronizada {
            resumo: ResumoSincronizacao {
                biblioteca: b.id,
                livros: total,
                removidos,
                formatos,
                formatos_ligados_ao_catalogo: ligados,
                hashes_importados: importados,
                hashes_calculados: calculados,
                formatos_sem_arquivo_no_catalogo: u64::try_from(sem_arquivo).unwrap_or(0),
                formatos_sem_hash: sem_hash,
                comparacao_metadata_db: format!("{:?}", snap.resultado).to_lowercase(),
                biblioteca_intacta: intacta,
                diferencas: difs.iter().map(|d| d.caminho().to_owned()).collect(),
                duracao_ms: u64::try_from(ini.elapsed().as_millis()).unwrap_or(u64::MAX),
            },
        }))
    }

    /// Merge-patch da configuração (campos ausentes ficam como estão), validando o resultado.
    pub(super) fn definir_config(
        &mut self,
        alt: &serde_json::Value,
    ) -> Result<Resposta, ErroAgente> {
        // A senha do servidor entra aqui (`homologacao.senha`), vai para o Cofre e é tirada do
        // patch antes de qualquer mesclagem, validação ou mensagem de erro: nunca é gravada no
        // JSON da configuração nem volta em resposta.
        let mut alt = alt.clone();
        let mut senha: Option<String> = None;
        if let Some(h) = alt
            .get_mut("homologacao")
            .and_then(serde_json::Value::as_object_mut)
        {
            if let Some(v) = h.remove("senha") {
                senha = Some(v.as_str().map(str::to_owned).unwrap_or_default());
            }
            h.remove("senha_no_cofre");
        }
        let atual = self.config_biblioteca();
        let mut v =
            serde_json::to_value(&atual).map_err(|e| ErroAgente::Invalido(e.to_string()))?;
        mesclar(&mut v, &alt);
        let mut nova: ConfigBiblioteca = serde_json::from_value(v)
            .map_err(|e| ErroAgente::biblio("config_invalida", e.to_string()))?;
        if nova.ia.ativa && !endereco_local(&nova.ia.endpoint) {
            return Err(ErroAgente::biblio(
                "config_invalida",
                "a IA só pode usar um endpoint local (loopback)",
            ));
        }
        if !nova.aging_por_hora.is_finite() || nova.aging_por_hora < 0.0 {
            return Err(ErroAgente::biblio(
                "config_invalida",
                "aging_por_hora precisa ser um número maior ou igual a zero",
            ));
        }
        if let Some(s) = &senha {
            self.guardar_senha_servidor(s)?;
        }
        nova.homologacao.senha_no_cofre = false; // derivado: nunca persistido
        let txt = serde_json::to_string(&nova).map_err(|e| ErroAgente::Invalido(e.to_string()))?;
        dag_store::configuracao::gravar(self.conexao(), CHAVE_CONFIG, &txt)?;
        Ok(dados(DadosBiblioteca::Config {
            config: self.config_biblioteca_publica(),
        }))
    }
}

/// Relatório de compatibilidade da clonagem em linhas de aviso (nada some em silêncio).
fn avisos_do_relatorio(rel: &dag_calibre::calibredb::RelatorioClone) -> Vec<String> {
    let mut v = Vec::new();
    if !rel.criadas.is_empty() {
        v.push(format!("colunas criadas: {}", rel.criadas.join(", ")));
    }
    for (e, m) in &rel.nao_clonadas {
        v.push(format!("não clonado: {e} ({m})"));
    }
    v.extend(rel.avisos.iter().map(|a| format!("aviso de clonagem: {a}")));
    v.extend(
        rel.divergencias
            .iter()
            .map(|d| format!("divergência após clonar: {d}")),
    );
    v
}

/// Campos da Homologada com o veredito de clonagem; o que não é coluna (biblioteca virtual,
/// categoria de usuário...) entra com o próprio elemento como rótulo.
fn campos_do_perfil(perfil: &dag_calibre::perfil::PerfilBiblioteca) -> Vec<store::CampoCalibre> {
    let mut v = Vec::new();
    for c in &perfil.colunas {
        let status = perfil
            .compatibilidade
            .status_de(&format!("coluna #{}", c.rotulo))
            .cloned();
        let (clonavel, motivo) = match status {
            Some(dag_calibre::perfil::StatusClone::NaoClonavel { motivos }) => {
                (false, Some(motivos.join("; ")))
            }
            Some(dag_calibre::perfil::StatusClone::ClonavelComAviso { avisos }) => {
                (true, Some(avisos.join("; ")))
            }
            _ => (true, None),
        };
        v.push(store::CampoCalibre {
            rotulo: c.rotulo.clone(),
            nome: c.nome.clone(),
            datatype: c.tipo.nome_calibre().to_owned(),
            multiplo: c.e_multipla,
            clonavel,
            motivo,
        });
    }
    for i in perfil.compatibilidade.nao_clonaveis() {
        if i.elemento.starts_with("coluna #") {
            continue;
        }
        if let dag_calibre::perfil::StatusClone::NaoClonavel { motivos } = &i.status {
            v.push(store::CampoCalibre {
                rotulo: i.elemento.clone(),
                nome: i.elemento.clone(),
                datatype: "n/a".into(),
                multiplo: false,
                clonavel: false,
                motivo: Some(motivos.join("; ")),
            });
        }
    }
    v
}

fn endereco_local(e: &str) -> bool {
    let sem = e
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    sem.starts_with("127.0.0.1") || sem.starts_with("localhost") || sem.starts_with("[::1]")
}

fn mesclar(base: &mut serde_json::Value, patch: &serde_json::Value) {
    use serde_json::Value;
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(k) {
                    Some(x) if v.is_object() => mesclar(x, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, p) => *b = p.clone(),
    }
}
