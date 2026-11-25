//! Homologação controlada (B-24): leva UM livro aprovado da A Classificar para a Biblioteca
//! Homologada, só via `calibredb`.
//!
//! Este é o único caminho do crate que escreve na Homologada. Garantias:
//! 1. exige [`AprovacaoHomologacao`] (quem, quando, para qual hash) válida e recente;
//! 2. recusa se o livro já existe lá (identificador `dag:<hash>` ou callback de hashes do
//!    Catálogo);
//! 3. instantâneo (tamanho+mtime de todos os arquivos, hash dos metadados) antes e depois;
//! 4. se qualquer passo falhar depois da criação, tenta remover o livro criado (rollback);
//! 5. verifica depois, lendo `metadata.db` em `mode=ro`: um único livro com o `dag:`, formatos
//!    presentes no disco com o mesmo BLAKE3 dos originais;
//! 6. devolve [`RegistroHomologacao`], com o id criado, para `reverter`.
//!
//! Com o `calibre-server` servindo a Homologada o caminho local é recusado (`trava`); use
//! `Alvo::Servidor` com usuário e senha.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dag_biblio::LivroCalibre;
use serde::{Deserialize, Serialize};

use crate::calibredb::{
    Alvo, BibliotecaAClassificar, CamposMetadados, ClienteCalibredb, IDENTIFICADOR_DAG,
    IDENTIFICADOR_PENDENTE, interpretar_ids_adicionados, validar_distinta,
};
use crate::erro::{ErroCalibre, Resultado};
use crate::executor::ExecutorCalibre;
use crate::instantaneo::{Diferenca, Escopo, comparar, hash_arquivo, tirar};
use crate::leitura::BibliotecaLeitura;

/// Aprovação humana explícita para homologar um livro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AprovacaoHomologacao {
    /// Quem aprovou (não vazio).
    pub aprovado_por: String,
    /// Quando, em segundos desde a época Unix.
    pub aprovado_em_unix: u64,
    /// O aprovado vale para este hash BLAKE3 (e só para ele).
    pub hash_blake3: String,
    pub motivo: String,
}

/// Consulta ao Catálogo por hash.
pub type ConsultaHash = dyn Fn(&str) -> Option<String>;

/// Opções da homologação.
pub struct OpcoesHomologacao<'a> {
    /// Idade máxima da aprovação.
    pub validade: Duration,
    /// Relógio injetável (testes); `None` = agora.
    pub agora_unix: Option<u64>,
    /// Consulta ao Catálogo: dado um hash, devolve a descrição de um formato homologado
    /// idêntico, se houver.
    pub hash_ja_homologado: Option<&'a ConsultaHash>,
}

impl Default for OpcoesHomologacao<'_> {
    fn default() -> Self {
        Self {
            validade: Duration::from_secs(7 * 24 * 3600),
            agora_unix: None,
            hash_ja_homologado: None,
        }
    }
}

impl std::fmt::Debug for OpcoesHomologacao<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpcoesHomologacao")
            .field("validade", &self.validade)
            .field("agora_unix", &self.agora_unix)
            .finish_non_exhaustive()
    }
}

/// Formato conferido depois da escrita.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatoVerificado {
    pub formato: String,
    pub tamanho: u64,
    pub blake3: String,
}

/// O que a homologação fez, para auditoria e rollback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistroHomologacao {
    /// `books.id` criado na Homologada: o dado de rollback.
    pub id_criado: i64,
    pub hash_blake3: String,
    pub id_a_classificar: i64,
    pub aprovacao: AprovacaoHomologacao,
    pub formatos_verificados: Vec<FormatoVerificado>,
    /// Campos que não puderam ser levados (coluna ausente/diferente na Homologada).
    pub campos_ignorados: Vec<(String, String)>,
    /// Todas as mudanças entre os instantâneos antes e depois.
    pub mudancas: Vec<Diferenca>,
    /// Mudanças que NÃO são explicadas pela adição (deve ser vazio).
    pub anomalias: Vec<Diferenca>,
}

impl RegistroHomologacao {
    /// `true` se nada além do esperado mudou na Homologada.
    #[must_use]
    pub fn integro(&self) -> bool {
        self.anomalias.is_empty()
    }
}

fn agora(op: &OpcoesHomologacao<'_>) -> u64 {
    op.agora_unix.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    })
}

