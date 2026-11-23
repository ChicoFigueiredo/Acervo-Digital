//! Classificação de finalidade por regras de caminho no Linux (A-08) e
//! consolidação de pastas. Caminhos são bytes brutos (nomes não UTF-8 válidos).

use serde::Serialize;

use crate::tipos::Finalidade;

/// Resultado de uma regra: finalidade, regra aplicada e confiança em `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ClassificacaoFinalidade {
    pub finalidade: Finalidade,
    pub regra_id: &'static str,
    pub confianca: f32,
}

const fn r(
    finalidade: Finalidade,
    regra_id: &'static str,
    confianca: f32,
) -> ClassificacaoFinalidade {
    ClassificacaoFinalidade {
        finalidade,
        regra_id,
        confianca,
    }
}

/// Pastas padrão do usuário (inglês e português, com e sem acento).
const PASTAS_USUARIO: &[&[u8]] = &[
    b"Documents",
    "Documentos".as_bytes(),
    b"Pictures",
    b"Imagens",
    b"Music",
    "Música".as_bytes(),
    b"Musica",
    "Músicas".as_bytes(),
    b"Videos",
    "Vídeos".as_bytes(),
    b"Downloads",
    "Transferências".as_bytes(),
    b"Desktop",
    "Área de Trabalho".as_bytes(),
    b"Templates",
    b"Modelos",
    b"Public",
    "Público".as_bytes(),
];

/// Diretórios de ferramentas de desenvolvimento/aplicativos na raiz do home.
const HOME_APLICATIVOS: &[&[u8]] = &[
    b".cargo", b".rustup", b".npm", b".nvm", b".pyenv", b".gradle", b".m2", b".sdkman", b".conda",
    b".var",
];

fn eh(c: &[u8], nome: &[u8]) -> bool {
    c == nome
}

fn qualquer(c: &[u8], nomes: &[&[u8]]) -> bool {
    nomes.contains(&c)
}

/// Prefixo de componentes (`/usr/local` etc.).
fn comeca_com(comps: &[&[u8]], prefixo: &[&[u8]]) -> bool {
    comps.len() >= prefixo.len() && comps.iter().zip(prefixo).all(|(a, b)| a == b)
}

/// Pasta/arquivo relativo a uma raiz de montagem Windows/NTFS ou pasta de usuário.
fn regra_nome_em_montagem(comps: &[&[u8]]) -> Option<ClassificacaoFinalidade> {
    for c in comps.iter().take(3) {
        if qualquer(c, PASTAS_USUARIO) {
            return Some(r(Finalidade::Usuario, "usuario.pasta_padrao_montagem", 0.6));
        }
        if eh(c, b"Windows") {
            return Some(r(Finalidade::Sistema, "sistema.windows_montagem", 0.8));
        }
        if qualquer(
            c,
            &[b"Program Files", b"Program Files (x86)", b"ProgramData"],
        ) {
            return Some(r(
                Finalidade::Aplicativo,
                "aplicativo.windows_montagem",
                0.8,
            ));
        }
        if eh(c, b"Users") {
            return Some(r(Finalidade::Usuario, "usuario.users_montagem", 0.7));
        }
    }
    None
}

/// Regras para o que fica abaixo do diretório pessoal (`rel` = componentes após o home).
fn regra_home(rel: &[&[u8]]) -> ClassificacaoFinalidade {
    let Some(&primeiro) = rel.first() else {
        return r(Finalidade::Usuario, "usuario.home", 0.5);
    };
    if eh(primeiro, b".cache") {
        return r(Finalidade::Aplicativo, "aplicativo.cache_usuario", 0.9);
    }
    if qualquer(primeiro, HOME_APLICATIVOS) {
        return r(
            Finalidade::Aplicativo,
            "aplicativo.ferramentas_usuario",
            0.9,
        );
    }
    if eh(primeiro, b".local") {
        if rel.get(1) == Some(&&b"share"[..]) && rel.get(2) == Some(&&b"flatpak"[..]) {
            return r(Finalidade::Aplicativo, "aplicativo.flatpak_usuario", 0.95);
        }
        if rel.get(1) == Some(&&b"share"[..]) && rel.get(2) == Some(&&b"Trash"[..]) {
            return r(Finalidade::Usuario, "usuario.lixeira", 0.7);
        }
        return r(
            Finalidade::Aplicativo,
            "aplicativo.dados_locais_usuario",
            0.7,
        );
    }
    if eh(primeiro, b".config") {
        return r(Finalidade::Aplicativo, "aplicativo.config_usuario", 0.7);
    }
    if qualquer(primeiro, PASTAS_USUARIO) {
        return r(Finalidade::Usuario, "usuario.pasta_padrao", 0.95);
    }
    if primeiro.first() == Some(&b'.') {
        return r(Finalidade::Aplicativo, "aplicativo.oculto_usuario", 0.55);
    }
    r(Finalidade::Usuario, "usuario.home", 0.6)
}

