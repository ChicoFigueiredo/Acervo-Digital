//! Vocabulário e estante aprendidos da Biblioteca Homologada (B-10, WOW-03, WOW-06).
//!
//! O espelho da Homologada (tags, `#pasta` como estante, tags de controle `!` de fora) e as
//! decisões humanas gravadas treinam um `dag_biblio::vocabulario::Classificador` (kNN sobre
//! TF-IDF, sem rede). O resultado fica em cache e é refeito quando o espelho ou as decisões mudam
//! (assinaturas baratas, por índice): uma correção humana influencia o próximo item.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dag_biblio::Campo;
use dag_biblio::autores::{AutorExistente, ConfirmacaoAutor, ContextoAutor, ResolucaoAutor};
use dag_biblio::normalizacao::{CompatAutor, comparar_autores_norm, normalizar_autor};
use dag_biblio::vocabulario::{
    Classificador, ConfigClassificador, DecisaoHumana, Exemplo, LivroTreino, Sugestoes,
    e_tag_controle,
};
use dag_biblio_ia::{DecisaoExemplo, ExemploAprovado};
use dag_store::biblioteca as store;
use rusqlite::Connection;

use super::{ConfigBiblioteca, homologada_ativa};
use crate::agente::Agente;
use crate::erro::ErroAgente;

const PAGINA_TREINO: usize = 5000;
/// Tags frequentes que sempre vão à IA, além das sugeridas pelo classificador.
const TAGS_FREQUENTES_IA: usize = 40;
const TAGS_CANDIDATAS_IA_MAX: usize = 80;
/// "~40 estantes" da Homologada (o limite da spec).
const ESTANTES_IA_MAX: usize = 40;
const EXEMPLOS_IA_MAX: usize = 5;
const AUTORES_PARECIDOS_POR_NOME: usize = 5;

type Assinatura = ((i64, i64, i64), (i64, i64), Option<String>);

/// Uma decisão humana já resolvida (o que o humano deixou como final).
#[derive(Debug, Clone)]
struct ExemploHumano {
    id: i64,
    titulo: String,
    tags: Vec<String>,
    estante: Option<String>,
}

/// Classificador treinado, autores da Homologada e exemplos de decisões aprovadas.
pub(crate) struct CacheVocab {
    assinatura: Assinatura,
    classificador: Classificador,
    existentes: Vec<AutorExistente>,
    por_sobrenome: HashMap<String, Vec<usize>>,
    confirmacoes: Vec<ConfirmacaoAutor>,
    humanos: Vec<ExemploHumano>,
    tags_frequentes: Vec<String>,
    /// Quantos livros da Homologada treinaram.
    pub livros_treino: usize,
}

impl std::fmt::Debug for CacheVocab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheVocab")
            .field("livros_treino", &self.livros_treino)
            .field("humanos", &self.humanos.len())
            .finish_non_exhaustive()
    }
}

/// Itens de uma lista como o humano escreveu (`|`), ou separados por vírgula/`&` se não houver `|`.
fn partes_lista(v: &str, alt: &[char]) -> Vec<String> {
    let it: Vec<&str> = if v.contains('|') {
        v.split('|').collect()
    } else {
        v.split(|c| alt.contains(&c)).collect()
    };
    it.into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Default)]
struct EstadoArquivo {
    tags: Option<Vec<String>>,
    estante: Option<String>,
    autores: Option<Vec<String>>,
    rejeitadas: Vec<String>,
}

