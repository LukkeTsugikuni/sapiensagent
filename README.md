# Sapiens Agent — MVP leve e local-first

> A especificação canônica da experiência de inicialização e configuração está em [PROMPT_MESTRE_SAPIENS_AGENT.md](PROMPT_MESTRE_SAPIENS_AGENT.md). A matriz de auditoria MyClaw/ZeroClaw está em [docs/CONFIGURATION_AUDIT.md](docs/CONFIGURATION_AUDIT.md). A separação entre capacidades prontas e opcionais está em [docs/CAPABILITIES.md](docs/CAPABILITIES.md). O catálogo completo de funções está em [output/pdf/sapiens-agent-catalogo-de-funcoes.pdf](output/pdf/sapiens-agent-catalogo-de-funcoes.pdf).

Este repositório implementa o primeiro incremento executável da especificação em `PROMPT_MESTRE_SAPIENS_AGENT.md`. O núcleo é Rust assíncrono e não requer GPU, CUDA, Node.js ou Python para iniciar. O runtime não embute modelo: ele usa providers remotos compatíveis, Anthropic, Gemini ou Ollama local, sempre configurados pelo usuário.

## Executar

Forma mais fácil no Windows: dê duplo clique em `start-sapiens-agent.cmd`. Ele compila o Sapiens Agent na primeira execução e inicia o servidor local no CMD; nenhum navegador é aberto automaticamente.

Para instalar os comandos globais no Windows, execute `install-sapiens-agent.cmd` uma vez. Feche o terminal, abra um novo PowerShell normal e digite apenas `sapiens` — funciona de qualquer pasta, sem `.exe` e sem entrar no diretório do projeto. `sapiens start` inicia o gateway diretamente e `sapiens setup` abre a configuração. Para atualizar com verificação e rollback local do release, use `powershell -ExecutionPolicy Bypass -File .\install-sapiens-agent.ps1 -Rebuild`.

Em Linux/macOS, execute `./install-sapiens-agent.sh`; os comandos ficam em `~/.local/bin`. A sintaxe e o dry-run do script POSIX foram verificados no Git Bash; a execução nativa Linux/macOS ainda depende de um runner desses sistemas.

O alias `sapiens.cmd` também encaminha comandos para o runtime principal.

Também é possível iniciar sem argumentos:

O comando `sapiens-agent` sem subcomando abre o menu interativo no PowerShell ou CMD. A configuração pode ser feita pelo PowerShell, pelo navegador de forma explícita ou pelos dois modos. Nenhuma interface abre o navegador automaticamente.

O menu principal é dividido em 8 áreas: configuração inicial; provider/API;
canais; gateway/interface; segurança/recursos; memória/identidade/workspace;
skills/ferramentas/automações; e iniciar/status/reconfigurar/ajuda/sair. Todas
as alterações usam a mesma configuração TOML do gateway e da WebUI. A área de
controle também oferece exportação, importação e restauração do backup
`config.toml.bak`.

Na configuração inicial, o terminal usa catálogos numerados e defaults seguros:
provider, autenticação, perfil de segurança, capacidades, interface e recursos
são escolhidos por menu. O fluxo não solicita texto livre para opções que já
possuem catálogo; caminhos, endpoints, modelos não catalogados e o modo
personalizado ficam em `Personalizado/Avançado`. A configuração existente é
oferecida primeiro para reutilização e a WebUI continua opcional, sem abertura
automática do navegador.

```powershell
 cargo run
```

Depois do build release, use diretamente:

```powershell
sapiens-agent init
sapiens-agent setup
sapiens-agent start
sapiens-agent start --open-browser   # opcional: abrir a WebUI
sapiens-agent restart
sapiens-agent status
sapiens-agent doctor
sapiens-agent tools list
sapiens-agent tools discover memory.search
sapiens-agent stop
```

O catálogo tipado pode ser consultado sob demanda com `tools list` e
`tools discover`. Cada item mostra feature, estado e risco; a descoberta não
executa a ferramenta. Operações de escrita continuam bloqueadas até aprovação
explícita, e features desligadas permanecem `opcional/desabilitado`.

Para programar uma tarefa:

