//! Duplicados, Marcações, Cobertura e espaço recuperável (A-24..A-31) sobre fixtures diretas no
//! Catálogo: volumes, dispositivos e hashes inseridos à mão para simular vários discos.
#![allow(clippy::unwrap_used)]

use dag_core::{
    Alvo, ArquivoId, Autor, ClasseRecuperavel, DadosRelatorio, EstadoMarcacao, GrupoId, MarcacaoId,
    NivelEspaco, TipoMarcacao, TipoRelatorio,
};
use dag_store::Catalogo;
use dag_store::duplicados::{
    self, ErroDuplicados, Escopo, FiltroGrupos, definir_copia_principal, definir_prioridade,
    detalhar_grupo, listar_grupos, reconstruir_grupos,
};
use rusqlite::params;

const T: i64 = 1_000_000;

fn h(n: u8) -> [u8; 32] {
    [n; 32]
}

struct Fx {
    c: Catalogo,
}

impl Fx {
    fn novo() -> Self {
        let c = Catalogo::em_memoria().unwrap();
        c.armazem()
            .conexao()
            .execute(
                "INSERT INTO scan_runs (mode, started_at) VALUES ('full', 1)",
                [],
            )
            .unwrap();
        Self { c }
    }

    fn conn(&self) -> &rusqlite::Connection {
        self.c.armazem().conexao()
    }

    fn dispositivo(&self, nome: &str) -> i64 {
        self.conn()
            .query_row(
                "INSERT INTO dispositivos (identificador, tipo, criado_em) VALUES (?1, 'ssd', 1) RETURNING id",
                [nome],
                |l| l.get(0),
            )
            .unwrap()
    }

    fn volume(&self, rotulo: &str, dev: Option<i64>, conectado: bool) -> i64 {
        self.conn()
            .query_row(
                "INSERT INTO volumes (platform_id, device_id, rotulo, mounted, last_seen_at)
                 VALUES (?1, ?2, ?1, ?3, ?4) RETURNING id",
                params![rotulo, dev, i64::from(conectado), 5000],
                |l| l.get(0),
            )
            .unwrap()
    }

    fn pasta(&self, vol: i64, pai: Option<i64>, caminho: &str) -> i64 {
        self.conn()
            .query_row(
                "INSERT INTO folders (volume_id, parent_folder_id, path_raw, path_display, name_raw, name_display)
                 VALUES (?1, ?2, CAST(?3 AS BLOB), ?3, CAST(?3 AS BLOB), ?3) RETURNING id",
                params![vol, pai, caminho],
                |l| l.get(0),
            )
            .unwrap()
    }

    /// Arquivo com hash válido. `inode` identifica o arquivo físico.
    fn arq(
        &self,
        vol: i64,
        pasta: i64,
        caminho: &str,
        tam: i64,
        inode: i64,
        hash: Option<[u8; 32]>,
    ) -> i64 {
        let id: i64 = self
            .conn()
            .query_row(
                "INSERT INTO files (volume_id, folder_id, path_raw, path_display, name_raw, name_display,
                                    native_dev, native_file_id, nlink, logical_bytes, mtime_ns, ctime_ns, seen_run_id)
                 VALUES (?1, ?2, CAST(?3 AS BLOB), ?3, CAST(?3 AS BLOB), ?3, 1, ?4, 1, ?5, 1000, 1000, 1) RETURNING id",
                params![vol, pasta, caminho, inode, tam],
                |l| l.get(0),
            )
            .unwrap();
        if let Some(hs) = hash {
            self.dar_hash(id, hs);
        }
        id
    }

    fn dar_hash(&self, id: i64, hs: [u8; 32]) {
        self.conn()
            .execute(
                "INSERT OR REPLACE INTO hashes (file_id, hash, hashed_at, size_bytes, mtime_ns, ctime_ns, native_file_id)
                 SELECT id, ?2, 1, logical_bytes, mtime_ns, ctime_ns, native_file_id FROM files WHERE id = ?1",
                params![id, hs.as_slice()],
            )
            .unwrap();
    }

    /// Simula conteúdo novo relido: muda mtime/ctime e regrava o hash.
    fn novo_conteudo(&self, id: i64, hs: [u8; 32]) {
        self.conn()
            .execute(
                "UPDATE files SET mtime_ns = 2000, ctime_ns = 2000 WHERE id = ?1",
                [id],
            )
            .unwrap();
        self.dar_hash(id, hs);
    }

    /// `n` caminhos do mesmo arquivo físico (hard links).
    #[allow(clippy::too_many_arguments)]
    fn hardlinks(
        &self,
        vol: i64,
        pasta: i64,
        base: &str,
        n: usize,
        tam: i64,
        inode: i64,
        hs: [u8; 32],
    ) -> Vec<i64> {
        let ids: Vec<i64> = (0..n)
            .map(|i| self.arq(vol, pasta, &format!("{base}{i}"), tam, inode, Some(hs)))
            .collect();
        self.conn()
            .execute(
                "UPDATE files SET hardlink_group = ?1, nlink = ?2 WHERE id IN (SELECT id FROM files WHERE native_file_id = ?3 AND volume_id = ?4)",
                params![ids[0], n as i64, inode, vol],
            )
            .unwrap();
        ids
    }

    fn reconstruir(&mut self) -> duplicados::ResumoReconstrucao {
        reconstruir_grupos(&mut self.c, Escopo::Tudo, Autor::Cli, T).unwrap()
    }

    fn grupos(&self) -> Vec<dag_core::GrupoDuplicados> {
        listar_grupos(self.conn(), &FiltroGrupos::default(), 1000, None)
            .unwrap()
            .grupos
    }

    fn marcar(
        &mut self,
        tipo: TipoMarcacao,
        alvo: Alvo,
    ) -> Result<dag_core::ResultadoMarcacao, ErroDuplicados> {
        duplicados::marcar(&mut self.c, tipo, &alvo, Autor::Cli, T + 1)
    }

    fn contar(&self, sql: &str) -> i64 {
        self.conn().query_row(sql, [], |l| l.get(0)).unwrap()
    }
}

fn arq_alvo(id: i64) -> Alvo {
    Alvo::Arquivo(ArquivoId(id))
}

// ---------------------------------------------------------------------- CA-A19

#[test]
fn ca_a19_tres_hard_links_de_10_mb_nao_formam_grupo_nem_espaco() {
    let mut f = Fx::novo();
    let d = f.dispositivo("d1");
    let v = f.volume("SSD", Some(d), true);
    let p = f.pasta(v, None, "/s");
    let tam = 10 * 1024 * 1024;
    f.hardlinks(v, p, "/s/link", 3, tam, 77, h(1));
    let r = f.reconstruir();
    assert_eq!(r.grupos_criados, 0);
    assert!(f.grupos().is_empty());
    let e = duplicados::espaco_de_grupos(f.conn()).unwrap();
    assert_eq!(e.computador.total(), 0);
}

#[test]
fn hard_links_contam_como_uma_copia_dentro_de_um_grupo_real() {
    let mut f = Fx::novo();
    let (d1, d2) = (f.dispositivo("d1"), f.dispositivo("d2"));
    let ssd = f.volume("SSD", Some(d1), true);
    let hd = f.volume("HD", Some(d2), true);
    let ps = f.pasta(ssd, None, "/s");
    let ph = f.pasta(hd, None, "/h");
    let links = f.hardlinks(ssd, ps, "/s/l", 3, 1000, 77, h(1));
    let b = f.arq(hd, ph, "/h/copia", 1000, 5, Some(h(1)));
    f.reconstruir();
    let gs = f.grupos();
    assert_eq!(gs.len(), 1);
    assert_eq!(
        gs[0].copias, 2,
        "3 hard links + 1 cópia = 2 arquivos físicos"
    );
    // Só o representante (menor id) é membro; recuperável = 1 cópia redundante (1000), não 3.
    let d = detalhar_grupo(f.conn(), gs[0].id).unwrap();
    let ids: Vec<i64> = d.copias.iter().map(|c| c.arquivo.0).collect();
    assert_eq!(ids, vec![links[0], b]);
    assert_eq!(d.copias[0].outros_caminhos, 2);
    assert_eq!(gs[0].espaco_recuperavel_bytes, 1000);
    // Marcar um dos outros caminhos resolve para o arquivo físico.
    let r = f.marcar(TipoMarcacao::Backup, arq_alvo(links[2])).unwrap();
    assert_eq!(r.marcacoes[0].arquivo, Some(ArquivoId(links[0])));
}

// ---------------------------------------------------------------------- CA-A20

