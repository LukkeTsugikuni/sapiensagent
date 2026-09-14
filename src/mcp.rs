use anyhow::{Context, Result, bail};
use reqwest::blocking::Client as BlockingClient;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdout, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};
use url::Url;

const PREFERRED_PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &[PREFERRED_PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub allowed: bool,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default = "default_transport")]
    pub transport: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub credential_env: String,
}

impl Default for McpServer {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            allowed: false,
            allowlist: vec![],
            transport: default_transport(),
            url: String::new(),
            credential_env: String::new(),
        }
    }
}

fn default_transport() -> String {
    "stdio".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub server: String,
    pub name: String,
    pub allowed: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing)]
    pub server_command: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpDiagnostics {
    pub server: String,
    pub transport: String,
    pub session_mode: String,
    pub lifecycle: Vec<String>,
    pub protocol_versions_supported: Vec<String>,
    pub endpoint_configured: bool,
    pub credential_env_configured: bool,
    pub healthy: bool,
    pub latency_ms: u128,
    pub tools: usize,
    pub allowed_tools: usize,
}

pub trait McpClient: Send + Sync {
    fn list_tools(&self, server: &McpServer) -> Result<Vec<McpTool>>;
    fn call(&self, tool: &McpTool, arguments: Value) -> Result<Value>;
}

pub struct StdioMcpClient {
    timeout: Duration,
}

impl StdioMcpClient {
    pub fn new(timeout: Duration) -> Result<Self> {
        if timeout.is_zero() || timeout > Duration::from_secs(300) {
            bail!("MCP timeout must be between 1 and 300 seconds");
        }
        Ok(Self { timeout })
    }

    fn session(&self, server: &McpServer) -> Result<McpSession> {
        if !server.transport.is_empty() && server.transport != "stdio" {
            bail!("MCP server transport is not stdio");
        }
        if !server.allowed {
            bail!("MCP server not allowlisted");
        }
        let parts = split_command_line(&server.command)?;
        let program = parts.first().context("MCP command cannot be empty")?;
        let mut child = Command::new(program)
            .args(&parts[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start MCP server {}", server.name))?;
        let stdout = child
            .stdout
            .take()
            .context("MCP server stdout unavailable")?;
        let (receiver, reader) = response_reader(stdout);
        let stdin = child.stdin.take().context("MCP server stdin unavailable")?;
        Ok(McpSession {
            child,
            stdin,
            receiver,
            reader: Some(reader),
            timeout: self.timeout,
        })
    }

    fn initialize(&self, server: &McpServer) -> Result<McpSession> {
        let mut session = self.session(server)?;
        let response = session.request(
            1,
            "initialize",
            json!({
                "protocolVersion": PREFERRED_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "sapiens-agent",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;
        validate_initialize_response(&response)?;
        session.notify("notifications/initialized", json!({}))?;
        Ok(session)
    }
}

pub struct HttpMcpClient {
    client: BlockingClient,
    timeout: Duration,
}

impl HttpMcpClient {
    pub fn new(timeout: Duration) -> Result<Self> {
        if timeout.is_zero() || timeout > Duration::from_secs(300) {
            bail!("MCP timeout must be between 1 and 300 seconds");
        }
        Ok(Self {
            client: BlockingClient::builder().timeout(timeout).build()?,
            timeout,
        })
    }

    fn endpoint(&self, server: &McpServer) -> Result<Url> {
        if !server.allowed {
            bail!("MCP server not allowlisted");
        }
        let url = validate_remote_url(&server.url)?;
        Ok(url)
    }

    fn request(
        &self,
        server: &McpServer,
        url: &Url,
        message: &Value,
    ) -> Result<reqwest::blocking::Response> {
        let mut request = self
            .client
            .post(url.clone())
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .json(message);
        if !server.credential_env.trim().is_empty() {
            let credential = std::env::var(&server.credential_env).with_context(|| {
                format!(
                    "missing credential environment variable {}",
                    server.credential_env
                )
            })?;
            request = request.bearer_auth(credential);
        }
        Ok(request.send()?)
    }

    fn stream_url(
        &self,
        server: &McpServer,
        url: &Url,
    ) -> Result<(reqwest::blocking::Response, Url)> {
        let mut request = self
            .client
            .get(url.clone())
            .header("accept", "text/event-stream");
        if !server.credential_env.trim().is_empty() {
            let credential = std::env::var(&server.credential_env).with_context(|| {
                format!(
                    "missing credential environment variable {}",
                    server.credential_env
                )
            })?;
            request = request.bearer_auth(credential);
        }
        let mut response = request.send()?;
        if !response.status().is_success() {
            bail!("MCP SSE endpoint returned HTTP {}", response.status());
        }
        let event = read_sse_event(&mut response, self.timeout)?;
        if event.event != "endpoint" {
            bail!("MCP SSE stream did not start with endpoint event");
        }
        let endpoint = url
            .join(event.data.trim())
            .context("MCP SSE endpoint is invalid")?;
        if endpoint.scheme() != "http" && endpoint.scheme() != "https" {
            bail!("MCP SSE endpoint must use http or https");
        }
        Ok((response, endpoint))
    }

    fn initialize_message() -> Value {
        json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params": {
                "protocolVersion": PREFERRED_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name":"sapiens-agent", "version":env!("CARGO_PKG_VERSION")}
            }
        })
    }
}

pub fn validate_remote_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).context("MCP HTTP URL is invalid")?;
    if url.scheme() != "http" && url.scheme() != "https" {
        bail!("MCP HTTP URL must use http or https");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("MCP HTTP URL userinfo is blocked");
    }
    for (key, _) in url.query_pairs() {
        let key = key.to_ascii_lowercase();
        if ["key", "api_key", "apikey", "token", "secret", "password"]
            .iter()
            .any(|marker| key == *marker || key.ends_with(marker))
        {
            bail!("MCP URL must not contain credentials; use --credential-env");
        }
    }
    Ok(url)
}

