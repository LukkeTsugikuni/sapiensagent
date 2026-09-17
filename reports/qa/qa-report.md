# Relatorio de QA - Sapiens Agent

Data: 16/09/2026  
Ambiente: Windows PowerShell 7, configuracoes temporarias locais  
Modo: quinta rodada de regressao, com TUI conversacional e chat WebUI

## Resultado executivo

O nucleo Rust e a CLI passaram novamente na bateria automatizada: 122 testes aprovados, nenhum falhou. Tambem passaram novamente `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings` e o build release Windows. A Control UI foi revalidada com navegacao por todas as areas, viewport responsivo e chamadas reais ao gateway.

A inicializacao no terminal foi validada com o comando sem subcomando: o banner SAPIENS AGENT abre uma conversa real, sem navegador. O TUI foi exercitado com `/help`, `/status`, `/exit`, sessão, histórico local, contexto entre mensagens e interrupção cooperativa. O menu operacional legado permanece disponível em `sapiens menu`.

## Cobertura executada

| Area | Resultado | Evidencia |
|---|---|---|
| Unitarios de core, policy, providers, canais, scheduler, memoria, MCP, plugins e tools | PASSOU | 111 testes |
| CLI principal e computer use | PASSOU | 4 testes |
| Gateway, reinicio, imagem, browser local e computer smoke | PASSOU | 6 testes de acceptance |
| Scheduler + canais Matrix/WhatsApp | PASSOU | 1 teste de integracao |
| Formatacao e lint | PASSOU | fmt + clippy sem warnings |
| PowerShell/CMD, banner e cancelamento | PASSOU | TTY temporario, saida 0 |
| Menu principal 1 a 8 | PASSOU | todos os submenus abriram e retornaram com 0 |
| Entrada invalida | PASSOU | 9 exibiu erro e permitiu continuar |
| Provider configuracao/lista/uso/rota | PASSOU | provider local temporario |
| Falha de provider | PASSOU | erro de conexao retornou exit 1 sem segredo |
| Scheduler dry-run | PASSOU | tarefa nao foi salva |
| Catalogo de canais | PASSOU | canais prontos e opcionais listados |
| Gateway /health, /v1/status, /v1/config, /v1/skills | PASSOU | HTTP 200 em todos |
| WebUI Configuracao | PASSOU | selects, checkbox, salvar e feedback |
| WebUI Chat vazio | PASSOU | exibiu `Digite uma mensagem.` |
| WebUI Canais e midia | PASSOU | conteudo exibido no release reconstruido |
| WebUI Skills | PASSOU | skills carregadas e estados exibidos |
| WebUI responsiva | PASSOU | viewport 390x844, canais e skills exibidos |
| WebUI Control UI | PASSOU | overview, providers, canais, skills, memoria, automacoes, ferramentas, seguranca, diagnostico e configuracao |
| TUI conversacional PowerShell/CMD | PASSOU | banner, transcript, status, slash commands, sessão e resposta real |
| Chat WebUI conversacional | PASSOU | bolhas de usuário/agente, histórico, nova conversa, limpeza e contexto |
| WebUI teste real de provider | PASSOU | qwen3:1.7b respondeu pelo Ollama e pelo gateway |
| skills create --dry-run | PASSOU | preview nao criou diretorio nem arquivos |

## Diagnostico do provider local

O modelo `qwen3:1.7b` esta instalado e aparece em `/api/tags`. A instalacao 0.34.0 estava incompleta e nao continha `llama-server.exe`; ela foi reinstalada para 0.34.1 preservando `C:\Users\Edson\.ollama`. A GPU GeForce GT 730M tambem apresentou terminacao nativa ao carregar via Vulkan, entao o Ollama foi iniciado com `OLLAMA_LLM_LIBRARY=cpu` e essa variavel foi salva no ambiente do usuario. O teste real do provider respondeu HTTP 200 com `PROVIDER_OK`, o Chat do Sapiens respondeu HTTP 200 com `SAPIENS_OK`, o TUI respondeu `CLI_OK` e o Chat WebUI respondeu `WEB_OK`.

## Falha reproduzivel

### WEBUI-001 - colisao de IDs nas abas

Severidade: alta  
Status: CORRIGIDO e validado na quarta rodada  

Passos:

1. Iniciar o gateway local.
2. Abrir a WebUI.
3. Clicar em `Canais e midia`.
4. Clicar em `Skills`.

Resultado observado antes da correcao: o botao selecionado mudava de estado e o titulo da pagina mudava, mas o conteudo da aba nao aparecia.

Causa provavel: o painel inicial usa `id="channels"` e `id="skills"` para os indicadores numericos, enquanto as secoes de conteudo usam os mesmos IDs. O JavaScript seleciona o primeiro elemento retornado por `getElementById`, remove `hide` do indicador e deixa a secao real escondida.

Evidencia no codigo: `src/gateway.rs:745` contem os indicadores; `src/gateway.rs:747-748` contem as secoes duplicadas. O comportamento foi confirmado por snapshot e screenshot do Playwright.

Correcao aplicada: os indicadores foram renomeados para `channelCount` e `skillCount`; os paineis passaram a usar `channelsPanel` e `skillsPanel`; o `data-tab` e o `refresh()` foram atualizados. As duas abas foram revalidadas no release reconstruido em viewport desktop e em 390x844.

### SKILLS-001 - dry-run criava skill no disco

Severidade: media  
Status: CORRIGIDO e coberto por teste automatizado

O comando de simulacao `skills create ... --dry-run` criava a pasta e os arquivos da skill. Foi criada uma etapa de preview sem escrita, o caminho real passou a gravar somente fora de dry-run e o teste confirma que nenhum arquivo e criado.

## Limites da validacao

Nenhuma credencial real foi usada. Nenhuma mensagem foi enviada para Telegram, Discord, Slack, WhatsApp ou outro canal. As respostas de provider foram cobertas por listeners locais, fixtures e testes existentes. Audio real, plugins externos e providers externos nao foram exercitados contra servicos de terceiros por seguranca.

## Conclusao

A CLI, o nucleo local, o TUI e a Control UI estao saudaveis conforme a quinta rodada da bateria executada. A interface foi validada nas 11 areas do painel, com conversa real em desktop e viewport de 390x844; nao houve overflow horizontal. O contexto enviado pelo navegador foi confirmado pelo modelo, que recuperou a informacao `azul` de uma mensagem anterior. WEBUI-001 e o ajuste responsivo foram corrigidos e revalidados. A geracao real do Ollama local foi reparada e confirmada; as integracoes externas continuam condicionadas a credenciais e adapters reais.

## Referencias de benchmark

1. ZeroClaw, [Onboarding Wizard](https://github.com/zeroclaw-labs/zeroclaw/wiki/02.2-Onboarding-Wizard) - referencia para modos interativo/rapido, defaults, canais e retomada.
2. OpenClaw, [Onboarding CLI](https://docs.openclaw.ai/start/wizard) - referencia para verificacao do provider escolhido, preservacao da configuracao existente e separacao entre setup rapido e avancado.
