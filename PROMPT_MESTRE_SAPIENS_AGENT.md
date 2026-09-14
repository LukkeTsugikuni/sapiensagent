# Prompt de continuação — Sapiens Agent

## Missão

Continue o desenvolvimento do Sapiens Agent nesta pasta a partir do estado real do código. Não reimplemente nem desfaça as fatias já funcionais; audite o projeto antes de editar, preserve mudanças existentes e implemente somente as lacunas abaixo. Trabalhe como uma equipe sênior de arquitetura Rust, integrações, segurança, DevOps, UX de terminal e QA.

O resultado deve ser funcional ou explicitamente marcado como `opcional/desabilitado`. Nunca declare suporte apenas porque existe um item no catálogo.

## Invariantes que devem permanecer

- A experiência do usuário é `sapiens-agent`/`sapiens`, sem exigir `.exe`, Cargo, Node ou Python.
- O início padrão é no CMD, com o banner `S A P I E N S AGENT`; não abrir navegador automaticamente.
- A WebUI permanece local e opcional, abrindo apenas com `--open-browser`.
- `supervised` e deny-by-default continuam sendo os padrões.
- Nenhuma credencial pode ser salva, exibida em log, enviada ao modelo ou colocada em receipt.
- Não inventar endpoint, modelo, capacidade, protocolo ou integração sem documentação e teste reproduzível.

## Trabalho restante obrigatório

### 1. Providers

- Ampliar métricas de uso/custo com dados reais de todos os protocolos que forem habilitados, mantendo estimativa explicitamente identificada quando `usage` não existir.
- `ollama` já possui adapter real para `/api/chat`, resposta normal e streaming NDJSON, com testes de sucesso/uso; `responses` já possui adapter real para criação de resposta, extração de texto, streaming SSE e uso; manter os demais protocolos opcionais/desabilitados até adapter próprio e testes de sucesso/falha.
- Entrada de imagens por CLI, REST/WebSocket e mídias de imagem recebidas por Matrix, Signal e WhatsApp já possui payload nativo para OpenAI compatível/Responses, Anthropic, Gemini e Ollama, com limites e testes locais; áudio e vídeo permanecem opcionais/desabilitados até adapters e testes próprios.

### 2. Canais reais

- Consolidar mídia inbound de Matrix e Signal e os demais canais prioritários com testes de integração; Matrix já possui sincronização incremental `/sync`, entrada/saída de mídia pelo worker do Gateway, cursor `next_batch`, filtro do próprio bot, allowlist e encaminhamento para sessions/provider. WhatsApp já possui handshake GET, assinatura HMAC, entrada de texto/mídia com verificação de integridade, status, resposta de texto governada e saída de texto/mídia. Signal já possui envio/recebimento local de texto, entrada de attachments via `getAttachment` e envio de mídia quando o binário está instalado.
- Para cada novo adapter, implementar setup, credencial por variável/cofre, allowlist, identidade, grupos/menções, mídia quando suportada, retries, rate limit, health check, deduplicação, resposta progressiva, modo simulado e teste de falha.
- Unificar entrada e saída dos novos adapters pelo Gateway, sessions, routing e policy; remetente não autorizado deve ser rejeitado e testado. Para Matrix e Signal, ampliar testes de mídia, grupos e menções; para WhatsApp, completar cobertura de grupos e testes de upload/envio de mídia.
- O que não tiver adapter real deve continuar `opcional/desabilitado` e aparecer assim no status/documentação.

### 3. Browser isolado

 - O adapter Playwright já expõe as ações avançadas disponíveis no CLI (voltar/avançar/recarregar, duplo clique, drag-and-drop, checkboxes, diálogos, redimensionamento e estado de sessão); o smoke E2E reproduzível contra página local já passou com `SAPIENS_RUN_BROWSER_E2E=1`; manter downloads, uploads, navegação, redirects e perfis persistentes sob a policy existente.
- Browser Use/Stagehand permanecem adapters opcionais até implementação e testes próprios.

### 4. Computer use

- Preservar `computer plan`, que gera uma sequência JSON validada pelo provider sem executar ações. `computer auto` já executa o plano somente após aprovação explícita, mantendo cada passo sob policy, screenshots, emergency stop e receipts; ampliar seus testes negativos antes de relaxar qualquer proteção.
- Manter e ampliar o smoke end-to-end somente leitura das sequências em Windows com desktop real; `computer plan`/`computer auto` já foram validados com provider local de teste, limites estruturais, segredo sem aprovação, cancelamento cooperativo e emergency stop já possuem testes. Manter a feature desligada por padrão.
- Exigir aprovação para senha, token, cartão, envio, publicação, compra, pagamento, exclusão ou alteração.

