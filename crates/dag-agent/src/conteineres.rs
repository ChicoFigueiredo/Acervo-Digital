//! Comandos do conteúdo de Contêineres: etapa de leitura, Fila de Serviço, Cofre de Senhas e
//! exploração de Membros (ADR 0006).
//!
//! Regra de segurança: uma senha só entra (`ResolverPedido`, `CofreAdicionar`); nenhuma resposta
//! a devolve. O Catálogo recebe apenas o blob cifrado pelo Cofre.

use std::sync::Arc;

use dag_core::{
    EntradaMembroInfo, ItemCofre, PEDIDO_SENHA, PedidoInfo, Progresso, Resposta, Senha,
    StatusConteineres,
};
use dag_scanner::{Cofre, DesfechoScan, EtapaConteineres, ResultadoConteineres};
use dag_store::conteineres as store;

use crate::agente::{Agente, agora};
use crate::erro::ErroAgente;

const MEMBROS_PADRAO: u32 = 200;
const MEMBROS_MAXIMO: u32 = 2000;

impl Agente {
    /// Roda a etapa "Conteúdo de contêineres" (persistente, retomável, um leitor por disco).
    pub(crate) fn etapa_conteineres(&mut self) -> Result<ResultadoConteineres, ErroAgente> {
        let cofre: Arc<dyn Cofre> = self.cofre.clone();
        let mut etapa = EtapaConteineres::nova()
            .com_controle(self.controle.clone())
            .com_cofre(cofre);
        if let Some(p) = self.progresso.clone() {
            etapa = etapa.com_progresso(move |c| p(&Progresso::Conteineres(c.clone())));
        }
        Ok(etapa.executar(&mut self.catalogo)?)
    }

    /// Comando `LerConteineres`: roda a etapa e, se mudou Membros, atualiza os duplicados.
    pub(crate) fn ler_conteineres(&mut self) -> Result<Resposta, ErroAgente> {
        let r = self.etapa_conteineres()?;
        if r.mudou_membros() && r.desfecho != DesfechoScan::Cancelada {
            self.atualizar_duplicados()?;
        }
        Ok(Resposta::Ok {
            mensagem: Some(format!(
                "Contêineres: {} lido(s), {} membro(s), {} cópia(s) reaproveitada(s), {} esperando senha, {} falha(s){}",
                r.lidos,
                r.membros,
                r.copiados,
                r.aguardando_senha,
                r.falhas,
                if r.desfecho == DesfechoScan::Cancelada {
                    " (cancelado; retoma de onde parou)"
                } else {
                    ""
                }
            )),
        })
    }

    pub(crate) fn status_conteineres(&self) -> Result<Resposta, ErroAgente> {
        let conn = self.catalogo.armazem().conexao();
        let a = store::andamento(conn)?;
        Ok(Resposta::Conteineres {
            status: StatusConteineres {
                pendentes: a.pendentes,
                lendo: a.lendo,
                lidos: a.lidos,
                aguardando_senha: a.aguardando_senha,
                aguardando_chaveiro: a.aguardando_chaveiro,
                falhas: a.falhas,
                nao_conteineres: a.nao_conteineres,
                membros: a.membros,
                bytes_lidos: a.bytes_lidos,
                bytes_pendentes: a.bytes_pendentes,
                pedidos_abertos: store::contar_abertos(conn)?,
            },
        })
    }

    pub(crate) fn listar_pedidos(&self, incluir_encerrados: bool) -> Result<Resposta, ErroAgente> {
        let v = store::listar_pedidos(self.catalogo.armazem().conexao(), !incluir_encerrados)?;
        Ok(Resposta::Pedidos {
            pedidos: v
                .into_iter()
                .map(|p| PedidoInfo {
                    id: p.id,
                    tipo: p.tipo,
                    arquivo: p.file_id,
                    objeto: p.objeto,
                    motivo: p.motivo,
                    estado: p.estado,
                    criado_em: p.criado_em,
                    resolvido_em: p.resolvido_em,
                })
                .collect(),
        })
    }

