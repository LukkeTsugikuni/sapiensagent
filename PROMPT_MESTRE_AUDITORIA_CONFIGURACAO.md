# Prompt mestre — auditoria e evolução da configuração do Sapiens Agent

## Instrução de execução

Atue como uma equipe sênior composta por arquiteto de runtime, engenheiro Rust/PowerShell, especialista em CLI/TUI, integração de providers e canais, segurança, skills, QA e release. Trabalhe no repositório atual do Sapiens Agent e execute as etapas abaixo em ordem. Não encerre por aparência: só conclua depois de implementar, testar e documentar cada item ou classificá-lo honestamente como pronto, opcional, bloqueado ou não validado.

## Objetivo

Transformar o comando global `sapiens` em uma experiência CMD-first, simples e completa, inspirada no fluxo `zeroclaw onboard --interactive`, sem copiar código, sem abrir navegador automaticamente e sem esconder falhas. O menu deve funcionar no PowerShell normal, a partir de qualquer pasta, e deve compartilhar uma única configuração persistente com a WebUI opcional.

## Referências que devem ser analisadas como somente leitura

1. MyClaw local do usuário: `C:\Users\Edson\.my_claw`. Inspecione apenas nomes de seções, chaves, fluxos e contratos; nunca exponha, copie ou registre valores de API keys, tokens, segredos ou conteúdo pessoal.
2. ZeroClaw oficial: `https://github.com/zeroclaw-labs/zeroclaw/wiki/04-Configuration` e `https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/reference/cli/commands-reference.md`.
3. Código, testes, documentação e PDF do próprio Sapiens Agent.

Registre em uma matriz o que foi observado, o que foi inferido e o que será adotado. Não declare que MyClaw foi integrado; ele é apenas referência até existir teste local equivalente.

## Menu obrigatório

Ao executar `sapiens` sem argumentos, mostrar banner SAPIENS AGENT e exatamente um menu navegável, com confirmação visual após cada operação:

1. **Configurar o agente** — wizard completo, sempre exibindo perguntas no modo interativo.
2. **Provider e API** — adicionar/editar/remover provider, URL/base path, protocolo, modelo, variável de credencial, temperatura, tokens, timeout, retries, fallback, limites de contexto/custo e teste de conexão. Nunca pedir ou imprimir a chave em texto aberto.
3. **Canais** — listar, adicionar, editar, testar, habilitar/desabilitar e iniciar canais; incluir Telegram, Discord, Slack, Google Chat, Microsoft Teams, Matrix, WhatsApp, Signal, webhooks e CLI conforme adapters realmente existentes. Mostrar dependências, allowlist, status e capability de texto/imagem/áudio/documento.
4. **Gateway e interface** — bind, porta, autenticação, rate limit, sessões, WebUI opcional, abrir navegador somente por solicitação explícita e opção de configurar novamente sem reiniciar quando for seguro.
5. **Segurança e recursos** — modo readonly/supervised/trusted, workspace permitido, domínios, rede privada, shell allowlist, aprovações, emergency stop, perfil economy/balanced/performance/custom e limites de CPU/GPU/memória/concurrency.
6. **Memória, identidade e workspace** — backend, retenção, exportação/limpeza, `IDENTITY.md`, `PREFERENCES.md`, `USER.md`, `SOUL.md`, `AGENTS.md`, `HEARTBEAT.md`, diretório do workspace e migração segura.
7. **Skills, ferramentas e automações** — catálogo e validação de skills, enable/disable, criação controlada pelo `skill-forge`, browser, computer use, shell, MCP, plugins, scheduler/cron/heartbeat, logs e receipts. Toda ação externa deve exibir risco e exigir aprovação quando aplicável.
8. **Iniciar, status, reconfigurar, ajuda e sair** — iniciar/parar/reiniciar gateway, status/doctor, reabrir qualquer seção, exportar/importar configuração, restaurar backup, ajuda contextual e saída limpa. Nunca deixar uma opção sem ação ou aparentando travamento.

## Requisitos de UX

- `sapiens` deve ser o comando curto global; `sapiens start`, `sapiens setup`, `sapiens configure`, `sapiens status` e `sapiens doctor` devem continuar disponíveis.
- As opções devem aparecer também quando a configuração já existe; valores atuais devem ser mostrados como padrão editável.
- Enter conserva o valor padrão; `q`, `Esc` ou `0` cancela com segurança; erros retornam ao menu sem perder configuração.
- Cada etapa informa `pronto`, `opcional`, `desabilitado`, `bloqueado` ou `não validado`, com a causa e o próximo comando.
- A experiência deve permanecer legível em CMD/PowerShell comum, sem TUI pesada obrigatória, sem dependência de navegador e sem pausa silenciosa.

## Configuração canônica e segurança

- Manter uma única fonte de verdade compartilhada por CLI, WebUI e gateway.
- Validar tudo antes de salvar; salvar atomicamente, criar backup e permitir diff/restore.
- Preservar campos desconhecidos quando seguro e criar migrações explícitas.
- Credenciais devem ser referências a variáveis de ambiente, store ou arquivo protegido; nunca gravar valores em logs, receipts, PDF, HTML ou mensagens de erro.
- Classificar operações como `read`, `external_write`, `destructive` ou `secret_input`; aplicar a policy centralizada e fail-closed.
- Áudio deve permanecer opt-in, limitado por tamanho/duração e processado somente quando canal, adapter e provider declararem capability. Transcrição, TTS e resposta de voz não podem ser simulados.

## Processo obrigatório

1. Mapear o estado atual do CLI, config, gateway, adapters, skills, scripts, testes e release.
2. Comparar com a matriz MyClaw/ZeroClaw e criar uma lacuna rastreável para cada opção do menu.
3. Implementar primeiro o fluxo vertical: `sapiens` → [1] → perguntas → validação → salvar → [4]/[8].
4. Implementar as demais seções com handlers reais; não criar botões ou perguntas sem persistência e diagnóstico correspondente.
5. Criar testes de unidade, integração e CLI para: primeira configuração, reconfiguração, cancelamento, defaults, configuração inválida, segredo redigido, provider ausente, canal desabilitado, PATH global, execução fora da pasta, processo/porta ocupados, áudio desligado e WebUI opcional.
6. Atualizar README, ARCHITECTURE, CAPABILITIES, catálogo de funções e release notes.
7. Rodar `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all --no-fail-fast`, `cargo build --release`, smoke do launcher global em diretório não relacionado e smoke Playwright da WebUI sem abrir navegador automaticamente.
8. Inspecionar visualmente o PDF regenerado e revisar o diff antes do commit. Nunca force-push.

## Critério de conclusão

Só declarar concluído quando as opções 1–8 forem selecionáveis e tiverem comportamento verificável, a configuração for persistida e redigida corretamente, o comando global funcionar em um PowerShell novo de qualquer diretório, os testes passarem e a documentação disser claramente o que é pronto, opcional, bloqueado ou não validado.