#[test]
fn ca_a20_grupos_entre_pastas_volumes_dispositivos_e_offline() {
    let mut f = Fx::novo();
    let (d1, d2, d3) = (
        f.dispositivo("d1"),
        f.dispositivo("d2"),
        f.dispositivo("d3"),
    );
    let ssd = f.volume("SSD", Some(d1), true);
    let hd = f.volume("HD", Some(d2), true);
    let off = f.volume("PENDRIVE", Some(d3), false);
    let ps = f.pasta(ssd, None, "/s");
    let ps2 = f.pasta(ssd, Some(ps), "/s/outra");
    let ph = f.pasta(hd, None, "/h");
    let po = f.pasta(off, None, "/o");
    let a = f.arq(ssd, ps, "/s/a", 500, 1, Some(h(1)));
    let b = f.arq(ssd, ps2, "/s/outra/b", 500, 2, Some(h(1)));
    let c = f.arq(hd, ph, "/h/c", 500, 3, Some(h(1)));
    let o = f.arq(off, po, "/o/d", 500, 4, Some(h(1)));
    // mesmo tamanho, conteúdo diferente: fora
    let e = f.arq(ssd, ps, "/s/e", 500, 5, Some(h(2)));
    // vazios iguais: fora
    f.arq(ssd, ps, "/s/v1", 0, 6, Some(h(9)));
    f.arq(hd, ph, "/h/v2", 0, 7, Some(h(9)));
    // mesmo hash, tamanho diferente: fora (a chave é tamanho + hash)
    f.arq(hd, ph, "/h/outro_tam", 501, 8, Some(h(1)));
    // sem hash: fora
    f.arq(hd, ph, "/h/sem_hash", 500, 9, None);
    f.reconstruir();
    let gs = f.grupos();
    assert_eq!(gs.len(), 1);
    assert_eq!(gs[0].copias, 4);
    assert_eq!(gs[0].copias_offline, 1);
    let d = detalhar_grupo(f.conn(), gs[0].id).unwrap();
    let ids: Vec<i64> = d.copias.iter().map(|x| x.arquivo.0).collect();
    assert_eq!(ids, vec![a, b, c, o]);
    assert!(!ids.contains(&e));
    let offline = d.copias.iter().find(|x| x.arquivo.0 == o).unwrap();
    assert!(!offline.conectado);
    assert_eq!(offline.visto_em, Some(5000));
    // Reconstruir de novo não muda nada e mantém o id.
    let r = f.reconstruir();
    assert_eq!(
        (
            r.grupos_criados,
            r.grupos_removidos,
            r.membros_adicionados,
            r.membros_removidos
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(f.grupos()[0].id, gs[0].id);
}

#[test]
fn reconstrucao_incremental_acompanha_mudancas() {
    let mut f = Fx::novo();
    let (d1, d2) = (f.dispositivo("d1"), f.dispositivo("d2"));
    let ssd = f.volume("SSD", Some(d1), true);
    let hd = f.volume("HD", Some(d2), true);
    let ps = f.pasta(ssd, None, "/s");
    let ph = f.pasta(hd, None, "/h");
    let a = f.arq(ssd, ps, "/s/a", 500, 1, Some(h(1)));
    let b = f.arq(hd, ph, "/h/b", 500, 2, Some(h(1)));
    f.reconstruir();
    let id = f.grupos()[0].id;
    // chega uma terceira cópia
    let c = f.arq(hd, ph, "/h/c", 500, 3, Some(h(1)));
    let r = f.reconstruir();
    assert_eq!((r.grupos_criados, r.membros_adicionados), (0, 1));
    assert_eq!(f.grupos()[0].id, id);
    assert_eq!(f.grupos()[0].copias, 3);
    // uma some (removida): sai do grupo
    f.conn()
        .execute("UPDATE files SET state = 'removido' WHERE id = ?1", [c])
        .unwrap();
    let r = f.reconstruir();
    assert_eq!(r.membros_removidos, 1);
    assert_eq!(f.grupos()[0].copias, 2);
    // restando uma só, o grupo se desfaz
    f.conn()
        .execute("UPDATE files SET state = 'removido' WHERE id = ?1", [b])
        .unwrap();
    let r = f.reconstruir();
    assert_eq!(r.grupos_removidos, 1);
    assert!(f.grupos().is_empty());
    let _ = a;
    // O escopo por hash só mexe nos grupos pedidos.
    let x = f.arq(ssd, ps, "/s/x", 70, 10, Some(h(7)));
    let y = f.arq(hd, ph, "/h/y", 70, 11, Some(h(7)));
    let _ = (x, y);
    let r = reconstruir_grupos(&mut f.c, Escopo::Hashes(&[(70, h(8))]), Autor::Cli, T).unwrap();
    assert_eq!(r.grupos_criados, 0);
    let r = reconstruir_grupos(&mut f.c, Escopo::Hashes(&[(70, h(7))]), Autor::Cli, T).unwrap();
    assert_eq!(r.grupos_criados, 1);
}

// ---------------------------------------------------------------------- CA-A21

#[test]
fn ca_a21_prioridade_escolhe_ssd_e_troca_manual_persiste() {
    let mut f = Fx::novo();
    let (d1, d2) = (f.dispositivo("d1"), f.dispositivo("d2"));
    // O HD é criado primeiro: sem prioridade, o menor id (HD) venceria.
    let hd = f.volume("HD-Externo", Some(d2), true);
    let ssd = f.volume("SSD", Some(d1), true);
    let ph = f.pasta(hd, None, "/h");
    let ps = f.pasta(ssd, None, "/s");
    let mut pares = Vec::new();
    for i in 0..3u8 {
        let a = f.arq(
            hd,
            ph,
            &format!("/h/f{i}"),
            100,
            i64::from(i),
            Some(h(i + 1)),
        );
        let b = f.arq(
            ssd,
            ps,
            &format!("/s/f{i}"),
            100,
            10 + i64::from(i),
            Some(h(i + 1)),
        );
        pares.push((a, b));
    }
    f.reconstruir();
    assert!(
        f.grupos()
            .iter()
            .all(|g| g.modo_principal == dag_core::ModoPrincipal::Auto)
    );
    // Prioridade SSD > HD-Externo.
    definir_prioridade(
        &mut f.c,
        &["SSD".into(), "HD-Externo".into()],
        Autor::Cli,
        T,
    )
    .unwrap();
    for g in f.grupos() {
        let p = g.copia_principal.unwrap().0;
        assert!(
            pares.iter().any(|x| x.1 == p),
            "principal deveria ser a do SSD"
        );
    }
    // Troca manual de um grupo para a cópia do HD.
    let g0 = f.grupos()[0].id;
    let hd_file = detalhar_grupo(f.conn(), g0)
        .unwrap()
        .copias
        .iter()
        .map(|c| c.arquivo.0)
        .find(|a| pares.iter().any(|p| p.0 == *a))
        .unwrap();
    definir_copia_principal(&mut f.c, g0, Some(&arq_alvo(hd_file)), T).unwrap();
    // Reconstruções (Fast Scan) e nova prioridade não desfazem a escolha manual.
    f.reconstruir();
    definir_prioridade(
        &mut f.c,
        &["SSD".into(), "HD-Externo".into()],
        Autor::Cli,
        T,
    )
    .unwrap();
    let g = f.grupos().into_iter().find(|g| g.id == g0).unwrap();
    assert_eq!(g.copia_principal, Some(ArquivoId(hd_file)));
    assert_eq!(g.modo_principal, dag_core::ModoPrincipal::Manual);
    // Cópia que não é do grupo é recusada; voltar ao automático devolve o SSD.
    let outra = pares.iter().find(|x| x.0 != hd_file).unwrap().0;
    assert!(matches!(
        definir_copia_principal(&mut f.c, g0, Some(&arq_alvo(outra)), T),
        Err(ErroDuplicados::Invalido(_))
    ));
    definir_copia_principal(&mut f.c, g0, None, T).unwrap();
    let g = f.grupos().into_iter().find(|g| g.id == g0).unwrap();
    assert_eq!(g.modo_principal, dag_core::ModoPrincipal::Auto);
    assert_ne!(g.copia_principal, Some(ArquivoId(hd_file)));
}

#[test]
fn prioridade_por_pasta_vence_a_do_volume() {
    let mut f = Fx::novo();
    let (d1, d2) = (f.dispositivo("d1"), f.dispositivo("d2"));
    let ssd = f.volume("SSD", Some(d1), true);
    let hd = f.volume("HD", Some(d2), true);
    let ps = f.pasta(ssd, None, "/s");
    let ph = f.pasta(hd, None, "/h");
    let curadas = f.pasta(hd, Some(ph), "/h/curadas");
    f.arq(ssd, ps, "/s/a", 100, 1, Some(h(1)));
    let b = f.arq(hd, curadas, "/h/curadas/a", 100, 2, Some(h(1)));
    // volume SSD > HD, mas a pasta /h/curadas vem antes de tudo
    definir_prioridade(
        &mut f.c,
        &["/h/curadas".into(), "SSD".into(), "HD".into()],
        Autor::Cli,
        T,
    )
    .unwrap();
    f.reconstruir();
    assert_eq!(f.grupos()[0].copia_principal, Some(ArquivoId(b)));
    // item repetido ou inexistente é recusado
    assert!(definir_prioridade(&mut f.c, &["SSD".into(), "ssd".into()], Autor::Cli, T).is_err());
    assert!(matches!(
        definir_prioridade(&mut f.c, &["Fantasma".into()], Autor::Cli, T),
        Err(ErroDuplicados::NaoEncontrado(_))
    ));
}

// ---------------------------------------------------------------------- CA-A22

struct Cobertura {
    f: Fx,
    ssd: i64,
    ssd_p2: i64,
    hd: i64,
    x_ssd: i64,
    x_hd: i64,
    y_ssd: i64,
    y_ssd_p2: i64,
}

fn cenario_cobertura() -> Cobertura {
    let mut f = Fx::novo();
    let (d1, d2) = (f.dispositivo("disco1"), f.dispositivo("disco2"));
    let ssd = f.volume("SSD", Some(d1), true);
    let ssd_p2 = f.volume("SSD-P2", Some(d1), true); // outra partição do MESMO disco
    let hd = f.volume("HD-Externo", Some(d2), true);
    let ps = f.pasta(ssd, None, "/s");
    let ps2 = f.pasta(ssd_p2, None, "/s2");
    let ph = f.pasta(hd, None, "/h");
    let x_ssd = f.arq(ssd, ps, "/s/x", 100, 1, Some(h(1)));
    let x_hd = f.arq(hd, ph, "/h/x", 100, 2, Some(h(1)));
    let y_ssd = f.arq(ssd, ps, "/s/y", 200, 3, Some(h(2)));
    let y_ssd_p2 = f.arq(ssd_p2, ps2, "/s2/y", 200, 4, Some(h(2)));
    f.reconstruir();
    Cobertura {
        f,
        ssd,
        ssd_p2,
        hd,
        x_ssd,
        x_hd,
        y_ssd,
        y_ssd_p2,
    }
}

fn cob_de(f: &Fx, arquivo: i64) -> dag_core::CoberturaArquivo {
    duplicados::cobertura_do_arquivo(f.conn(), arquivo).unwrap()
}

#[test]
fn ca_a22_backup_por_regra_cobertura_outro_dispositivo_e_offline() {
    let mut c = cenario_cobertura();
    // Sem Marcação nenhuma, ninguém tem Cobertura.
    assert!(!cob_de(&c.f, c.x_ssd).coberto);
    // Regra de Backup no HD-Externo: o arquivo do SSD passa a ter Cobertura.
    c.f.marcar(TipoMarcacao::Backup, Alvo::Volume("HD-Externo".into()))
        .unwrap();
    assert!(cob_de(&c.f, c.x_ssd).coberto);
    assert_eq!(cob_de(&c.f, c.x_ssd).nao_verificada_desde, None);
    // A própria cópia do HD não tem Cobertura (a outra cópia, no SSD, não é backup).
    assert!(!cob_de(&c.f, c.x_hd).coberto);
    // Outra partição do mesmo disco: Backup na SSD-P2 não cobre o arquivo do SSD.
    c.f.marcar(TipoMarcacao::Backup, Alvo::Volume("SSD-P2".into()))
        .unwrap();
    assert!(
        !cob_de(&c.f, c.y_ssd).coberto,
        "mesmo Dispositivo Físico não conta"
    );
    assert!(!cob_de(&c.f, c.y_ssd_p2).coberto);
    // O HD some (desconectado): a Cobertura fica, com o selo da data.
    c.f.conn()
        .execute(
            "UPDATE volumes SET mounted = 0, last_seen_at = 7777 WHERE id = ?1",
            [c.hd],
        )
        .unwrap();
    let cob = cob_de(&c.f, c.x_ssd);
    assert!(cob.coberto);
    assert_eq!(cob.nao_verificada_desde, Some(7777));
    // Relatório de arquivos sem Cobertura: tudo menos x_ssd.
    let (d, _) = duplicados::relatorio_sem_cobertura(c.f.conn(), None, 100, None).unwrap();
    let DadosRelatorio::SemCobertura { arquivos } = d else {
        panic!()
    };
    let ids: Vec<i64> = arquivos.iter().map(|a| a.arquivo.0).collect();
    assert_eq!(ids, vec![c.x_hd, c.y_ssd, c.y_ssd_p2]);
    // Resumo por volume.
    let (d, _) = duplicados::relatorio_cobertura(c.f.conn(), None, 100, None).unwrap();
    let DadosRelatorio::Cobertura { volumes, .. } = d else {
        panic!()
    };
    let v = volumes.iter().find(|v| v.volume.0 == c.ssd).unwrap();
    assert_eq!(
        (
            v.arquivos,
            v.com_cobertura,
            v.cobertura_nao_verificada,
            v.sem_cobertura
        ),
        (2, 1, 1, 1)
    );
    let v = volumes.iter().find(|v| v.volume.0 == c.ssd_p2).unwrap();
    assert_eq!((v.com_cobertura, v.sem_cobertura), (0, 1));
}

#[test]
fn volume_sem_dispositivo_conhecido_nao_da_nem_recebe_cobertura() {
    let mut f = Fx::novo();
    let d = f.dispositivo("d");
    let a = f.volume("A", Some(d), true);
    let b = f.volume("B", None, true);
    let pa = f.pasta(a, None, "/a");
    let pb = f.pasta(b, None, "/b");
    let fa = f.arq(a, pa, "/a/x", 10, 1, Some(h(1)));
    let fb = f.arq(b, pb, "/b/x", 10, 2, Some(h(1)));
    f.reconstruir();
    f.marcar(TipoMarcacao::Backup, Alvo::Volume("B".into()))
        .unwrap();
    assert!(!cob_de(&f, fa).coberto);
    assert!(!cob_de(&f, fb).coberto);
}

#[test]
fn cobertura_por_regra_de_pasta_e_manual() {
    let mut c = cenario_cobertura();
    // manual no arquivo do HD
    c.f.marcar(TipoMarcacao::Backup, arq_alvo(c.x_hd)).unwrap();
    assert!(cob_de(&c.f, c.x_ssd).coberto);
    c.f.marcar(TipoMarcacao::Backup, Alvo::Pasta("/s2".into()))
        .unwrap();
    // y_ssd_p2 está no mesmo disco de y_ssd: nada
    assert!(!cob_de(&c.f, c.y_ssd).coberto);
    let _ = (c.ssd, c.ssd_p2);
}

// ---------------------------------------------------------------------- marcações

#[test]
fn uma_marcacao_por_copia_e_conflitos_claros() {
    let mut c = cenario_cobertura();
    let f = &mut c.f;
    f.marcar(TipoMarcacao::Backup, arq_alvo(c.x_hd)).unwrap();
    // manual duplicado e de outro tipo
    for tipo in [TipoMarcacao::Backup, TipoMarcacao::Delecao] {
        let e = f.marcar(tipo, arq_alvo(c.x_hd)).unwrap_err();
        let ErroDuplicados::Conflito(m) = &e else {
            panic!("{e:?}")
        };
        assert!(
            m.contains("já tem") && m.contains("uma Marcação") && m.contains("dag desmarcar"),
            "{m}"
        );
        assert_eq!(e.codigo(), "conflito_marcacao");
    }
    // regra de volume por cima de marcação manual de outro tipo: recusada
    let e = f
        .marcar(TipoMarcacao::Delecao, Alvo::Volume("HD-Externo".into()))
        .unwrap_err();
    assert!(matches!(e, ErroDuplicados::Conflito(_)), "{e:?}");
    // regra do mesmo tipo por cima da manual: permitida
    f.marcar(TipoMarcacao::Backup, Alvo::Volume("HD-Externo".into()))
        .unwrap();
    // arquivo coberto por regra não aceita marcação manual
    let e = f
        .marcar(TipoMarcacao::Delecao, arq_alvo(c.x_hd))
        .unwrap_err();
    assert!(matches!(e, ErroDuplicados::Conflito(_)));
    // mesmo alvo de regra duas vezes
    let e = f
        .marcar(TipoMarcacao::Backup, Alvo::Volume("HD-Externo".into()))
        .unwrap_err();
    assert!(matches!(e, ErroDuplicados::Conflito(m) if m.contains("já tem uma regra")));
    // desmarcar arquivo marcado só por regra diz qual regra
    let e = duplicados::desmarcar(&mut f.c, &arq_alvo(c.x_hd), Autor::Cli, T);
    // x_hd tem marca manual própria: removida normalmente
    assert!(e.is_ok());
    let e = duplicados::desmarcar(&mut f.c, &arq_alvo(c.x_hd), Autor::Cli, T).unwrap_err();
    assert!(
        matches!(&e, ErroDuplicados::Conflito(m) if m.contains("regra")),
        "{e:?}"
    );
    // arquivo sem hash não é marcável manualmente
    let d = f.dispositivo("novo");
    let v = f.volume("NOVO", Some(d), true);
    let p = f.pasta(v, None, "/n");
    let sem = f.arq(v, p, "/n/z", 5, 99, None);
    let e = f.marcar(TipoMarcacao::Delecao, arq_alvo(sem)).unwrap_err();
    assert!(matches!(e, ErroDuplicados::SemHash(_)), "{e:?}");
    // caminho desconhecido e grupo inexistente
    assert!(matches!(
        f.marcar(TipoMarcacao::Backup, Alvo::Caminho("/nao/existe".into())),
        Err(ErroDuplicados::NaoEncontrado(_))
    ));
    assert!(matches!(
        f.marcar(TipoMarcacao::Backup, Alvo::Grupo(GrupoId(999))),
        Err(ErroDuplicados::NaoEncontrado(_))
    ));
}

#[test]
fn marcar_grupo_backup_marca_todas_e_delecao_poupa_a_principal() {
    let mut c = cenario_cobertura();
    let f = &mut c.f;
    let g = f.grupos().into_iter().find(|g| g.tamanho == 100).unwrap();
    // Deleção do grupo: todas menos a Cópia Principal.
    let r = f.marcar(TipoMarcacao::Delecao, Alvo::Grupo(g.id)).unwrap();
    assert_eq!(r.marcacoes.len(), 1);
    assert_ne!(r.marcacoes[0].arquivo, g.copia_principal);
    assert_eq!(r.unicas_copias_total, 0);
    // repetir é idempotente
    let r = f.marcar(TipoMarcacao::Delecao, Alvo::Grupo(g.id)).unwrap();
    assert!(r.marcacoes.is_empty());
    assert_eq!(r.ja_marcadas, 1);
    // Backup do grupo agora conflita com a Deleção já feita e nada é gravado.
    let antes = f.contar("SELECT COUNT(*) FROM marcacoes");
    let antes_h = f.contar("SELECT COUNT(*) FROM marcacoes_historico");
    let e = f
        .marcar(TipoMarcacao::Backup, Alvo::Grupo(g.id))
        .unwrap_err();
    assert!(matches!(e, ErroDuplicados::Conflito(_)));
    assert_eq!(f.contar("SELECT COUNT(*) FROM marcacoes"), antes);
    assert_eq!(
        f.contar("SELECT COUNT(*) FROM marcacoes_historico"),
        antes_h
    );
    // Desmarcar o grupo remove as marcações dele.
    let rem = duplicados::desmarcar(&mut f.c, &Alvo::Grupo(g.id), Autor::Cli, T).unwrap();
    assert_eq!(rem.len(), 1);
    let r = f.marcar(TipoMarcacao::Backup, Alvo::Grupo(g.id)).unwrap();
    assert_eq!(r.marcacoes.len(), 2);
}

#[test]
fn unica_copia_alerta_em_delecao_manual_e_por_regra() {
    let mut c = cenario_cobertura();
    let f = &mut c.f;
    // Arquivo sem duplicado.
    let p = f.pasta(c.ssd, None, "/s/solto");
    let unico = f.arq(c.ssd, p, "/s/solto/u", 33, 50, Some(h(50)));
    let r = f.marcar(TipoMarcacao::Delecao, arq_alvo(unico)).unwrap();
    assert_eq!(r.unicas_copias_total, 1);
    assert_eq!(r.unicas_copias[0].arquivo.0, unico);
    // Arquivo com cópia: sem alerta.
    let r = f.marcar(TipoMarcacao::Delecao, arq_alvo(c.x_hd)).unwrap();
    assert_eq!(r.unicas_copias_total, 0);
    // Backup nunca alerta.
    let u2 = f.arq(c.ssd, p, "/s/solto/u2", 34, 51, Some(h(51)));
    assert_eq!(
        f.marcar(TipoMarcacao::Backup, arq_alvo(u2))
            .unwrap()
            .unicas_copias_total,
        0
    );
    // Regra de Deleção na pasta conta as Únicas Cópias debaixo dela (u e u2 foram marcados:
    // por isso usa pasta nova).
    let p2 = f.pasta(c.ssd, None, "/s/outra");
    f.arq(c.ssd, p2, "/s/outra/a", 70, 60, Some(h(60)));
    f.arq(c.ssd, p2, "/s/outra/b", 71, 61, Some(h(61)));
    f.arq(c.ssd, p2, "/s/outra/c", 72, 62, Some(h(1))); // cópia de x (tam diferente: ainda única)
    let r = f
        .marcar(TipoMarcacao::Delecao, Alvo::Pasta("/s/outra".into()))
        .unwrap();
    assert_eq!(r.unicas_copias_total, 3);
    assert!(duplicados::eh_unica_copia(f.conn(), unico).unwrap());
    assert!(!duplicados::eh_unica_copia(f.conn(), c.x_ssd).unwrap());
}

// ---------------------------------------------------------------------- CA-A24

#[test]
fn ca_a24_conteudo_muda_marcacao_vira_pendente_e_sai_das_contas() {
    let mut f = Fx::novo();
    let (d1, d2) = (f.dispositivo("d1"), f.dispositivo("d2"));
    let ssd = f.volume("SSD", Some(d1), true);
    let hd = f.volume("HD", Some(d2), true);
    let ps = f.pasta(ssd, None, "/s");
    let ph = f.pasta(hd, None, "/h");
    // Grupo X (hash 1): x_ssd, x_hd. Grupo Y (hash 2): y1, y2 (HD).
    let x_ssd = f.arq(ssd, ps, "/s/x", 100, 1, Some(h(1)));
    let x_hd = f.arq(hd, ph, "/h/x", 100, 2, Some(h(1)));
    let y_hd = f.arq(hd, ph, "/h/y", 100, 3, Some(h(2)));
    let y_hd2 = f.arq(hd, ph, "/h/y2", 100, 4, Some(h(2)));
    f.reconstruir();
    // Backup manual no x_ssd (cobre x_hd) e deleção manual de x_hd.
    f.marcar(TipoMarcacao::Backup, arq_alvo(x_ssd)).unwrap();
    f.marcar(TipoMarcacao::Delecao, arq_alvo(x_hd)).unwrap();
    assert!(cob_de(&f, x_hd).coberto);
    assert!(
        f.grupos()
            .iter()
            .any(|g| g.tamanho == 100 && g.copias == 2 && g.bytes_marcados_delecao == 100)
    );
    let marca_ssd = f.contar("SELECT id FROM marcacoes WHERE file_id = (SELECT id FROM files WHERE path_display = '/s/x')");
    // O conteúdo de x_ssd muda e vira exatamente o do grupo Y.
    f.novo_conteudo(x_ssd, h(2));
    f.reconstruir();
    let n = duplicados::reavaliar_marcacoes(&mut f.c, Autor::Cli, T + 5).unwrap();
    assert_eq!(n, 1);
    // A marcação de Backup ficou pendente; a de x_hd (hash igual) segue ativa.
    let pend: Vec<_> = duplicados::listar_marcacoes(f.conn(), None, 100).unwrap().0;
    let m_ssd = pend
        .iter()
        .find(|m| m.arquivo == Some(ArquivoId(x_ssd)))
        .unwrap();
    assert_eq!(m_ssd.estado, EstadoMarcacao::Pendente);
    let m_hd = pend
        .iter()
        .find(|m| m.arquivo == Some(ArquivoId(x_hd)))
        .unwrap();
    assert_eq!(m_hd.estado, EstadoMarcacao::Ativa);
    // x_hd ficou sozinho (o outro saiu): sem grupo, logo sem cobertura nem espaço.
    assert!(!cob_de(&f, x_hd).coberto);
    // Grupo Y agora tem 3 cópias; x_ssd (pendente) fica fora de Cobertura e de espaço.
    let gy = f.grupos().into_iter().find(|g| g.copias == 3).unwrap();
    let d = detalhar_grupo(f.conn(), gy.id).unwrap();
    let c_x = d.copias.iter().find(|c| c.arquivo.0 == x_ssd).unwrap();
    assert_eq!(c_x.recuperavel, ClasseRecuperavel::Nao);
    assert_eq!(
        c_x.marcacao.as_ref().unwrap().estado,
        EstadoMarcacao::Pendente
    );
    for c in d
        .copias
        .iter()
        .filter(|c| c.arquivo.0 == y_hd || c.arquivo.0 == y_hd2)
    {
        assert!(!c.cobertura.coberto, "backup pendente não cobre");
    }
    // Só as duas cópias do HD podem ser redundantes: principal (SSD) = x_ssd; sem marcas,
    // a estimativa exclui a pendente e a principal.
    assert!(gy.bytes_redundantes_estimados <= 200);
    // Histórico: criada, criada, pendente (com hash novo), todos com autor.
    let hist = duplicados::historico_da_marcacao(f.conn(), Some(MarcacaoId(marca_ssd))).unwrap();
    let eventos: Vec<&str> = hist.iter().map(|h| h.evento.as_str()).collect();
    assert_eq!(eventos, ["criada", "pendente"]);
    assert_eq!(hist[1].estado_anterior, Some(EstadoMarcacao::Ativa));
    assert_eq!(hist[1].estado_novo, Some(EstadoMarcacao::Pendente));
    assert_eq!(hist[1].hash.unwrap().0, h(2));
    assert_eq!(hist[1].autor, Autor::Cli);
    // Reavaliar de novo é inofensivo.
    assert_eq!(
        duplicados::reavaliar_marcacoes(&mut f.c, Autor::Cli, T + 6).unwrap(),
        0
    );
    // Confirmar adota o hash novo e reativa; o histórico registra.
    let r = duplicados::confirmar(&mut f.c, MarcacaoId(marca_ssd), Autor::Gui, T + 7).unwrap();
    assert_eq!(r.marcacoes[0].estado, EstadoMarcacao::Ativa);
    assert_eq!(r.marcacoes[0].hash_na_marcacao.unwrap().0, h(2));
    let hist = duplicados::historico_da_marcacao(f.conn(), Some(MarcacaoId(marca_ssd))).unwrap();
    assert_eq!(hist.last().unwrap().evento, "confirmada");
    assert_eq!(hist.last().unwrap().autor, Autor::Gui);
    // Agora cobre as cópias do HD.
    assert!(cob_de(&f, y_hd).coberto);
    // Confirmar de novo (já ativa) é erro claro.
    assert!(matches!(
        duplicados::confirmar(&mut f.c, MarcacaoId(marca_ssd), Autor::Cli, T),
        Err(ErroDuplicados::Invalido(_))
    ));
}

#[test]
fn marcacao_de_grupo_sobrevive_quando_a_copia_sai_do_grupo() {
    let mut c = cenario_cobertura();
    let f = &mut c.f;
    let g = f.grupos().into_iter().find(|g| g.tamanho == 100).unwrap();
    let r = f.marcar(TipoMarcacao::Backup, Alvo::Grupo(g.id)).unwrap();
    assert_eq!(r.marcacoes.len(), 2);
    // x_hd muda de conteúdo e sai do grupo; o grupo se desfaz (sobra uma cópia).
    f.novo_conteudo(c.x_hd, h(77));
    let rr = f.reconstruir();
    assert_eq!(rr.grupos_removidos, 1);
    assert_eq!(rr.marcacoes_convertidas, 2);
    // As marcações continuam (agora de arquivo), e a mudança ficou no histórico.
    assert_eq!(
        f.contar("SELECT COUNT(*) FROM marcacoes WHERE escopo = 'manual_arquivo'"),
        2
    );
    assert_eq!(
        f.contar("SELECT COUNT(*) FROM marcacoes_historico WHERE evento = 'alterada'"),
        2
    );
    let n = duplicados::reavaliar_marcacoes(&mut f.c, Autor::Cli, T).unwrap();
    assert_eq!(n, 1);
}

// ---------------------------------------------------------------------- histórico

#[test]
fn historico_so_de_acrescimo_e_na_mesma_transacao() {
    let mut c = cenario_cobertura();
    let f = &mut c.f;
    f.marcar(TipoMarcacao::Backup, arq_alvo(c.x_hd)).unwrap();
    duplicados::desmarcar(&mut f.c, &arq_alvo(c.x_hd), Autor::Mcp, T + 2).unwrap();
    let hist = duplicados::historico_da_marcacao(f.conn(), None).unwrap();
    let ev: Vec<(&str, Autor)> = hist.iter().map(|h| (h.evento.as_str(), h.autor)).collect();
    assert_eq!(ev, [("criada", Autor::Cli), ("removida", Autor::Mcp)]);
    // Sobrevive à remoção da marcação e guarda o caminho.
    assert_eq!(f.contar("SELECT COUNT(*) FROM marcacoes"), 0);
    assert_eq!(hist[1].caminho.as_deref(), Some("/h/x"));
    assert_eq!(hist[1].estado_anterior, Some(EstadoMarcacao::Ativa));
    // UPDATE e DELETE no histórico são recusados pelo banco.
    assert!(
        f.conn()
            .execute("UPDATE marcacoes_historico SET autor = 'gui'", [])
            .is_err()
    );
    assert!(
        f.conn()
            .execute("DELETE FROM marcacoes_historico", [])
            .is_err()
    );
    // Mesma transação: uma operação que falha no meio não deixa nem marcação nem histórico.
    let antes = f.contar("SELECT COUNT(*) FROM marcacoes_historico");
    f.marcar(TipoMarcacao::Delecao, arq_alvo(c.y_ssd)).unwrap();
    let g = f.grupos().into_iter().find(|g| g.tamanho == 200).unwrap();
    let e = f
        .marcar(TipoMarcacao::Backup, Alvo::Grupo(g.id))
        .unwrap_err();
    assert!(matches!(e, ErroDuplicados::Conflito(_)));
    assert_eq!(
        f.contar("SELECT COUNT(*) FROM marcacoes_historico"),
        antes + 1
    );
    assert_eq!(f.contar("SELECT COUNT(*) FROM marcacoes"), 1);
    // Cada criação tem exatamente uma linha de histórico.
    assert_eq!(
        f.contar("SELECT COUNT(*) FROM marcacoes m WHERE (SELECT COUNT(*) FROM marcacoes_historico h WHERE h.marcacao_id = m.id AND h.evento = 'criada') <> 1"),
        0
    );
}

// ---------------------------------------------------------------------- CA-A27

#[test]
fn ca_a27_espaco_recuperavel_bate_com_o_calculo_manual() {
    let mut f = Fx::novo();
    let (d1, d2) = (f.dispositivo("d1"), f.dispositivo("d2"));
    let ssd = f.volume("SSD", Some(d1), true);
    let ssd2 = f.volume("SSD2", Some(d1), true);
    let hd = f.volume("HD", Some(d2), true);
    let ps = f.pasta(ssd, None, "/s");
    let ps_sub = f.pasta(ssd, Some(ps), "/s/sub");
    let ps2 = f.pasta(ssd2, None, "/s2");
    let ph = f.pasta(hd, None, "/h");
    let ph_sub = f.pasta(hd, Some(ph), "/h/sub");
    // G1 (100 B): a1 SSD, a2 HD, a3 SSD2, mais um hard link de a1.
    let a1 = f.hardlinks(ssd, ps, "/s/a1_", 2, 100, 1, h(1))[0];
    let a2 = f.arq(hd, ph, "/h/a2", 100, 2, Some(h(1)));
    let a3 = f.arq(ssd2, ps2, "/s2/a3", 100, 3, Some(h(1)));
    // G2 (50 B): b1 SSD, b2 e b3 HD (b3 em subpasta).
    let b1 = f.arq(ssd, ps_sub, "/s/sub/b1", 50, 4, Some(h(2)));
    let _b2 = f.arq(hd, ph, "/h/b2", 50, 5, Some(h(2)));
    let _b3 = f.arq(hd, ph_sub, "/h/sub/b3", 50, 6, Some(h(2)));
    // G3 (10 B): c1 SSD, c2 HD.
    let c1 = f.arq(ssd, ps, "/s/c1", 10, 7, Some(h(3)));
    let c2 = f.arq(hd, ph, "/h/c2", 10, 8, Some(h(3)));
    // Fora de qualquer grupo.
    f.arq(ssd, ps, "/s/unico", 999, 9, Some(h(9)));
    definir_prioridade(
        &mut f.c,
        &["SSD".into(), "SSD2".into(), "HD".into()],
        Autor::Cli,
        T,
    )
    .unwrap();
    f.reconstruir();
    definir_prioridade(
        &mut f.c,
        &["SSD".into(), "SSD2".into(), "HD".into()],
        Autor::Cli,
        T,
    )
    .unwrap();
    // Marcações: a2 backup; a3 deleção; c1 (principal) deleção; c2 backup.
    f.marcar(TipoMarcacao::Backup, arq_alvo(a2)).unwrap();
    f.marcar(TipoMarcacao::Delecao, arq_alvo(a3)).unwrap();
    f.marcar(TipoMarcacao::Delecao, arq_alvo(c1)).unwrap();
    f.marcar(TipoMarcacao::Backup, arq_alvo(c2)).unwrap();

    // Cálculo manual:
    //  G1: principal a1 (SSD). marcada: a3 = 100. redundante sem marca: nenhuma (a2 é backup).
    //  G2: principal b1. marcadas 0. redundantes: b2 e b3 = 100.
    //  G3: c1 (principal) marcada = 10; c2 backup sobrevive; redundantes 0.
    let gs = f.grupos();
    let por_tam = |t: u64| gs.iter().find(|g| g.tamanho == t).unwrap();
    assert_eq!(
        (
            por_tam(100).bytes_marcados_delecao,
            por_tam(100).bytes_redundantes_estimados
        ),
        (100, 0)
    );
    assert_eq!(
        (
            por_tam(50).bytes_marcados_delecao,
            por_tam(50).bytes_redundantes_estimados
        ),
        (0, 100)
    );
    assert_eq!(
        (
            por_tam(10).bytes_marcados_delecao,
            por_tam(10).bytes_redundantes_estimados
        ),
        (10, 0)
    );
    assert_eq!(por_tam(100).copia_principal, Some(ArquivoId(a1)));
    assert_eq!(por_tam(50).copia_principal, Some(ArquivoId(b1)));

    let e = duplicados::espaco_de_grupos(f.conn()).unwrap();
    assert_eq!((e.computador.marcado, e.computador.estimado), (110, 100));
    // Por volume: SSD (c1 marcada 10), SSD2 (a3 marcada 100), HD (b2+b3 estimadas 100).
    assert_eq!(e.por_volume[&ssd].total(), 10);
    assert_eq!(e.por_volume[&ssd2].total(), 100);
    assert_eq!(e.por_volume[&hd].total(), 100);
    assert_eq!(
        (e.por_volume[&hd].marcado, e.por_volume[&hd].estimado),
        (0, 100)
    );

    // Por pasta (soma recursiva): /s = 10 (c1), /s/sub = 0 (b1 principal), /s2 = 100,
    // /h = 100 (b2 + b3 em /h/sub), /h/sub = 50.
    let (d, _) = duplicados::relatorio_espaco(f.conn(), NivelEspaco::Pasta, 100, None).unwrap();
    let DadosRelatorio::Espaco {
        total,
        itens,
        limitacoes,
    } = d
    else {
        panic!()
    };
    assert_eq!(total.bytes_total, 210);
    assert_eq!(
        (
            total.bytes_marcados_delecao,
            total.bytes_redundantes_estimados
        ),
        (110, 100)
    );
    assert!(limitacoes.iter().any(|l| l.contains("reflink")));
    let pasta = |n: &str| itens.iter().find(|i| i.nome == n).map(|i| i.bytes_total);
    assert_eq!(pasta("/s"), Some(10));
    assert_eq!(pasta("/s/sub"), None);
    assert_eq!(pasta("/s2"), Some(100));
    assert_eq!(
        pasta("/h"),
        Some(100),
        "b2 (50) + b3 (50); a2 e c2 são Backup"
    );
    assert_eq!(pasta("/h/sub"), Some(50));
}

// ---------------------------------------------------------------------- paginação e exportação

#[test]
fn listagem_de_grupos_pagina_por_cursor_sem_pular_nem_repetir() {
    let mut f = Fx::novo();
    let (d1, d2) = (f.dispositivo("d1"), f.dispositivo("d2"));
    let ssd = f.volume("SSD", Some(d1), true);
    let hd = f.volume("HD", Some(d2), true);
    let so_hd = f.volume("SO-HD", Some(d2), true);
    let ps = f.pasta(ssd, None, "/s");
    let ph = f.pasta(hd, None, "/h");
    let pq = f.pasta(so_hd, None, "/q");
    // 5 grupos de tamanhos 10, 20, 30, 40, 50 (cada cópia extra vale o tamanho).
    for i in 1..=5i64 {
        f.arq(ssd, ps, &format!("/s/{i}"), i * 10, i, Some(h(i as u8)));
        f.arq(hd, ph, &format!("/h/{i}"), i * 10, 10 + i, Some(h(i as u8)));
    }
    // Um grupo (tamanho 5) só entre HD e SO-HD (mesmo disco).
    f.arq(hd, ph, "/h/z", 5, 30, Some(h(30)));
    f.arq(so_hd, pq, "/q/z", 5, 31, Some(h(30)));
    f.reconstruir();
    let filtro = FiltroGrupos::default();
    let mut vistos = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let p = listar_grupos(f.conn(), &filtro, 2, cursor.as_deref()).unwrap();
        assert_eq!(p.total, 6);
        vistos.extend(p.grupos.iter().map(|g| g.tamanho));
        match p.proximo_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(vistos, vec![50, 40, 30, 20, 10, 5]);
    // Ordem por tamanho e filtros.
    let por_tamanho = FiltroGrupos {
        ordem: dag_core::OrdemDuplicados::Tamanho,
        tamanho_minimo: 20,
        ..FiltroGrupos::default()
    };
    let p = listar_grupos(f.conn(), &por_tamanho, 100, None).unwrap();
    assert_eq!(
        p.grupos.iter().map(|g| g.tamanho).collect::<Vec<_>>(),
        vec![50, 40, 30, 20]
    );
    let v = duplicados::resolver_volume(f.conn(), "SO-HD").unwrap();
    let so = FiltroGrupos {
        volume: Some(v),
        ..FiltroGrupos::default()
    };
    let p = listar_grupos(f.conn(), &so, 100, None).unwrap();
    assert_eq!(p.total, 1);
    assert_eq!(p.grupos[0].tamanho, 5);
    assert!(listar_grupos(f.conn(), &filtro, 2, Some("lixo")).is_err());
    // Volume por rótulo (sem diferenciar maiúsculas), id e inexistente.
    assert_eq!(duplicados::resolver_volume(f.conn(), "so-hd").unwrap(), v);
    assert_eq!(
        duplicados::resolver_volume(f.conn(), &v.to_string()).unwrap(),
        v
    );
    assert!(duplicados::resolver_volume(f.conn(), "nada").is_err());
}

#[test]
fn exportacao_csv_e_json_lista_exatamente_as_marcacoes() {
    let mut c = cenario_cobertura();
    let f = &mut c.f;
    // Caminho com vírgula e aspas, para testar o escape.
    let p = f.pasta(c.ssd, None, "/s/\"esp\",cial");
    let esquisito = f.arq(c.ssd, p, "/s/\"esp\",cial/a", 100, 40, Some(h(1)));
    f.reconstruir();
    f.marcar(TipoMarcacao::Backup, arq_alvo(c.x_hd)).unwrap();
    f.marcar(TipoMarcacao::Delecao, arq_alvo(esquisito))
        .unwrap();
    f.marcar(TipoMarcacao::Delecao, Alvo::Volume("SSD-P2".into()))
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let csv = tmp.path().join("m.csv");
    let json = tmp.path().join("m.json");
    let (fmt, n) =
        duplicados::exportar_relatorio(f.conn(), TipoRelatorio::Marcacoes, &csv, T).unwrap();
    assert_eq!((fmt, n), (dag_core::FormatoExportacao::Csv, 3));
    let (_, n) =
        duplicados::exportar_relatorio(f.conn(), TipoRelatorio::Marcacoes, &json, T).unwrap();
    assert_eq!(n, 3);
    let texto = std::fs::read_to_string(&csv).unwrap();
    let linhas: Vec<&str> = texto.lines().collect();
    assert_eq!(linhas.len(), 4);
    assert!(linhas[0].starts_with("id,tipo,estado,escopo,alvo"));
    assert!(linhas[1].contains("backup,ativa,manual_arquivo,/h/x"));
    assert!(texto.contains("\"/s/\"\"esp\"\",cial/a\""), "{texto}");
    assert!(linhas[3].contains("delecao,ativa,regra_volume,SSD-P2"));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(v["relatorio"], "marcacoes");
    let l = v["linhas"].as_array().unwrap();
    assert_eq!(l.len(), 3);
    assert_eq!(l[0]["alvo"], "/h/x");
    assert_eq!(l[1]["alvo"], "/s/\"esp\",cial/a");
    assert_eq!(l[2]["escopo"], "regra_volume");
    // Exatamente as marcações do Catálogo.
    let (todas, _) = duplicados::listar_marcacoes(f.conn(), None, 100).unwrap();
    assert_eq!(todas.len(), l.len());
    // Não sobrescreve; extensão desconhecida é recusada; arquivo privado.
    assert!(duplicados::exportar_relatorio(f.conn(), TipoRelatorio::Marcacoes, &csv, T).is_err());
    assert!(matches!(
        duplicados::exportar_relatorio(
            f.conn(),
            TipoRelatorio::Marcacoes,
            &tmp.path().join("m.txt"),
            T
        ),
        Err(ErroDuplicados::Invalido(_))
    ));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&csv).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // Os outros relatórios também exportam.
    for (t, nome) in [
        (TipoRelatorio::Cobertura, "c.csv"),
        (TipoRelatorio::SemCobertura, "s.json"),
        (TipoRelatorio::Espaco, "e.csv"),
    ] {
        let (_, n) =
            duplicados::exportar_relatorio(f.conn(), t, &tmp.path().join(nome), T).unwrap();
        assert!(n > 0, "{nome}");
    }
}

#[test]
fn arquivo_unico_marcado_para_delecao_entra_no_espaco_de_volume_pasta_e_computador() {
    let mut f = Fx::novo();
    let d = f.dispositivo("d");
    let v = f.volume("V", Some(d), true);
    let raiz = f.pasta(v, None, "/v");
    let sub = f.pasta(v, Some(raiz), "/v/sub");
    let manual = f.arq(v, raiz, "/v/m", 300, 1, Some(h(1)));
    f.arq(v, sub, "/v/sub/r1", 20, 2, Some(h(2)));
    f.arq(v, sub, "/v/sub/r2", 30, 3, Some(h(3)));
    f.arq(v, raiz, "/v/fora", 999, 4, Some(h(4)));
    f.reconstruir();
    f.marcar(TipoMarcacao::Delecao, arq_alvo(manual)).unwrap();
    f.marcar(TipoMarcacao::Delecao, Alvo::Pasta("/v/sub".into()))
        .unwrap();
    let e = duplicados::espaco_de_grupos(f.conn()).unwrap();
    assert_eq!(
        (
            e.computador.marcado,
            e.computador.estimado,
            e.computador.copias
        ),
        (350, 0, 3)
    );
    assert_eq!(e.por_volume[&v].total(), 350);
    let (dados, _) = duplicados::relatorio_espaco(f.conn(), NivelEspaco::Pasta, 10, None).unwrap();
    let DadosRelatorio::Espaco { itens, .. } = dados else {
        panic!()
    };
    let pasta = |n: &str| itens.iter().find(|i| i.nome == n).map(|i| i.bytes_total);
    assert_eq!((pasta("/v"), pasta("/v/sub")), (Some(350), Some(50)));
    // Marcação pendente sai da conta.
    f.novo_conteudo(manual, h(11));
    duplicados::reavaliar_marcacoes(&mut f.c, Autor::Cli, T).unwrap();
    let e = duplicados::espaco_de_grupos(f.conn()).unwrap();
    assert_eq!(e.computador.marcado, 50);
}

// ---------------------------------------------------------------------- consultas em SQL

/// Cenário com todos os tipos de Marcação e de efeito (regra de Volume, regra de pasta, manual
/// de arquivo e de grupo, Pendente, offline, Cópia Principal manual).
fn cenario_misto() -> Fx {
    let mut f = Fx::novo();
    let (d1, d2, d3) = (
        f.dispositivo("d1"),
        f.dispositivo("d2"),
        f.dispositivo("d3"),
    );
    let ssd = f.volume("SSD", Some(d1), true);
    let hd = f.volume("HD", Some(d2), true);
    let off = f.volume("OFF", Some(d3), false);
    let sem_disp = f.volume("SEMDISP", None, true);
    let ps = f.pasta(ssd, None, "/s");
    let pss = f.pasta(ssd, Some(ps), "/s/sub");
    let ph = f.pasta(hd, None, "/h");
    let po = f.pasta(off, None, "/o");
    let pq = f.pasta(sem_disp, None, "/q");
    let mut inode = 100;
    let mut novo_inode = || {
        inode += 1;
        inode
    };
    // 12 grupos de tamanhos variados, espalhados nos volumes.
    for g in 1..=12u8 {
        let tam = i64::from(g) * 7;
        f.arq(ssd, ps, &format!("/s/g{g}"), tam, novo_inode(), Some(h(g)));
        f.arq(
            ssd,
            pss,
            &format!("/s/sub/g{g}"),
            tam,
            novo_inode(),
            Some(h(g)),
        );
        if g % 2 == 0 {
            f.arq(hd, ph, &format!("/h/g{g}"), tam, novo_inode(), Some(h(g)));
        }
        if g % 3 == 0 {
            f.arq(off, po, &format!("/o/g{g}"), tam, novo_inode(), Some(h(g)));
        }
        if g % 4 == 0 {
            f.arq(
                sem_disp,
                pq,
                &format!("/q/g{g}"),
                tam,
                novo_inode(),
                Some(h(g)),
            );
        }
    }
    // Hard links dentro de um grupo.
    f.hardlinks(hd, ph, "/h/hl", 3, 21, 9000, h(3));
    // Grupo cuja Cópia Principal será marcada para Deleção, sem Backup: poupa a de menor id.
    let px = f.pasta(ssd, None, "/x");
    for k in 1..=3 {
        f.arq(ssd, px, &format!("/x/x{k}"), 11, 8000 + k, Some(h(50)));
    }
    f.reconstruir();
    let x1: i64 = f.contar("SELECT id FROM files WHERE path_display = '/x/x1'");
    f.marcar(TipoMarcacao::Delecao, arq_alvo(x1)).unwrap();
    // Marcações de todos os tipos.
    f.marcar(TipoMarcacao::Backup, Alvo::Volume("HD".into()))
        .unwrap();
    f.marcar(TipoMarcacao::Delecao, Alvo::Pasta("/s/sub".into()))
        .unwrap();
    let g5 = f.grupos().into_iter().find(|g| g.tamanho == 35).unwrap();
    f.marcar(TipoMarcacao::Delecao, Alvo::Grupo(g5.id)).unwrap();
    let sete: i64 = f.contar("SELECT id FROM files WHERE path_display = '/s/g7'");
    f.marcar(TipoMarcacao::Backup, arq_alvo(sete)).unwrap();
    let um: i64 = f.contar("SELECT id FROM files WHERE path_display = '/s/g1'");
    f.marcar(TipoMarcacao::Delecao, arq_alvo(um)).unwrap();
    let dois: i64 = f.contar("SELECT id FROM files WHERE path_display = '/s/g11'");
    f.marcar(TipoMarcacao::Backup, arq_alvo(dois)).unwrap();
    // Principal manual.
    let g8 = f.grupos().into_iter().find(|g| g.tamanho == 56).unwrap();
    let hd8: i64 = f.contar("SELECT id FROM files WHERE path_display = '/h/g8'");
    definir_copia_principal(&mut f.c, g8.id, Some(&arq_alvo(hd8)), T).unwrap();
    // Marcação Pendente: o conteúdo muda depois de marcado.
    let tres: i64 = f.contar("SELECT id FROM files WHERE path_display = '/s/g9'");
    f.marcar(TipoMarcacao::Delecao, arq_alvo(tres)).unwrap();
    f.novo_conteudo(tres, h(99));
    duplicados::reavaliar_marcacoes(&mut f.c, Autor::Cli, T + 2).unwrap();
    f.reconstruir();
    f
}

fn normaliza(e: &duplicados::EspacoCatalogo) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}",
        e.computador, e.por_volume, e.por_pasta_direta, e.por_grupo
    )
}

