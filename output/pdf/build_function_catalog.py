from html import escape
from pathlib import Path

from reportlab.lib import colors
from reportlab.lib.enums import TA_CENTER, TA_LEFT
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle, getSampleStyleSheet
from reportlab.lib.units import mm
from reportlab.platypus import (
    PageBreak,
    Paragraph,
    SimpleDocTemplate,
    Spacer,
    Table,
    TableStyle,
)


ROOT = Path(__file__).resolve().parents[2]
OUTPUT = Path(__file__).resolve().parent / "sapiens-agent-catalogo-de-funcoes.pdf"

NAVY = colors.HexColor("#0B132B")
BLUE = colors.HexColor("#2563EB")
CYAN = colors.HexColor("#0EA5E9")
INK = colors.HexColor("#172033")
MUTED = colors.HexColor("#52627A")
LINE = colors.HexColor("#D7E0EE")
PALE = colors.HexColor("#F4F7FC")
GREEN = colors.HexColor("#0F766E")
AMBER = colors.HexColor("#A16207")


def p(text, style):
    return Paragraph(escape(text).replace("\n", "<br/>"), style)


def rich(text, style):
    return Paragraph(text, style)


def function_rows(items):
    rows = []
    for command, purpose, risk, state in items:
        risk_color = {
            "read": GREEN,
            "external_write": BLUE,
            "destructive": colors.HexColor("#B91C1C"),
            "secret_input": colors.HexColor("#9D174D"),
        }.get(risk, INK)
        state_color = GREEN if state == "pronto" else AMBER
        rows.append(
            [
                p(command, styles["Command"]),
                p(purpose, styles["Cell"]),
                rich(f'<font color="{risk_color.hexval()}">{escape(risk)}</font>', styles["Cell"]),
                rich(f'<font color="{state_color.hexval()}">{escape(state)}</font>', styles["Cell"]),
            ]
        )
    return rows


def function_table(items):
    data = [
        [
            p("Comando", styles["TableHead"]),
            p("O que faz", styles["TableHead"]),
            p("Risco", styles["TableHead"]),
            p("Estado", styles["TableHead"]),
        ]
    ] + function_rows(items)
    table = Table(data, colWidths=[48 * mm, 82 * mm, 27 * mm, 27 * mm], repeatRows=1)
    table.setStyle(
        TableStyle(
            [
                ("BACKGROUND", (0, 0), (-1, 0), NAVY),
                ("TEXTCOLOR", (0, 0), (-1, 0), colors.white),
                ("GRID", (0, 0), (-1, -1), 0.35, LINE),
                ("VALIGN", (0, 0), (-1, -1), "TOP"),
                ("LEFTPADDING", (0, 0), (-1, -1), 7),
                ("RIGHTPADDING", (0, 0), (-1, -1), 7),
                ("TOPPADDING", (0, 0), (-1, -1), 6),
                ("BOTTOMPADDING", (0, 0), (-1, -1), 6),
                ("ROWBACKGROUNDS", (0, 1), (-1, -1), [colors.white, PALE]),
            ]
        )
    )
    return table


def section(title, intro, items):
    return [
        rich(f'<font color="{BLUE.hexval()}">{escape(title)}</font>', styles["Section"]),
        p(intro, styles["Body"]),
        Spacer(1, 3 * mm),
        function_table(items),
        Spacer(1, 7 * mm),
    ]