### 5. Ferramentas, shell, MCP e plugins

- O catálogo tipado já possui descoberta sob demanda, estados por feature, risco e aprovação centralizados via \`tools list\`/\`tools discover\`; só adicionar novos tipos quando houver executor real, testes e policy próprios.
- Preservar a allowlist configurável, o limite de saída e as quotas OS Windows via Job Object do shell; adicionar backend equivalente para outros sistemas ou mantê-los explicitamente indisponíveis.
- Preservar MCP stdio, HTTP streamable, fallback SSE legado, negociação das versões suportadas e diagnóstico de sessão já validados, sem relaxar validação de protocolo/schema e allowlist.
- Validar sandbox verificável e habilitação controlada de plugins; a instalação/atualização transacional, rollback, origem confiável, hash e marcador `DISABLED` já estão implementados e testados.
- Usar WASM/container sandbox quando disponível; sem sandbox, manter plugin/ferramenta não confiável desabilitado.

### 6. Scheduler avançado

- Validar entrega, retry e estado para todos os adapters de canal habilitados.
- Recovery/retomada após reinício, cancelamento cooperativo e deduplicação já possuem cobertura; a entrega agendada já é testada em Discord, Slack, Google Chat, Teams, Matrix e WhatsApp. Preservar essa cobertura e acrescentar somente entrega real do Signal quando `signal-cli` estiver instalado e houver ambiente de teste seguro, mantendo retry/idempotência, tarefa sem provider e limites básicos.
- Preservar o estado persistido `running/interrupted/succeeded/failed` e a retomada explícita via `schedule resume`; testar os fluxos com provider simulado, falha, reinício e tarefa sem provider.

### 7. Sessions, identidade e governança

- A persistência, concorrência e retomada após reinício real de sessions já possuem cobertura; preservar esse contrato ao evoluir o Gateway.
- Completar defesas contextuais contra prompt injection, exfiltração e impersonação, com testes negativos e sem enviar segredos ao modelo.
- Adicionar embeddings opcionais somente com backend real e política explícita de retenção/consentimento.

### 8. Gateway, instalação e operação

- Validar equivalentes PATH, atualização, integridade e rollback do instalador em Linux/macOS sem mostrar `target/...exe` na documentação normal.
- O instalador Windows já possui smoke `-DryRun -SkipBuild` validado sem alterar PATH ou arquivos; preservar essa garantia e completar apenas testes reais de atualização/rollback quando houver uma execução isolada e reversível.
- Preservar as medições Windows já documentadas (RAM, CPU, latência e tamanho do release) e validar equivalentes no pipeline Linux/macOS quando houver runners; documentar números observados e limitações reais.

## Processo de execução

1. Inspecione o estado atual e a documentação antes de qualquer mudança.
2. Escolha a próxima fatia vertical com maior impacto e implemente-a de ponta a ponta.
3. Para cada capacidade, prefira adapter real testável; se faltar dependência, credencial ou sandbox, deixe-a explicitamente opcional/desabilitada.
4. Não peça ao usuário para repetir este prompt. Tome decisões seguras, reversíveis e compatíveis com os invariantes.
5. Atualize README, arquitetura e matriz de capacidades após cada fatia.
6. Execute `cargo fmt --check`, `cargo check`, `cargo test`, `cargo build --release` e smoke tests do fluxo alterado.
7. Valide segurança com testes negativos para SSRF, caminho externo, remetente não autorizado, segredo, prompt injection e ação destrutiva.
8. Só considere uma fatia concluída quando houver comportamento reproduzível, teste e documentação correspondente.

## Aceite final

- Nenhuma integração é declarada pronta sem adapter, health check, teste simulado e teste de falha.
- Todo recurso não implementado aparece como opcional/desabilitado no status e na documentação.
- O CMD continua sendo o caminho principal e nenhum início abre navegador por padrão.
- O projeto compila em release, os testes passam e os limites/limitações estão medidos.
- Ao terminar uma rodada, informe objetivamente o que foi implementado, o que foi validado e quais itens ainda permanecem neste prompt.
