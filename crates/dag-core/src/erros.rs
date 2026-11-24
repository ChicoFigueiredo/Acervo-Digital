//! Classificação dos problemas de uma varredura em três desfechos: **esperado** (benigno
//! conhecido), **aceito** (casa uma regra do usuário) e **erro** (real).
//!
//! Tudo aqui é puro (sem I/O): as regras embutidas ficam em [`REGRAS_EMBUTIDAS`] (documentadas e
//! visíveis na CLI, na GUI e no MCP) e as regras do usuário vêm do Catálogo.

use serde::{Deserialize, Serialize};

/// Desfecho de um problema registrado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClasseErro {
    /// Benigno conhecido (arquivo vivo que mudou ou sumiu, symlink de NTFS, limite de volume).
    Esperado,
    /// Casa uma regra "Erros aceitos" do usuário.
    Aceito,
    /// Nenhuma regra casou: precisa de atenção (ou, se for só aviso, é só um aviso).
    Erro,
}

impl ClasseErro {
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::Esperado => "esperado",
            Self::Aceito => "aceito",
            Self::Erro => "erro",
        }
    }

    #[must_use]
    pub fn de_str(s: &str) -> Option<Self> {
        Some(match s {
            "esperado" => Self::Esperado,
            "aceito" => Self::Aceito,
            "erro" => Self::Erro,
            _ => return None,
        })
    }
}

/// Regra embutida: documentada, sempre ativa, não editável.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RegraEmbutida {
    pub id: &'static str,
    pub descricao: &'static str,
    /// Códigos de erro a que a regra se aplica.
    pub codigos: &'static [&'static str],
    /// Padrões de caminho (glob); vazio = qualquer caminho.
    pub padroes: &'static [&'static str],
}

/// Padrões de caminho de dados vivos, que mudam ou somem durante uma leitura longa.
pub const CAMINHOS_VOLATEIS: &[&str] = &[
    "**/.cache/**",
    "**/.bun/**",
    "**/.npm/**",
    "**/.cargo/registry/**",
    "/var/log/**",
    "/var/cache/**",
    "/var/tmp/**",
    "*-wal",
    "*-shm",
    "*.lock",
    "**/.config/google-chrome/**/Cache/**",
    "**/.config/google-chrome/**/Code Cache/**",
    "**/.config/google-chrome/**/GPUCache/**",
    "**/.config/chromium/**/Cache/**",
    "**/.config/chromium/**/Code Cache/**",
    "**/.config/chromium/**/GPUCache/**",
    "**/.config/BraveSoftware/**/Cache/**",
    "**/.mozilla/firefox/**/cache2/**",
    "**/.mozilla/firefox/**/startupCache/**",
];

/// Códigos de "o arquivo mudou ou sumiu durante a leitura".
pub const CODIGOS_MUDOU_OU_SUMIU: &[&str] = &[
    "hash_alterado_desde_inventario",
    "hash_alterado_durante_leitura",
    "hash_ausente",
    "nao_encontrado",
];

/// Código do symlink de NTFS cujo alvo não se lê (`EINVAL`): registrado como informação.
pub const SYMLINK_ALVO_ILEGIVEL: &str = "symlink_alvo_ilegivel";
/// Texto gravado como alvo desses symlinks.
pub const ALVO_ILEGIVEL_NTFS: &str = "alvo ilegível (reparse point NTFS)";
/// Regra gravada quando o reteste do fim da etapa de hash confirma que o arquivo ainda muda.
pub const REGRA_RETESTE: &str = "reteste";

/// Regras embutidas (visíveis em `dag erros regras`).
pub const REGRAS_EMBUTIDAS: &[RegraEmbutida] = &[
    RegraEmbutida {
        id: "volateis",
        descricao: "arquivo vivo (cache, log, SQLite -wal/-shm, *.lock, perfis de navegador) \
                    que mudou ou sumiu durante o hash: esperado",
        codigos: CODIGOS_MUDOU_OU_SUMIU,
        padroes: CAMINHOS_VOLATEIS,
    },
    RegraEmbutida {
        id: REGRA_RETESTE,
        descricao: "arquivo que mudou ou sumiu durante o hash em qualquer pasta e, relido uma \
                    vez no fim da etapa, continuou mudando (ou sumiu): esperado. Se estável, é \
                    hasheado e o problema some",
        codigos: &[
            "hash_alterado_desde_inventario",
            "hash_alterado_durante_leitura",
            "hash_ausente",
        ],
        padroes: &[],
    },
    RegraEmbutida {
        id: "symlink_ntfs",
        descricao: "symlink de NTFS/WSL (reparse point) cujo alvo o Linux não lê (EINVAL): \
                    entrada registrada como symlink, sem erro",
        codigos: &[SYMLINK_ALVO_ILEGIVEL],
        padroes: &[],
    },
    RegraEmbutida {
        id: "limites",
        descricao: "limite de volume, volume pseudo e pastas excluídas: ignorados de propósito",
        codigos: &["limite_de_volume", "volume_pseudo", "excluido"],
        padroes: &[],
    },
];