styles = getSampleStyleSheet()
styles.add(
    ParagraphStyle(
        name="CoverTitle",
        parent=styles["Title"],
        fontName="Helvetica-Bold",
        fontSize=30,
        leading=34,
        textColor=colors.white,
        alignment=TA_CENTER,
        spaceAfter=8 * mm,
    )
)
styles.add(
    ParagraphStyle(
        name="CoverSub",
        parent=styles["Normal"],
        fontName="Helvetica",
        fontSize=12,
        leading=17,
        textColor=colors.HexColor("#D8E7FF"),
        alignment=TA_CENTER,
    )
)
styles.add(
    ParagraphStyle(
        name="CoverBody",
        parent=styles["BodyText"],
        fontName="Helvetica",
        fontSize=9.5,
        leading=14,
        textColor=colors.HexColor("#D8E7FF"),
        alignment=TA_LEFT,
        spaceAfter=2 * mm,
    )
)
styles.add(
    ParagraphStyle(
        name="CoverSmall",
        parent=styles["BodyText"],
        fontName="Helvetica",
        fontSize=8,
        leading=11,
        textColor=colors.HexColor("#AFC5E8"),
    )
)
styles.add(
    ParagraphStyle(
        name="Kicker",
        parent=styles["Normal"],
        fontName="Helvetica-Bold",
        fontSize=9,
        leading=12,
        textColor=CYAN,
        alignment=TA_CENTER,
        tracking=1,
    )
)
styles.add(
    ParagraphStyle(
        name="Section",
        parent=styles["Heading1"],
        fontName="Helvetica-Bold",
        fontSize=18,
        leading=22,
        textColor=BLUE,
        spaceBefore=2 * mm,
        spaceAfter=3 * mm,
    )
)
styles.add(
    ParagraphStyle(
        name="Body",
        parent=styles["BodyText"],
        fontName="Helvetica",
        fontSize=9.5,
        leading=14,
        textColor=INK,
        alignment=TA_LEFT,
        spaceAfter=2 * mm,
    )
)
styles.add(
    ParagraphStyle(
        name="Small",
        parent=styles["BodyText"],
        fontName="Helvetica",
        fontSize=8,
        leading=11,
        textColor=MUTED,
    )
)
styles.add(
    ParagraphStyle(
        name="TableHead",
        parent=styles["BodyText"],
        fontName="Helvetica-Bold",
        fontSize=8,
        leading=10,
        textColor=colors.white,
    )
)
styles.add(
    ParagraphStyle(
        name="Cell",
        parent=styles["BodyText"],
        fontName="Helvetica",
        fontSize=7.8,
        leading=10,
        textColor=INK,
    )
)
styles.add(
    ParagraphStyle(
        name="Command",
        parent=styles["BodyText"],
        fontName="Courier",
        fontSize=7.4,
        leading=9.5,
        textColor=NAVY,
    )
)
styles.add(
    ParagraphStyle(
        name="Callout",
        parent=styles["BodyText"],
        fontName="Helvetica-Bold",
        fontSize=10,
        leading=14,
        textColor=NAVY,
        backColor=colors.HexColor("#E8F1FF"),
        borderColor=colors.HexColor("#B8D1FF"),
        borderWidth=0.7,
        borderPadding=10,
        spaceBefore=3 * mm,
        spaceAfter=5 * mm,
    )
)


def header_footer(canvas, doc):
    canvas.saveState()
    width, height = A4
    is_cover = doc.page == 1
    if is_cover:
        canvas.setFillColor(NAVY)
        canvas.rect(0, 0, width, height, stroke=0, fill=1)
    canvas.setStrokeColor(colors.HexColor("#334A77") if is_cover else LINE)
    canvas.setLineWidth(0.5)
    canvas.line(16 * mm, 14 * mm, width - 16 * mm, 14 * mm)
    canvas.setFont("Helvetica", 7.5)
    canvas.setFillColor(colors.HexColor("#D8E7FF") if is_cover else MUTED)
    canvas.drawString(16 * mm, 9 * mm, "Sapiens Agent - catalogo de funcoes")
    canvas.drawRightString(width - 16 * mm, 9 * mm, f"Pagina {doc.page}")
    canvas.restoreState()