impl CacheVocab {
    fn construir(
        conn: &Connection,
        biblioteca_id: i64,
        cfg: &ConfigBiblioteca,
        assinatura: Assinatura,
    ) -> Result<Self, ErroAgente> {
        let mut treino: Vec<LivroTreino> = Vec::new();
        let mut apos = None;
        loop {
            let pag = store::listar_livros(conn, biblioteca_id, apos, PAGINA_TREINO)?;
            for l in pag.itens {
                treino.push(LivroTreino {
                    id: l.id,
                    titulo: l.titulo,
                    autores: l.autores,
                    comentarios: None,
                    editora: l.editora,
                    serie: l.serie,
                    tags: l.tags,
                    estante: l.estante,
                });
            }
            match pag.proximo {
                Some(c) => apos = Some(c),
                None => break,
            }
        }
        let vocab = dag_biblio::vocabulario::Vocabulario::de_livros(&treino);
        let config = ConfigClassificador {
            tag_controle_livro_novo: cfg.tag_controle_livro_novo.clone(),
            ..ConfigClassificador::default()
        };
        let mut classificador = Classificador::treinar(vocab, &treino, config);

        // Autores da Homologada, com o que se sabe deles (coautores, assuntos, editoras).
        let mut existentes: Vec<AutorExistente> = Vec::new();
        let mut indice: HashMap<String, usize> = HashMap::new();
        for l in &treino {
            for a in &l.autores {
                let i = *indice.entry(a.clone()).or_insert_with(|| {
                    existentes.push(AutorExistente {
                        id: existentes.len() as i64 + 1,
                        nome: a.clone(),
                        ..AutorExistente::default()
                    });
                    existentes.len() - 1
                });
                let e = &mut existentes[i];
                for o in l.autores.iter().filter(|o| *o != a) {
                    if e.coautores.len() < 20 && !e.coautores.contains(o) {
                        e.coautores.push(o.clone());
                    }
                }
                for t in l.tags.iter().filter(|t| !e_tag_controle(t)) {
                    if e.assuntos.len() < 30 && !e.assuntos.contains(t) {
                        e.assuntos.push(t.clone());
                    }
                }
                if let Some(ed) = &l.editora
                    && e.editoras.len() < 10
                    && !e.editoras.contains(ed)
                {
                    e.editoras.push(ed.clone());
                }
            }
        }
        let mut por_sobrenome: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, e) in existentes.iter().enumerate() {
            por_sobrenome
                .entry(normalizar_autor(&e.nome).sobrenome_final().to_owned())
                .or_default()
                .push(i);
        }

