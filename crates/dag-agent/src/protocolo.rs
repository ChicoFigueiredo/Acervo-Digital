//! Protocolo do socket do agente (spec §11): quadros JSON delimitados por `\n`.
//!
//! Cliente → agente: `{"id": N, "pedido": {autor, comando...}}`.
//! Agente → cliente, para o mesmo `id`: zero ou mais `{"id": N, "progresso": {...}}` e, por fim,
//! exatamente um `{"id": N, "resposta": {...}}`. Um cliente atende um pedido por vez por
//! conexão (abra outra conexão para controlar um trabalho em andamento). Extensão opcional:
//! `{"id": N, "ola": true}` devolve `{"id": N, "servidor": {...}}` (qual catálogo é servido).
//! Recusas de conexão chegam como `{"id": 0, "resposta": {"resposta": "erro", ...}}`.

use std::io::{self, BufRead, Read};

use dag_core::{Pedido, Progresso, Resposta};
use serde::{Deserialize, Serialize};

/// Maior quadro aceito (bytes), contra entrada malformada ou hostil.
pub const TAMANHO_MAXIMO_QUADRO: usize = 8 * 1024 * 1024;

/// Quadro do cliente.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Requisicao {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pedido: Option<Pedido>,
    /// Pede só a identificação do servidor.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ola: bool,
}

/// Quem responde: qual Catálogo o agente serve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InfoServidor {
    pub catalogo: String,
    pub pid: u32,
    pub versao: String,
    /// O agente monitora as raízes catalogadas.
    pub monitor: bool,
}

/// Quadro do agente. Exatamente um dos três campos vem preenchido.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quadro {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progresso: Option<Progresso>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resposta: Option<Resposta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servidor: Option<InfoServidor>,
}

impl Quadro {
    #[must_use]
    pub fn de_progresso(id: u64, p: Progresso) -> Self {
        Self {
            id,
            progresso: Some(p),
            resposta: None,
            servidor: None,
        }
    }

    #[must_use]
    pub fn de_resposta(id: u64, r: Resposta) -> Self {
        Self {
            id,
            progresso: None,
            resposta: Some(r),
            servidor: None,
        }
    }

    #[must_use]
    pub fn de_servidor(id: u64, s: InfoServidor) -> Self {
        Self {
            id,
            progresso: None,
            resposta: None,
            servidor: Some(s),
        }
    }
}

/// Lê uma linha (sem o `\n`) acumulando em `buf`, que sobrevive a leituras interrompidas por
/// timeout (o chamador repete). `Ok(None)` = fim da conexão sem bytes pendentes.
pub(crate) fn ler_quadro(
    leitor: &mut impl BufRead,
    buf: &mut Vec<u8>,
) -> io::Result<Option<Vec<u8>>> {
    loop {
        let restante = (TAMANHO_MAXIMO_QUADRO + 1).saturating_sub(buf.len()) as u64;
        let n = leitor.by_ref().take(restante).read_until(b'\n', buf)?;
        if buf.last() == Some(&b'\n') {
            buf.pop();
            return Ok(Some(std::mem::take(buf)));
        }
        if buf.len() > TAMANHO_MAXIMO_QUADRO {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "quadro grande demais",
            ));
        }
        if n == 0 {
            // Fim da conexão: sem quebra de linha final, o que sobrou é descartado.
            return Ok(None);
        }
    }
}

/// Serializa um quadro com `\n`.
pub(crate) fn linha<T: Serialize>(q: &T) -> io::Result<Vec<u8>> {
    let mut v = serde_json::to_vec(q).map_err(io::Error::other)?;
    v.push(b'\n');
    Ok(v)
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_core::{Autor, Comando};
    use std::io::Cursor;

    #[test]
    fn formato_dos_quadros() {
        let r = Requisicao {
            id: 7,
            pedido: Some(Pedido {
                autor: Autor::Cli,
                comando: Comando::Status { execucao: None },
            }),
            ola: false,
        };
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["pedido"]["comando"]["comando"], "status");
        assert!(v.get("ola").is_none());
        let q = Quadro::de_resposta(7, Resposta::Ok { mensagem: None });
        let v: serde_json::Value = serde_json::to_value(&q).unwrap();
        assert_eq!(v["resposta"]["resposta"], "ok");
        assert!(v.get("progresso").is_none());
    }

    #[test]
    fn le_linhas_e_recusa_quadro_gigante() {
        let mut c = Cursor::new(b"{\"a\":1}\nresto".to_vec());
        let mut buf = Vec::new();
        assert_eq!(
            ler_quadro(&mut c, &mut buf).unwrap().unwrap(),
            b"{\"a\":1}".to_vec()
        );
        assert_eq!(ler_quadro(&mut c, &mut buf).unwrap(), None);
        let mut grande = Cursor::new(vec![b'x'; TAMANHO_MAXIMO_QUADRO + 10]);
        let mut buf = Vec::new();
        assert!(ler_quadro(&mut grande, &mut buf).is_err());
    }
}