catalog = [
    (
        "Inicio, configuracao e operacao",
        "Comandos para instalar, iniciar, configurar e diagnosticar o runtime. O fluxo principal continua no CMD e os wrappers escondem o binario interno.",
        [
            ("sapiens-agent init", "Inicializa a configuracao local e o workspace.", "external_write", "pronto"),
            ("sapiens-agent setup", "Executa o assistente de configuracao no terminal.", "external_write", "pronto"),
            ("sapiens-agent configure", "Reabre a configuracao guiada.", "external_write", "pronto"),
            ("sapiens-agent start", "Inicia o gateway local com banner SAPIENS AGENT.", "external_write", "pronto"),
            ("sapiens-agent restart", "Reinicia o gateway preservando estado.", "external_write", "pronto"),
            ("sapiens-agent stop", "Solicita parada do processo local.", "external_write", "pronto"),
            ("sapiens-agent status --json", "Mostra estado, features, providers, canais e limites.", "read", "pronto"),
            ("sapiens-agent doctor", "Verifica configuracao e dependencias sem expor credenciais.", "read", "pronto"),
            ("sapiens-agent version", "Mostra a versao do runtime.", "read", "pronto"),
            ("sapiens-agent tools list", "Lista ferramentas tipadas por feature e risco.", "read", "pronto"),
            ("sapiens-agent tools discover NOME", "Descobre uma ferramenta sem executa-la; aplica a policy.", "read", "pronto"),
        ],
    ),
    (
        "Providers, chat e roteamento",
        "O runtime usa providers remotos ou locais configurados por alias. Credenciais ficam somente em variaveis de ambiente.",
        [
            ("provider list", "Lista providers configurados de forma redigida.", "read", "pronto"),
            ("provider catalog", "Lista familias de providers e protocolos suportados.", "read", "pronto"),
            ("provider add", "Adiciona provider, alias, protocolo, modelo e limites.", "external_write", "pronto"),
            ("provider configure ALIAS", "Completa credencial e parametros pelo terminal.", "external_write", "pronto"),
            ("provider test ALIAS", "Executa health check e informa latencia/status.", "read", "pronto"),
            ("provider models ALIAS", "Consulta modelos do provider configurado.", "read", "pronto"),
            ("provider use ALIAS", "Seleciona o provider ativo.", "external_write", "pronto"),
            ("route set TAREFA ALIAS", "Roteia uma tarefa para um alias.", "external_write", "pronto"),
            ("route fallback ALIASES", "Define a cadeia de fallback.", "external_write", "pronto"),
            ("chat [PROMPT]", "Envia conversa one-shot ou interativa pelo CMD.", "external_write", "pronto"),
            ("chat --image ARQUIVO", "Envia imagem local a provider multimodal compativel.", "external_write", "pronto"),
        ],
    ),
    (
        "Gateway, WebUI e sessoes",
        "O gateway e local por padrao, com REST, WebSocket, webhooks, rate limit, autenticacao opcional e sessoes persistentes.",
        [
            ("POST /health", "Confirma que o processo esta vivo.", "read", "pronto"),
            ("POST /v1/chat", "Processa mensagem, session, imagens e routing.", "external_write", "pronto"),
            ("GET /v1/ws", "Oferece conversa por WebSocket.", "external_write", "pronto"),
            ("GET/POST /v1/webhooks/NOME", "Recebe verificacoes e eventos de canais.", "external_write", "pronto"),
            ("POST /v1/sessions/ID/cancel", "Cancela uma sessao especifica.", "destructive", "pronto"),
            ("POST /v1/emergency-stop", "Ativa parada de emergencia fail-closed.", "destructive", "pronto"),
            ("session list", "Lista sessoes sem abrir navegador.", "read", "pronto"),
            ("session cleanup", "Remove sessoes antigas mediante aprovacao.", "destructive", "pronto"),
            ("session rotate ID", "Gira uma sessao e persiste o novo identificador.", "destructive", "pronto"),
        ],
    ),
    (
        "Canais e mensageria",
        "Adapters reais sao habilitados individualmente e usam allowlist, credenciais por ambiente, policy de rede e receipts redigidos.",
        [
            ("channel list", "Exibe catalogo de canais e estado do adapter.", "read", "pronto"),
            ("channel add TIPO", "Cria uma configuracao de canal.", "external_write", "pronto"),
            ("channel configure NOME", "Configura credencial e allowlist no terminal.", "secret_input", "pronto"),
            ("channel test NOME", "Executa health check do canal.", "read", "pronto"),
            ("channel send NOME DEST MSG", "Envia texto com aprovacao explicita.", "external_write", "pronto"),
            ("channel send-media", "Envia imagem, audio, video ou documento suportado.", "external_write", "pronto"),
            ("channel start", "Inicia workers de canais habilitados.", "external_write", "pronto"),
            ("Telegram", "Texto por Bot API, allowlist e health check.", "external_write", "pronto"),
            ("Discord/Slack/Google Chat/Teams", "Texto por webhooks com payload especifico.", "external_write", "pronto"),
            ("Matrix", "Texto, /sync incremental, cursor e midia.", "external_write", "pronto"),
            ("WhatsApp Cloud API", "Texto, midia, HMAC, status e deduplicacao.", "external_write", "pronto"),
            ("Signal", "Adapter signal-cli para texto, grupos e midia.", "external_write", "opcional"),
        ],
    ),
]