/// Classifica um caminho absoluto Linux (arquivo ou pasta) por finalidade.
///
/// A primeira regra aplicável vence, da mais específica à mais genérica.
/// Caminhos relativos ou sem regra resultam em `Desconhecido` com baixa confiança.
#[must_use]
pub fn classificar_finalidade_caminho(caminho: &[u8]) -> ClassificacaoFinalidade {
    if caminho.first() != Some(&b'/') {
        return r(Finalidade::Desconhecido, "caminho.relativo", 0.0);
    }
    let comps: Vec<&[u8]> = caminho
        .split(|&b| b == b'/')
        .filter(|c| !c.is_empty() && *c != b".")
        .collect();
    let Some(&raiz) = comps.first() else {
        return r(Finalidade::Desconhecido, "raiz", 0.3);
    };

    // Dependências embutidas em qualquer lugar (mais específico que a localização).
    if comps.iter().any(|c| eh(c, b"node_modules")) {
        return r(Finalidade::Aplicativo, "aplicativo.node_modules", 0.9);
    }
    if comps
        .iter()
        .any(|c| qualquer(c, &[b"venv", b".venv", b"site-packages"]))
    {
        return r(Finalidade::Aplicativo, "aplicativo.ambiente_virtual", 0.85);
    }

    // Diretórios pessoais.
    if eh(raiz, b"home") {
        if comps.len() == 1 {
            return r(Finalidade::Desconhecido, "usuario.home_raiz", 0.3);
        }
        if eh(comps[1], b"Shared") {
            return r(Finalidade::Compartilhado, "compartilhado.home_shared", 0.85);
        }
        return regra_home(&comps[2..]);
    }
    if eh(raiz, b"root") {
        let c = regra_home(&comps[1..]);
        return if c.regra_id == "usuario.home" {
            r(Finalidade::Usuario, "usuario.home_root", 0.7)
        } else {
            c
        };
    }

    // Aplicativos (antes de /usr e /var/lib, mais genéricos).
    if comeca_com(&comps, &[b"usr", b"local"])
        || comeca_com(&comps, &[b"var", b"lib", b"flatpak"])
        || comeca_com(&comps, &[b"var", b"lib", b"snapd"])
        || comeca_com(&comps, &[b"var", b"snap"])
        || eh(raiz, b"snap")
        || eh(raiz, b"opt")
    {
        let (id, conf) = match raiz {
            b"opt" => ("aplicativo.opt", 0.85),
            b"snap" => ("aplicativo.snap", 0.95),
            b"usr" => ("aplicativo.usr_local", 0.85),
            _ => ("aplicativo.pacotes_sandbox", 0.95),
        };
        return r(Finalidade::Aplicativo, id, conf);
    }

    // Compartilhado.
    if eh(raiz, b"srv") {
        return r(Finalidade::Compartilhado, "compartilhado.srv", 0.8);
    }
    if comeca_com(&comps, &[b"var", b"www"]) {
        return r(Finalidade::Compartilhado, "compartilhado.var_www", 0.8);
    }

    // Sistema.
    if comeca_com(&comps, &[b"var", b"cache"]) {
        return r(Finalidade::Sistema, "sistema.var_cache", 0.85);
    }
    if comeca_com(&comps, &[b"var", b"lib"]) {
        return r(Finalidade::Sistema, "sistema.var_lib", 0.85);
    }
    if comeca_com(&comps, &[b"var", b"log"])
        || comeca_com(&comps, &[b"var", b"spool"])
        || comeca_com(&comps, &[b"var", b"run"])
    {
        return r(Finalidade::Sistema, "sistema.var_dinamico", 0.85);
    }
    let raiz_str = raiz;
    if qualquer(
        raiz_str,
        &[
            b"usr",
            b"bin",
            b"sbin",
            b"lib",
            b"lib32",
            b"lib64",
            b"libx32",
            b"boot",
            b"etc",
            b"efi",
            b"lost+found",
        ],
    ) {
        return r(Finalidade::Sistema, "sistema.hierarquia_fhs", 0.95);
    }
    if qualquer(raiz_str, &[b"proc", b"sys", b"dev", b"run"]) {
        return r(Finalidade::Sistema, "sistema.pseudo_fs", 0.95);
    }
    if eh(raiz, b"tmp") || comeca_com(&comps, &[b"var", b"tmp"]) {
        return r(Finalidade::Desconhecido, "temporario", 0.4);
    }

    // Montagens removíveis: desconhecido, salvo regra mais profunda.
    if qualquer(raiz, &[b"media", b"mnt"]) {
        return regra_nome_em_montagem(&comps[1..])
            .unwrap_or_else(|| r(Finalidade::Desconhecido, "montagem_removivel", 0.3));
    }

    r(Finalidade::Desconhecido, "sem_regra", 0.1)
}