        // Decisões humanas: o que ficou como final por arquivo (as mais novas valem).
        let decisoes = store::decisoes_de_vocabulario(conn)?;
        let mut por_arquivo: HashMap<i64, EstadoArquivo> = HashMap::new();
        let mut ordem: Vec<i64> = Vec::new();
        for d in &decisoes {
            if !por_arquivo.contains_key(&d.file_id) {
                ordem.push(d.file_id);
            }
            let e = por_arquivo.entry(d.file_id).or_default();
            let sug_lista = || -> Vec<String> {
                if d.sugerido_lista.is_empty() {
                    d.sugerido_valor
                        .as_deref()
                        .map(|v| partes_lista(v, &[',', '&']))
                        .unwrap_or_default()
                } else {
                    d.sugerido_lista.clone()
                }
            };
            match (d.campo, d.acao) {
                (Campo::Tags, store::AcaoDecisao::Aceitar) => e.tags = Some(sug_lista()),
                (Campo::Tags, store::AcaoDecisao::Corrigir) => {
                    e.tags = d.valor_novo.as_deref().map(|v| partes_lista(v, &[',']));
                }
                (Campo::Tags, store::AcaoDecisao::Rejeitar) => e.rejeitadas.extend(sug_lista()),
                (Campo::Estante, store::AcaoDecisao::Aceitar) => {
                    e.estante.clone_from(&d.sugerido_valor);
                }
                (Campo::Estante, store::AcaoDecisao::Corrigir) => {
                    e.estante = d.valor_novo.clone();
                }
                (Campo::Autores, store::AcaoDecisao::Aceitar) => e.autores = Some(sug_lista()),
                (Campo::Autores, store::AcaoDecisao::Corrigir) => {
                    e.autores = d.valor_novo.as_deref().map(|v| partes_lista(v, &['&']));
                }
                _ => {}
            }
        }
        let mut humanos = Vec::new();
        let mut confirmacoes = Vec::new();
        for fid in ordem {
            let Some(e) = por_arquivo.remove(&fid) else {
                continue;
            };
            let titulo = store::valor_do_arquivo(conn, fid, Campo::Titulo)?
                .and_then(|(v, l)| v.or_else(|| l.into_iter().next()));
            let Some(titulo) = titulo else { continue };
            let autores = match e.autores.clone() {
                Some(a) => a,
                None => store::valor_do_arquivo(conn, fid, Campo::Autores)?
                    .map(|(v, l)| {
                        if l.is_empty() {
                            v.map(|v| partes_lista(&v, &['&'])).unwrap_or_default()
                        } else {
                            l
                        }
                    })
                    .unwrap_or_default(),
            };
            let editora = store::valor_do_arquivo(conn, fid, Campo::Editora)?.and_then(|(v, _)| v);
            // Confirmação de autor: o humano deixou este nome e só um autor da Homologada o
            // reconhece como igual (homônimos exigem decisão, nunca união automática).
            if let Some(nomes) = &e.autores {
                for n in nomes {
                    let nn = normalizar_autor(n);
                    let iguais: Vec<&AutorExistente> = existentes
                        .iter()
                        .filter(|x| {
                            comparar_autores_norm(&nn, &normalizar_autor(&x.nome))
                                == CompatAutor::Igual
                        })
                        .collect();
                    if let [unico] = iguais.as_slice() {
                        confirmacoes.push(ConfirmacaoAutor {
                            nome: n.clone(),
                            autor_id: unico.id,
                        });
                    }
                }
            }
            if e.tags.is_none() && e.estante.is_none() && e.rejeitadas.is_empty() {
                continue;
            }
            let tags = e.tags.clone().unwrap_or_default();
            let livro = LivroTreino {
                id: -fid,
                titulo: titulo.clone(),
                autores,
                comentarios: None,
                editora,
                serie: None,
                tags: tags.clone(),
                estante: e.estante.clone(),
            };
            classificador.registrar_decisao(&DecisaoHumana {
                livro,
                tags_rejeitadas: e.rejeitadas.clone(),
            });
            humanos.push(ExemploHumano {
                id: -fid,
                titulo,
                tags,
                estante: e.estante,
            });
        }
        let tags_frequentes: Vec<String> = classificador
            .vocabulario()
            .tags()
            .iter()
            .take(TAGS_FREQUENTES_IA)
            .cloned()
            .collect();
        Ok(Self {
            assinatura,
            classificador,
            existentes,
            por_sobrenome,
            confirmacoes,
            humanos,
            tags_frequentes,
            livros_treino: treino.len(),
        })
    }

    /// Tags e estante sugeridos (só do vocabulário da Homologada; o resto vira proposta).
    pub(crate) fn sugerir(&self, l: &LivroTreino) -> Sugestoes {
        self.classificador.sugerir(l)
    }

    pub(crate) fn filtrar_candidatas(
        &self,
        valores: &[String],
    ) -> (Vec<String>, Vec<dag_biblio::vocabulario::PropostaTag>) {
        self.classificador.filtrar_candidatas(valores)
    }

    pub(crate) fn estante_por_taxonomia(&self, titulo: &str, texto: &str) -> Option<String> {
        let det = dag_biblio::taxonomia::detectar(titulo, texto);
        dag_biblio::taxonomia::estante_por_assunto(&det, self.classificador.vocabulario())
            .map(str::to_owned)
    }

    /// Tags candidatas para a IA: as sugeridas, as mais usadas e os ancestrais existentes.
    pub(crate) fn tags_candidatas(&self, s: &Sugestoes) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        let mut vistos: HashSet<String> = HashSet::new();
        let vocab = self.classificador.vocabulario();
        let mut push = |t: &str, v: &mut Vec<String>| {
            if v.len() < TAGS_CANDIDATAS_IA_MAX && vistos.insert(t.to_owned()) {
                v.push(t.to_owned());
            }
        };
        for t in &s.tags {
            push(&t.tag, &mut v);
            for a in vocab.ancestrais_existentes(&t.tag) {
                push(a, &mut v);
            }
        }
        for t in &self.tags_frequentes {
            push(t, &mut v);
        }
        v
    }

    /// As estantes da Homologada (cerca de 40).
    pub(crate) fn estantes(&self) -> Vec<String> {
        self.classificador
            .vocabulario()
            .estantes()
            .iter()
            .take(ESTANTES_IA_MAX)
            .cloned()
            .collect()
    }

    /// Autores da Homologada de sobrenome igual ao dos autores do arquivo.
    pub(crate) fn autores_parecidos(&self, nomes: &[String]) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for n in nomes {
            let k = normalizar_autor(n);
            if let Some(ix) = self.por_sobrenome.get(k.sobrenome_final()) {
                for i in ix.iter().take(AUTORES_PARECIDOS_POR_NOME) {
                    let nome = &self.existentes[*i].nome;
                    if !v.contains(nome) {
                        v.push(nome.clone());
                    }
                }
            }
        }
        v
    }

    /// Resolve um nome de autor contra os da Homologada (nunca une homônimos sozinho).
    pub(crate) fn resolver_autor(&self, nome: &str, ctx: &ContextoAutor) -> ResolucaoAutor {
        let k = normalizar_autor(nome);
        let candidatos: Vec<AutorExistente> = self
            .por_sobrenome
            .get(k.sobrenome_final())
            .map(|ix| ix.iter().map(|i| self.existentes[*i].clone()).collect())
            .unwrap_or_default();
        dag_biblio::autores::resolver(nome, ctx, &candidatos, &self.confirmacoes)
    }

    /// Decisões aprovadas parecidas (few-shot): as humanas citadas como vizinhas, as mais
    /// recentes se faltar, e vizinhas da Homologada para completar.
    pub(crate) fn exemplos_ia(&self, conn: &Connection, s: &Sugestoes) -> Vec<ExemploAprovado> {
        let mut ids: Vec<i64> = Vec::new();
        let ex = Self::todos_exemplos(s);
        for e in &ex {
            if e.humano && !ids.contains(&e.livro_id) {
                ids.push(e.livro_id);
            }
        }
        let mut v: Vec<ExemploAprovado> = Vec::new();
        for id in &ids {
            if let Some(h) = self.humanos.iter().find(|h| h.id == *id) {
                v.push(exemplo_de(&h.titulo, &h.tags, h.estante.as_deref()));
            }
        }
        for h in self.humanos.iter().rev() {
            if v.len() >= 3.min(EXEMPLOS_IA_MAX) {
                break;
            }
            if !ids.contains(&h.id) {
                v.push(exemplo_de(&h.titulo, &h.tags, h.estante.as_deref()));
            }
        }
        for e in ex.iter().filter(|e| !e.humano && e.livro_id > 0) {
            if v.len() >= EXEMPLOS_IA_MAX {
                break;
            }
            if let Ok(Some(l)) = store::obter_livro(conn, e.livro_id) {
                v.push(exemplo_de(&l.titulo, &l.tags, l.estante.as_deref()));
            }
        }
        v.truncate(EXEMPLOS_IA_MAX);
        v
    }

    fn todos_exemplos(s: &Sugestoes) -> Vec<Exemplo> {
        let mut ex: Vec<Exemplo> = Vec::new();
        let mut vistos = HashSet::new();
        let fontes = s
            .tags
            .iter()
            .flat_map(|t| t.exemplos.iter())
            .chain(s.estante.iter().flat_map(|e| e.exemplos.iter()));
        for e in fontes {
            if vistos.insert(e.livro_id) {
                ex.push(e.clone());
            }
        }
        ex.sort_by(|a, b| b.similaridade.total_cmp(&a.similaridade));
        ex
    }

    /// "Parecido com «Título» (similaridade 0,52)": o porquê de uma sugestão (WOW-03).
    pub(crate) fn explicar(&self, conn: &Connection, ex: &[Exemplo]) -> Vec<String> {
        ex.iter()
            .take(3)
            .map(|e| {
                let titulo = if e.livro_id < 0 {
                    self.humanos
                        .iter()
                        .find(|h| h.id == e.livro_id)
                        .map(|h| h.titulo.clone())
                } else {
                    store::obter_livro(conn, e.livro_id)
                        .ok()
                        .flatten()
                        .map(|l| l.titulo)
                };
                format!(
                    "parecido com «{}»{} (similaridade {:.2})",
                    titulo.unwrap_or_else(|| format!("#{}", e.livro_id)),
                    if e.humano { ", decisão sua" } else { "" },
                    e.similaridade
                )
            })
            .collect()
    }
}