```powershell
sapiens-agent schedule add --every 1h --task "verificar novidades"
sapiens-agent schedule once --at now --task "rodar uma vez"
sapiens-agent schedule add --every 1h --task "resumo" --channel suporte --recipient 123 --yes
sapiens-agent schedule cancel job-...
sapiens-agent schedule list
sapiens-agent channel list
sapiens-agent channel add telegram --name suporte
sapiens-agent channel configure suporte
sapiens-agent channel test suporte
sapiens-agent channel send suporte 123 "mensagem" --yes
sapiens-agent provider add ollama --alias local --base-url http://127.0.0.1:11434 --model qwen3
sapiens-agent provider use local
sapiens-agent provider remove local
sapiens-agent identity init
sapiens-agent identity show
sapiens-agent session list
sapiens-agent session cleanup --older-than 2592000 --yes
sapiens-agent session rotate webchat:local --yes
sapiens-agent skills list
sapiens-agent logs
sapiens-agent skills enable shell
sapiens-agent shell --command "dir" --yes
sapiens-agent config export .\config.export.toml
sapiens-agent config import .\config.import.toml
sapiens-agent config restore
sapiens-agent computer plan "abrir o bloco de notas e escrever uma saudação" --output computer-plan.json
sapiens-agent computer run computer-plan.json --yes
sapiens-agent computer auto "abrir o bloco de notas e escrever uma saudação" --yes
sapiens-agent skills enable browser
sapiens-agent browser open https://example.com --session pesquisa
sapiens-agent browser snapshot --session pesquisa
sapiens-agent browser double-click "text=Example" --session pesquisa --yes
sapiens-agent browser state-save .\browser-state.json --session pesquisa --yes
sapiens-agent chat --image .\foto.png "descreva esta imagem"
sapiens-agent memory list
sapiens-agent memory search "termo"
sapiens-agent memory export backup.json
sapiens-agent memory delete sessao-1 --yes
```

Para configurar um provider e conversar:

```powershell
sapiens-agent provider add custom --alias principal --base-url https://host.example/v1 --api-key-env SAPIENS_API_KEY --model modelo
$env:SAPIENS_API_KEY = "..."
sapiens-agent provider use principal
sapiens-agent chat
sapiens-agent start
```

O servidor escuta apenas `127.0.0.1:8787` por padrão. A WebUI está em `http://127.0.0.1:8787/`; a API é `POST /v1/chat` com `{ "message": "...", "images": [{ "mime_type": "image/png", "data_base64": "..." }], "session": "opcional" }`. São aceitas até 4 imagens de no máximo 10 MiB cada; imagens locais também podem ser anexadas pelo CLI com `chat --image caminho.png`.

## O que está funcional no MVP