fn validar_aprovacao(
    a: &AprovacaoHomologacao,
    hash: &str,
    op: &OpcoesHomologacao<'_>,
) -> Resultado<()> {
    if a.aprovado_por.trim().is_empty() {
        return Err(ErroCalibre::AprovacaoInvalida(
            "falta quem aprovou".to_owned(),
        ));
    }
    if a.hash_blake3 != hash {
        return Err(ErroCalibre::AprovacaoInvalida(
            "a aprovação é de outro hash".to_owned(),
        ));
    }
    let n = agora(op);
    if a.aprovado_em_unix == 0 || a.aprovado_em_unix > n + 300 {
        return Err(ErroCalibre::AprovacaoInvalida(
            "data da aprovação ausente ou no futuro".to_owned(),
        ));
    }
    if n.saturating_sub(a.aprovado_em_unix) > op.validade.as_secs() {
        return Err(ErroCalibre::AprovacaoInvalida(
            "aprovação expirada".to_owned(),
        ));
    }
    Ok(())
}

fn hash_do_livro(l: &LivroCalibre) -> Option<&str> {
    l.identificadores
        .iter()
        .find(|(t, _)| t == IDENTIFICADOR_DAG)
        .map(|(_, v)| v.as_str())
}

fn explicada(d: &Diferenca, pasta_nova: &str) -> bool {
    let c = d.caminho();
    // Efeitos colaterais conhecidos de abrir a biblioteca com o Calibre (9.x grava
    // `last_expired_trash_at`, cria `full-text-search.db`, mexe em `.calnotes`).
    if c == "metadata.db"
        || c == "metadata_db_prefs_backup.json"
        || c.starts_with("full-text-search.db")
        || c.starts_with(".calnotes/")
        || c.starts_with(".caltrash/")
    {
        return !matches!(d, Diferenca::Removido { .. });
    }
    matches!(d, Diferenca::Adicionado { .. })
        && !pasta_nova.is_empty()
        && c.starts_with(&format!("{pasta_nova}/"))
}

