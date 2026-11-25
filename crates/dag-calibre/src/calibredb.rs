//! Wrapper do `calibredb` (B-02, B-13): criação da A Classificar, clone de colunas, adição
//! idempotente, metadados, listagem e rollback.
//!
//! Regras:
//! - Sempre `--library-path` explícito; nunca a biblioteca padrão do usuário.
//! - Só existe API pública para a [`BibliotecaAClassificar`], cujo construtor recusa o caminho
//!   (canônico, aninhado ou mesmo inode de `metadata.db`) e o `library_id` da Homologada.
//! - Nunca se roda `calibredb` contra a Homologada fora do módulo `homologacao`: até um
//!   `list` pode gravar (backup de preferências, `.calnotes`, migração de esquema).
//! - `add` usa `--duplicates` porque a detecção do Calibre (título+autores) rejeitaria
//!   silenciosamente edições/volumes legítimos com o mesmo título. A idempotência vem do
//!   identificador `dag:<blake3>`, procurado antes de cada adição e gravado no próprio `add`.
//! - Senha de servidor só trafega por stdin (`--password <f:/dev/stdin>`), nunca em argv/log.

use std::collections::BTreeMap;
use std::fmt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dag_biblio::LivroCalibre;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::erro::{ErroCalibre, Resultado};
use crate::executor::{Comando, ExecutorCalibre, SaidaProcesso};
use crate::leitura::{BibliotecaLeitura, ColunaPersonalizada, TipoColuna};
use crate::perfil::{PerfilBiblioteca, StatusClone};
use crate::trava::{PoliticaTrava, ProcessoCalibre, processos_calibre, verificar_escrita_local};

/// Tipo do identificador que liga um livro do Calibre ao hash BLAKE3 do DAG.
pub const IDENTIFICADOR_DAG: &str = "dag";
/// Coluna personalizada (`#dag_hash`, text) com o BLAKE3 do arquivo, para rastreabilidade.
pub const COLUNA_DAG_HASH: &str = "dag_hash";
/// Coluna personalizada (`#dag_confianca`, float) com a confiança da classificação.
pub const COLUNA_DAG_CONFIANCA: &str = "dag_confianca";
/// Marca de livro adicionado cujos metadados ainda não foram completados (retomada de falha).
pub const IDENTIFICADOR_PENDENTE: &str = "dagpend";

/// Senha que nunca aparece em `Debug`.
#[derive(Clone)]
pub struct Segredo(String);

impl Segredo {
    #[must_use]
    pub fn novo(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl fmt::Debug for Segredo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Segredo(<oculto>)")
    }
}

/// Servidor de conteúdo do Calibre (`calibre-server`) como via de acesso.
#[derive(Debug, Clone)]
pub struct AcessoServidor {
    /// Ex.: `http://127.0.0.1:17790`.
    pub url: String,
    /// `library_id` do servidor (o que vai depois do `#`).
    pub id_biblioteca: String,
    pub usuario: String,
    pub senha: Segredo,
}

/// Onde o `calibredb` age.
#[derive(Debug, Clone)]
pub enum Alvo {
    Local(PathBuf),
    Servidor(AcessoServidor),
}

/// Versão do `calibredb`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersaoCalibre {
    pub texto: String,
    pub maior: u32,
    pub menor: u32,
    pub patch: u32,
}

/// Interpreta `calibredb (calibre 7.6)` ou `... 9.14.0)`.
///
/// # Errors
/// Se não houver número de versão.
pub fn interpretar_versao(saida: &str) -> Resultado<VersaoCalibre> {
    let i = saida
        .find("calibre ")
        .ok_or_else(|| ErroCalibre::SaidaInvalida(format!("versão: {saida:?}")))?;
    let resto = &saida[i + "calibre ".len()..];
    let texto: String = resto
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut p = texto.split('.').map(|x| x.parse::<u32>().ok());
    let maior = p.next().flatten();
    let menor = p.next().flatten().unwrap_or(0);
    let patch = p.next().flatten().unwrap_or(0);
    match maior {
        Some(maior) => Ok(VersaoCalibre {
            texto,
            maior,
            menor,
            patch,
        }),
        None => Err(ErroCalibre::SaidaInvalida(format!("versão: {saida:?}"))),
    }
}

/// Biblioteca A Classificar: provadamente distinta da Homologada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibliotecaAClassificar {
    raiz: PathBuf,
    library_id: String,
}

impl BibliotecaAClassificar {
    /// Raiz canônica.
    #[must_use]
    pub fn raiz(&self) -> &Path {
        &self.raiz
    }

    /// `library_id` (UUID) da A Classificar.
    #[must_use]
    pub fn library_id(&self) -> &str {
        &self.library_id
    }

    /// Reabre uma A Classificar existente, repetindo todas as recusas.
    ///
    /// # Errors
    /// `CaminhoProibido` se for/estiver dentro da Homologada ou compartilhar `library_id`.
    pub fn abrir(destino: &Path, homologada: &Path) -> Resultado<Self> {
        let raiz = validar_distinta(destino, homologada)?;
        let leitura = BibliotecaLeitura::abrir(&raiz)?;
        let id = leitura.library_id()?.ok_or_else(|| {
            ErroCalibre::EsquemaInesperado("biblioteca sem library_id".to_owned())
        })?;
        confirmar_library_id_distinto(&id, homologada)?;
        Ok(Self {
            raiz,
            library_id: id,
        })
    }
}

