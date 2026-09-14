# Prompt complementar — Sapiens Agent

Continue o desenvolvimento do Sapiens Agent a partir do estado real desta pasta. Este prompt cobre somente o trabalho ainda pendente; não reimplemente nem desfaça o que já está funcional. Audite o código antes de editar, preserve mudanças existentes e trabalhe em fatias verticais, como uma equipe sênior de Rust, integrações, segurança, DevOps, UX de CMD e QA.

## Lacunas restantes

1. **Canais e mensageria**

   - Matrix já possui entrada/sincronização incremental `/sync`, mídia inbound/outbound pelo Client-Server, cursor `next_batch`, filtro do próprio bot, allowlist e encaminhamento pelo worker do Gateway; ampliar testes de grupos/menções e integração, mantendo deduplicação, retry, rate limit, health check, policy, sessões e routing.
   - Signal já possui entrada JSON incremental, grupos allowlisted, mídia inbound via `getAttachment`, envio de mídia e encaminhamento para sessions/provider; ampliar testes de grupos/menções e integração, incluindo deduplicação, retry, rate limit, health check, policy e modo simulado.
   - WhatsApp já possui entrada de mídia com download limitado e SHA-256, status, upload/envio de mídia, grupos via allowlist e resposta outbound de texto governada, mantendo verificação GET, HMAC, deduplicação e proteção de payload; completar cobertura de grupos e testes adicionais.
   - Implementar apenas adapters reais e testáveis para outros canais prioritários. Tudo que não tiver implementação real deve permanecer `opcional/desabilitado` e aparecer assim no `status` e na documentação.

2. **Providers e métricas**

   - Completar uso, custo, limites e origem do custo para cada protocolo habilitado. Quando o provider não fornecer `usage`, marcar a estimativa explicitamente.
   - Imagens via CLI, REST/WebSocket e mídia inbound de Matrix, Signal e WhatsApp já são encaminhadas em formato nativo aos providers multimodais habilitados; preservar limites, validação e ausência de credenciais em logs. Áudio/vídeo continuam `opcional/desabilitado` até adapter real e testes próprios.
   - Não habilitar protocolo ou catálogo sem adapter próprio, health check, teste simulado, teste de falha e tratamento seguro de credenciais.

3. **Browser e computer use**

   - O adapter Playwright já expõe as ações avançadas disponíveis no CLI (voltar/avançar/recarregar, duplo clique, drag-and-drop, checkboxes, diálogos, redimensionamento e estado de sessão), preservando isolamento, policy contra SSRF, redirects, uploads, downloads e perfis persistentes; o smoke E2E reproduzível contra página local já passou com `SAPIENS_RUN_BROWSER_E2E=1`. Só ampliar essa cobertura se surgir risco reproduzível.
   - O smoke real de `computer screenshot` e o smoke end-to-end somente leitura de `computer plan`/`computer auto` no desktop Windows já foram validados com PNG, plano, artefatos antes/depois e receipts. Manter cobertura negativa para limites estruturais, entrada sensível sem aprovação, cancelamento cooperativo e emergency stop; ampliar apenas quando houver risco reproduzível. Senhas, tokens, cartões, envio, publicação, compra, pagamento, exclusão e alterações continuam exigindo aprovação explícita; a feature permanece desligada por padrão.
   - Browser Use e Stagehand só podem sair de `opcional/desabilitado` com adapter e testes próprios.

4. **Ferramentas, shell, MCP e plugins**

   - O catálogo tipado já possui descoberta sob demanda, estados por feature, classificação de risco e aprovação centralizada via \`tools list\`/\`tools discover\`; só adicionar novos tipos quando houver executor real, testes e policy próprios.
   - Manter allowlist, limites de saída, workspace e quotas do shell; adicionar backend equivalente para Linux/macOS ou declarar a capacidade indisponível nesses sistemas.
   - Preservar MCP stdio, HTTP streamable e SSE legado, acrescentando testes de negociação, schema inválido, timeout, allowlist, SSRF e falha de sessão.
   - Implementar sandbox verificável e habilitação controlada de plugins, preferencialmente WASM/container. Sem sandbox real, plugins e ferramentas não confiáveis continuam desabilitados.

5. **Scheduler e sessions**

   - Recovery de job `running` após reinício real, cancelamento durante provider lento e entrega scheduler → Discord, Slack, Google Chat, Teams, Matrix e WhatsApp já possuem cobertura. Manter essa cobertura; acrescentar somente entrega real do Signal quando `signal-cli` estiver instalado e houver ambiente de teste seguro. Deduplicação persistida de webhook, retry/idempotência, tarefa sem provider e falhas básicas de provider já possuem cobertura.
   - Testar limites de profundidade, tokens, custo, tempo e concorrência, preservando os estados persistidos `running`, `interrupted`, `succeeded` e `failed` e a retomada explícita.
   - A persistência, concorrência e retomada após reinício real de sessions já possuem cobertura; preservar esse contrato ao evoluir o Gateway.

6. **Segurança, memória e operação multiplataforma**

   - Completar testes e defesas contra prompt injection, exfiltração, impersonação, segredo em logs/receipts e remetente não autorizado.
   - Adicionar embeddings somente como recurso opcional, com backend real, consentimento, retenção e limpeza documentados.
   - Validar instalação, PATH, atualização, integridade e rollback em Linux/macOS quando houver runners; registrar medições reais e limitações, sem prometer suporte não testado.
   - O smoke Windows `install-sapiens-agent.ps1 -DryRun -SkipBuild` já foi validado sem mutações; manter mensagens ASCII e não repetir esse teste como se fosse uma instalação real.

## Regras de execução

- O caminho principal continua sendo `sapiens`/`sapiens-agent` no CMD, com o banner atual e sem abrir navegador automaticamente.
- Não exigir `.exe`, Cargo, Node ou Python ao usuário final.
- Não salvar, exibir, registrar, enviar ao modelo ou incluir em receipt qualquer credencial.
- Não inventar API, endpoint, modelo, protocolo ou integração; use documentação e testes reproduzíveis.
- Escolha a próxima lacuna de maior impacto, implemente-a de ponta a ponta e atualize README, arquitetura e matriz de capacidades.
- Execute `cargo fmt --check`, `cargo check`, `cargo test`, `cargo build --release` e smoke tests do fluxo alterado.
- Só marque uma capacidade como pronta quando houver implementação real, health check, teste de sucesso, teste de falha e documentação. Caso contrário, marque-a claramente como `opcional/desabilitado`.
- Continue em rodadas sucessivas até esgotar as lacunas possíveis com segurança. Ao final de cada rodada, informe objetivamente o que foi feito, validado e o que ainda falta; nunca declare 100% funcional sem evidência.

## Critério de aceite

O complemento está concluído quando todas as lacunas acima estiverem implementadas e testadas, ou explicitamente classificadas como `opcional/desabilitado` por uma limitação documentada e verificável; o release compila, os testes passam, o CMD permanece o fluxo principal e nenhuma proteção de segurança foi relaxada.