/// Leva um livro aprovado da A Classificar para a Homologada.
///
/// `destino` é `Alvo::Local(caminho da Homologada)` ou `Alvo::Servidor(...)`; neste último caso
/// `homologada` ainda é necessário (leitura `mode=ro` e instantâneos).
///
/// # Errors
/// `AprovacaoInvalida`, `JaExisteNaHomologada`, `ServidorServindo`/`CalibreOcupado`,
/// `HomologacaoParcial` (com rollback tentado) e falhas do calibredb.
pub fn homologar<E: ExecutorCalibre>(
    cli: &ClienteCalibredb<E>,
    a_classificar: &BibliotecaAClassificar,
    id_a_classificar: i64,
    homologada: &Path,
    destino: &Alvo,
    aprovacao: &AprovacaoHomologacao,
    op: &OpcoesHomologacao<'_>,
) -> Resultado<RegistroHomologacao> {
    // Homologada e A Classificar precisam ser bibliotecas diferentes.
    let h = homologada
        .canonicalize()
        .map_err(|e| ErroCalibre::io(format!("canonicalizar {}", homologada.display()), e))?;
    validar_distinta(a_classificar.raiz(), &h)?;
    let alvo = match destino {
        Alvo::Local(p) => {
            let p = p
                .canonicalize()
                .map_err(|e| ErroCalibre::io(format!("canonicalizar {}", p.display()), e))?;
            if p != h {
                return Err(ErroCalibre::CaminhoProibido(
                    "o destino local não é a Homologada informada".into(),
                ));
            }
            Alvo::Local(h.clone())
        }
        Alvo::Servidor(s) => Alvo::Servidor(s.clone()),
    };

    // Livro de origem (leitura mode=ro da A Classificar).
    let origem = BibliotecaLeitura::abrir(a_classificar.raiz())?;
    let livro = origem
        .livro(id_a_classificar)?
        .ok_or_else(|| ErroCalibre::LivroInexistente(format!("id {id_a_classificar}")))?;
    let hash = hash_do_livro(&livro)
        .ok_or_else(|| {
            ErroCalibre::LivroInexistente(format!(
                "o livro {id_a_classificar} não tem identificador dag:"
            ))
        })?
        .to_owned();
    validar_aprovacao(aprovacao, &hash, op)?;
    if livro.formatos.is_empty() {
        return Err(ErroCalibre::LivroInexistente(
            "livro sem arquivos de formato".into(),
        ));
    }
    let mut hashes_origem = Vec::new();
    for f in &livro.formatos {
        let p = PathBuf::from(&f.caminho_absoluto);
        crate::calibredb::arquivo_valido(&p)?;
        hashes_origem.push((f.formato.clone(), hash_arquivo(&p)?));
    }

    // Já existe na Homologada?
    let destino_leitura = BibliotecaLeitura::abrir(&h)?;
    if !destino_leitura
        .livros_por_identificador(IDENTIFICADOR_DAG, &hash)?
        .is_empty()
    {
        return Err(ErroCalibre::JaExisteNaHomologada(format!(
            "identificador dag:{hash}"
        )));
    }
    if let Some(f) = op.hash_ja_homologado {
        for (_, hf) in &hashes_origem {
            if let Some(desc) = f(hf) {
                return Err(ErroCalibre::JaExisteNaHomologada(desc));
            }
        }
    }

    // Campos: só o que existe na Homologada com o mesmo tipo.
    let colunas_dest = destino_leitura.colunas_personalizadas().to_vec();
    let colunas_orig = origem.colunas_personalizadas().to_vec();
    let mut campos = CamposMetadados::de_livro(&livro, &colunas_orig);
    let mut ignorados = Vec::new();
    campos.personalizados.retain(
        |p| match colunas_dest.iter().find(|c| c.rotulo == p.rotulo) {
            Some(c) if c.tipo == p.tipo && c.materializada => true,
            Some(_) => {
                ignorados.push((
                    format!("#{}", p.rotulo),
                    "tipo diferente na Homologada".into(),
                ));
                false
            }
            None => {
                ignorados.push((
                    format!("#{}", p.rotulo),
                    "coluna ausente na Homologada".into(),
                ));
                false
            }
        },
    );
    drop(destino_leitura);
    drop(origem);

    // Escrita: antes, executa, depois.
    let antes = tirar(&h, Escopo::Listagem, None)?;
    let mut args = vec![
        "--duplicates".to_owned(),
        "-I".into(),
        format!("{IDENTIFICADOR_DAG}:{hash}"),
        "-I".into(),
        format!("{IDENTIFICADOR_PENDENTE}:1"),
    ];
    if livro.tem_capa {
        let capa = a_classificar.raiz().join(&livro.caminho).join("cover.jpg");
        if capa.is_file() {
            args.push("--cover".into());
            args.push(capa.to_string_lossy().into_owned());
        }
    }
    args.push(livro.formatos[0].caminho_absoluto.clone());
    let id = match cli
        .rodar_h(&alvo, "add", &args, true)
        .and_then(|s| interpretar_ids_adicionados(&s.stdout))
    {
        Ok(id) => id,
        // O `add` pode ter criado o livro e falhado depois (timeout, saída ilegível): procura o
        // livro com o nosso `dag:` e a marca de pendência e desfaz, em vez de deixá-lo órfão.
        Err(e) => return Err(desfazer_add_interrompido(cli, &alvo, &h, &hash, e)),
    };

    let continuar = || -> Resultado<()> {
        for f in livro.formatos.iter().skip(1) {
            cli.rodar_h(
                &alvo,
                "add_format",
                &[id.to_string(), f.caminho_absoluto.clone()],
                true,
            )?;
        }
        let mut a = vec![id.to_string()];
        a.extend(campos.argumentos()?);
        cli.rodar_h(&alvo, "set_metadata", &a, true)?;
        Ok(())
    };
    if let Err(e) = continuar() {
        return Err(reverter_apos_falha(cli, &alvo, id, &e.to_string()));
    }

    // Verificação depois (leitura mode=ro nova).
    let verif = (|| -> Resultado<Vec<FormatoVerificado>> {
        let dep = BibliotecaLeitura::abrir(&h)?;
        let ids = dep.livros_por_identificador(IDENTIFICADOR_DAG, &hash)?;
        if ids != [id] {
            return Err(ErroCalibre::VerificacaoFalhou(format!(
                "esperava só o livro {id} com dag:{hash}, achei {ids:?}"
            )));
        }
        let novo = dep
            .livro(id)?
            .ok_or_else(|| ErroCalibre::VerificacaoFalhou("livro criado sumiu".into()))?;
        if novo
            .identificadores
            .iter()
            .any(|(t, _)| t == IDENTIFICADOR_PENDENTE)
        {
            return Err(ErroCalibre::VerificacaoFalhou(
                "marca de pendência não foi removida".into(),
            ));
        }
        let mut out = Vec::new();
        for (formato, hash_o) in &hashes_origem {
            let f = novo
                .formatos
                .iter()
                .find(|f| &f.formato == formato)
                .ok_or_else(|| {
                    ErroCalibre::VerificacaoFalhou(format!("formato {formato} não foi gravado"))
                })?;
            let h2 = hash_arquivo(Path::new(&f.caminho_absoluto))?;
            if &h2 != hash_o {
                return Err(ErroCalibre::VerificacaoFalhou(format!(
                    "BLAKE3 do {formato} difere do original"
                )));
            }
            out.push(FormatoVerificado {
                formato: formato.clone(),
                tamanho: f.tamanho,
                blake3: h2,
            });
        }
        Ok(out)
    })();
    let formatos_verificados = match verif {
        Ok(v) => v,
        Err(e) => return Err(reverter_apos_falha(cli, &alvo, id, &e.to_string())),
    };

    let depois = tirar(&h, Escopo::Listagem, None)?;
    let mudancas = comparar(&antes, &depois);
    let pasta_nova = BibliotecaLeitura::abrir(&h)?
        .livro(id)?
        .map(|l| l.caminho)
        .unwrap_or_default();
    // A pasta do autor também é nova; considera-se a pasta do livro e a do autor.
    let pasta_autor = pasta_nova.split('/').next().unwrap_or("").to_owned();
    let anomalias = mudancas
        .iter()
        .filter(|d| !explicada(d, &pasta_nova) && !explicada(d, &pasta_autor))
        .cloned()
        .collect();

    Ok(RegistroHomologacao {
        id_criado: id,
        hash_blake3: hash,
        id_a_classificar,
        aprovacao: aprovacao.clone(),
        formatos_verificados,
        campos_ignorados: ignorados,
        mudancas,
        anomalias,
    })
}