impl McpClient for HttpMcpClient {
    fn list_tools(&self, server: &McpServer) -> Result<Vec<McpTool>> {
        let url = self.endpoint(server)?;
        if server.transport.eq_ignore_ascii_case("sse") {
            let (mut stream, endpoint) = self.stream_url(server, &url)?;
            let init = self.request(server, &endpoint, &Self::initialize_message())?;
            if !init.status().is_success() && init.status() != reqwest::StatusCode::ACCEPTED {
                bail!("MCP initialize request returned HTTP {}", init.status());
            }
            let init_response = read_sse_event(&mut stream, self.timeout)?;
            let init_value: Value = serde_json::from_str(&init_response.data)
                .context("invalid MCP SSE initialize response")?;
            validate_initialize_response(&init_value)?;
            let _ = self.request(
                server,
                &endpoint,
                &json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
            )?;
            let tools_response = self.request(
                server,
                &endpoint,
                &json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
            )?;
            if !tools_response.status().is_success()
                && tools_response.status() != reqwest::StatusCode::ACCEPTED
            {
                bail!("MCP tools/list returned HTTP {}", tools_response.status());
            }
            let event = read_sse_event(&mut stream, self.timeout)?;
            return parse_tools_response(&serde_json::from_str(&event.data)?, server);
        }
        let response = self.request(server, &url, &Self::initialize_message())?;
        let init_value = response_value(response)?;
        validate_initialize_response(&init_value)?;
        let _ = self.request(
            server,
            &url,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        )?;
        let response = self.request(
            server,
            &url,
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        )?;
        parse_tools_response(&response_value(response)?, server)
    }

    fn call(&self, tool: &McpTool, arguments: Value) -> Result<Value> {
        let server = McpServer {
            name: tool.server.clone(),
            command: tool.server_command.clone(),
            allowed: true,
            allowlist: vec![tool.name.clone()],
            transport: "http".into(),
            url: tool.server_command.clone(),
            credential_env: String::new(),
        };
        self.call_with_server(&server, tool, arguments)
    }
}

