# Matriz de capacidades — Sapiens Agent

Esta matriz distingue capacidades implementadas e testadas das que dependem de
uma ferramenta, credencial, sandbox ou sistema operacional externo. Um item
marcado como `opcional/desabilitado` não é anunciado como suporte pronto.

| Área | Estado | Evidência ou condição |
|---|---|---|
| Início no CMD (`sapiens`/`sapiens-agent`) | pronto | Banner `S A P I E N S AGENT`, wrappers sem `.exe` no comando do usuário, sem abertura automática de navegador |
| Gateway local REST/WebSocket/Webhooks | pronto | `/health` respondeu `200 ok`; sessões, rate limit, autenticação opcional e emergency stop |
| CLI/WebUI | pronto | CLI é o caminho principal; WebUI local opcional e `--open-browser` explícito |
| Providers chat completions, Responses, Anthropic, Gemini e Ollama | pronto | Adapters reais, health/modelos, retry/fallback, limites, uso/custo e testes de sucesso/falha |
| Imagens multimodais | pronto para providers compatíveis | CLI, REST/WebSocket, Matrix, Signal e WhatsApp; payloads nativos e limites testados |
| Áudio e vídeo multimodal | opcional/desabilitado | Sem adapter e testes próprios no runtime atual |
| Telegram | pronto para texto | Bot API, allowlist, health check e envio |
| Discord, Slack, Google Chat e Teams | pronto para texto/webhook | Webhook, payload específico, allowlist, retry e testes locais |
| Matrix | pronto para texto e mídia | `whoami`, `/sync` incremental, cursor, allowlist, mídia e entrega agendada testados |
| WhatsApp Cloud API | pronto para texto e mídia | HMAC, deduplicação, status, download verificado, upload e entrega agendada testados |
| Signal | opcional até dependência local | Adapter `signal-cli` implementado; health/envio/recebimento/mídia exigem `signal-cli` instalado e conta configurada; executável ausente no ambiente de validação |
| Outros canais do catálogo | opcional/desabilitado | Sem adapter real próprio, permanecem bloqueados e identificados como opcionais |
| Browser Playwright | opcional, implementado quando habilitado | Sessões isoladas, navegação, snapshot, ações avançadas, abas, estado, upload/download e policy SSRF; smoke real na WebUI e E2E contra página local passaram; requer Node.js/`@playwright/cli` somente ao habilitar |
| Browser Use/Stagehand | opcional/desabilitado | Sem adapter próprio validado |
| Computer use Windows | pronto sob aprovação | Screenshot, plano JSON, execução supervisionada, receipts, cancelamento e emergency stop; escrita exige aprovação |
| Shell | opcional/desabilitado por padrão | Workspace, allowlist, deny-list, timeout, limite de saída e Job Object no Windows; quotas OS não disponíveis nos demais sistemas |
| MCP stdio/HTTP/SSE | opcional/desabilitado por padrão | Transporte, negociação, schema, allowlist, health e diagnóstico testados quando habilitado |
| Plugins | opcional/desabilitado | Instalação/verificação/rollback implementados; execução bloqueada até sandbox verificável |
| Memória | opcional/desabilitado por padrão | JSONL, retenção, busca, exclusão, exportação e redaction |
| Scheduler | pronto sob configuração | Intervalo, once, cron UTC, checkpoint, recovery, limites, cancelamento, retry/idempotência e entrega em canais testados |
| Segurança | pronto na camada validada | `supervised`, deny-by-default, redaction, policy URL/path, allowlist, prompt injection, autenticação e approvals |
| Catálogo tipado de ferramentas | pronto sob configuração | `tools list`/`tools discover`, estados por feature, classificação de risco e gate central de aprovação; descoberta não executa processos |
| Embeddings | opcional/desabilitado | Nenhum backend real habilitado |
| Instalação Windows | pronto para o fluxo validado | PATH, wrappers, hash do release, rollback de build e dry-run sem mutação |
| Instalação Linux/macOS | opcional até runner | Script POSIX teve sintaxe e dry-run validados pelo Git Bash; WSL/Linux real não está instalado neste ambiente, então build/PATH/rollback POSIX continuam sem validação nativa |

Validação atual do projeto: `cargo fmt --check`, `cargo check`, `cargo test
--all --no-fail-fast`, `cargo clippy --all-targets -- -D warnings` e
`cargo build --release`. A suíte atual possui 117 testes, além do smoke de
health do gateway, status do CMD e dry-run do instalador Windows.