#[test]
fn consultas_sql_equivalem_as_de_referencia_em_memoria() {
    let f = cenario_misto();
    let conn = f.conn();
    let vol_hd = duplicados::resolver_volume(conn, "HD").unwrap();
    let vol_ssd = duplicados::resolver_volume(conn, "SSD").unwrap();
    for ordem in [
        dag_core::OrdemDuplicados::Recuperavel,
        dag_core::OrdemDuplicados::Tamanho,
    ] {
        for volume in [None, Some(vol_hd), Some(vol_ssd)] {
            for tamanho_minimo in [0, 30, 70] {
                let filtro = FiltroGrupos {
                    volume,
                    tamanho_minimo,
                    ordem,
                };
                for limite in [1u32, 3, 1000] {
                    let (mut a, mut b) = (None::<String>, None::<String>);
                    loop {
                        let novo = listar_grupos(conn, &filtro, limite, a.as_deref()).unwrap();
                        let ref_ = duplicados::listar_grupos_referencia(
                            conn,
                            &filtro,
                            limite,
                            b.as_deref(),
                        )
                        .unwrap();
                        assert_eq!(novo, ref_, "{filtro:?} limite {limite}");
                        a = novo.proximo_cursor;
                        b = ref_.proximo_cursor;
                        if a.is_none() {
                            break;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(
        duplicados::resumo_duplicados(conn).unwrap(),
        duplicados::resumo_duplicados_referencia(conn).unwrap()
    );
    assert!(duplicados::resumo_duplicados(conn).unwrap().grupos >= 12);
    let novo = duplicados::espaco_de_grupos(conn).unwrap();
    let ref_ = duplicados::espaco_de_grupos_referencia(conn).unwrap();
    assert_eq!(normaliza(&novo), normaliza(&ref_));
    assert!(novo.computador.marcado > 0 && novo.computador.estimado > 0);
}

#[test]
fn relatorio_de_espaco_por_grupo_pagina_sem_pular_nem_repetir() {
    let f = cenario_misto();
    let conn = f.conn();
    let ref_ = duplicados::espaco_de_grupos_referencia(conn).unwrap();
    let mut esperado: Vec<(u64, i64)> = ref_
        .por_grupo
        .iter()
        .map(|(g, _, a)| (a.total(), *g))
        .collect();
    esperado.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut vistos = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let (d, prox) =
            duplicados::relatorio_espaco(conn, NivelEspaco::Grupo, 2, cursor.as_deref()).unwrap();
        let DadosRelatorio::Espaco { itens, total, .. } = d else {
            panic!()
        };
        assert_eq!(
            total.bytes_total,
            ref_.computador.total(),
            "o total do computador vem em toda página"
        );
        vistos.extend(itens.iter().map(|i| (i.bytes_total, i.id.unwrap())));
        match prox {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(vistos, esperado);
    // Por volume: igual à referência.
    let (d, _) = duplicados::relatorio_espaco(conn, NivelEspaco::Volume, 100, None).unwrap();
    let DadosRelatorio::Espaco { itens, .. } = d else {
        panic!()
    };
    for it in itens {
        let r = ref_.por_volume[&it.id.unwrap()];
        assert_eq!(
            (
                it.bytes_marcados_delecao,
                it.bytes_redundantes_estimados,
                it.copias
            ),
            (r.marcado, r.estimado, r.copias)
        );
    }
}

#[test]
fn listagem_em_sql_aguenta_muitos_grupos_sem_alterar_a_semantica() {
    // 6.000 grupos de 2 cópias inseridos direto: a página custa o mesmo que a de 5 grupos e o
    // total/resumo batem com o cálculo manual.
    let mut f = Fx::novo();
    let (d1, d2) = (f.dispositivo("d1"), f.dispositivo("d2"));
    let a = f.volume("A", Some(d1), true);
    let b = f.volume("B", Some(d2), true);
    let pa = f.pasta(a, None, "/a");
    let pb = f.pasta(b, None, "/b");
    let n = 6_000i64;
    {
        let conn = f.conn();
        conn.execute_batch("BEGIN").unwrap();
        for i in 0..n {
            let mut hs = [0u8; 32];
            hs[..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
            let tam = 1 + (i % 50);
            for (vol, pasta, pre) in [(a, pa, "/a"), (b, pb, "/b")] {
                conn.execute(
                    "INSERT INTO files (volume_id, folder_id, path_raw, path_display, name_raw, name_display,
                                        native_dev, native_file_id, nlink, logical_bytes, mtime_ns, ctime_ns, seen_run_id)
                     VALUES (?1, ?2, CAST(?3 AS BLOB), ?3, CAST(?3 AS BLOB), ?3, 1, ?4, 1, ?5, 1, 1, 1)",
                    params![vol, pasta, format!("{pre}/{i}"), 10 * i + vol, tam],
                )
                .unwrap();
                let id = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO hashes (file_id, hash, hashed_at, size_bytes, mtime_ns, ctime_ns, native_file_id)
                     SELECT id, ?2, 1, logical_bytes, mtime_ns, ctime_ns, native_file_id FROM files WHERE id = ?1",
                    params![id, hs.as_slice()],
                )
                .unwrap();
            }
        }
        conn.execute_batch("COMMIT").unwrap();
    }
    let r = f.reconstruir();
    assert_eq!(r.grupos_criados as i64, n);
    let p = listar_grupos(f.conn(), &FiltroGrupos::default(), 5, None).unwrap();
    assert_eq!((p.total as i64, p.grupos.len()), (n, 5));
    assert!(p.proximo_cursor.is_some());
    // Cada grupo recupera uma cópia: soma = Σ tamanho.
    let esperado: i64 = (0..n).map(|i| 1 + (i % 50)).sum();
    let s = duplicados::resumo_duplicados(f.conn()).unwrap();
    assert_eq!(
        (
            s.grupos as i64,
            s.copias as i64,
            s.espaco_recuperavel_bytes as i64
        ),
        (n, 2 * n, esperado)
    );
    let ref_ = duplicados::resumo_duplicados_referencia(f.conn()).unwrap();
    assert_eq!(s, ref_);
    // Filtro por volume e por tamanho mínimo.
    let filtro = FiltroGrupos {
        volume: Some(b),
        tamanho_minimo: 50,
        ..FiltroGrupos::default()
    };
    let p = listar_grupos(f.conn(), &filtro, 1000, None).unwrap();
    assert_eq!(p.total as i64, n / 50);
    assert!(p.grupos.iter().all(|g| g.tamanho == 50));
}

// ------------------------------------------------------------ Membros de Contêineres (ADR 0006)

impl Fx {
    /// Registra o Contêiner como lido e grava um Membro `caminho` de hash `hs`.
    fn membro(&self, conteiner: i64, caminho: &str, tam: i64, hs: [u8; 32]) {
        self.conn()
            .execute(
                "INSERT INTO conteineres (file_id, volume_id, estado, formato, hash_lido, enfileirado_em, atualizado_em)
                 SELECT ?1, (SELECT volume_id FROM files WHERE id = ?1), 'lido', 'zip', hash, 1, 1 FROM hashes WHERE file_id = ?1",
                [conteiner],
            )
            .unwrap();
        self.conn()
            .execute(
                "INSERT INTO membros (conteiner_file_id, ordem, caminho_raw, caminho_display, tipo,
                                      tamanho, hash, estado)
                 VALUES (?1, 0, CAST(?2 AS BLOB), ?2, 'arquivo', ?3, ?4, 'lido')",
                params![conteiner, caminho, tam, hs.as_slice()],
            )
            .unwrap();
    }
}

#[test]
fn membro_conta_como_copia_mas_nunca_como_espaco_recuperavel() {
    let mut f = Fx::novo();
    let (d1, d2) = (f.dispositivo("d1"), f.dispositivo("d2"));
    let ssd = f.volume("SSD", Some(d1), true);
    let hd = f.volume("HD", Some(d2), true);
    let ps = f.pasta(ssd, None, "/s");
    let ph = f.pasta(hd, None, "/h");
    let solto = f.arq(ssd, ps, "/s/foto.bin", 1000, 1, Some(h(1)));
    let zip = f.arq(hd, ph, "/h/pacote.zip", 5000, 2, Some(h(9)));
    f.membro(zip, "dentro.bin", 1000, h(1));
    f.reconstruir();
    let gs = f.grupos();
    assert_eq!(gs.len(), 1);
    assert_eq!(gs[0].copias_em_conteineres, 1);
    assert_eq!(
        gs[0].espaco_recuperavel_bytes, 0,
        "Membro nunca é espaço recuperável"
    );
    assert_eq!(
        duplicados::espaco_de_grupos(f.conn())
            .unwrap()
            .computador
            .total(),
        0
    );
    assert!(!duplicados::eh_unica_copia(f.conn(), solto).unwrap());
    let d = detalhar_grupo(f.conn(), gs[0].id).unwrap();
    assert_eq!(d.copias_em_conteineres.len(), 1);
    assert_eq!(d.copias_em_conteineres[0].rotulo, "dentro de /h/pacote.zip");
    // Backup no Volume do contêiner cobre o arquivo solto.
    f.marcar(TipoMarcacao::Backup, Alvo::Volume("HD".into()))
        .unwrap();
    assert!(
        duplicados::cobertura_do_arquivo(f.conn(), solto)
            .unwrap()
            .coberto
    );
}