/// [`RegraEmbutida`] com dados próprios, para trafegar entre processos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegraEmbutidaInfo {
    pub id: String,
    pub descricao: String,
    pub codigos: Vec<String>,
    pub padroes: Vec<String>,
}

impl RegraEmbutida {
    #[must_use]
    pub fn info(&self) -> RegraEmbutidaInfo {
        RegraEmbutidaInfo {
            id: self.id.to_owned(),
            descricao: self.descricao.to_owned(),
            codigos: self.codigos.iter().map(|s| (*s).to_owned()).collect(),
            padroes: self.padroes.iter().map(|s| (*s).to_owned()).collect(),
        }
    }
}

/// Dica mostrada junto de um grupo de problemas.
#[must_use]
pub fn dica_do_codigo(codigo: &str) -> Option<&'static str> {
    match codigo {
        "hash_permissao" | "permissao_negada" | "permissao_negada_elevada" => Some(
            "use `dag scan --elevar` (Polkit, só metadados/leitura) ou aceite esta pasta \
             (`dag erros aceitar <pasta>/** --codigo <código>`)",
        ),
        "hash_alterado_desde_inventario" | "hash_alterado_durante_leitura" | "hash_ausente" => {
            Some("arquivo vivo: aceite a pasta ou exclua-a (`dag excluir <glob>`)")
        }
        "erro_es" => Some("falha de leitura do sistema de arquivos; veja o motivo nas amostras"),
        _ => None,
    }
}

/// Regra "Erros aceitos" criada pelo usuário.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegraAceita {
    pub id: i64,
    /// Glob de caminho (`~` = pasta do usuário).
    pub padrao: String,
    /// Código de erro ou `*`.
    pub codigo: String,
    pub motivo: String,
    pub criada_em: i64,
}

/// Resultado de [`classificar`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classificacao {
    pub classe: ClasseErro,
    /// `usuario:<id>` ou `embutida:<id>`; `None` para `Erro`.
    pub regra: Option<String>,
}

/// Classifica um problema pelo código e pelo caminho (bytes brutos). Regras do usuário vencem as
/// embutidas. `home` expande `~` nos padrões.
#[must_use]
pub fn classificar(
    codigo: &str,
    caminho: Option<&[u8]>,
    usuario: &[RegraAceita],
    home: Option<&str>,
) -> Classificacao {
    let texto = caminho.map(|c| String::from_utf8_lossy(c).into_owned());
    if let Some(t) = &texto {
        for r in usuario {
            if (r.codigo == "*" || r.codigo == codigo) && casa_caminho(&r.padrao, t, home) {
                return Classificacao {
                    classe: ClasseErro::Aceito,
                    regra: Some(format!("usuario:{}", r.id)),
                };
            }
        }
    }
    for r in REGRAS_EMBUTIDAS {
        if r.id == REGRA_RETESTE || !r.codigos.contains(&codigo) {
            continue;
        }
        let casa = r.padroes.is_empty()
            || texto
                .as_deref()
                .is_some_and(|t| r.padroes.iter().any(|p| casa_caminho(p, t, home)));
        if casa {
            return Classificacao {
                classe: ClasseErro::Esperado,
                regra: Some(format!("embutida:{}", r.id)),
            };
        }
    }
    Classificacao {
        classe: ClasseErro::Erro,
        regra: None,
    }
}

// ------------------------------------------------------------------ resumo para clientes

/// Como agrupar o resumo de `dag erros`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgruparErros {
    #[default]
    Codigo,
    Pasta,
}

/// Rótulo de exibição de um problema: `esperado`, `aceito`, `aviso`, `erro` ou `info`.
#[must_use]
pub fn rotulo_exibicao(classe: ClasseErro, severidade: &str) -> &'static str {
    match (classe, severidade) {
        (_, "info") => "info",
        (ClasseErro::Esperado, _) => "esperado",
        (ClasseErro::Aceito, _) => "aceito",
        (_, "aviso") => "aviso",
        _ => "erro",
    }
}