fn exemplo_de(titulo: &str, tags: &[String], estante: Option<&str>) -> ExemploAprovado {
    let mut decisoes = Vec::new();
    if !tags.is_empty() {
        decisoes.push(DecisaoExemplo {
            campo: Campo::Tags,
            valores: tags.to_vec(),
        });
    }
    if let Some(e) = estante {
        decisoes.push(DecisaoExemplo {
            campo: Campo::Estante,
            valores: vec![e.to_owned()],
        });
    }
    ExemploAprovado {
        nome_arquivo: titulo.to_owned(),
        decisoes,
    }
}

impl Agente {
    /// O classificador treinado, do cache quando o espelho e as decisões não mudaram.
    /// `None` = sem Biblioteca Homologada registrada.
    pub(crate) fn vocabulario(
        &mut self,
        cfg: &ConfigBiblioteca,
    ) -> Result<Option<Arc<CacheVocab>>, ErroAgente> {
        let conn = self.conexao();
        let Some(b) = homologada_ativa(conn)? else {
            return Ok(None);
        };
        let assinatura: Assinatura = (
            store::assinatura_espelho(conn, b.id)?,
            store::assinatura_decisoes(conn)?,
            cfg.tag_controle_livro_novo.clone(),
        );
        if let Some(c) = &self.biblio.vocab
            && c.assinatura == assinatura
        {
            return Ok(Some(Arc::clone(c)));
        }
        let novo = Arc::new(CacheVocab::construir(conn, b.id, cfg, assinatura)?);
        self.biblio.vocab = Some(Arc::clone(&novo));
        Ok(Some(novo))
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn lista_separada_por_barra_ou_alternativo() {
        assert_eq!(partes_lista("a|b c|d", &[',']), vec!["a", "b c", "d"]);
        assert_eq!(partes_lista("a, b ,c", &[',']), vec!["a", "b", "c"]);
        assert_eq!(partes_lista("A & B", &['&']), vec!["A", "B"]);
        assert!(partes_lista("  ", &[',']).is_empty());
    }
}