    /// Guarda a senha (cifrada, ligada ao Hash do Contêiner), reenfileira e relê o Contêiner.
    pub(crate) fn resolver_pedido(
        &mut self,
        id: i64,
        senha: &Senha,
    ) -> Result<Resposta, ErroAgente> {
        let (file_id, hash) = {
            let conn = self.catalogo.armazem().conexao();
            let p = store::pedido(conn, id)?
                .ok_or_else(|| ErroAgente::NaoEncontrado(format!("pedido {id} não existe")))?;
            if p.estado != "aberto" {
                return Err(ErroAgente::Invalido(format!(
                    "o pedido {id} já foi encerrado"
                )));
            }
            if p.tipo != PEDIDO_SENHA {
                return Err(ErroAgente::Invalido(format!(
                    "o pedido {id} não é um Pedido de Senha (use `tentar de novo`)"
                )));
            }
            let file_id = p
                .file_id
                .ok_or_else(|| ErroAgente::Invalido("pedido sem Contêiner".into()))?;
            let hash = store::hash_do_arquivo(conn, file_id)?.ok_or_else(|| {
                ErroAgente::Invalido("o Contêiner ainda não tem Hash Completo".into())
            })?;
            (file_id, hash)
        };
        if senha.expor().is_empty() {
            return Err(ErroAgente::Invalido("a senha não pode ser vazia".into()));
        }
        let blob = self.cofre.cifrar(senha.expor()).ok_or_else(|| {
            ErroAgente::Invalido(
                "o Cofre de Senhas está indisponível (keyring do sistema trancado ou ausente); \
                 destranque o chaveiro e tente de novo"
                    .into(),
            )
        })?;
        let t = agora();
        self.catalogo
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroAgente> {
                store::guardar_senha(tx, &hash, &blob, t)?;
                store::reenfileirar(tx, file_id, t)?;
                store::encerrar_pedido(tx, id, "resolvido", t)?;
                Ok(())
            })?;
        self.ler_conteineres()
    }

    pub(crate) fn dispensar_pedido(&mut self, id: i64) -> Result<Resposta, ErroAgente> {
        let t = agora();
        if !store::encerrar_pedido(self.catalogo.armazem().conexao(), id, "dispensado", t)? {
            return Err(ErroAgente::NaoEncontrado(format!(
                "pedido {id} não existe ou já foi encerrado"
            )));
        }
        Ok(Resposta::Ok {
            mensagem: Some(format!("Pedido {id} dispensado")),
        })
    }

    /// "Tentar de novo": reenfileira o Contêiner do pedido (Falha de Leitura ou senha) e relê.
    pub(crate) fn tentar_de_novo(&mut self, id: i64) -> Result<Resposta, ErroAgente> {
        let file_id = {
            let conn = self.catalogo.armazem().conexao();
            let p = store::pedido(conn, id)?
                .ok_or_else(|| ErroAgente::NaoEncontrado(format!("pedido {id} não existe")))?;
            p.file_id
                .ok_or_else(|| ErroAgente::Invalido("pedido sem Contêiner".into()))?
        };
        let t = agora();
        self.catalogo
            .armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroAgente> {
                store::reenfileirar(tx, file_id, t)?;
                store::encerrar_pedido(tx, id, "resolvido", t)?;
                Ok(())
            })?;
        self.ler_conteineres()
    }

    pub(crate) fn cofre_adicionar(
        &mut self,
        rotulo: &str,
        senha: &Senha,
    ) -> Result<Resposta, ErroAgente> {
        if rotulo.trim().is_empty() || senha.expor().is_empty() {
            return Err(ErroAgente::Invalido(
                "informe um rótulo e uma senha não vazios".into(),
            ));
        }
        let blob = self.cofre.cifrar(senha.expor()).ok_or_else(|| {
            ErroAgente::Invalido(
                "o Cofre de Senhas está indisponível (keyring trancado ou ausente)".into(),
            )
        })?;
        let id = store::cofre_adicionar(
            self.catalogo.armazem().conexao(),
            rotulo.trim(),
            &blob,
            agora(),
        )?;
        // Contêineres que esperavam senha ganham outra chance com o Cofre maior.
        let n = store::reenfileirar_aguardando_senha(self.catalogo.armazem().conexao(), agora())?;
        if n > 0 {
            return self.ler_conteineres();
        }
        Ok(Resposta::Ok {
            mensagem: Some(format!("Senha guardada no Cofre (id {id})")),
        })
    }

    pub(crate) fn cofre_listar(&self) -> Result<Resposta, ErroAgente> {
        let itens = store::cofre_rotulos(self.catalogo.armazem().conexao())?
            .into_iter()
            .map(|(id, rotulo)| ItemCofre { id, rotulo })
            .collect();
        Ok(Resposta::Cofre {
            situacao: self.cofre.situacao(),
            itens,
        })
    }

    pub(crate) fn cofre_remover(&mut self, id: i64) -> Result<Resposta, ErroAgente> {
        if !store::cofre_remover(self.catalogo.armazem().conexao(), id)? {
            return Err(ErroAgente::NaoEncontrado(format!(
                "senha {id} não está no Cofre"
            )));
        }
        Ok(Resposta::Ok {
            mensagem: Some(format!("Senha {id} removida do Cofre")),
        })
    }

    /// Filhos de uma pasta virtual dentro de um Contêiner (Explorador).
    pub(crate) fn explorar_conteiner(
        &self,
        conteiner: &str,
        pai_ordem: Option<i64>,
        pasta: &str,
        limite: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<Resposta, ErroAgente> {
        let conn = self.catalogo.armazem().conexao();
        let bruto = std::fs::canonicalize(conteiner).map_or_else(
            |_| conteiner.as_bytes().to_vec(),
            |p| std::os::unix::ffi::OsStrExt::as_bytes(p.as_os_str()).to_vec(),
        );
        let file_id = store::arquivo_por_caminho(conn, &bruto)?.ok_or_else(|| {
            ErroAgente::NaoEncontrado(format!("`{conteiner}` não está no Catálogo"))
        })?;
        let mut prefixo = pasta.trim_matches('/').as_bytes().to_vec();
        if !prefixo.is_empty() {
            prefixo.push(b'/');
        }
        let limite = limite.unwrap_or(MEMBROS_PADRAO).clamp(1, MEMBROS_MAXIMO);
        let mut v = store::listar_filhos(
            conn,
            file_id,
            pai_ordem,
            &prefixo,
            cursor.map(str::as_bytes),
            limite + 1,
        )?;
        let proximo_cursor = if v.len() > limite as usize {
            v.truncate(limite as usize);
            v.last()
                .map(|e| String::from_utf8_lossy(&e.nome).into_owned())
        } else {
            None
        };
        let entradas = v
            .into_iter()
            .map(|e| EntradaMembroInfo {
                nome: String::from_utf8_lossy(&e.nome).into_owned(),
                navegavel: e.navegavel,
                bytes: e.tamanho,
                arquivos: e.arquivos,
                mtime_ns: e.mtime_ns,
                aninhado_ordem: e.aninhado_ordem,
                hash: e
                    .hash
                    .map(|h| h.iter().map(|b| format!("{b:02x}")).collect()),
                estado: e.estado,
            })
            .collect();
        Ok(Resposta::Membros {
            conteiner: conteiner.to_string(),
            entradas,
            proximo_cursor,
        })
    }
}