story = [
    rich("SAPIENS AGENT", styles["Kicker"]),
    Spacer(1, 12 * mm),
    rich("Catalogo de<br/>funcoes", styles["CoverTitle"]),
    rich("Referencia pratica do CLI, gateway, canais, automacao e seguranca", styles["CoverSub"]),
    Spacer(1, 20 * mm),
    rich("Experiencia CMD-first - local-first - supervised por padrao", styles["Callout"]),
    Spacer(1, 12 * mm),
    p(
        "Este documento registra as funcoes implementadas no projeto e as condicoes de cada capacidade. "
        "Comandos de escrita, envio, publicacao, exclusao, credenciais e computer use exigem aprovacao conforme a policy. "
        "Itens opcionais permanecem desabilitados ate que suas dependencias e testes estejam disponiveis.",
        styles["CoverBody"],
    ),
    Spacer(1, 8 * mm),
    p("Versao do runtime: 0.1.0 | Validacao: 119 testes, cargo fmt, cargo clippy e release Windows", styles["CoverSmall"]),
    p("Gateway padrao: 127.0.0.1:8787 | Navegador nao abre automaticamente", styles["CoverSmall"]),
    PageBreak(),
    rich("Como ler este catalogo", styles["Section"]),
    p(
        "O primeiro bloco de cada linha e o comando ou superficie. O risco indica o nivel usado pela Policy: "
        "read nao modifica o mundo externo; external_write pode enviar ou alterar dados; destructive remove ou cancela; "
        "secret_input aceita credenciais sensiveis e nunca as grava em logs ou receipts.",
        styles["Body"],
    ),
    rich("Atalhos de inicio", styles["Callout"]),
    p("1. sapiens-agent setup", styles["Command"]),
    p("0. sapiens-agent  (menu interativo)", styles["Command"]),
    p("2. sapiens-agent provider add custom --alias principal --base-url URL --model MODELO", styles["Command"]),
    p("3. sapiens-agent provider use principal", styles["Command"]),
    p("4. sapiens-agent start", styles["Command"]),
    p("5. sapiens-agent chat", styles["Command"]),
    p("6. sapiens-agent resources status", styles["Command"]),
    Spacer(1, 6 * mm),
    p(
        "Os wrappers sapiens-agent.cmd e sapiens.cmd permitem usar os aliases no CMD sem que o usuario precise chamar "
        "target/release/sapiens-agent.exe. Para abrir a WebUI, use explicitamente --open-browser.",
        styles["Body"],
    ),
    Spacer(1, 4 * mm),
]

for title, intro, items in catalog:
    story.extend(section(title, intro, items))

