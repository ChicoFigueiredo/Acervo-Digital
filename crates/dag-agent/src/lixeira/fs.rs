//! **O único módulo do DAG que muda o disco sob arquivos catalogados** (ADR 0005).
//!
//! Operações permitidas, e só estas:
//!
//! * `renameat2(RENAME_NOREPLACE)` de um arquivo regular catalogado, com Marcação para Deleção,
//!   para dentro de `<ponto de montagem do Volume>/DAG-Lixeira/<caminho relativo>` e de volta
//!   (restaurar). `RENAME_NOREPLACE` faz o kernel recusar (`EEXIST`) em vez de sobrescrever;
//!   nessa colisão o nome ganha um sufixo e tenta de novo.
//! * `mkdir` das pastas que faltam no destino (a própria `DAG-Lixeira` e os subdiretórios).
//!
//! Nunca: apagar, truncar, escrever, copiar, trocar permissões, criar link ou cruzar sistemas de
//! arquivos. Antes de mover, confere no disco que o arquivo ainda é o catalogado (inode, tamanho,
//! mtime, ctime), que é regular e sem outros hard links, e que origem, ponto de montagem e
//! destino estão no mesmo `st_dev` (a conferência de dispositivo é injetável para os testes).

use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use dag_core::lixeira::{destino_na_lixeira, esta_na_lixeira, raiz_da_lixeira, relativo_a};
use dag_store::lixeira::{Candidato, PastaDoDisco};
use rustix::fs::{CWD, RenameFlags, renameat_with};

/// Quem diz em que sistema de arquivos (`st_dev`) uma pasta está. Injetável: os testes simulam
/// pastas em outro Volume sem precisar de um segundo disco.
pub trait Dispositivos: Send + Sync {
    /// `st_dev` da pasta `p` (sem seguir symlink).
    ///
    /// # Errors
    /// Falha de E/S ao consultar.
    fn dev(&self, p: &Path) -> io::Result<u64>;
}

/// Consulta o sistema (`lstat`).
#[derive(Debug, Clone, Copy, Default)]
pub struct DispositivosReais;

impl Dispositivos for DispositivosReais {
    fn dev(&self, p: &Path) -> io::Result<u64> {
        Ok(std::fs::symlink_metadata(p)?.dev())
    }
}

/// Por que um arquivo não foi movido. Nada é alterado em nenhum destes casos.
#[derive(Debug)]
pub enum ErroMover {
    /// O arquivo no disco já não é o catalogado (ou não é um arquivo regular).
    Mudou(String),
    /// Mais de um nome para o mesmo conteúdo físico: mover só um não remove nada.
    HardLink,
    /// Origem, ponto de montagem e Lixeira não estão no mesmo sistema de arquivos.
    OutroVolume(String),
    /// Sem permissão (o helper Polkit para o rename ainda não existe).
    Permissao(String),
    /// Caminho de destino inválido (componente que não é pasta, symlink...).
    Destino(String),
    /// A origem de restauração não está Na Lixeira, ou fora do Volume.
    ForaDaLixeira(String),
    Es(io::Error, String),
}

impl ErroMover {
    /// Código estável.
    #[must_use]
    pub fn codigo(&self) -> &'static str {
        match self {
            Self::Mudou(_) => "mudou_no_disco",
            Self::HardLink => "hard_link",
            Self::OutroVolume(_) => "outro_volume",
            Self::Permissao(_) => "permissao_negada",
            Self::Destino(_) => "destino_invalido",
            Self::ForaDaLixeira(_) => "fora_da_lixeira",
            Self::Es(..) => "es",
        }
    }

    /// Texto para o usuário.
    #[must_use]
    pub fn mensagem(&self) -> String {
        match self {
            Self::Mudou(m) | Self::OutroVolume(m) | Self::Destino(m) | Self::ForaDaLixeira(m) => {
                m.clone()
            }
            Self::HardLink => "o arquivo tem outros nomes (hard links) para o mesmo conteúdo: mover um só não remove nada, então foi recusado".into(),
            Self::Permissao(m) => format!(
                "{m}. O helper do Polkit para mover sem permissão ainda não existe nesta versão: ajuste a permissão da pasta ou mova com outro usuário"
            ),
            Self::Es(e, c) => format!("{c}: {e}"),
        }
    }
}

