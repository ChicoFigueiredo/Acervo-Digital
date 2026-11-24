//! Corrupção aleatória reutilizável.

use dag_imagens::{Controle, abrir};
use std::io::Cursor;

struct Rng(u64);
impl Rng {
    fn prox(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn ate(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.prox() % n as u64) as usize
        }
    }
}

fn corromper(base: &[u8], r: &mut Rng) -> Vec<u8> {
    let mut v = base.to_vec();
    let quant = 1 + r.ate(8);
    for _ in 0..quant {
        let regiao = if r.ate(3) == 0 {
            v.len()
        } else {
            v.len().min(160 * 1024)
        };
        match r.ate(5) {
            0 => {
                let p = r.ate(regiao);
                v[p] ^= 1 << r.ate(8);
            }
            1 => {
                let p = r.ate(regiao);
                v[p] = r.prox() as u8;
            }
            2 => {
                let p = r.ate(regiao);
                let n = (1 + r.ate(512)).min(v.len() - p);
                v[p..p + n].fill(0);
            }
            3 => {
                let p = r.ate(regiao.saturating_sub(4));
                let w = [0xFFu8, 0, 0x80, 0x7F][r.ate(4)];
                v[p..p + 4].copy_from_slice(&[w, 0xFF, 0xFF, w]);
            }
            _ => {
                if r.ate(4) == 0 {
                    let n = r.ate(v.len());
                    v.truncate(n.max(16));
                }
            }
        }
    }
    v
}

/// Abre e percorre lendo todo o conteúdo; qualquer resultado vale, menos pânico.
pub fn exercitar(bytes: Vec<u8>) {
    let Ok(mut img) = abrir(Cursor::new(bytes)) else {
        return;
    };
    let _ = img.particoes().len();
    let mut n = 0u64;
    let _ = img.percorrer(&mut |_, l| -> Result<Controle, std::io::Error> {
        n += 1;
        if n > 200_000 {
            return Ok(Controle::Parar);
        }
        let mut buf = [0u8; 8192];
        let mut total = 0usize;
        while let Ok(k) = l.read(&mut buf) {
            if k == 0 {
                break;
            }
            total += k;
            if total > 4 << 20 {
                break;
            }
        }
        Ok(Controle::Continuar)
    });
}

pub fn fuzz(base: &[u8], iteracoes: usize, semente: u64) {
    let mut r = Rng(semente);
    for _ in 0..iteracoes {
        exercitar(corromper(base, &mut r));
    }
    // aleatório puro com cabeçalho válido truncado em vários pontos
    for cortes in [16usize, 100, 511, 512, 1000, 4096, 40_000] {
        exercitar(base[..cortes.min(base.len())].to_vec());
    }
}