/// Depois de um `add` que falhou sem devolver id: lê a Homologada em `mode=ro`; se houver
/// exatamente um livro com `dag:<hash>` ainda marcado como pendente, ele foi criado por este
/// `add` (a pré-checagem garantiu que não havia nenhum `dag:<hash>` antes) e é removido.
fn desfazer_add_interrompido<E: ExecutorCalibre>(
    cli: &ClienteCalibredb<E>,
    alvo: &Alvo,
    homologada: &Path,
    hash: &str,
    erro: ErroCalibre,
) -> ErroCalibre {
    let achados = BibliotecaLeitura::abrir(homologada).and_then(|b| {
        let ids = b.livros_por_identificador(IDENTIFICADOR_DAG, hash)?;
        let mut pendentes = Vec::new();
        for id in ids {
            if b.livro(id)?.is_some_and(|l| {
                l.identificadores
                    .iter()
                    .any(|(t, _)| t == IDENTIFICADOR_PENDENTE)
            }) {
                pendentes.push(id);
            }
        }
        Ok(pendentes)
    });
    match achados {
        Ok(ids) if ids.is_empty() => erro,
        Ok(ids) if ids.len() == 1 => {
            reverter_apos_falha(cli, alvo, ids[0], &format!("add interrompido: {erro}"))
        }
        Ok(ids) => ErroCalibre::HomologacaoParcial {
            id_criado: ids[0],
            situacao: format!("NÃO revertido: vários livros pendentes com dag:{hash} ({ids:?})"),
            detalhe: erro.to_string(),
        },
        Err(e2) => ErroCalibre::HomologacaoParcial {
            id_criado: 0,
            situacao: format!("não deu para conferir a Homologada depois da falha ({e2})"),
            detalhe: erro.to_string(),
        },
    }
}

fn reverter_apos_falha<E: ExecutorCalibre>(
    cli: &ClienteCalibredb<E>,
    alvo: &Alvo,
    id: i64,
    motivo: &str,
) -> ErroCalibre {
    let r = cli.rodar_h(
        alvo,
        "remove",
        &["--permanent".into(), id.to_string()],
        true,
    );
    ErroCalibre::HomologacaoParcial {
        id_criado: id,
        situacao: match r {
            Ok(_) => "revertido (removido)".to_owned(),
            Err(e) => format!("NÃO revertido, remova à mão ({e})"),
        },
        detalhe: motivo.to_owned(),
    }
}

/// Desfaz uma homologação: remove o livro criado, depois de conferir que ele ainda tem o
/// `dag:` do registro (nunca remove um livro que não seja o criado).
///
/// # Errors
/// `VerificacaoFalhou` se o id não corresponder ao registro.
pub fn reverter_homologacao<E: ExecutorCalibre>(
    cli: &ClienteCalibredb<E>,
    homologada: &Path,
    destino: &Alvo,
    registro: &RegistroHomologacao,
) -> Resultado<()> {
    let h = homologada
        .canonicalize()
        .map_err(|e| ErroCalibre::io(format!("canonicalizar {}", homologada.display()), e))?;
    let leitura = BibliotecaLeitura::abrir(&h)?;
    let ids = leitura.livros_por_identificador(IDENTIFICADOR_DAG, &registro.hash_blake3)?;
    if ids != [registro.id_criado] {
        return Err(ErroCalibre::VerificacaoFalhou(format!(
            "o livro {} não corresponde mais ao dag:{} (achei {ids:?})",
            registro.id_criado, registro.hash_blake3
        )));
    }
    drop(leitura);
    let alvo = match destino {
        Alvo::Local(_) => Alvo::Local(h),
        Alvo::Servidor(s) => Alvo::Servidor(s.clone()),
    };
    cli.rodar_h(
        &alvo,
        "remove",
        &["--permanent".into(), registro.id_criado.to_string()],
        true,
    )?;
    Ok(())
}