fn canonico_ou_futuro(p: &Path) -> Resultado<PathBuf> {
    if p.exists() {
        return p
            .canonicalize()
            .map_err(|e| ErroCalibre::io(format!("canonicalizar {}", p.display()), e));
    }
    let mut resto = Vec::new();
    let mut atual = p.to_path_buf();
    while !atual.exists() {
        let Some(nome) = atual.file_name().map(std::ffi::OsStr::to_owned) else {
            return Err(ErroCalibre::CaminhoProibido(format!(
                "caminho sem ancestral existente: {}",
                p.display()
            )));
        };
        resto.push(nome);
        atual = match atual.parent() {
            Some(pai) if !pai.as_os_str().is_empty() => pai.to_path_buf(),
            _ => PathBuf::from("."),
        };
    }
    let mut base = atual
        .canonicalize()
        .map_err(|e| ErroCalibre::io(format!("canonicalizar {}", atual.display()), e))?;
    for n in resto.into_iter().rev() {
        base.push(n);
    }
    Ok(base)
}

/// Garante que `candidato` não é, não contém, não está dentro de nem aponta (mesmo inode do
/// `metadata.db`) para a Homologada. Devolve o caminho canônico do candidato.
pub(crate) fn validar_distinta(candidato: &Path, homologada: &Path) -> Resultado<PathBuf> {
    if !homologada.join("metadata.db").is_file() {
        return Err(ErroCalibre::BibliotecaInexistente(homologada.to_path_buf()));
    }
    let h = homologada
        .canonicalize()
        .map_err(|e| ErroCalibre::io(format!("canonicalizar {}", homologada.display()), e))?;
    let c = canonico_ou_futuro(candidato)?;
    if c == h || c.starts_with(&h) || h.starts_with(&c) {
        return Err(ErroCalibre::CaminhoProibido(format!(
            "{} coincide com ou contém/está dentro da Biblioteca Homologada {}",
            c.display(),
            h.display()
        )));
    }
    let (mh, mc) = (
        std::fs::metadata(h.join("metadata.db")),
        std::fs::metadata(c.join("metadata.db")),
    );
    if let (Ok(a), Ok(b)) = (mh, mc) {
        if a.dev() == b.dev() && a.ino() == b.ino() {
            return Err(ErroCalibre::CaminhoProibido(
                "o metadata.db é o mesmo arquivo da Homologada (link ou bind mount)".into(),
            ));
        }
    }
    Ok(c)
}

fn confirmar_library_id_distinto(id: &str, homologada: &Path) -> Resultado<()> {
    let h = BibliotecaLeitura::abrir(homologada)?;
    if h.library_id()?.as_deref() == Some(id) {
        return Err(ErroCalibre::CaminhoProibido(
            "library_id igual ao da Homologada (cópia ou espelho dela)".into(),
        ));
    }
    Ok(())
}

/// Valor de coluna personalizada a gravar.
#[derive(Debug, Clone, PartialEq)]
pub struct CampoPersonalizado {
    pub rotulo: String,
    pub tipo: TipoColuna,
    pub valor: Value,
}

/// Campos a gravar com `set_metadata --field`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CamposMetadados {
    pub titulo: Option<String>,
    /// `title_sort`: o Calibre NÃO o recalcula a partir de `title` no `set_metadata`
    /// (medido no 9.14), então é definido explicitamente.
    pub titulo_ordenacao: Option<String>,
    /// `author_sort`: idem. Vem do `dag-biblio` quando existir; até lá, parâmetro.
    pub autor_ordenacao: Option<String>,
    pub autores: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub serie: Option<String>,
    pub indice_serie: Option<f64>,
    pub editora: Option<String>,
    pub idiomas: Option<Vec<String>>,
    pub identificadores: Option<Vec<(String, String)>>,
    pub comentarios: Option<String>,
    /// Escala crua do Calibre (0..=10, meia estrela = 1). Só pares são aceitos.
    pub avaliacao: Option<i64>,
    /// `AAAA-MM-DD` ou RFC 3339.
    pub data_publicacao: Option<String>,
    pub personalizados: Vec<CampoPersonalizado>,
}

fn sem(valor: &str, proibidos: &[char], campo: &str) -> Resultado<()> {
    if let Some(c) = valor.chars().find(|c| proibidos.contains(c)) {
        return Err(ErroCalibre::ValorInvalido(format!(
            "{campo} contém {c:?}, que o calibredb usa como separador: {valor:?}"
        )));
    }
    Ok(())
}

fn estrelas(raw: i64) -> Resultado<String> {
    if !(0..=10).contains(&raw) || raw % 2 != 0 {
        return Err(ErroCalibre::ValorInvalido(format!(
            "avaliação {raw}: o calibredb só aceita estrelas inteiras (0..=10 em passos de 2)"
        )));
    }
    Ok((raw / 2).to_string())
}