/// O que o `rename` fez.
#[derive(Debug, Clone)]
pub struct Movido {
    /// Caminho final (bruto).
    pub destino: Vec<u8>,
    /// Pastas do destino (da primeira sob o ponto de montagem até a que contém o arquivo).
    pub pastas: Vec<PastaDoDisco>,
    pub ctime_ns: Option<i64>,
}

fn p(b: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(b))
}

fn ns(s: i64, n: i64) -> i64 {
    s.saturating_mul(1_000_000_000).saturating_add(n)
}

fn mapear(e: io::Error, contexto: &str) -> ErroMover {
    match e.kind() {
        io::ErrorKind::PermissionDenied => ErroMover::Permissao(format!("{contexto}: {e}")),
        _ => match e.raw_os_error() {
            Some(code) if code == rustix::io::Errno::ROFS.raw_os_error() => {
                ErroMover::Permissao(format!(
                    "{contexto}: sistema de arquivos somente leitura para este processo (um agente rodando como serviço systemd usa ProtectHome=read-only; rode `dag agente` fora do serviço ou acrescente ReadWritePaths= do Volume à unidade)"
                ))
            }
            Some(code) if code == rustix::io::Errno::XDEV.raw_os_error() => ErroMover::OutroVolume(
                format!("{contexto}: o destino está em outro sistema de arquivos"),
            ),
            _ => ErroMover::Es(e, contexto.to_string()),
        },
    }
}

/// Confere que o arquivo no disco é o catalogado.
fn conferir_arquivo(c: &Candidato, caminho: &[u8]) -> Result<std::fs::Metadata, ErroMover> {
    let md = std::fs::symlink_metadata(p(caminho)).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => ErroMover::Mudou(format!(
            "{} não existe mais no disco; rode um Fast Scan antes",
            String::from_utf8_lossy(caminho)
        )),
        _ => mapear(e, "lendo o arquivo"),
    })?;
    let nome = String::from_utf8_lossy(caminho).into_owned();
    if !md.file_type().is_file() {
        return Err(ErroMover::Mudou(format!(
            "{nome} não é mais um arquivo regular"
        )));
    }
    if md.nlink() > 1 {
        return Err(ErroMover::HardLink);
    }
    let igual = c.dev.is_none_or(|d| d == md.dev())
        && c.ino.is_none_or(|i| i == md.ino())
        && c.bytes == md.size()
        && c.mtime_ns
            .is_none_or(|m| m == ns(md.mtime(), md.mtime_nsec()))
        && c.ctime_ns
            .is_none_or(|t| t == ns(md.ctime(), md.ctime_nsec()));
    if !igual {
        return Err(ErroMover::Mudou(format!(
            "{nome} mudou no disco depois da última varredura (inode, tamanho ou data); rode um Fast Scan e marque de novo"
        )));
    }
    Ok(md)
}

fn pasta_de(caminho: &[u8]) -> &[u8] {
    match caminho.iter().rposition(|b| *b == b'/') {
        Some(0) => b"/",
        Some(i) => &caminho[..i],
        None => b".",
    }
}

fn nome_de(caminho: &[u8]) -> &[u8] {
    caminho.rsplit(|b| *b == b'/').next().unwrap_or_default()
}