/// Fração mínima (por bytes) para uma finalidade dominar uma pasta.
pub const LIMIAR_DOMINANCIA: f32 = 0.8;

const ORDEM: [Finalidade; 6] = [
    Finalidade::Sistema,
    Finalidade::Aplicativo,
    Finalidade::Usuario,
    Finalidade::Compartilhado,
    Finalidade::Misto,
    Finalidade::Desconhecido,
];

/// Parcela de uma finalidade dentro de uma pasta.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ProporcaoFinalidade {
    pub finalidade: Finalidade,
    pub arquivos: u64,
    pub bytes: u64,
    /// Fração por quantidade de arquivos (`0..=1`).
    pub fracao_arquivos: f32,
    /// Fração por bytes (`0..=1`).
    pub fracao_bytes: f32,
}

/// Resultado da consolidação de uma pasta.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConsolidacaoPasta {
    pub finalidade: Finalidade,
    pub confianca: f32,
    /// Somente finalidades presentes, em ordem fixa do enum.
    pub proporcoes: Vec<ProporcaoFinalidade>,
}

fn fracao(parte: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (parte as f64 / total as f64) as f32
    }
}

/// Consolida contagens `(finalidade, arquivos, bytes)` de uma pasta.
///
/// A dominância é medida por bytes (por arquivos quando o total de bytes é zero).
/// Se a maior finalidade tiver ao menos [`LIMIAR_DOMINANCIA`], a pasta recebe
/// essa finalidade com confiança igual à fração; caso contrário é `Misto`, com
/// confiança entre 0.5 (perto do limiar) e 1.0 (muito fragmentada). Pasta vazia
/// é `Desconhecido` com confiança 0.
#[must_use]
pub fn consolidar_finalidade_pasta(contagens: &[(Finalidade, u64, u64)]) -> ConsolidacaoPasta {
    let mut soma = [(0u64, 0u64); 6];
    for &(f, arq, bytes) in contagens {
        let i = ORDEM.iter().position(|&o| o == f).unwrap_or(5);
        soma[i].0 = soma[i].0.saturating_add(arq);
        soma[i].1 = soma[i].1.saturating_add(bytes);
    }
    let total_arq = soma.iter().fold(0u64, |a, s| a.saturating_add(s.0));
    let total_bytes = soma.iter().fold(0u64, |a, s| a.saturating_add(s.1));

    let proporcoes: Vec<ProporcaoFinalidade> = ORDEM
        .iter()
        .zip(soma)
        .filter(|(_, (a, b))| *a > 0 || *b > 0)
        .map(|(&finalidade, (arquivos, bytes))| ProporcaoFinalidade {
            finalidade,
            arquivos,
            bytes,
            fracao_arquivos: fracao(arquivos, total_arq),
            fracao_bytes: fracao(bytes, total_bytes),
        })
        .collect();

    if proporcoes.is_empty() {
        return ConsolidacaoPasta {
            finalidade: Finalidade::Desconhecido,
            confianca: 0.0,
            proporcoes,
        };
    }

    let medida = |p: &ProporcaoFinalidade| {
        if total_bytes > 0 {
            p.fracao_bytes
        } else {
            p.fracao_arquivos
        }
    };
    let topo = proporcoes.iter().fold(
        proporcoes[0],
        |m, p| if medida(p) > medida(&m) { *p } else { m },
    );
    let frac = medida(&topo);

    let (finalidade, confianca) = if frac >= LIMIAR_DOMINANCIA {
        (topo.finalidade, frac)
    } else {
        let c = 0.5 + 0.5 * ((LIMIAR_DOMINANCIA - frac) / LIMIAR_DOMINANCIA);
        (Finalidade::Misto, c.clamp(0.5, 1.0))
    };
    ConsolidacaoPasta {
        finalidade,
        confianca,
        proporcoes,
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use Finalidade::{Aplicativo, Compartilhado, Desconhecido, Misto, Sistema, Usuario};

    fn c(caminho: &[u8]) -> (Finalidade, &'static str, f32) {
        let r = classificar_finalidade_caminho(caminho);
        (r.finalidade, r.regra_id, r.confianca)
    }

    #[test]
    fn sistema() {
        for p in [
            &b"/usr/share/doc"[..],
            b"/bin/ls",
            b"/sbin",
            b"/lib64/ld.so",
            b"/lib/x86_64-linux-gnu",
            b"/boot/vmlinuz",
            b"/etc/passwd",
            b"/var/lib/dpkg/status",
            b"/var/cache/apt",
            b"/var/log/syslog",
            b"/proc/1",
            b"/dev/null",
        ] {
            assert_eq!(c(p).0, Sistema, "{}", String::from_utf8_lossy(p));
        }
        assert!(c(b"/etc/passwd").2 >= 0.9);
    }

    #[test]
    fn aplicativo() {
        for p in [
            &b"/opt/google/chrome"[..],
            b"/usr/local/bin/foo",
            b"/snap/core/1",
            b"/var/lib/flatpak/app",
            b"/var/lib/snapd/snaps",
            b"/home/ana/.local/share/flatpak/app",
            b"/home/ana/.cargo/registry",
            b"/home/ana/.rustup/toolchains",
            b"/home/ana/Documents/proj/node_modules/x/index.js",
            b"/home/ana/proj/venv/bin/python",
            b"/home/ana/proj/.venv/lib",
        ] {
            assert_eq!(c(p).0, Aplicativo, "{}", String::from_utf8_lossy(p));
        }
        // /var/lib geral continua Sistema.
        assert_eq!(c(b"/var/lib/dpkg").0, Sistema);
    }

    #[test]
    fn cache_do_usuario_tem_razao_cache() {
        let (f, id, conf) = c(b"/home/ana/.cache/mozilla");
        assert_eq!(f, Aplicativo);
        assert!(id.contains("cache"));
        assert!(conf >= 0.9);
    }

    #[test]
    fn usuario() {
        for p in [
            &b"/home/ana/Documents/a.txt"[..],
            b"/home/ana/Documentos",
            b"/home/ana/Pictures/2020",
            b"/home/ana/Imagens",
            b"/home/ana/Music",
            b"/home/ana/Videos",
            b"/home/ana/Downloads/x.iso",
            b"/home/ana/Desktop",
            "/home/ana/Área de Trabalho/x".as_bytes(),
            b"/root/anotacoes.txt",
        ] {
            assert_eq!(c(p).0, Usuario, "{}", String::from_utf8_lossy(p));
        }
        let (f, _, conf_pasta) = c(b"/home/ana/Documents");
        let (f2, _, conf_geral) = c(b"/home/ana/projetos/x");
        assert_eq!((f, f2), (Usuario, Usuario));
        assert!(conf_geral < conf_pasta, "home em geral tem confiança menor");
        assert_eq!(c(b"/home/ana").0, Usuario);
    }

    #[test]
    fn compartilhado() {
        assert_eq!(c(b"/srv/ftp").0, Compartilhado);
        assert_eq!(c(b"/var/www/html").0, Compartilhado);
        assert_eq!(c(b"/home/Shared/fotos").0, Compartilhado);
    }

    #[test]
    fn montagens_removiveis_desconhecidas_salvo_regra_mais_profunda() {
        assert_eq!(c(b"/media/ana/PENDRIVE/x.bin").0, Desconhecido);
        assert_eq!(c(b"/mnt/disco").0, Desconhecido);
        assert_eq!(c(b"/media/ana/PENDRIVE/Documents/a.txt").0, Usuario);
        assert_eq!(c(b"/mnt/win/Windows/System32").0, Sistema);
        assert_eq!(c(b"/mnt/win/Program Files/App").0, Aplicativo);
        assert_eq!(c(b"/media/ana/DISCO/proj/node_modules/a").0, Aplicativo);
    }

    #[test]
    fn caminho_nao_utf8() {
        assert_eq!(c(b"/home/ana/Documents/\xff\xfe\x80.txt").0, Usuario);
        assert_eq!(c(b"/home/jo\xe3o/.cache/\xff").0, Aplicativo);
        assert_eq!(c(b"/usr/lib/\xc0\xaf").0, Sistema);
        assert_eq!(c(b"/opt/\xff/bin").0, Aplicativo);
        assert_eq!(c(b"/mnt/\xff/Downloads").0, Usuario);
    }

    #[test]
    fn relativo_vazio_e_sem_regra() {
        assert_eq!(c(b"relativo/x").0, Desconhecido);
        assert_eq!(c(b"").0, Desconhecido);
        assert_eq!(c(b"/").0, Desconhecido);
        assert_eq!(c(b"/algo/estranho").1, "sem_regra");
        assert_eq!(c(b"//usr//bin/").0, Sistema, "barras duplicadas");
        assert_eq!(c(b"/tmp/x").0, Desconhecido);
    }

    #[test]
    fn confianca_sempre_no_intervalo() {
        for p in [
            &b"/usr"[..],
            b"/home/a/.config/x",
            b"/home/a/.ssh/id",
            b"/zzz",
        ] {
            let conf = c(p).2;
            assert!((0.0..=1.0).contains(&conf));
        }
    }

    #[test]
    fn rollup_dominante_por_bytes() {
        let r = consolidar_finalidade_pasta(&[(Usuario, 1, 900), (Aplicativo, 99, 100)]);
        assert_eq!(
            r.finalidade, Usuario,
            "domina por bytes mesmo com poucos arquivos"
        );
        assert!((r.confianca - 0.9).abs() < 1e-6);
        assert_eq!(r.proporcoes.len(), 2);
        let ap = r
            .proporcoes
            .iter()
            .find(|p| p.finalidade == Aplicativo)
            .unwrap();
        assert!((ap.fracao_arquivos - 0.99).abs() < 1e-6);
        assert!((ap.fracao_bytes - 0.1).abs() < 1e-6);
    }

    #[test]
    fn rollup_misto_quando_ninguem_domina() {
        let r = consolidar_finalidade_pasta(&[(Usuario, 10, 600), (Aplicativo, 10, 400)]);
        assert_eq!(r.finalidade, Misto);
        assert!((0.5..=1.0).contains(&r.confianca));
        let soma: f32 = r.proporcoes.iter().map(|p| p.fracao_bytes).sum();
        assert!((soma - 1.0).abs() < 1e-5);
    }

    #[test]
    fn rollup_limiar_exato_e_logo_abaixo() {
        let r = consolidar_finalidade_pasta(&[(Usuario, 1, 80), (Sistema, 1, 20)]);
        assert_eq!(r.finalidade, Usuario);
        let r = consolidar_finalidade_pasta(&[(Usuario, 1, 79), (Sistema, 1, 21)]);
        assert_eq!(r.finalidade, Misto);
    }

    #[test]
    fn rollup_vazio_e_bytes_zero_usa_contagem() {
        let r = consolidar_finalidade_pasta(&[]);
        assert_eq!((r.finalidade, r.confianca), (Desconhecido, 0.0));
        assert!(r.proporcoes.is_empty());
        let r = consolidar_finalidade_pasta(&[(Usuario, 9, 0), (Sistema, 1, 0)]);
        assert_eq!(r.finalidade, Usuario);
        let r = consolidar_finalidade_pasta(&[(Usuario, 5, 0), (Sistema, 5, 0)]);
        assert_eq!(r.finalidade, Misto);
    }

    #[test]
    fn rollup_agrega_repetidos_e_nao_estoura() {
        let r = consolidar_finalidade_pasta(&[
            (Sistema, 1, u64::MAX),
            (Sistema, 1, u64::MAX),
            (Usuario, 1, 1),
        ]);
        assert_eq!(r.finalidade, Sistema);
        assert_eq!(r.proporcoes[0].arquivos, 2);
    }

    #[test]
    fn rollup_misto_e_desconhecido_sao_categorias_proprias() {
        let r = consolidar_finalidade_pasta(&[(Misto, 1, 95), (Usuario, 1, 5)]);
        assert_eq!(r.finalidade, Misto);
        let r = consolidar_finalidade_pasta(&[(Desconhecido, 1, 90), (Usuario, 1, 10)]);
        assert_eq!(r.finalidade, Desconhecido);
    }
}