impl CamposMetadados {
    /// Constrói os campos a partir de um livro lido (usado na homologação).
    #[must_use]
    pub fn de_livro(livro: &LivroCalibre, colunas: &[ColunaPersonalizada]) -> Self {
        let mut personalizados = Vec::new();
        for (rot, v) in &livro.personalizados {
            if let Some(c) = colunas.iter().find(|c| &c.rotulo == rot) {
                if c.materializada && !v.is_null() {
                    personalizados.push(CampoPersonalizado {
                        rotulo: rot.clone(),
                        tipo: c.tipo.clone(),
                        valor: v.clone(),
                    });
                }
            }
        }
        Self {
            titulo: Some(livro.titulo.clone()),
            titulo_ordenacao: livro.titulo_ordenacao.clone(),
            autor_ordenacao: livro.autor_ordenacao.clone(),
            autores: (!livro.autores.is_empty()).then(|| livro.autores.clone()),
            tags: (!livro.tags.is_empty()).then(|| livro.tags.clone()),
            serie: livro.serie.clone(),
            indice_serie: livro.indice_serie.filter(|_| livro.serie.is_some()),
            editora: livro.editora.clone(),
            idiomas: (!livro.idiomas.is_empty()).then(|| livro.idiomas.clone()),
            identificadores: (!livro.identificadores.is_empty())
                .then(|| livro.identificadores.clone()),
            comentarios: livro.comentarios.clone(),
            avaliacao: livro.avaliacao,
            data_publicacao: livro.data_publicacao.clone(),
            personalizados,
        }
    }

    /// Argumentos `-f campo:valor`.
    ///
    /// # Errors
    /// `ValorInvalido` quando um valor contém o separador do calibredb (`,` em listas, `&`
    /// em autores): melhor recusar do que gravar errado em silêncio.
    pub fn argumentos(&self) -> Resultado<Vec<String>> {
        let mut a = Vec::new();
        let mut f = |k: &str, v: String| {
            a.push("-f".to_owned());
            a.push(format!("{k}:{v}"));
        };
        if let Some(t) = &self.titulo {
            f("title", t.clone());
        }
        if let Some(t) = &self.titulo_ordenacao {
            f("title_sort", t.clone());
        }
        if let Some(v) = &self.autores {
            for x in v {
                sem(x, &['&'], "autor")?;
            }
            f("authors", v.join("&"));
        }
        // `author_sort` depois de `authors`: definir autores o reescreveria.
        if let Some(t) = &self.autor_ordenacao {
            f("author_sort", t.clone());
        }
        if let Some(v) = &self.tags {
            for x in v {
                sem(x, &[','], "tag")?;
            }
            f("tags", v.join(","));
        }
        if let Some(s) = &self.serie {
            f("series", s.clone());
            if let Some(i) = self.indice_serie {
                f("series_index", i.to_string());
            }
        }
        if let Some(e) = &self.editora {
            f("publisher", e.clone());
        }
        if let Some(v) = &self.idiomas {
            for x in v {
                sem(x, &[','], "idioma")?;
            }
            f("languages", v.join(","));
        }
        if let Some(v) = &self.identificadores {
            for (t, x) in v {
                sem(t, &[',', ':'], "tipo de identificador")?;
                sem(x, &[','], "identificador")?;
            }
            f(
                "identifiers",
                v.iter()
                    .map(|(t, x)| format!("{t}:{x}"))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        if let Some(c) = &self.comentarios {
            f("comments", c.clone());
        }
        if let Some(r) = self.avaliacao {
            f("rating", estrelas(r)?);
        }
        if let Some(d) = &self.data_publicacao {
            f("pubdate", d.clone());
        }
        for p in &self.personalizados {
            let k = format!("#{}", p.rotulo);
            match (&p.tipo, &p.valor) {
                (TipoColuna::Composite, _) => {
                    return Err(ErroCalibre::ValorInvalido(format!(
                        "a coluna #{} é composite e não aceita valor",
                        p.rotulo
                    )));
                }
                (TipoColuna::Series, Value::Object(o)) => {
                    if let Some(s) = o.get("serie").and_then(Value::as_str) {
                        f(&k, s.to_owned());
                        if let Some(i) = o.get("indice").and_then(Value::as_f64) {
                            f(&format!("{k}_index"), i.to_string());
                        }
                    }
                }
                (TipoColuna::Rating, Value::Number(n)) => {
                    f(&k, estrelas(n.as_i64().unwrap_or(0))?);
                }
                (_, Value::Array(v)) => {
                    let mut partes = Vec::new();
                    for x in v {
                        let s = x.as_str().map_or_else(|| x.to_string(), str::to_owned);
                        sem(&s, &[','], &format!("valor de #{}", p.rotulo))?;
                        partes.push(s);
                    }
                    f(&k, partes.join(","));
                }
                (_, Value::Bool(b)) => f(&k, b.to_string()),
                (_, Value::String(s)) => f(&k, s.clone()),
                (_, Value::Number(n)) => f(&k, n.to_string()),
                (_, Value::Null) => {}
                (_, outro) => {
                    return Err(ErroCalibre::ValorInvalido(format!(
                        "valor não suportado para #{}: {outro}",
                        p.rotulo
                    )));
                }
            }
        }
        Ok(a)
    }
}

/// Livro a cadastrar na A Classificar.
#[derive(Debug, Clone)]
pub struct NovoLivro {
    /// Caminho absoluto do arquivo (só leitura).
    pub arquivo: PathBuf,
    /// BLAKE3 hexadecimal (64 caracteres) do arquivo, já conhecido pelo Catálogo.
    pub hash_blake3: String,
    pub capa: Option<PathBuf>,
    /// Confiança da classificação (0..=1), gravada em `#dag_confianca` se a coluna existir.
    pub confianca: Option<f64>,
    /// Outros formatos do mesmo livro (caminhos absolutos), via `add_format`.
    pub formatos_extras: Vec<PathBuf>,
    pub metadados: CamposMetadados,
}

/// Resultado de uma adição.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultadoAdicao {
    pub id: i64,
    /// `false` quando o livro já estava lá (reprocessamento).
    pub criado: bool,
    /// Outros ids com o mesmo `dag:` (não deveria existir; reportado, nunca removido).
    pub duplicados_extras: Vec<i64>,
    /// A adição retomou metadados que faltavam de uma execução interrompida.
    pub retomado: bool,
}

/// Linha de `calibredb list --for-machine`.
#[derive(Debug, Clone, PartialEq)]
pub struct LinhaLista {
    pub id: i64,
    pub campos: BTreeMap<String, Value>,
}

/// Resultado do clone de colunas.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatorioClone {
    pub criadas: Vec<String>,
    pub ja_existiam: Vec<String>,
    /// `(elemento, motivo)`: nada some em silêncio.
    pub nao_clonadas: Vec<(String, String)>,
    pub avisos: Vec<String>,
    pub buscas_salvas_criadas: Vec<String>,
    /// Diferenças encontradas ao reabrir a A Classificar e comparar as colunas.
    pub divergencias: Vec<String>,
}

