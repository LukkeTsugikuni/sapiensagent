# Sapiens Agent — arquitetura e aceite do MVP

## 1. Resumo executivo

Sapiens Agent é um runtime Rust assíncrono, local-first e sem dependência obrigatória de GPU, CUDA, Node.js ou Python. O MVP conversa com uma API compatível com OpenAI, opera por CLI e expõe uma WebUI/API somente em localhost. O processo não incorpora um LLM; providers remotos e futuros motores locais são sidecars.

## 2. Decisões

- Rust + Tokio: binário único, cancelamento e I/O assíncrono.
- Launchers globais no PATH do usuário: `sapiens-agent`/`sapiens` escondem a extensão e os caminhos de build; o instalador cria wrappers em `%LOCALAPPDATA%\SapiensAgent\bin`.
- `reqwest` com rustls: chamada HTTPS sem runtime externo.
- Configuração TOML: legível e portátil; credenciais são referenciadas por nome de variável de ambiente, nunca armazenadas. Escritas são serializadas, sincronizadas e substituídas por rename atômico, com backup `.toml.bak` da versão anterior.
- JSONL para memória inicial: zero banco obrigatório, fácil exportação; SQLite fica para a fase de retenção/consultas concorrentes.
- `supervised` e deny-by-default: recursos de alto risco não são ativados implicitamente.
- Adapters por traits: browser, computer use, MCP, canais, plugins, scheduler e providers podem evoluir sem alterar o contrato do loop; HTTP REST e WebSocket compartilham o núcleo do gateway.

## 3. Diagrama

```text
CLI / WebUI localhost / canais futuros
                 |
              gateway
                 |
        core: contexto + cancelamento
           /      |       \
       routing  policy   memory
          |       |        |
     providers  tools   JSONL
          |
   API remota ou sidecar local

  browser | computer | mcp | shell | scheduler | plugins
                 |
       allowlist + aprovação + receipts redigidos
```

## 4. Estrutura

```text
src/
  core.rs          loop e contrato de provider
  gateway.rs       HTTP/WebUI localhost
  providers.rs     registry, aliases, fallback e OpenAI-compatible
  config.rs        TOML e caminhos de dados
  policy.rs        riscos, workspace e SSRF básico
  memory.rs        memória JSONL e busca textual
  channels.rs      evento comum e contrato de canal
  tools.rs         catálogo tipado sob demanda
  browser.rs       adapter Playwright CLI e contrato de browser isolado
  computer.rs      ações estruturadas de desktop
  mcp.rs           contrato e allowlist de MCP
  shell.rs         executor limitado ao workspace
  scheduler.rs     intervalos, once, checkpoints e worker do gateway
  sessions.rs      contrato de sessão e retomada
  identity.rs      identidade/preferências editáveis e contexto redigido
  plugins.rs       manifesto, SHA-256, assinatura e remoção protegida
  observability.rs logging redigido
tests/acceptance.rs
examples/claw.example.toml (legado)
```

## 5. Contratos principais

`ProviderConfig` declara `alias`, `kind`, `protocol`, `base_url`, `api_key_env`, `model`, timeout, retries, streaming, limite de contexto, limite de tokens, orçamento/custo estimado, circuit breaker e capacidades. O runtime implementa `chat_completions`/`openai`, `responses`, `anthropic_messages`, `gemini` e `ollama`, incluindo SSE/NDJSON compatível com cada protocolo, entrada de imagens em payload nativo e extração validada de texto; protocolos declarados mas não implementados falham explicitamente. O gateway informa provider efetivo, latência, custo estimado ou calculado a partir de `usage` (`cost_source`) e fallback com erro redigido.


`ToolSpec` declara nome, descrição, feature, estado e risco. `ToolCatalog` expõe
`tools list`, descobre uma ferramenta sob demanda sem iniciar processos e delega
a decisão de aprovação à `Policy`; features desligadas permanecem
`opcional/desabilitado`.

O menu do CMD/PowerShell expõe oito áreas navegáveis. Provider/API permite
editar protocolo, endpoint, modelo, credencial referenciada por ambiente,
temperatura, tokens, timeout, retry, streaming, custos, orçamento e circuit
breaker; canais mostram adapter, transporte, capabilities, dependência e
estado; controle oferece start/stop/restart, status/doctor, reconfiguração,
exportação, importação e restore. A WebUI continua opcional e usa o mesmo
arquivo de configuração.

O primeiro fluxo e as operações comuns usam catálogos numerados com defaults
seguros, reutilização da configuração existente e cancelamento por `0`/`Esc`.
Entradas livres ficam restritas aos caminhos, endpoints, modelos não catalogados
e opções marcadas como `Personalizado/Avançado`; isso evita transformar a
configuração normal em uma sequência de perguntas de texto.