/// Cria (se faltar) cada pasta de `dir` abaixo de `montagem`, conferindo que são pastas de
/// verdade (nunca symlink) no mesmo `st_dev`. Devolve as pastas, da primeira à última.
fn garantir_pastas(
    montagem: &[u8],
    dir: &[u8],
    dev_esperado: u64,
    disp: &dyn Dispositivos,
) -> Result<Vec<PastaDoDisco>, ErroMover> {
    let rel = if dir == montagem {
        Vec::new()
    } else {
        relativo_a(montagem, dir).ok_or_else(|| {
            ErroMover::Destino(format!(
                "{} não está sob o ponto de montagem {}",
                String::from_utf8_lossy(dir),
                String::from_utf8_lossy(montagem)
            ))
        })?
    };
    let mut atual = montagem.to_vec();
    if atual.len() > 1 && atual.ends_with(b"/") {
        atual.pop();
    }
    let mut out = Vec::new();
    for comp in rel.split(|b| *b == b'/').filter(|c| !c.is_empty()) {
        if atual != b"/" {
            atual.push(b'/');
        } else {
            atual.clear();
            atual.push(b'/');
        }
        atual.extend_from_slice(comp);
        let md = match std::fs::symlink_metadata(p(&atual)) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Única criação de pasta do DAG sob um Volume: mkdir, nunca recursivo às cegas.
                std::fs::create_dir(p(&atual)).map_err(|e| {
                    mapear(e, &format!("criando {}", String::from_utf8_lossy(&atual)))
                })?;
                std::fs::symlink_metadata(p(&atual))
                    .map_err(|e| mapear(e, "lendo a pasta criada"))?
            }
            Err(e) => return Err(mapear(e, "lendo a pasta de destino")),
        };
        if !md.file_type().is_dir() {
            return Err(ErroMover::Destino(format!(
                "{} existe e não é uma pasta (ou é um link simbólico)",
                String::from_utf8_lossy(&atual)
            )));
        }
        let dev = disp
            .dev(p(&atual))
            .map_err(|e| mapear(e, "consultando o dispositivo da pasta de destino"))?;
        if dev != dev_esperado {
            return Err(ErroMover::OutroVolume(format!(
                "{} está em outro sistema de arquivos: a Lixeira do Volume tem de ficar no mesmo Volume do arquivo",
                String::from_utf8_lossy(&atual)
            )));
        }
        out.push(PastaDoDisco {
            path_raw: atual.clone(),
            dev: md.dev(),
            ino: md.ino(),
            mtime_ns: Some(ns(md.mtime(), md.mtime_nsec())),
        });
    }
    Ok(out)
}

/// Nome alternativo `n` (>= 1) para colisão: `nome (n+1).ext` na ida, `nome (restaurado[ n]).ext`
/// na volta.
fn nome_com_sufixo(nome: &[u8], n: u32, restaurando: bool) -> Vec<u8> {
    let (base, ext) = match nome.iter().rposition(|b| *b == b'.') {
        Some(i) if i > 0 => (&nome[..i], &nome[i..]),
        _ => (nome, &b""[..]),
    };
    let mut v = base.to_vec();
    let sufixo = if restaurando {
        if n == 1 {
            " (restaurado)".to_string()
        } else {
            format!(" (restaurado {n})")
        }
    } else {
        format!(" ({})", n + 1)
    };
    v.extend_from_slice(sufixo.as_bytes());
    v.extend_from_slice(ext);
    v
}

const MAX_TENTATIVAS: u32 = 1000;

/// `rename` que nunca sobrescreve; na colisão tenta nomes com sufixo.
fn renomear_sem_sobrescrever(
    origem: &[u8],
    dir_destino: &[u8],
    nome: &[u8],
    restaurando: bool,
) -> Result<Vec<u8>, ErroMover> {
    for n in 0..MAX_TENTATIVAS {
        let nome_n = if n == 0 {
            nome.to_vec()
        } else {
            nome_com_sufixo(nome, n, restaurando)
        };
        let mut destino = dir_destino.to_vec();
        if destino != b"/" {
            destino.push(b'/');
        }
        destino.extend_from_slice(&nome_n);
        // RENAME_NOREPLACE: o kernel recusa com EEXIST se o destino existe; nunca sobrescreve.
        match renameat_with(CWD, p(origem), CWD, p(&destino), RenameFlags::NOREPLACE) {
            Ok(()) => return Ok(destino),
            Err(e) if e == rustix::io::Errno::EXIST => {}
            Err(e) if e == rustix::io::Errno::INVAL || e == rustix::io::Errno::NOSYS => {
                return Err(ErroMover::Destino(
                    "o sistema de arquivos não suporta RENAME_NOREPLACE; por segurança nada foi movido"
                        .into(),
                ));
            }
            Err(e) => return Err(mapear(io::Error::from(e), "movendo o arquivo")),
        }
    }
    Err(ErroMover::Destino(format!(
        "não achei nome livre para {} depois de {MAX_TENTATIVAS} tentativas",
        String::from_utf8_lossy(nome)
    )))
}

fn concluir(
    destino: Vec<u8>,
    pastas: Vec<PastaDoDisco>,
    c: &Candidato,
) -> Result<Movido, ErroMover> {
    let md =
        std::fs::symlink_metadata(p(&destino)).map_err(|e| mapear(e, "conferindo o destino"))?;
    // Depois do rename o arquivo é o mesmo (inode); só o ctime muda.
    if c.ino.is_some_and(|i| i != md.ino()) {
        return Err(ErroMover::Mudou(format!(
            "o destino {} não é o arquivo movido",
            String::from_utf8_lossy(&destino)
        )));
    }
    Ok(Movido {
        destino,
        pastas,
        ctime_ns: Some(ns(md.ctime(), md.ctime_nsec())),
    })
}