impl HttpMcpClient {
    pub fn call_with_server(
        &self,
        server: &McpServer,
        tool: &McpTool,
        arguments: Value,
    ) -> Result<Value> {
        ensure_allowed(server, tool)?;
        let url = self.endpoint(server)?;
        let response = self.request(server, &url, &Self::initialize_message())?;
        validate_initialize_response(&response_value(response)?)?;
        let _ = self.request(
            server,
            &url,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        )?;
        let response = self.request(server, &url, &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool.name,"arguments":arguments}}))?;
        let response = response_value(response)?;
        if let Some(error) = response.get("error") {
            bail!("MCP tool call failed: {}", redact_json(error));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

impl McpClient for StdioMcpClient {
    fn list_tools(&self, server: &McpServer) -> Result<Vec<McpTool>> {
        let mut session = self.initialize(server)?;
        let response = session.request(2, "tools/list", json!({}))?;
        let result = parse_tools_response(&response, server)?;
        session.finish();
        Ok(result)
    }

    fn call(&self, tool: &McpTool, arguments: Value) -> Result<Value> {
        let server = McpServer {
            name: tool.server.clone(),
            command: tool.server_command.clone(),
            allowed: true,
            allowlist: vec![tool.name.clone()],
            ..Default::default()
        };
        self.call_with_server(&server, tool, arguments)
    }
}

impl StdioMcpClient {
    pub fn call_with_server(
        &self,
        server: &McpServer,
        tool: &McpTool,
        arguments: Value,
    ) -> Result<Value> {
        ensure_allowed(server, tool)?;
        let mut session = self.initialize(server)?;
        let response = session.request(
            2,
            "tools/call",
            json!({
                "name": tool.name,
                "arguments": arguments,
            }),
        )?;
        session.finish();
        if let Some(error) = response.get("error") {
            bail!("MCP tool call failed: {}", redact_json(error));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

struct McpSession {
    child: Child,
    stdin: std::process::ChildStdin,
    receiver: Receiver<Value>,
    reader: Option<thread::JoinHandle<()>>,
    timeout: Duration,
}

impl McpSession {
    fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value> {
        write_json(
            &mut self.stdin,
            &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        )?;
        loop {
            let value = self
                .receiver
                .recv_timeout(self.timeout)
                .with_context(|| format!("MCP {method} response timed out"))?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                    bail!("MCP {method} response has invalid jsonrpc version");
                }
                if let Some(error) = value.get("error") {
                    bail!("MCP {method} failed: {}", redact_json(error));
                }
                return Ok(value);
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        write_json(
            &mut self.stdin,
            &json!({"jsonrpc":"2.0","method":method,"params":params}),
        )
    }

    fn finish(mut self) {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn validate_initialize_response(response: &Value) -> Result<()> {
    let version = response
        .pointer("/result/protocolVersion")
        .and_then(Value::as_str)
        .context("MCP initialize response missing result.protocolVersion")?;
    if !SUPPORTED_PROTOCOL_VERSIONS.contains(&version) {
        bail!(
            "unsupported MCP protocol version '{}'; supported versions are {}",
            version,
            SUPPORTED_PROTOCOL_VERSIONS.join(", ")
        );
    }
    let server_info = response
        .pointer("/result/serverInfo")
        .and_then(Value::as_object)
        .context("MCP initialize response missing result.serverInfo")?;
    if server_info
        .get("name")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
        || server_info
            .get("version")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        bail!("MCP initialize response has incomplete serverInfo");
    }
    Ok(())
}

struct SseEvent {
    event: String,
    data: String,
}

fn response_value(mut response: reqwest::blocking::Response) -> Result<Value> {
    if !response.status().is_success() && response.status() != reqwest::StatusCode::ACCEPTED {
        bail!("MCP HTTP request returned HTTP {}", response.status());
    }
    let is_sse = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    if is_sse {
        let event = read_sse_event(&mut response, Duration::from_secs(300))?;
        return serde_json::from_str(&event.data).context("invalid MCP SSE JSON message");
    }
    let text = response.text().context("read MCP HTTP response")?;
    serde_json::from_str(&text).context("invalid MCP HTTP JSON response")
}

fn read_sse_event(reader: &mut impl std::io::Read, timeout: Duration) -> Result<SseEvent> {
    let started = std::time::Instant::now();
    let mut buffer = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        if started.elapsed() >= timeout {
            bail!("MCP SSE event timed out");
        }
        let count = reader.read(&mut byte).context("read MCP SSE stream")?;
        if count == 0 {
            bail!("MCP SSE stream closed before an event");
        }
        buffer.push(byte[0]);
        if buffer.ends_with(b"\n\n") || buffer.ends_with(b"\r\n\r\n") {
            let text = String::from_utf8(buffer).context("MCP SSE event was not UTF-8")?;
            let mut event = String::new();
            let mut data = String::new();
            for line in text.lines() {
                if let Some(value) = line.strip_prefix("event:") {
                    event = value.trim().to_string();
                } else if let Some(value) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(value.trim());
                }
            }
            return Ok(SseEvent { event, data });
        }
    }
}

fn parse_tools_response(response: &Value, server: &McpServer) -> Result<Vec<McpTool>> {
    let tools = response
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .context("MCP tools/list response missing result.tools")?;
    tools
        .iter()
        .map(|tool| {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .context("MCP tools/list returned a tool without a name")?;
            Ok(McpTool {
                server: server.name.clone(),
                name: name.to_string(),
                allowed: server
                    .allowlist
                    .iter()
                    .any(|item| item == name || item == "*"),
                description: tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                server_command: if server.url.is_empty() {
                    server.command.clone()
                } else {
                    server.url.clone()
                },
            })
        })
        .collect()
}

pub fn list_tools(server: &McpServer, timeout: Duration) -> Result<Vec<McpTool>> {
    if server.transport.eq_ignore_ascii_case("stdio") {
        StdioMcpClient::new(timeout)?.list_tools(server)
    } else if server.transport.eq_ignore_ascii_case("http")
        || server.transport.eq_ignore_ascii_case("sse")
    {
        HttpMcpClient::new(timeout)?.list_tools(server)
    } else {
        bail!("unsupported MCP transport: {}", server.transport)
    }
}

pub fn call_tool(
    server: &McpServer,
    tool: &McpTool,
    arguments: Value,
    timeout: Duration,
) -> Result<Value> {
    if server.transport.eq_ignore_ascii_case("stdio") {
        StdioMcpClient::new(timeout)?.call_with_server(server, tool, arguments)
    } else if server.transport.eq_ignore_ascii_case("http")
        || server.transport.eq_ignore_ascii_case("sse")
    {
        HttpMcpClient::new(timeout)?.call_with_server(server, tool, arguments)
    } else {
        bail!("unsupported MCP transport: {}", server.transport)
    }
}

pub fn diagnose(server: &McpServer, timeout: Duration) -> Result<McpDiagnostics> {
    let started = std::time::Instant::now();
    let tools = list_tools(server, timeout)?;
    Ok(McpDiagnostics {
        server: server.name.clone(),
        transport: if server.transport.trim().is_empty() {
            "stdio".into()
        } else {
            server.transport.to_ascii_lowercase()
        },
        session_mode: "ephemeral".into(),
        lifecycle: vec![
            "initialize".into(),
            "notifications/initialized".into(),
            "tools/list".into(),
            "close".into(),
        ],
        protocol_versions_supported: SUPPORTED_PROTOCOL_VERSIONS
            .iter()
            .map(|version| (*version).to_string())
            .collect(),
        endpoint_configured: !server.url.trim().is_empty(),
        credential_env_configured: !server.credential_env.trim().is_empty(),
        healthy: true,
        latency_ms: started.elapsed().as_millis(),
        tools: tools.len(),
        allowed_tools: tools.iter().filter(|tool| tool.allowed).count(),
    })
}

impl Drop for McpSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn response_reader(stdout: ChildStdout) -> (Receiver<Value>, thread::JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && sender.send(value).is_err()
            {
                break;
            }
        }
    });
    (receiver, reader)
}