- CLI one-shot/interativa, servidor HTTP/WebSocket e WebUI local.
- Registry configurável por alias para APIs `chat_completions`/OpenAI-compatible, com seleção por tarefa e fallback.
- Streaming SSE para providers compatíveis com `chat_completions`, com retries limitados e timeout.
- Adapters Anthropic Messages e Gemini Generate Content com contexto de sistema, autenticação própria, SSE e custo real por `usage`, validados em servidores simulados.
- Adapter OpenAI Responses com `input`/`instructions`, `store=false`, extração de `output_text`, streaming SSE e custo baseado em `usage`, validado em servidor simulado.
- Orçamento por provider, custo estimado ou calculado a partir do `usage` retornado pelo provider (`cost_source`), e circuit breaker em falhas consecutivas, configuráveis sem expor credenciais.
- Resposta do gateway com provider efetivamente usado, latência e motivo redigido de fallback; `provider test --json` mede saúde e status.
- Channel Registry com catálogo amplo, adapters locais CLI/WebChat/HTTP/WebSocket/webhook, configuração por variável de ambiente, allowlist e teste explícito de adapter.
- Comandos operacionais `restart`, `channel`, `skills`, `logs`, `version` e `help`.
- Menu interativo no PowerShell/CMD, organizado em 8 áreas, para configurar, iniciar, diagnosticar e sair sem uma pausa silenciosa.
- Skills reais em `skills/`, com `SKILL.md`, validação, habilitação, desabilitação e rollback. A `skill-forge` analisa receipts repetidos e cria candidatas revisáveis sem conceder permissões externas automaticamente.
- WebUI local com dashboard, configuração rápida, canais/mídia, skills, recursos e chat; ela usa a mesma configuração do PowerShell e só é aberta com solicitação explícita.
- Perfis de recursos `economy`, `balanced`, `performance` e `custom`, com limites configuráveis de GPU, CPU, memória e concorrência. GPU é permitida, mas governada para não monopolizar a máquina.
- Configuração de áudio opcional para canais multimídia, com limites de tamanho/duração e seleção de provider. O runtime identifica e preserva mídia recebida, mas só envia áudio ao modelo quando houver adapter validado.
- Credenciais somente por variável de ambiente; configuração redigida via `sapiens-agent config show`.
- Memória opt-in em JSONL e busca textual via `sapiens-agent memory search`.
- Memória com listagem, busca, exportação e exclusão por sessão; o gateway só grava quando `features.memory=true`.
- Scheduler persistente para intervalo, execução única e cron UTC, com checkpoint, contagem, pausa/retomada, concorrência limitada, cancelamento cooperativo, limites de profundidade/tokens estimados/custo/tempo e logs redigidos.
- Scheduler pode entregar eventos/resultados a webhook explicitamente configurado, com retry limitado, timeout, idempotency key e estado da entrega persistido.
- Canais Telegram, Discord, Slack, Google Chat e Microsoft Teams possuem adapters reais de texto; Matrix possui saída e sincronização incremental Client-Server pelo worker do gateway; WhatsApp Cloud API possui saída e entrada de texto/mídia assinada; Signal possui envio e recebimento JSON via `signal-cli` local quando o binário e a conta estão instalados, incluindo grupos allowlisted. Matrix usa cursor `next_batch`, ignora mensagens do próprio bot, aceita allowlist por sala/remetente e registra falhas sem expor credenciais. WhatsApp baixa mídia somente dentro do workspace, com limite e SHA-256, e registra status de entrega; resposta automática de texto exige opt-in e aprovação. Os webhooks usam URL por variável de ambiente, allowlist, policy de rede, retries e receipts redigidos.
- WhatsApp Cloud API também aceita `channel send-media` para upload seguro de imagem, áudio, vídeo ou documento dentro do workspace, com limite de tamanho, MIME validado e envio do `media_id` retornado pela API.
- Matrix usa `SAPIENS_MATRIX_TOKEN` (ou a variável indicada em `channel configure`) e `SAPIENS_MATRIX_HOMESERVER`; WhatsApp Cloud API usa o token indicado no canal, `SAPIENS_WHATSAPP_PHONE_NUMBER_ID` e opcionalmente `SAPIENS_WHATSAPP_GRAPH_BASE_URL` para ambientes de teste/bridge. Para receber WhatsApp, configure também `SAPIENS_WHATSAPP_VERIFY_TOKEN` e `SAPIENS_WHATSAPP_APP_SECRET`; o endpoint é `/v1/webhooks/<nome-do-canal>`. `SAPIENS_WHATSAPP_MAX_MEDIA_BYTES` limita downloads e `SAPIENS_WHATSAPP_AUTO_REPLY=true` habilita resposta de texto somente com aprovação. IDs de webhook são deduplicados por 24 horas no arquivo local `whatsapp-webhook-events.json`, limitado a 10.000 entradas. Signal usa a variável de conta indicada no canal (recomendado `SAPIENS_SIGNAL_ACCOUNT`), aceita `SAPIENS_SIGNAL_CLI_COMMAND` para apontar para o executável local e `SAPIENS_SIGNAL_AUTO_REPLY=true` para resposta governada.
- Browser Playwright permite navegação, snapshot, clique/duplo clique, drag-and-drop, preenchimento, hover, teclado, checkboxes, selects, rolagem, diálogos, abas, redimensionamento, upload/download, tracing e estado de sessão; perfil persistente somente com `browser open --persistent`; `browser profile-revoke --session NOME` apaga os dados da sessão mediante aprovação.
- Computer use Windows aceita planejamento assistido por provider (`computer plan`), execução automática explícita (`computer auto`) e sequências JSON estruturadas com screenshot antes/depois de cada passo, policy/aprovação por ação e `computer emergency-stop`/`computer reset-stop`.
- Policy supervised, allowlist de domínio, bloqueio básico de redes privadas e proteção de caminhos do workspace.
- `computer auto` exige aprovação explícita em modo `supervised`, mantém o plano salvo para auditoria, verifica o emergency stop antes de cada passo e pode ser interrompido cooperativamente.
- Shell restrito ao workspace, com deny-list de comandos destrutivos/de rede, aprovação explícita, timeout, kill-on-drop, limite de saída e quotas OS no Windows via Job Object; permanece desligado por padrão.
- A allowlist e os limites podem ser configurados com `config set shell.allowlist echo,dir`, `shell.max_output_bytes`, `shell.max_memory_mb`, `shell.max_processes` e `shell.max_cpu_secs`. Em sistemas sem Job Object, as quotas OS permanecem explicitamente indisponíveis.
- Contratos estáveis para core, canais, ferramentas, browser, computer use, MCP, plugins, scheduler e sessões.
- Registro persistente de servidores MCP stdio/HTTP/SSE com URL, versão, allowlist, health check, diagnóstico de sessão e comandos `mcp add`, `mcp servers`, `mcp health`, `mcp diagnose`, `mcp list --server` e `mcp call --server`.
- Aprovações persistentes por escopo e risco, com expiração/revogação e comandos `approval grant`, `approval list` e `approval revoke`.
- Arquivos editáveis `IDENTITY.md` e `PREFERENCES.md`, com precedência explícita, visualização redigida e contexto aplicado ao chat do CMD e do Gateway.