/// Cliente do `calibredb`.
#[derive(Debug)]
pub struct ClienteCalibredb<E: ExecutorCalibre> {
    exec: E,
    timeout: Duration,
    politica: PoliticaTrava,
    processos: Option<Vec<ProcessoCalibre>>,
    tentativas_lock: u32,
    homologadas: Vec<(PathBuf, Option<String>)>,
}

/// O `add` do calibredb com caminho vazio importou o diretório corrente inteiro (medido no
/// 9.14): todo caminho passado precisa ser absoluto, existir, ser arquivo regular e não vazio.
pub(crate) fn arquivo_valido(p: &Path) -> Resultado<()> {
    if p.as_os_str().is_empty() || !p.is_absolute() {
        return Err(ErroCalibre::ValorInvalido(format!(
            "caminho vazio ou relativo: {p:?}"
        )));
    }
    let md = std::fs::metadata(p)
        .map_err(|e| ErroCalibre::ValorInvalido(format!("{}: {e}", p.display())))?;
    if !md.is_file() || md.len() == 0 {
        return Err(ErroCalibre::ValorInvalido(format!(
            "{} não é arquivo regular ou está vazio",
            p.display()
        )));
    }
    Ok(())
}

fn validar_hash(h: &str) -> Resultado<()> {
    if h.len() == 64
        && h.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(ErroCalibre::ValorInvalido(format!(
            "hash BLAKE3 deve ter 64 dígitos hexadecimais minúsculos: {h:?}"
        )))
    }
}

pub(crate) fn extrair_json_array(saida: &str) -> Resultado<Value> {
    let (i, f) = (saida.find('['), saida.rfind(']'));
    match (i, f) {
        (Some(i), Some(f)) if f > i => serde_json::from_str(&saida[i..=f])
            .map_err(|e| ErroCalibre::SaidaInvalida(format!("JSON do list: {e}"))),
        _ => Err(ErroCalibre::SaidaInvalida(format!(
            "esperava um array JSON, veio: {:?}",
            saida.chars().take(200).collect::<String>()
        ))),
    }
}

impl<E: ExecutorCalibre> ClienteCalibredb<E> {
    #[must_use]
    pub fn novo(exec: E) -> Self {
        Self {
            exec,
            timeout: Duration::from_secs(300),
            politica: PoliticaTrava::default(),
            processos: None,
            tentativas_lock: 0,
            homologadas: Vec::new(),
        }
    }

    /// Registra uma Biblioteca Homologada como PROTEGIDA: qualquer comando local (de qualquer
    /// subcomando, inclusive `list` e `clone`) cujo `--library-path` seja ela, esteja dentro
    /// dela, a contenha, compartilhe o inode do `metadata.db` ou o `library_id` é recusado
    /// com `CaminhoProibido`. Motivo: abrir uma biblioteca com o Calibre ESCREVE nela
    /// (`last_expired_trash_at`, `full-text-search.db`). A única exceção é o fluxo explícito
    /// de [`crate::homologacao::homologar`].
    ///
    /// # Errors
    /// Se a Homologada não for uma biblioteca legível.
    pub fn proteger_homologada(mut self, homologada: &Path) -> Resultado<Self> {
        let leitura = BibliotecaLeitura::abrir(homologada)?;
        self.homologadas
            .push((leitura.raiz().to_path_buf(), leitura.library_id()?));
        Ok(self)
    }

