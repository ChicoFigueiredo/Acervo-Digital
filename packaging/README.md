# Empacotamento: serviço em segundo plano e elevação sob demanda

Nada aqui é instalado automaticamente. O DAG roda sem nenhum destes itens.

## 1. Serviço opcional (`dag-agente.service`, systemd de usuário)

```
dag servico estado          # instalado? autostart? ativo? PID, desde quando, linger
dag servico instalar        # escreve ~/.config/systemd/user/dag-agente.service (NÃO habilita, NÃO inicia)
dag servico iniciar         # sobe agora
dag servico autostart on    # sobe a cada login (systemctl --user enable)
dag servico linger on       # opcional: sobe no boot e sobrevive ao último logout
dag servico parar | reiniciar | autostart off | linger off
dag servico remover         # para, desabilita, apaga a unidade e recarrega o systemd
```

Três modos: **não instalado** → **instalado sem autostart** (sob demanda) → **instalado com autostart**.

Serviço de usuário vs. linger: um serviço de usuário só vive enquanto o usuário tem alguma
sessão aberta. Sem *linger*, `autostart on` faz o agente subir no primeiro login e cair no último
logout. `dag servico linger on` chama `loginctl enable-linger` (pode pedir autorização do Polkit);
é uma ação separada e nunca é feita por conta própria. Um serviço de **sistema** (root) não é
oferecido: a unidade é sempre de usuário, com `NoNewPrivileges`, `ProtectSystem=strict`,
`ProtectHome=read-only` e gravação só nos diretórios do DAG (dados, estado, runtime).

## 2. Helper privilegiado (`dag-helper`) e Polkit

O helper lê **somente metadados** (nomes, tamanhos, datas, modo, dono) de uma pasta que o seu
usuário não consegue listar. Não abre conteúdo de arquivo, não faz hash, não escreve nada além de
uma linha de auditoria (syslog `authpriv`; e `/var/log/dag-helper.log` se o arquivo existir), não
executa comandos e não toca no Catálogo SQLite. A GUI, a CLI e o SQLite nunca rodam como root.

Instalação (como root, uma vez):

```
cargo build --release -p dag-privileged-helper
install -D -m 0755 -o root -g root target/release/dag-helper /usr/libexec/dag/dag-helper
install -D -m 0644 -o root -g root packaging/polkit/org.dag.helper.policy \
        /usr/share/polkit-1/actions/org.dag.helper.policy
# opcional: restringe as raízes que o helper aceita (uma por linha, caminhos canônicos)
printf '/mnt/dados\n/srv\n' | install -D -m 0644 /dev/stdin /etc/dag/helper.roots
# opcional: auditoria em arquivo (o helper só acrescenta; nunca cria o arquivo)
install -m 0640 -o root -g adm /dev/null /var/log/dag-helper.log
```

Uso: `dag scan --full --elevar <raiz>` (ou `--fast`). Quando uma pasta nega acesso, o DAG executa
`pkexec /usr/libexec/dag/dag-helper`, o Polkit mostra o diálogo do sistema e a senha é digitada
só ali (auth_admin_keep: a autorização vale por alguns minutos nesta sessão). Sem `--elevar`
nada muda: a pasta é registrada como inacessível. Variáveis: `DAG_ELEVAR=1` (liga a elevação no
agente em segundo plano) e `DAG_HELPER=/outro/caminho` (para desenvolvimento; a política Polkit
só autoriza o caminho instalado).

Protocolo (stdin, uma linha JSON; qualquer outro formato é recusado):
`{"raizes":["/mnt/dados"],"caminho":"/mnt/dados/privado"}` → linhas JSON `dir`, `entrada`... e
`fim` (ou `erro`). O caminho precisa ser absoluto, canônico (sem `.`, `..`, `//`), dentro de uma
das raízes e, se `/etc/dag/helper.roots` existir, também de uma raiz do administrador. O percurso
usa `openat` com `O_PATH|O_NOFOLLOW` componente a componente e recusa symlinks.

### Sem agente Polkit / sem pkexec

Em desktops sem agente Polkit ativo (WMs minimalistas, SSH, contêineres) o `pkexec` sai com 126
ou 127 e o DAG explica isso. Alternativas: iniciar um agente (polkit-gnome-authentication-agent-1,
lxqt-policykit-agent, polkit-kde-authentication-agent-1...), conceder leitura ao usuário na pasta
(`setfacl -R -m u:$USER:rX <pasta>`), ou varrer sem `--elevar`.