## Limites honestos

Telegram já possui adapter real de texto via Bot API; Discord, Slack, Google Chat e Microsoft Teams possuem envio/health check por webhook; Matrix possui `whoami`, envio de `m.room.message`, sincronização incremental `/sync` e mídia inbound/outbound pelo gateway; WhatsApp possui health check, envio de texto/mídia e entrada assinada de mensagens de texto/mídia; Signal possui envio/recebimento JSON, mídia inbound via `getAttachment` e mídia outbound local via `signal-cli`, incluindo grupos allowlisted; e Ollama possui adapter real para `/api/chat` com resposta normal e NDJSON. Imagens podem ser anexadas pelo CLI, REST/WebSocket e mídias de imagem recebidas por Matrix, Signal e WhatsApp são encaminhadas aos providers que suportam visão: OpenAI compatível/Responses, Anthropic, Gemini e Ollama; limites e payloads nativos têm testes locais. Áudio inbound é reconhecido e limitado nos canais que entregam MIME/attachments, mas permanece opt-in; transcrição, síntese e resposta de voz só serão ativadas quando houver adapter e capability do provider validados. Vídeo, menções avançadas e alguns eventos continuam pendentes e não são declarados prontos. O smoke real de `computer screenshot` e o smoke end-to-end somente leitura de `computer plan`/`computer auto` foram validados no desktop Windows com plano, screenshots e receipts. Os demais adapters externos do catálogo permanecem opcionais/desabilitados até implementação e teste próprios. Anthropic Messages, Gemini Generate Content e Ollama possuem adapters reais validados com uso quando o provider fornece contadores; os demais protocolos ainda permanecem opcionais até testes equivalentes. `IDENTITY.md` é a base do comportamento e `PREFERENCES.md` prevalece apenas em estilo/formato; ambos são redigidos antes de seguir ao provider. A policy do browser valida esquema, userinfo, allowlist, DNS, redes privadas e endpoints de metadata também após redirects; o adapter Playwright oferece as ações avançadas suportadas pelo CLI, estado de sessão e perfil persistente opt-in, que pode ser revogado com `browser profile-revoke`. Browser Use/Stagehand, llama.cpp, vLLM, OAuth, SQLite e sandbox OS permanecem deny-by-default quando não validados. Plugins verificam SHA-256, assinatura HMAC opcional e origem, são instalados/atualizados de forma transacional com rollback e permanecem desabilitados até sandbox verificável. MCP stdio, HTTP streamable e fallback SSE legado validam JSON-RPC/schema, timeout, allowlist, policy de URL e negociação das versões suportadas; `mcp diagnose` informa ciclo de vida efêmero, transporte, versões aceitas, latência e ferramentas sem revelar credenciais. O computer use Windows oferece `computer plan` e `computer auto` com aprovação explícita, com limites, aprovação, cancelamento cooperativo e emergency stop testados; testes com providers reais e ações de escrita ainda permanecem pendentes. O adapter Playwright exige Node.js e `@playwright/cli` somente quando habilitado. Não há suporte nativo fingido para provedores ou protocolos que não foram validados.