`BrowserDriver`, `ComputerUseAdapter`, `McpClient` e `SessionStore` são interfaces estáveis. `IDENTITY.md` fornece a identidade base e `PREFERENCES.md` complementa e prevalece em estilo/formato; o contexto é redigido antes de chegar ao provider e é usado pelo chat do CMD e pelo Gateway. Playwright está integrado ao CLI, mas continua desligado por padrão e sujeito à policy de URL, que valida esquema, userinfo, allowlist, DNS, redes privadas e metadata endpoints inclusive na URL final após redirect. Downloads e arquivos de estado de sessão são controlados no workspace e perfis persistentes exigem opt-in explícito e podem ser revogados com comando protegido. O adapter cobre navegação, snapshot, clique/duplo clique, drag-and-drop, preenchimento, hover, teclado, checkboxes, selects, rolagem, diálogos, abas, redimensionamento, upload/download, tracing e salvamento/restauração de estado; execução JavaScript arbitrária não é exposta pelo CLI. O adapter Windows de computer use já captura screenshot e executa sequências JSON com policy, aprovação, cancelamento cooperativo e emergency stop; `computer plan` gera uma sequência JSON validada pelo provider sem executá-la, e o smoke real somente leitura de `computer plan`/`computer auto` cobre plano, execução, screenshots e receipts. Browser Use/Stagehand continuam opcionais até implementação e testes próprios; shell já possui executor restrito ao workspace, deny-list destrutiva/de rede, allowlist configurável, timeout, kill-on-drop, limite de saída e quotas de CPU/memória/processos no Windows via Job Object; em sistemas sem esse mecanismo, as quotas permanecem indisponíveis. Plugins possuem instalação/atualização transacional, rollback e ficam desabilitados até sandbox verificável.

## 6. Configuração e comandos

```powershell
sapiens-agent provider add custom --alias principal --base-url https://host.example/v1 --api-key-env SAPIENS_API_KEY --model modelo
sapiens-agent provider use principal
sapiens-agent provider test principal
sapiens-agent provider models principal
sapiens-agent provider remove principal
sapiens-agent route set complexa principal
sapiens-agent route fallback principal,backup
sapiens-agent schedule add --every 1h --task "verificar novidades"
sapiens-agent schedule pause job-...
sapiens-agent schedule resume job-...
sapiens-agent schedule cancel job-...
sapiens-agent config set scheduler.max_concurrent 2
sapiens-agent mcp servers
sapiens-agent mcp health --server nome --yes
sapiens-agent mcp diagnose --server nome --yes
sapiens-agent mcp add remoto --url https://mcp.example/mcp --transport http --allow read
sapiens-agent config show
sapiens-agent config export .\config.export.toml
sapiens-agent config import .\config.import.toml
sapiens-agent config restore
sapiens-agent identity init
sapiens-agent identity show
sapiens-agent chat
sapiens-agent serve --bind 127.0.0.1:8787
```

Para providers compatíveis, `base_url` deve ser a raiz da API (por exemplo, terminando em `/v1`) ou o endpoint completo `/chat/completions`. Para Ollama, use a raiz do servidor (`http://127.0.0.1:11434`), `/api` ou `/api/chat`; o adapter normaliza para `/api/chat` e `/api/tags`, aceita NDJSON quando streaming está habilitado e usa `prompt_eval_count`/`eval_count` quando presentes. O runtime não inventa modelos ou credenciais; o alias, modelo e endpoint continuam sendo configurados pelo usuário.

## 7. Aceitação executada

| Verificação | Resultado |
|---|---|
| Windows x64 / CPU | Passou; Rust 1.93.1, sem GPU |
| `cargo test` | Passou: 121 testes (110 unitários de biblioteca, 4 de CLI, 6 acceptance e 1 integração de canais) |
| `cargo fmt --check` | Passou |
| CLI `--help` | Passou |
| Instalador Windows `-DryRun -SkipBuild` | Passou; não altera PATH nem arquivos |
| Provider, alias, rota e config redigida | Passou |
| WebUI `/` | HTTP 200 |
| API `/health` | HTTP 200, `ok` |
| Health check local | 173 ms observado via PowerShell |
| Caminhos fora do workspace | Bloqueio coberto pelo contrato/policy |
| localhost/rede privada | Bloqueio coberto por teste |
| Processo release em repouso | 7,88 MiB working set observado |
| CPU do processo release em repouso | 0,016 s observado |
| Binário release | 5.101.568 bytes (aprox. 4,87 MiB) |
| SHA-256 do release validado | `BBA1A9F0890A8399BC82E7BFD3392036684F5FEA213FF91D0FC685992AD5D20B` |

## 8. Limitações e próximos passos