/// Um grupo de problemas com mesmo código, fase e rótulo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrupoErros {
    pub fase: String,
    pub codigo: String,
    pub severidade: String,
    /// `esperado`, `aceito`, `aviso`, `erro` ou `info`.
    pub rotulo: String,
    pub total: u64,
    pub dica: Option<String>,
}

/// Pasta com mais problemas (a pasta que contém o caminho).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrupoPasta {
    pub pasta: String,
    pub total: u64,
    /// Códigos mais frequentes na pasta, com contagem.
    pub codigos: Vec<(String, u64)>,
}

/// Amostra de problema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmostraErro {
    pub fase: String,
    pub codigo: String,
    pub rotulo: String,
    pub caminho: Option<String>,
    pub motivo: Option<String>,
}

/// Erros de uma execução: totais por desfecho, grupos por código, pastas e amostras.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumoErros {
    pub execucao: i64,
    /// Qual visão o cliente mostra primeiro.
    #[serde(default)]
    pub agrupar: AgruparErros,
    /// Estado gravado da execução (`concluida_com_avisos`...).
    pub estado: String,
    pub total: u64,
    pub reais: u64,
    pub avisos: u64,
    pub esperados: u64,
    pub aceitos: u64,
    pub informativos: u64,
    pub por_codigo: Vec<GrupoErros>,
    pub por_pasta: Vec<GrupoPasta>,
    pub amostras: Vec<AmostraErro>,
}

/// Exclusão do usuário (`dag excluir`): não varre o que casa o glob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExclusaoUsuario {
    pub id: i64,
    pub padrao: String,
    pub motivo: String,
}

// ------------------------------------------------------------------ glob

/// Casa `texto` inteiro com o glob `padrao`: `*` = qualquer coisa menos `/`, `**` = qualquer
/// coisa (inclusive `/`), `?` = um caractere menos `/`.
#[must_use]
pub fn casa_glob(padrao: &str, texto: &str) -> bool {
    glob(padrao.as_bytes(), texto.as_bytes())
}

fn glob(p: &[u8], t: &[u8]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some(b'*') => {
            let dupla = p.get(1) == Some(&b'*');
            let resto = &p[if dupla { 2 } else { 1 }..];
            // `**/` também casa "nenhuma pasta".
            if dupla && resto.first() == Some(&b'/') && glob(&resto[1..], t) {
                return true;
            }
            for i in 0..=t.len() {
                if glob(resto, &t[i..]) {
                    return true;
                }
                if i < t.len() && !dupla && t[i] == b'/' {
                    break;
                }
            }
            false
        }
        Some(b'?') => !t.is_empty() && t[0] != b'/' && glob(&p[1..], &t[1..]),
        Some(c) => t.first() == Some(c) && glob(&p[1..], &t[1..]),
    }
}

/// Expande `~` e `~/` no começo do padrão.
#[must_use]
pub fn expandir_til(padrao: &str, home: Option<&str>) -> String {
    match (padrao.strip_prefix('~'), home) {
        (Some(resto), Some(h)) if resto.is_empty() || resto.starts_with('/') => {
            format!("{}{resto}", h.trim_end_matches('/'))
        }
        _ => padrao.to_owned(),
    }
}

/// O padrão casa o caminho, ou algum diretório acima dele (uma regra para uma pasta vale para
/// tudo que está dentro). Padrão sem `/` casa o nome de qualquer componente.
#[must_use]
pub fn casa_caminho(padrao: &str, caminho: &str, home: Option<&str>) -> bool {
    let padrao = expandir_til(padrao, home);
    if !padrao.contains('/') {
        return caminho
            .split('/')
            .any(|c| !c.is_empty() && casa_glob(&padrao, c));
    }
    let padrao = if padrao.len() > 1 {
        padrao.trim_end_matches('/')
    } else {
        padrao.as_str()
    };
    if !padrao.contains(['*', '?']) {
        return caminho == padrao
            || caminho
                .strip_prefix(padrao)
                .is_some_and(|r| r.starts_with('/'));
    }
    if casa_glob(padrao, caminho) {
        return true;
    }
    // Ancestrais: cada prefixo terminado antes de uma `/`.
    caminho
        .match_indices('/')
        .filter(|(i, _)| *i > 0)
        .any(|(i, _)| casa_glob(padrao, &caminho[..i]))
}

#[cfg(test)]
mod testes {
    use super::*;

    fn esp(codigo: &str, caminho: &str) -> ClasseErro {
        classificar(codigo, Some(caminho.as_bytes()), &[], Some("/home/u")).classe
    }

