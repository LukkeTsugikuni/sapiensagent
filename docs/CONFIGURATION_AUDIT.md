# Matriz de auditoria da configuração

Este registro separa observação, inferência e decisão de implementação. O
MyClaw local foi consultado somente por nomes de seções, chaves e contratos;
nenhum valor de credencial, token ou conteúdo pessoal foi copiado para o
projeto.

| Fonte/área | Observado | Inferido | Adotado no Sapiens Agent |
|---|---|---|---|
| MyClaw: provider e roteamento | Provider/modelo padrão, temperatura, rotas, embeddings, fallback e custos | Provider deve ser escolhido por alias e ter limites explícitos | Provider/API permite alias, protocolo, endpoint, modelo, temperatura, tokens, timeout, retry, streaming, orçamento, custos e circuit breaker; fallback fica persistido |
| MyClaw: autonomia e runtime | Nível de autonomia, workspace-only, comandos permitidos, caminhos proibidos e limites de runtime | Recursos externos precisam de escopo, quota e aprovação | Modo readonly/supervised/trusted, workspace, shell allowlist, limites, policy central e approvals persistentes |
| MyClaw: canais | Catálogo de canais, configuração por ambiente, allowlist, timeout e retry; Telegram tem contrato próprio | Canal deve declarar dependência, transporte, capability e estado | Catálogo tipado com Telegram, Discord, Slack, Google Chat, Teams, Matrix, WhatsApp, Signal, webhook e CLI; canais opcionais permanecem desabilitados até dependência/credencial |
| MyClaw: scheduler e heartbeat | Tarefas recorrentes, cron, concorrência, histórico e polling | Automação deve ser controlável, persistida e cancelável | Wizard de scheduler com every/once/cron, pausa, retomada, cancelamento, remoção, limites, retry, idempotência e receipts |
| MyClaw: memória e identidade | Retenção, higiene, arquivos de identidade, snapshots e contexto | Memória e identidade precisam de exportação, redaction e escopo | Backend JSONL explícito, retenção, busca, limpeza, exportação, `IDENTITY.md`, `PREFERENCES.md`, `USER.md`, `SOUL.md`, `AGENTS.md`, `HEARTBEAT.md` e manifest de workspace |
| MyClaw: gateway, browser e multimodal | Bind/porta, autenticação, pairing/rate limit, allowlist de browser/computer use, limites de imagem/áudio | Interface web deve ser opcional e não abrir sozinha; mídia requer capability | Gateway/interface no terminal, WebUI opcional, `--open-browser` explícito, policy SSRF/path, computer use supervisionado e áudio opt-in com limites |
| MyClaw: skills e ferramentas | Skills, classificação de prompt, dispatcher, ferramentas e plugins | Trabalho repetitivo deve sugerir uma skill, mas criação/execução deve ser governada | Catálogo de ferramentas por feature/risco, `skills suggest`, `skills create`, validação, enable/disable, rollback, MCP, plugins desabilitados até sandbox |
| ZeroClaw oficial | Onboarding interativo organizado em etapas de workspace, provider, canais, túnel, segurança, hardware, memória, contexto e arquivos | O fluxo deve ser linear, legível, retomar configuração e mostrar defaults | O Sapiens reúne o fluxo em 8 áreas CMD-first, preserva Enter/defaults, confirma operações, salva na fonte TOML compartilhada e não abre navegador automaticamente |

## Decisões de segurança e estado

- A chave nunca é perguntada nem gravada: o wizard coleta somente o nome da
  variável de ambiente ou outra referência suportada.
- A configuração é validada antes de salvar, escrita em arquivo temporário
  sincronizado, substituída atomicamente e mantém `config.toml.bak`.
- Importação e restauração recusam arquivo ausente ou configuração inválida.
- Cada capacidade é apresentada como pronta, opcional/desabilitada ou
  dependente de credencial/runner; o catálogo não declara suporte que não foi
  testado.
- O repositório público exato `MyClaw` informado pelo usuário não estava
  acessível/encontrado nesta validação; portanto ele não é tratado como fonte
  integrada. A referência local foi usada apenas para modelar a matriz acima.

## Referências externas

- ZeroClaw: <https://github.com/zeroclaw-labs/zeroclaw/wiki/04-Configuration>
- ZeroClaw CLI: <https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/reference/cli/commands-reference.md>