## Configuração e roteamento

```powershell
sapiens-agent provider list
sapiens-agent provider test principal
sapiens-agent provider models principal
sapiens-agent route set complexa principal
sapiens-agent route fallback principal,backup
sapiens-agent config show
sapiens-agent config set provider.principal.budget_usd 5
sapiens-agent config set provider.principal.circuit_breaker_threshold 3
sapiens-agent config set scheduler.max_concurrent 2
sapiens-agent config set scheduler.max_tokens 4096
sapiens-agent config set shell.max_memory_mb 512
sapiens-agent config set shell.max_processes 16
sapiens-agent config set shell.max_cpu_secs 60
```

A configuração fica em `%APPDATA%\sapiens-agent\config.toml` no Windows, `$XDG_CONFIG_HOME/sapiens-agent/config.toml` no Linux, ou em `.sapiens-agent/` no workspace. `CLAW_HOME` continua aceito como override de compatibilidade.

## Próximas fases

1. Áudio/vídeo inbound, menções/eventos avançados e protocolos adicionais de providers.
2. Uso/custo real reportado por provider, métricas de recursos, SQLite e autenticação mais completa da WebUI.
3. Testes adicionais de entrega e retomada/cancelamento após reinício no scheduler, além de execução de computer use com providers reais e ações de escrita supervisionadas.
4. Sandbox verificável e habilitação controlada de plugins, governança contextual de identidade/prompt injection e testes multiplataforma.

## Segurança

Conteúdo de páginas, mensagens e resultados externos deve ser tratado como dados. O servidor é localhost-only por padrão; não coloque-o diretamente na internet. Recursos de alto risco precisam de uma camada de aprovação antes de serem ativados.
- Adapter Playwright opcional por CLI, com sessão isolada, snapshot, navegação e ações avançadas suportadas pelo CLI, download/estado controlados, perfil persistente opt-in, revogação e política de URL; requer Node.js somente quando habilitado.
- Adapter Windows de computer use com screenshot PNG e sequências estruturadas; exige `skills enable computer-use` e aprovação explícita para ações de escrita.
- Entrada de imagens por `chat --image`, REST/WebSocket e mídias recebidas de Matrix, Signal e WhatsApp, com payload nativo para providers multimodais habilitados.
- Cliente MCP stdio/HTTP streamable/SSE legado JSON-RPC opcional, com registro persistente, health check, listagem/chamada por CLI, timeout, policy de URL e allowlist de ferramentas.
- Plugins com instalação/atualização transacional, rollback, origem, versão, permissões, verificação SHA-256, assinatura HMAC opcional e remoção protegida; execução permanece desabilitada até sandbox.
- Para verificar assinatura HMAC, forneça `SAPIENS_PLUGIN_SIGNING_KEY` apenas no ambiente do comando; a chave nunca é gravada em configuração, log ou receipt.
- Scheduler com intervalo, execução única e cron UTC:

  ```powershell
  sapiens-agent schedule cron --expression "0 9 * * 1-5" --task "resumo da manhã"
  ```

- O scheduler também grava heartbeats estruturados em `sapiens-agent.metrics.jsonl`, incluindo tarefas ativas, tarefas em execução e o limite de concorrência.

- Receipts JSONL redigidos para ações de browser, computer use, shell, MCP, plugins e emergency stop; consulta via `sapiens-agent receipts`.
- Autenticação local opcional por variável, rate limit por identidade/canal, risk scoring/bloqueio básico de prompt injection e endpoint de emergency stop fail-closed.
- Tarefas agendadas também passam pelo mesmo bloqueio de prompt injection de alto risco antes de chamar o provider, com teste negativo dedicado.
- A entrega agendada foi exercitada localmente para Discord, Slack, Google Chat, Teams, Matrix e WhatsApp; Signal continua dependente do `signal-cli` instalado.
- Aprovação persistente, por exemplo: `sapiens-agent approval grant shell.exec --risk external_write --expires-secs 3600 --yes`.
- Retenção configurável da memória via `memory_retention_days`; embeddings permanecem opcionais/desabilitados.