    #[test]
    fn glob_basico() {
        assert!(casa_glob("*.lock", "a.lock"));
        assert!(!casa_glob("*.lock", "d/a.lock"));
        assert!(casa_glob("**/.cache/**", "/home/u/.cache/x/y"));
        assert!(casa_glob("/var/log/**", "/var/log/syslog"));
        assert!(!casa_glob("/var/log/**", "/var/logs/x"));
        assert!(casa_glob("a?c", "abc"));
        assert!(!casa_glob("a?c", "a/c"));
    }

    #[test]
    fn caminho_vale_para_ancestrais_e_nomes() {
        assert!(casa_caminho(
            "~/.cache",
            "/home/u/.cache/a/b",
            Some("/home/u")
        ));
        assert!(casa_caminho("/mnt/p2/Dev", "/mnt/p2/Dev/x/y", None));
        assert!(!casa_caminho("/mnt/p2/Dev", "/mnt/p2/Devx/y", None));
        assert!(casa_caminho("*-wal", "/d/app.db-wal", None));
        assert!(!casa_caminho("*-wal", "/d/app.db", None));
    }

    #[test]
    fn classificacao_tabelada() {
        let casos: &[(&str, &str, ClasseErro)] = &[
            (
                "hash_alterado_desde_inventario",
                "/var/log/syslog",
                ClasseErro::Esperado,
            ),
            (
                "hash_alterado_durante_leitura",
                "/home/u/.cache/x/y",
                ClasseErro::Esperado,
            ),
            (
                "hash_ausente",
                "/home/u/.bun/install/cache/z",
                ClasseErro::Esperado,
            ),
            (
                "hash_ausente",
                "/home/u/.config/app/db.sqlite-wal",
                ClasseErro::Esperado,
            ),
            ("hash_ausente", "/home/u/proj/x.lock", ClasseErro::Esperado),
            (
                "hash_alterado_desde_inventario",
                "/home/u/.npm/_cacache/a",
                ClasseErro::Esperado,
            ),
            (
                "hash_alterado_desde_inventario",
                "/home/u/.cargo/registry/src/a",
                ClasseErro::Esperado,
            ),
            (
                "hash_ausente",
                "/home/u/.config/google-chrome/Default/Cache/Cache_Data/f",
                ClasseErro::Esperado,
            ),
            // fora de caminho volátil: continua erro até o reteste confirmar
            (
                "hash_alterado_desde_inventario",
                "/home/u/docs/a.odt",
                ClasseErro::Erro,
            ),
            ("hash_permissao", "/etc/shadow", ClasseErro::Erro),
            ("permissao_negada", "/root", ClasseErro::Erro),
            ("hash_io", "/home/u/a", ClasseErro::Erro),
            ("erro_es", "/mnt/x/node_modules/.bin/y", ClasseErro::Erro),
            (
                SYMLINK_ALVO_ILEGIVEL,
                "/mnt/x/node_modules/.bin/y",
                ClasseErro::Esperado,
            ),
            ("limite_de_volume", "/mnt", ClasseErro::Esperado),
            ("volume_pseudo", "/proc", ClasseErro::Esperado),
        ];
        for (codigo, caminho, esperado) in casos {
            assert_eq!(esp(codigo, caminho), *esperado, "{codigo} {caminho}");
        }
    }

    #[test]
    fn regra_do_usuario_vence_e_filtra_por_codigo() {
        let regras = [
            RegraAceita {
                id: 7,
                padrao: "/mnt/p2/Dev".into(),
                codigo: "hash_permissao".into(),
                motivo: "banco de dados do SQL Server".into(),
                criada_em: 0,
            },
            RegraAceita {
                id: 8,
                padrao: "~/.config".into(),
                codigo: "*".into(),
                motivo: "config viva".into(),
                criada_em: 0,
            },
        ];
        let c = |cod: &str, p: &str| classificar(cod, Some(p.as_bytes()), &regras, Some("/home/u"));
        let a = c("hash_permissao", "/mnt/p2/Dev/db/x.mdf");
        assert_eq!(a.classe, ClasseErro::Aceito);
        assert_eq!(a.regra.as_deref(), Some("usuario:7"));
        assert_eq!(
            c("hash_io", "/mnt/p2/Dev/db/x.mdf").classe,
            ClasseErro::Erro
        );
        assert_eq!(
            c("hash_io", "/home/u/.config/a/b").classe,
            ClasseErro::Aceito
        );
    }
}