fn write_json(writer: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

pub fn ensure_allowed(server: &McpServer, tool: &McpTool) -> Result<()> {
    if !server.allowed
        || tool.server != server.name
        || !server
            .allowlist
            .iter()
            .any(|item| item == "*" || item == &tool.name)
        || !tool.allowed
    {
        bail!("MCP server/tool not allowlisted")
    }
    Ok(())
}

fn redact_json(value: &Value) -> String {
    crate::observability::redact(&value.to_string())
}

fn split_command_line(input: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut chars = input.chars().peekable();
    while let Some(character) = chars.next() {
        match quote {
            Some(delimiter) if character == delimiter => quote = None,
            Some(_) => current.push(character),
            None if character == '\'' || character == '\"' => quote = Some(character),
            None if character.is_whitespace() => {
                if !current.is_empty() {
                    result.push(std::mem::take(&mut current));
                }
            }
            None if character == '\\' && matches!(chars.peek(), Some('\\' | '\'' | '\"')) => {
                current.push(chars.next().unwrap_or_default());
            }
            None => current.push(character),
        }
    }
    if quote.is_some() {
        bail!("unterminated quote in MCP command");
    }
    if !current.is_empty() {
        result.push(current);
    }
    if result.is_empty() {
        bail!("MCP command cannot be empty");
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_by_default_requires_server_and_tool_allowlists() {
        let server = McpServer {
            name: "demo".into(),
            command: "demo-server".into(),
            allowed: true,
            allowlist: vec![],
            ..Default::default()
        };
        let tool = McpTool {
            server: "demo".into(),
            name: "read".into(),
            allowed: false,
            description: String::new(),
            server_command: "demo-server".into(),
        };
        assert!(ensure_allowed(&server, &tool).is_err());
    }

    #[test]
    fn wildcard_allowlist_allows_a_discovered_tool() {
        let server = McpServer {
            name: "demo".into(),
            command: "demo-server".into(),
            allowed: true,
            allowlist: vec!["*".into()],
            ..Default::default()
        };
        let tool = McpTool {
            server: "demo".into(),
            name: "read".into(),
            allowed: true,
            description: String::new(),
            server_command: "demo-server".into(),
        };
        assert!(ensure_allowed(&server, &tool).is_ok());
    }

    #[test]
    fn command_parser_preserves_windows_paths_and_quotes() {
        let parts = split_command_line(
            r#"powershell.exe -File "C:\Users\Edson\Desktop\Sapiens agent\mock.ps1""#,
        )
        .expect("parts");
        assert_eq!(parts[0], "powershell.exe");
        assert_eq!(parts[2], r#"C:\Users\Edson\Desktop\Sapiens agent\mock.ps1"#);
    }

    #[test]
    fn rejects_unsupported_protocol_and_incomplete_server_info() {
        let unsupported = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"protocolVersion": "1.0", "serverInfo": {"name": "demo", "version": "1"}}
        });
        assert!(validate_initialize_response(&unsupported).is_err());
        let incomplete = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"protocolVersion": PREFERRED_PROTOCOL_VERSION, "serverInfo": {"name": "demo"}}
        });
        assert!(validate_initialize_response(&incomplete).is_err());
    }

    #[test]
    fn rejects_malformed_tool_discovery_instead_of_silently_dropping_it() {
        let server = McpServer {
            name: "demo".into(),
            command: "demo-server".into(),
            allowed: true,
            allowlist: vec!["read".into()],
            ..Default::default()
        };
        let response = json!({"result": {"tools": [{"description": "missing name"}]}});
        assert!(parse_tools_response(&response, &server).is_err());
    }

    #[test]
    fn http_transport_runs_initialize_and_tool_discovery() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server_thread = std::thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().expect("request");
                let mut request = [0_u8; 16_384];
                let count = std::io::Read::read(&mut stream, &mut request).expect("read");
                let text = String::from_utf8_lossy(&request[..count]);
                assert!(text.starts_with("POST /mcp HTTP/1.1"));
                let body = match index {
                    0 => {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"local","version":"1"}}}"#
                    }
                    1 => "",
                    _ => {
                        r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"read","description":"read data"}]}}"#
                    }
                };
                let status = if index == 1 { "202 Accepted" } else { "200 OK" };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                std::io::Write::write_all(&mut stream, response.as_bytes()).expect("write");
            }
        });
        let server = McpServer {
            name: "local".into(),
            allowed: true,
            allowlist: vec!["read".into()],
            transport: "http".into(),
            url: format!("http://{address}/mcp"),
            ..Default::default()
        };
        let diagnostics = diagnose(&server, Duration::from_secs(5)).expect("diagnostics");
        assert!(diagnostics.healthy);
        assert_eq!(diagnostics.transport, "http");
        assert_eq!(diagnostics.tools, 1);
        assert_eq!(diagnostics.allowed_tools, 1);
        assert_eq!(diagnostics.session_mode, "ephemeral");
        assert_eq!(diagnostics.lifecycle.len(), 4);
        server_thread.join().expect("server");
    }

    #[test]
    fn parses_legacy_sse_endpoint_event() {
        let mut stream = std::io::Cursor::new(b"event: endpoint\ndata: /messages\n\n".to_vec());
        let event = read_sse_event(&mut stream, Duration::from_secs(1)).expect("event");
        assert_eq!(event.event, "endpoint");
        assert_eq!(event.data, "/messages");
    }
}