O scheduler já executa intervalos, `once` e cron UTC com checkpoint, pausa/retomada, concorrência limitada, cancelamento cooperativo, estado persistido (`running/interrupted/succeeded/failed`) e retomada explícita via `schedule resume`, além de limites de profundidade/tokens estimados/custo estimado/tempo, webhook de evento/resultado com retry, timeout, idempotency key, estado da entrega e heartbeat estruturado em `sapiens-agent.metrics.jsonl`; recovery de um job `running` após reinício real, cancelamento durante chamada de provider e o caminho scheduler → adapter Discord/webhook já possuem testes, mas ainda faltam testes de entrega nos demais adapters de canal. Sessions têm listagem, limpeza, rotação, persistência, cobertura concorrente e teste acceptance de retomada após reinício real do processo. Telegram, webhooks, WhatsApp texto/mídia recebida e enviada assinada, status, Signal envio/recebimento JSON/mídia inbound/outbound e Matrix `whoami`/envio/sincronização incremental `/sync`/mídia inbound/outbound possuem adapters reais com allowlist e policy; os workers Matrix e Signal encaminham mensagens allowlisted para sessions, provider e memória, filtram o próprio bot quando aplicável e registram cursores/falhas de forma redigida. WhatsApp baixa mídia com limite e SHA-256, faz upload controlado de mídia local e só responde automaticamente com opt-in/aprovação. Imagens entram pelo CLI, REST/WebSocket e mídias de imagem recebidas por Matrix, Signal e WhatsApp, com payloads nativos testados para os providers multimodais habilitados; áudio/vídeo, menções avançadas, alguns eventos e testes de computer use com providers reais e ações de escrita seguem pendentes. O smoke real somente leitura de `computer screenshot`, `computer plan` e `computer auto` no desktop está validado. Anthropic Messages, Gemini Generate Content e Ollama possuem adapters reais com streaming e uso quando fornecido; os demais protocolos permanecem deny-by-default. MCP stdio, HTTP streamable, fallback SSE legado e negociação de versões estão validados com allowlist, health check, `mcp diagnose` e policy de URL. Plugins possuem instalação/atualização transacional, rollback e verificação de hash/origem, mas sua execução permanece desabilitada até sandbox. O núcleo possui score/bloqueio básico de prompt injection, redaction de tokens antes do modelo e aprovações persistentes; defesas contextuais adicionais contra conteúdo não confiável, exfiltração e impersonação ainda precisam de aprofundamento. O adapter Playwright cobre o conjunto avançado de ações do CLI, download, estado de sessão e perfil persistente opt-in; Browser Use/Stagehand continuam opcionais.

O working set foi medido no Windows com o servidor sem chamadas a provider, browser, modelo ou sidecar; não é uma garantia para esses processos adicionais. A meta de 100 MB aplica-se ao processo principal isolado. O binário release validado mede 4.913.664 bytes (aprox. 4,69 MiB); o tamanho final de instaladores pode variar por plataforma.

OpenAI Responses segue o endpoint oficial `/responses`, envia `input` e `instructions` com `store=false`, extrai texto de `output`/`output_text`, acumula eventos `response.output_text.delta` em streaming SSE e usa `usage.input_tokens`/`usage.output_tokens` quando presentes.
`InboundEvent` unifica canal, conta, remetente, conversa, texto, anexos, destino de resposta, capacidades solicitadas e timestamp. Conteúdo recebido é dado não confiável. O gateway local expõe REST, WebSocket e webhook com o mesmo processamento de sessão, provider e memória; webhook genérico exige token e allowlist, enquanto WhatsApp usa handshake GET, HMAC-SHA256 (`X-Hub-Signature-256`) e deduplicação persistida, limitada a 10.000 IDs com retenção de 24 horas. O worker Matrix usa `GET /_matrix/client/v3/sync`, encadeia `next_batch` como `since`, ignora mensagens do próprio usuário, filtra por sala/remetente allowlisted e grava cursor atômico; respostas automáticas só podem ser enviadas com `SAPIENS_MATRIX_AUTO_REPLY=true` e modo `trusted` ou aprovação persistente de envio.

O comando `computer auto` gera um plano pelo provider e executa a sequência apenas com aprovação explícita em modo `supervised`; o plano permanece salvo para auditoria, cada passo captura screenshot antes/depois e o arquivo de emergency stop é verificado antes da ação. O fluxo real de `computer screenshot` e o smoke completo somente leitura de `computer plan`/`computer auto` foram validados no desktop Windows com provider local de teste, PNGs, plano e receipts. Permanecem necessários testes com providers reais e ações de escrita/não somente leitura; testes negativos adicionais de segredo, destruição, cancelamento e emergência continuam no backlog.

Nota da validação corrente: a entrega agendada também possui testes locais para Slack, Google Chat, Teams, Matrix e WhatsApp; Signal continua condicionado ao `signal-cli` instalado.