/// Move `c` para a Lixeira do Volume dele.
///
/// # Errors
/// [`ErroMover`]: nada foi alterado quando falha antes do `rename`.
pub fn mover_para_lixeira(c: &Candidato, disp: &dyn Dispositivos) -> Result<Movido, ErroMover> {
    if c.montagem_raw.is_empty() {
        return Err(ErroMover::Destino(
            "o Volume do arquivo não tem ponto de montagem no Catálogo".into(),
        ));
    }
    let destino = destino_na_lixeira(&c.montagem_raw, &c.path_raw).ok_or_else(|| {
        ErroMover::ForaDaLixeira(format!(
            "{} já está na Lixeira ou fora do Volume",
            String::from_utf8_lossy(&c.path_raw)
        ))
    })?;
    conferir_arquivo(c, &c.path_raw)?;
    // O ponto de montagem e a pasta do arquivo têm de estar no mesmo sistema de arquivos.
    let dev_origem = disp
        .dev(p(pasta_de(&c.path_raw)))
        .map_err(|e| mapear(e, "consultando o dispositivo da origem"))?;
    let dev_montagem = disp
        .dev(p(&c.montagem_raw))
        .map_err(|e| mapear(e, "consultando o ponto de montagem"))?;
    if dev_origem != dev_montagem {
        return Err(ErroMover::OutroVolume(format!(
            "{} está num sistema de arquivos diferente do ponto de montagem do Volume; a Lixeira é por Volume e o DAG nunca copia entre Volumes",
            String::from_utf8_lossy(&c.path_raw)
        )));
    }
    let dir_destino = pasta_de(&destino).to_vec();
    let pastas = garantir_pastas(&c.montagem_raw, &dir_destino, dev_origem, disp)?;
    // A conferência foi há instantes, mas a pasta foi criada depois: confere de novo.
    conferir_arquivo(c, &c.path_raw)?;
    let final_ = renomear_sem_sobrescrever(&c.path_raw, &dir_destino, nome_de(&destino), false)?;
    concluir(final_, pastas, c)
}

/// Devolve `c` (Na Lixeira) para a origem registrada; se estiver ocupada, nome com sufixo.
///
/// # Errors
/// [`ErroMover`]: nada foi alterado quando falha antes do `rename`.
pub fn restaurar(c: &Candidato, disp: &dyn Dispositivos) -> Result<Movido, ErroMover> {
    let origem = c.origem_raw.clone().ok_or_else(|| {
        ErroMover::ForaDaLixeira("o Catálogo não guarda a origem deste arquivo".into())
    })?;
    if !esta_na_lixeira(&c.montagem_raw, &c.path_raw) {
        return Err(ErroMover::ForaDaLixeira(format!(
            "{} não está na {}",
            String::from_utf8_lossy(&c.path_raw),
            String::from_utf8_lossy(&raiz_da_lixeira(&c.montagem_raw))
        )));
    }
    if esta_na_lixeira(&c.montagem_raw, &origem) || relativo_a(&c.montagem_raw, &origem).is_none() {
        return Err(ErroMover::Destino(format!(
            "origem inválida para restaurar: {}",
            String::from_utf8_lossy(&origem)
        )));
    }
    conferir_arquivo(c, &c.path_raw)?;
    let dev_atual = disp
        .dev(p(pasta_de(&c.path_raw)))
        .map_err(|e| mapear(e, "consultando o dispositivo da Lixeira"))?;
    let dev_montagem = disp
        .dev(p(&c.montagem_raw))
        .map_err(|e| mapear(e, "consultando o ponto de montagem"))?;
    if dev_montagem != dev_atual {
        return Err(ErroMover::OutroVolume(
            "a Lixeira e o ponto de montagem estão em sistemas de arquivos diferentes".into(),
        ));
    }
    let dir_origem = pasta_de(&origem).to_vec();
    let pastas = garantir_pastas(&c.montagem_raw, &dir_origem, dev_atual, disp)?;
    conferir_arquivo(c, &c.path_raw)?;
    let final_ = renomear_sem_sobrescrever(&c.path_raw, &dir_origem, nome_de(&origem), true)?;
    concluir(final_, pastas, c)
}
