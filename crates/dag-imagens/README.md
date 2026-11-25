# dag-imagens

Leitor **somente leitura** de Imagens de Disco em Rust puro (sem `unsafe`, sem
dependências C). Nada é extraído, montado nem gravado, nem em temporário
(ADR 0006): o conteúdo de cada membro sai em fluxo direto da imagem.

## API

```rust
use dag_imagens::{abrir, detectar, Controle};

let mut f = std::fs::File::open("disco.vhdx")?;
let formato = detectar(&mut f);            // Option<FormatoImagem>
let mut img = abrir(f)?;                   // Imagem
for p in img.particoes() { /* inicio, tamanho, tipo, nome, sistema, nao_lida */ }
let resumo = img.percorrer(&mut |membro, conteudo| -> Result<Controle, std::io::Error> {
    // membro: particao, caminho (bytes), tamanho, mtime, tipo, id, repetido
    // conteudo: &mut dyn Read com os bytes do arquivo (vazio para pastas)
    Ok(Controle::Continuar)                // ou Pular (pasta) / Parar
})?;
```

- `id` é o identificador interno (inode, registro MFT, cluster inicial, extent,
  ICB). `repetido` marca um `id` já visto na partição (hard link): o conteúdo é o
  mesmo e o adaptador pode deduplicar.
- Problemas locais (diretório corrompido, cadeia truncada, conteúdo comprimido do
  NTFS) viram `Resumo::avisos` e o percurso continua. Partições ilegíveis vão em
  `Resumo::nao_lidas` com o motivo. Só erro do visitante interrompe.
- Limites: profundidade 128, 200 milhões de membros, 64 MiB por diretório em
  memória, 4 milhões de trechos por arquivo.

## Suporte

| Camada | Suportado | Sem suporte (erro `SemSuporte` ou partição `nao_lida`) |
|---|---|---|
| Contêiner | raw `.img`, VHDX fixo e dinâmico, qcow2 v2/v3 (inclui clusters comprimidos deflate), ISO/UDF de mídia óptica | VHDX diferencial (disco pai), VHDX com log pendente (lido como está, sem replay), qcow2 com arquivo de apoio, cifrado, zstd, L2 estendida ou dados externos |
| Partições | MBR (primárias e estendidas/EBR), GPT (setores de 512 e 4096), sem tabela | LVM, LUKS, btrfs, XFS, swap (registrados como não lidos) |
| ext2/3/4 | extents, mapa de blocos, inline data, 64 bits, htree (lido linearmente), esparsos, links simbólicos, hard links; journal sujo lido como está | compressão, `meta_bg`, arquivos/diretórios cifrados (listados, conteúdo não lido) |
| NTFS | MFT, atributos residentes/não residentes, `$ATTRIBUTE_LIST`, índices `$I30` (raiz e `INDX`), esparsos, tamanho inicializado, hard links, symlinks/junções | conteúdo comprimido (LZNT1) e cifrado (EFS): listado com aviso |
| FAT | FAT12/16/32 com nomes longos (LFN) | |
| exFAT | com e sem `NoFatChain` | |
| ISO 9660 | Joliet (preferido), Rock Ridge (nomes, links, `CL`/`RE`), multiextent | |
| UDF | 1.02 a 2.01, partição simples/esparsa, File Entry e Extended File Entry, descritores curtos/longos/embutidos, links simbólicos | VAT (multissessão), partição de metadados (UDF 2.50+), stream directories; CRC e checksums não são verificados |

## Testes

`cargo test -p dag-imagens`. Os núcleos (FAT, exFAT, ISO, UDF, NTFS, VHDX, qcow2,
MBR, GPT) são imagens montadas em Rust durante o teste, sem root nem ferramentas
externas. ext2/3/4 usa `mke2fs -d` (pula com mensagem se não existir). Cada
formato tem teste estilo fuzz com corrupção aleatória: nunca pode haver pânico.