    fn recusar_se_protegida(&self, p: &Path) -> Resultado<()> {
        for (h, id) in &self.homologadas {
            let c = canonico_ou_futuro(p)?;
            if c == *h || c.starts_with(h) || h.starts_with(&c) {
                return Err(ErroCalibre::CaminhoProibido(format!(
                    "{} é (ou está ligada à) Biblioteca Homologada protegida {}",
                    c.display(),
                    h.display()
                )));
            }
            if c.join("metadata.db").is_file() {
                let mesmo_inode = match (
                    std::fs::metadata(c.join("metadata.db")),
                    std::fs::metadata(h.join("metadata.db")),
                ) {
                    (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
                    _ => false,
                };
                let mesmo_id = id.is_some()
                    && BibliotecaLeitura::abrir(&c)
                        .and_then(|l| l.library_id())
                        .ok()
                        .flatten()
                        == *id;
                if mesmo_inode || mesmo_id {
                    return Err(ErroCalibre::CaminhoProibido(format!(
                        "{} é a Homologada protegida (mesmo arquivo ou library_id)",
                        c.display()
                    )));
                }
            }
        }
        Ok(())
    }

    /// Repete (a cada 250 ms) quando o lock global do `calibredb` está ocupado por outro
    /// `calibredb` nosso. Não adianta com o GUI/servidor abertos: o erro final é `CalibreOcupado`.
    #[must_use]
    pub fn com_tentativas_lock(mut self, n: u32) -> Self {
        self.tentativas_lock = n;
        self
    }

    #[must_use]
    pub fn com_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    #[must_use]
    pub fn com_politica(mut self, p: PoliticaTrava) -> Self {
        self.politica = p;
        self
    }

    /// O executor (para inspeção em testes).
    #[must_use]
    pub fn executor(&self) -> &E {
        &self.exec
    }

    /// Fixa a lista de processos Calibre em vez de varrer `/proc` (testes).
    #[must_use]
    pub fn com_processos(mut self, p: Vec<ProcessoCalibre>) -> Self {
        self.processos = Some(p);
        self
    }

    /// Roda um subcomando contra `alvo`. Escritas locais passam pela trava.
    pub(crate) fn rodar(
        &self,
        alvo: &Alvo,
        sub: &str,
        args: &[String],
        escreve: bool,
    ) -> Resultado<SaidaProcesso> {
        self.rodar_com(alvo, sub, args, escreve, false)
    }

    /// Atalho da homologação: dispensa a proteção das Homologadas.
    pub(crate) fn rodar_h(
        &self,
        alvo: &Alvo,
        sub: &str,
        args: &[String],
        escreve: bool,
    ) -> Resultado<SaidaProcesso> {
        self.rodar_com(alvo, sub, args, escreve, true)
    }

    /// Como `rodar`, mas só o módulo `homologacao` passa `homologacao = true`, o que dispensa
    /// a proteção das Homologadas registradas.
    pub(crate) fn rodar_com(
        &self,
        alvo: &Alvo,
        sub: &str,
        args: &[String],
        escreve: bool,
        homologacao: bool,
    ) -> Resultado<SaidaProcesso> {
        if let (Alvo::Local(p), false) = (alvo, homologacao) {
            self.recusar_se_protegida(p)?;
        }
        let mut a: Vec<String> = vec![sub.to_owned()];
        a.extend(args.iter().cloned());
        let mut stdin = None;
        match alvo {
            Alvo::Local(p) => {
                if escreve {
                    let procs = self.processos.clone().unwrap_or_else(processos_calibre);
                    verificar_escrita_local(p, self.politica, &procs)?;
                }
                a.push("--library-path".into());
                a.push(p.to_string_lossy().into_owned());
            }
            Alvo::Servidor(s) => {
                a.push("--library-path".into());
                a.push(format!(
                    "{}/#{}",
                    s.url.trim_end_matches('/'),
                    s.id_biblioteca
                ));
                a.push("--username".into());
                a.push(s.usuario.clone());
                a.push("--password".into());
                a.push("<f:/dev/stdin>".into());
                stdin = Some(s.senha.0.clone().into_bytes());
            }
        }
        let cmd = Comando {
            args: a,
            stdin,
            timeout: self.timeout,
        };
        let mut restantes = self.tentativas_lock;
        loop {
            let s = self.exec.executar(&cmd)?;
            if s.sucesso() {
                return Ok(s);
            }
            let erro = classificar_falha(&cmd, &s);
            if matches!(erro, ErroCalibre::CalibreOcupado { .. }) && restantes > 0 {
                restantes -= 1;
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            return Err(erro);
        }
    }

    /// Versão do `calibredb`.
    ///
    /// # Errors
    /// Binário ausente ou saída incompreensível.
    pub fn versao(&self) -> Resultado<VersaoCalibre> {
        let s = self.exec.executar(&Comando {
            args: vec!["--version".into()],
            stdin: None,
            timeout: Duration::from_secs(60),
        })?;
        if !s.sucesso() {
            return Err(ErroCalibre::CalibredbAusente(s.stderr));
        }
        interpretar_versao(&s.stdout)
    }

    /// Cria a A Classificar em `destino` (inexistente ou vazio), recusando o caminho e o
    /// `library_id` da Homologada.
    ///
    /// # Errors
    /// `CaminhoProibido`, `DiretorioNaoVazio`, falhas do calibredb.
    pub fn criar_a_classificar(
        &self,
        destino: &Path,
        homologada: &Path,
    ) -> Resultado<BibliotecaAClassificar> {
        let raiz = validar_distinta(destino, homologada)?;
        if raiz.join("metadata.db").exists() {
            return Err(ErroCalibre::DiretorioNaoVazio(raiz));
        }
        if raiz.exists() {
            let vazio = std::fs::read_dir(&raiz)
                .map_err(|e| ErroCalibre::io(format!("listar {}", raiz.display()), e))?
                .next()
                .is_none();
            if !vazio {
                return Err(ErroCalibre::DiretorioNaoVazio(raiz));
            }
        } else {
            std::fs::create_dir_all(&raiz)
                .map_err(|e| ErroCalibre::io(format!("criar {}", raiz.display()), e))?;
        }
        // `calibredb list` num diretório vazio cria o metadata.db (verificado em 7.6).
        self.rodar(
            &Alvo::Local(raiz.clone()),
            "list",
            &["--fields".into(), "id".into()],
            true,
        )?;
        if !raiz.join("metadata.db").is_file() {
            return Err(ErroCalibre::SaidaInvalida(
                "o calibredb não criou metadata.db no diretório vazio".into(),
            ));
        }
        BibliotecaAClassificar::abrir(&raiz, homologada)
    }

    /// Cria `#dag_hash` (text) e `#dag_confianca` (float) na A Classificar se faltarem.
    ///
    /// # Errors
    /// Falha do calibredb.
    pub fn garantir_colunas_dag(&self, bib: &BibliotecaAClassificar) -> Resultado<Vec<String>> {
        let existentes = BibliotecaLeitura::abrir(&bib.raiz)?
            .colunas_personalizadas()
            .to_vec();
        let mut criadas = Vec::new();
        for (rot, nome, tipo) in [
            (COLUNA_DAG_HASH, "DAG hash", "text"),
            (COLUNA_DAG_CONFIANCA, "DAG confiança", "float"),
        ] {
            if !existentes.iter().any(|c| c.rotulo == rot) {
                self.rodar(
                    &Alvo::Local(bib.raiz.clone()),
                    "add_custom_column",
                    &[rot.into(), nome.into(), tipo.into()],
                    true,
                )?;
                criadas.push(rot.to_owned());
            }
        }
        Ok(criadas)
    }

    /// Clona as colunas personalizadas compatíveis e as buscas salvas do perfil da Homologada.
    /// O que não se clona fica em `nao_clonadas`, com o motivo.
    ///
    /// # Errors
    /// Falha do calibredb ou de leitura.
    pub fn clonar_estrutura(
        &self,
        bib: &BibliotecaAClassificar,
        perfil: &PerfilBiblioteca,
    ) -> Resultado<RelatorioClone> {
        let alvo = Alvo::Local(bib.raiz.clone());
        let existentes = BibliotecaLeitura::abrir(&bib.raiz)?
            .colunas_personalizadas()
            .to_vec();
        let mut rel = RelatorioClone::default();
        for col in &perfil.colunas {
            let nome_item = format!("coluna #{}", col.rotulo);
            let status = perfil
                .compatibilidade
                .status_de(&nome_item)
                .cloned()
                .unwrap_or(StatusClone::Clonavel);
            match status {
                StatusClone::NaoClonavel { motivos } => {
                    rel.nao_clonadas.push((nome_item, motivos.join("; ")));
                    continue;
                }
                StatusClone::ClonavelComAviso { avisos } => {
                    for a in avisos {
                        rel.avisos.push(format!("{nome_item}: {a}"));
                    }
                }
                StatusClone::Clonavel => {}
            }
            if let Some(e) = existentes.iter().find(|e| e.rotulo == col.rotulo) {
                if e.tipo == col.tipo && e.e_multipla == col.e_multipla {
                    rel.ja_existiam.push(col.rotulo.clone());
                } else {
                    rel.nao_clonadas.push((
                        nome_item,
                        "já existe na A Classificar com tipo/multiplicidade diferentes".into(),
                    ));
                }
                continue;
            }
            let mut a = vec![
                col.rotulo.clone(),
                col.nome.clone(),
                col.tipo.nome_calibre().into(),
            ];
            if col.e_multipla {
                a.push("--is-multiple".into());
            }
            a.push("--display".into());
            a.push(col.display.to_string());
            self.rodar(&alvo, "add_custom_column", &a, true)?;
            rel.criadas.push(col.rotulo.clone());
        }
        for b in &perfil.buscas_salvas {
            self.rodar(
                &alvo,
                "saved_searches",
                &["add".into(), b.nome.clone(), b.busca.clone()],
                true,
            )?;
            rel.buscas_salvas_criadas.push(b.nome.clone());
        }
        for i in perfil.compatibilidade.nao_clonaveis() {
            if !i.elemento.starts_with("coluna #") {
                if let StatusClone::NaoClonavel { motivos } = &i.status {
                    rel.nao_clonadas
                        .push((i.elemento.clone(), motivos.join("; ")));
                }
            }
        }
        // Reabre e confere: o que foi criado precisa estar lá com o mesmo tipo.
        let depois = BibliotecaLeitura::abrir(&bib.raiz)?;
        for r in rel.criadas.iter().chain(&rel.ja_existiam) {
            let orig = perfil.colunas.iter().find(|c| &c.rotulo == r);
            match (
                orig,
                depois
                    .colunas_personalizadas()
                    .iter()
                    .find(|c| &c.rotulo == r),
            ) {
                (Some(o), Some(n)) => {
                    if o.tipo != n.tipo || o.e_multipla != n.e_multipla || o.nome != n.nome {
                        rel.divergencias.push(format!(
                            "#{r}: tipo/nome/multiplicidade divergem após criar"
                        ));
                    }
                }
                _ => rel.divergencias.push(format!("#{r}: ausente após criar")),
            }
        }
        Ok(rel)
    }

    /// Ids dos livros com `dag:<hash>`, e se algum está marcado como pendente.
    ///
    /// # Errors
    /// Falha do calibredb ou JSON inesperado.
    pub fn buscar_por_hash(
        &self,
        bib: &BibliotecaAClassificar,
        hash: &str,
    ) -> Resultado<Vec<(i64, bool)>> {
        validar_hash(hash)?;
        let linhas = self.listar(
            bib,
            &["id", "identifiers"],
            Some(&format!("identifiers:={IDENTIFICADOR_DAG}:{hash}")),
            None,
        )?;
        let mut out = Vec::new();
        for l in linhas {
            let ids = l.campos.get("identifiers");
            let confere = ids
                .and_then(|v| v.get(IDENTIFICADOR_DAG))
                .and_then(Value::as_str)
                == Some(hash);
            if confere {
                let pendente = ids.and_then(|v| v.get(IDENTIFICADOR_PENDENTE)).is_some();
                out.push((l.id, pendente));
            }
        }
        Ok(out)
    }

    /// Adiciona o livro de forma idempotente: se já existir `dag:<hash>`, não duplica.
    ///
    /// # Errors
    /// Trava (Calibre aberto), falhas do calibredb, valores inválidos.
    pub fn adicionar(
        &self,
        bib: &BibliotecaAClassificar,
        livro: &NovoLivro,
    ) -> Resultado<ResultadoAdicao> {
        validar_hash(&livro.hash_blake3)?;
        arquivo_valido(&livro.arquivo)?;
        for f in &livro.formatos_extras {
            arquivo_valido(f)?;
        }
        if let Some(c) = &livro.capa {
            arquivo_valido(c)?;
        }
        let alvo = Alvo::Local(bib.raiz.clone());
        let achados = self.buscar_por_hash(bib, &livro.hash_blake3)?;
        // Metadados completos: identificadores + dag (sem a marca de pendência).
        let mut meta = livro.metadados.clone();
        let colunas_destino = BibliotecaLeitura::abrir(&bib.raiz)?
            .colunas_personalizadas()
            .to_vec();
        if colunas_destino.iter().any(|c| c.rotulo == COLUNA_DAG_HASH) {
            meta.personalizados.retain(|p| p.rotulo != COLUNA_DAG_HASH);
            meta.personalizados.push(CampoPersonalizado {
                rotulo: COLUNA_DAG_HASH.into(),
                tipo: TipoColuna::Text,
                valor: Value::String(livro.hash_blake3.clone()),
            });
        }
        if let (Some(c), true) = (
            livro.confianca,
            colunas_destino
                .iter()
                .any(|c| c.rotulo == COLUNA_DAG_CONFIANCA),
        ) {
            meta.personalizados
                .retain(|p| p.rotulo != COLUNA_DAG_CONFIANCA);
            meta.personalizados.push(CampoPersonalizado {
                rotulo: COLUNA_DAG_CONFIANCA.into(),
                tipo: TipoColuna::Float,
                valor: Value::from(c),
            });
        }
        let mut ids = meta.identificadores.take().unwrap_or_default();
        ids.retain(|(t, _)| t != IDENTIFICADOR_DAG && t != IDENTIFICADOR_PENDENTE);
        ids.push((IDENTIFICADOR_DAG.to_owned(), livro.hash_blake3.clone()));
        meta.identificadores = Some(ids);

        if let Some(&(id, pendente)) = achados.first() {
            let extras = achados.iter().skip(1).map(|x| x.0).collect();
            if pendente {
                self.adicionar_formatos(bib, id, &livro.formatos_extras)?;
                self.definir_metadados(bib, id, &meta)?;
                return Ok(ResultadoAdicao {
                    id,
                    criado: false,
                    duplicados_extras: extras,
                    retomado: true,
                });
            }
            return Ok(ResultadoAdicao {
                id,
                criado: false,
                duplicados_extras: extras,
                retomado: false,
            });
        }

        // Primeiro passo atômico: `add` já grava dag + marca de pendência.
        let mut a: Vec<String> = vec![
            "--duplicates".into(),
            "-I".into(),
            format!("{IDENTIFICADOR_DAG}:{}", livro.hash_blake3),
            "-I".into(),
            format!("{IDENTIFICADOR_PENDENTE}:1"),
        ];
        if let Some(c) = &livro.capa {
            a.push("--cover".into());
            a.push(c.to_string_lossy().into_owned());
        }
        a.push(livro.arquivo.to_string_lossy().into_owned());
        let s = self.rodar(&alvo, "add", &a, true)?;
        let id = interpretar_ids_adicionados(&s.stdout)?;
        // Se falhar daqui em diante a marca de pendência fica e a próxima execução retoma.
        self.adicionar_formatos(bib, id, &livro.formatos_extras)?;
        self.definir_metadados(bib, id, &meta)?;
        Ok(ResultadoAdicao {
            id,
            criado: true,
            duplicados_extras: Vec::new(),
            retomado: false,
        })
    }

    fn adicionar_formatos(
        &self,
        bib: &BibliotecaAClassificar,
        id: i64,
        arquivos: &[PathBuf],
    ) -> Resultado<()> {
        for f in arquivos {
            arquivo_valido(f)?;
            self.rodar(
                &Alvo::Local(bib.raiz.clone()),
                "add_format",
                &[id.to_string(), f.to_string_lossy().into_owned()],
                true,
            )?;
        }
        Ok(())
    }

    /// `set_metadata --field ...` num livro.
    ///
    /// # Errors
    /// Valores inválidos ou falha do calibredb.
    pub fn definir_metadados(
        &self,
        bib: &BibliotecaAClassificar,
        id: i64,
        campos: &CamposMetadados,
    ) -> Resultado<()> {
        let mut a = vec![id.to_string()];
        a.extend(campos.argumentos()?);
        if a.len() == 1 {
            return Ok(());
        }
        self.rodar(&Alvo::Local(bib.raiz.clone()), "set_metadata", &a, true)?;
        Ok(())
    }

    /// `list --for-machine` com os campos pedidos (use `*rotulo` para colunas personalizadas).
    ///
    /// # Errors
    /// Falha do calibredb ou JSON inesperado.
    pub fn listar(
        &self,
        bib: &BibliotecaAClassificar,
        campos: &[&str],
        busca: Option<&str>,
        limite: Option<u32>,
    ) -> Resultado<Vec<LinhaLista>> {
        listar_em(self, &Alvo::Local(bib.raiz.clone()), campos, busca, limite)
    }

    /// Remove livros SÓ da A Classificar (rollback), exigindo que tenham o identificador
    /// `dag:`. Remoção definitiva (`--permanent`): a A Classificar não tem Lixeira útil.
    ///
    /// # Errors
    /// `LivroInexistente` se o id não tiver `dag:`.
    pub fn remover(&self, bib: &BibliotecaAClassificar, ids: &[i64]) -> Result<(), ErroCalibre> {
        if ids.is_empty() {
            return Ok(());
        }
        let leitura = BibliotecaLeitura::abrir(&bib.raiz)?;
        for id in ids {
            let l = leitura
                .livro(*id)?
                .ok_or_else(|| ErroCalibre::LivroInexistente(format!("id {id}")))?;
            if !l
                .identificadores
                .iter()
                .any(|(t, _)| t == IDENTIFICADOR_DAG)
            {
                return Err(ErroCalibre::LivroInexistente(format!(
                    "o livro {id} não foi criado pelo DAG (sem identificador dag:)"
                )));
            }
        }
        drop(leitura);
        let a = vec![
            "--permanent".to_owned(),
            ids.iter().map(i64::to_string).collect::<Vec<_>>().join(","),
        ];
        self.rodar(&Alvo::Local(bib.raiz.clone()), "remove", &a, true)?;
        Ok(())
    }
}

/// `list --for-machine` contra qualquer alvo (local ou servidor).
pub(crate) fn listar_em<E: ExecutorCalibre>(
    cli: &ClienteCalibredb<E>,
    alvo: &Alvo,
    campos: &[&str],
    busca: Option<&str>,
    limite: Option<u32>,
) -> Resultado<Vec<LinhaLista>> {
    let mut a: Vec<String> = vec![
        "--for-machine".into(),
        "--fields".into(),
        campos.join(","),
        "--sort-by".into(),
        "id".into(),
    ];
    if let Some(b) = busca {
        a.push("--search".into());
        a.push(b.to_owned());
    }
    if let Some(l) = limite {
        a.push("--limit".into());
        a.push(l.to_string());
    }
    let s = cli.rodar(alvo, "list", &a, false)?;
    interpretar_lista(&s.stdout)
}

/// Interpreta a saída JSON de `list --for-machine`.
///
/// # Errors
/// `SaidaInvalida`.
pub fn interpretar_lista(saida: &str) -> Resultado<Vec<LinhaLista>> {
    let v = extrair_json_array(saida)?;
    let arr = v
        .as_array()
        .ok_or_else(|| ErroCalibre::SaidaInvalida("list: não é array".into()))?;
    let mut out = Vec::new();
    for item in arr {
        let o = item
            .as_object()
            .ok_or_else(|| ErroCalibre::SaidaInvalida("list: item não é objeto".into()))?;
        let id = o
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| ErroCalibre::SaidaInvalida("list: item sem id".into()))?;
        out.push(LinhaLista {
            id,
            campos: o.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        });
    }
    Ok(out)
}

/// `Added book ids: 5` -> 5. Mais de um id é erro (esperamos um arquivo por vez).
///
/// # Errors
/// `SaidaInvalida`.
pub fn interpretar_ids_adicionados(saida: &str) -> Resultado<i64> {
    let linha = saida
        .lines()
        .find_map(|l| l.trim().strip_prefix("Added book ids:"))
        .ok_or_else(|| {
            ErroCalibre::SaidaInvalida(format!(
                "o calibredb não informou ids adicionados: {:?}",
                saida.chars().take(300).collect::<String>()
            ))
        })?;
    let ids: Vec<i64> = linha
        .split(',')
        .filter_map(|x| x.trim().parse().ok())
        .collect();
    match ids.as_slice() {
        [id] => Ok(*id),
        _ => Err(ErroCalibre::SaidaInvalida(format!(
            "esperava exatamente um id adicionado, veio {linha:?}"
        ))),
    }
}

fn classificar_falha(cmd: &Comando, s: &SaidaProcesso) -> ErroCalibre {
    let texto = format!("{}\n{}", s.stderr, s.stdout);
    if texto.contains("Another calibre program") {
        let linha = texto
            .lines()
            .find(|l| l.contains("Another calibre program"))
            .unwrap_or("lock global do calibredb")
            .chars()
            .take(160)
            .collect();
        return ErroCalibre::CalibreOcupado { detalhe: linha };
    }
    ErroCalibre::CalibredbFalhou {
        comando: cmd.descricao(),
        status: s.status,
        stderr: s.stderr.trim().chars().take(2000).collect(),
    }
}