story.append(PageBreak())
story.extend(
    section(
        "Scheduler, memoria e identidade",
        "Automacoes sao persistidas localmente e executadas com limites de concorrencia, tokens, custo, tempo, retry e retomada.",
        [
            ("schedule list", "Lista tarefas agendadas e estados persistidos.", "read", "pronto"),
            ("schedule add --every", "Agenda execucao recorrente.", "external_write", "pronto"),
            ("schedule once --at", "Agenda execucao unica.", "external_write", "pronto"),
            ("schedule cron --expression", "Agenda cron UTC de cinco campos.", "external_write", "pronto"),
            ("schedule pause ID", "Pausa uma tarefa.", "external_write", "pronto"),
            ("schedule resume ID", "Retoma uma tarefa interrompida.", "external_write", "pronto"),
            ("schedule cancel ID", "Cancela uma tarefa.", "destructive", "pronto"),
            ("schedule remove ID", "Remove a configuracao de uma tarefa.", "destructive", "pronto"),
            ("memory list/search", "Consulta memoria JSONL quando habilitada.", "read", "opcional"),
            ("memory export PATH", "Exporta memoria para arquivo no workspace.", "read", "opcional"),
            ("memory delete/clear", "Apaga memoria por sessao ou toda a memoria.", "destructive", "opcional"),
            ("identity init/show/path", "Cria e consulta IDENTITY.md e PREFERENCES.md.", "read", "pronto"),
        ],
    )
)
story.extend(
    section(
        "Browser isolado e computer use",
        "Browser e desktop sao superficies poderosas. Por isso seguem isolamento, allowlist, SSRF policy, screenshots, receipts e aprovacao.",
        [
            ("browser open/goto", "Abre ou navega uma sessao Playwright isolada.", "external_write", "opcional"),
            ("browser snapshot", "Le a arvore de acessibilidade atual.", "read", "opcional"),
            ("browser click/fill/press", "Interage com elementos por alvo sem JS arbitrario.", "external_write", "opcional"),
            ("browser tabs", "Cria, seleciona, fecha e lista abas.", "external_write", "opcional"),
            ("browser state-save/load", "Salva ou restaura estado sob o workspace.", "external_write", "opcional"),
            ("browser upload/download", "Transfere arquivos sob policy de caminho.", "external_write", "opcional"),
            ("computer screenshot", "Captura PNG da area de trabalho.", "read", "pronto"),
            ("computer plan", "Gera e valida plano JSON sem executar.", "read", "pronto"),
            ("computer auto/run", "Executa sequencia apos aprovacao por passo.", "external_write", "pronto"),
            ("computer emergency-stop", "Interrompe sequencia cooperativamente.", "destructive", "pronto"),
        ],
    )
)
story.extend(
    section(
        "Shell, MCP, plugins e aprovacoes",
        "Ferramentas externas nao sao executadas por descoberta casual. Cada superficie valida allowlist, risco, limites e estado.",
        [
            ("skills list/enable", "Lista ou habilita features opcionais.", "external_write", "pronto"),
            ("shell --command", "Executa comando allowlisted no workspace.", "external_write", "opcional"),
            ("mcp servers", "Lista servidores MCP sem expor segredos.", "read", "opcional"),
            ("mcp add/remove", "Registra ou remove servidor MCP.", "destructive", "opcional"),
            ("mcp health/diagnose", "Verifica ciclo de vida, transporte e ferramentas.", "read", "opcional"),
            ("mcp list/call", "Descobre ou chama ferramenta allowlisted.", "external_write", "opcional"),
            ("plugin verify", "Valida manifesto, hash, origem e assinatura.", "read", "opcional"),
            ("plugin install/update", "Instala de modo transacional, marcado DISABLED.", "external_write", "opcional"),
            ("plugin remove", "Remove plugin com aprovacao.", "destructive", "opcional"),
            ("approval list/grant/revoke", "Administra aprovacoes por escopo, risco e expiracao.", "external_write", "pronto"),
            ("logs/receipts", "Consulta logs e receipts redigidos.", "read", "pronto"),
        ],
    )
)
story.extend(
    section(
        "Skills, recursos e audio opcional",
        "Skills sao arquivos reais validaveis; recursos sao governados por perfil e audio so e processado quando o canal e o provider suportam a capacidade.",
        [
            ("skills list/validate", "Descobre skills reais e valida SKILL.md.", "read", "pronto"),
            ("skills suggest", "Identifica acoes repetidas nos receipts.", "read", "pronto"),
            ("skills create NOME", "Cria candidata versionavel pelo skill-forge.", "external_write", "pronto"),
            ("skills enable/disable", "Controla escopo e habilitacao de uma skill.", "external_write", "pronto"),
            ("skills rollback NOME", "Reverte skill gerada com aprovacao.", "destructive", "pronto"),
            ("resources status", "Mostra perfil e limites de GPU, CPU, memoria e concorrencia.", "read", "pronto"),
            ("resources profile NOME", "Aplica economy, balanced, performance ou custom.", "external_write", "pronto"),
            ("channel audio inbound", "Identifica e preserva audio recebido sob limites.", "read", "opcional"),
            ("provider audio_input/output", "Usa audio somente com capability declarada e adapter validado.", "external_write", "opcional"),
        ],
    )
)
story.extend(
    [
        PageBreak(),
        rich("Matriz de capacidades e limites", styles["Section"]),
        p(
            "Pronto: providers chat completions, Responses, Anthropic, Gemini e Ollama; canais Telegram, Discord, Slack, "
            "Google Chat, Teams, Matrix e WhatsApp; scheduler, sessoes, policy, computer use Windows e CLI CMD.",
            styles["Body"],
        ),
        p(
            "Opcional/desabilitado por condicao: Signal depende de signal-cli e conta local; audio/video/transcricao dependem "
            "de adapters e capabilities do provider; embeddings nao possuem adapter habilitado; Browser Use/Stagehand e plugins "
            "aguardam sandbox; MCP, shell, memoria e browser sao opt-in; Linux/macOS aguardam runner nativo.",
            styles["Body"],
        ),
        rich("Evidencia de release", styles["Section"]),
        p(
            "O release Windows validado mede 5.034.496 bytes e possui SHA-256 "
            "82ABB1D2EAE5E6F43CE9CF611BE80BBF9A24B9306A16C45640DF8671B9C1D29C. "
            "A documentacao detalhada esta em docs/ARCHITECTURE.md e docs/CAPABILITIES.md.",
            styles["Body"],
        ),
    ]
)

doc = SimpleDocTemplate(
    str(OUTPUT),
    pagesize=A4,
    rightMargin=16 * mm,
    leftMargin=16 * mm,
    topMargin=16 * mm,
    bottomMargin=19 * mm,
    title="Sapiens Agent - Catalogo de funcoes",
    author="Sapiens Agent",
)
doc.build(story, onFirstPage=header_footer, onLaterPages=header_footer)
print(OUTPUT)
