//! Bounded local CLI transports. Credentials remain in the provider's own store.
use crate::{NodeError, execution::ExecutionConfig, journal::Journal};
use orchestrator_agents::Agent;
use orchestrator_protocol::{
    activity::ActivityPhase,
    agent::{AgentAction, AgentCommand, AgentEvent, AgentEventKind},
    execution::LocalProvider,
    permission::ProviderBinding,
    values::MessageText,
};
use serde_json::{Value, json};
use std::{collections::VecDeque, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::Command,
};

// Native image events contain base64 media; keep a separate bounded frame budget.
const MAX_PROVIDER_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// One specialist, owned by a durable worker rather than the supervisor.
#[derive(Debug)]
pub struct LocalAgent {
    journal: Journal,
    instance: String,
    config: ExecutionConfig,
}
impl LocalAgent {
    /// Construct a provider adapter for an already registered worker.
    #[must_use]
    pub fn new(journal: Journal, instance: String, config: ExecutionConfig) -> Self {
        Self {
            journal,
            instance,
            config,
        }
    }
    async fn activity(
        &self,
        command: &AgentCommand,
        phase: ActivityPhase,
    ) -> Result<(), NodeError> {
        self.journal
            .record_activity(
                command.command_id,
                &self.instance,
                self.config.workspace.provider,
                phase,
            )
            .await
    }
    async fn bind(&self, command: &AgentCommand, id: &str) -> Result<(), NodeError> {
        let binding = ProviderBinding {
            group_id: command.group_id,
            node_id: command.node_id,
            target: command.target,
            provider: orchestrator_protocol::capabilities::Capability::local_provider(
                self.config.workspace.provider,
            ),
            provider_session_id: id
                .to_owned()
                .try_into()
                .map_err(|_| NodeError::Rejected("invalid provider session"))?,
            resumable: true,
        };
        self.journal
            .bind_provider(command.command_id, &self.instance, &binding)
            .await
    }
    async fn run(&self, command: &AgentCommand, message: &str) -> Result<String, NodeError> {
        self.run_with_timeout(command, message, Duration::from_mins(10))
            .await
    }
    async fn run_with_timeout(
        &self,
        command: &AgentCommand,
        message: &str,
        timeout: Duration,
    ) -> Result<String, NodeError> {
        crate::execution::check_directory(&self.config, &self.config.directory)?;
        let binding = self
            .journal
            .provider_binding(command.target.session_id)
            .await?;
        if matches!(command.action, AgentAction::Send { .. }) && binding.is_none() {
            return Err(NodeError::Rejected(
                "provider session is unavailable; automatic replay refused",
            ));
        }
        self.activity(command, ActivityPhase::Preparing).await?;
        let message = crate::attachments::input_context(&self.config, command, message)?;
        let mut process = self.process(command, binding.as_ref()).await?;
        self.activity(command, ActivityPhase::Connecting).await?;
        let (mut child, mut wire) = spawn(&mut process)?;
        let result = tokio::time::timeout(timeout, async {
            match self.config.workspace.provider {
                LocalProvider::Codex => {
                    self.codex(command, &message, binding.as_ref(), &mut wire)
                        .await
                }
                LocalProvider::Claude => {
                    wire.input.write_all(message.as_bytes()).await?;
                    drop(wire.input);
                    self.activity(command, ActivityPhase::Submitted).await?;
                    let mut output = Wire {
                        input: Box::new(tokio::io::sink()),
                        output: wire.output,
                        remaining: wire.remaining,
                        pending: wire.pending,
                    };
                    let text = self.claude(command, &mut output).await?;
                    successful_exit(child.wait().await)?;
                    Ok(text)
                }
            }
        })
        .await
        .map_err(|_| NodeError::Rejected("provider timeout; inspect workspace before retrying"))?;
        // App-server is session-scoped here; the durable worker survives supervisor restarts.
        let _ = child.kill().await;
        result
    }
    async fn process(
        &self,
        command: &AgentCommand,
        binding: Option<&ProviderBinding>,
    ) -> Result<Command, NodeError> {
        let mut process = Command::new(&self.config.executable);
        process
            .current_dir(&self.config.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for key in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ] {
            process.env_remove(key);
        }
        match self.config.workspace.provider {
            LocalProvider::Codex => {
                process.args([
                    "app-server",
                    "-c",
                    "mcp_servers={}",
                    "-c",
                    "features.apps=false",
                    "-c",
                    "features.plugins=false",
                    "-c",
                    "features.multi_agent=false",
                    "-c",
                    "skills.include_instructions=false",
                    "-c",
                    "tool_output_token_limit=2000",
                ]);
            }
            LocalProvider::Claude => {
                let id = binding.map_or_else(
                    || command.target.session_id.to_string(),
                    |b| b.provider_session_id.as_str().to_owned(),
                );
                self.bind(command, &id).await?;
                process
                    .args([
                        "--print",
                        "--output-format",
                        "stream-json",
                        "--verbose",
                        "--safe-mode",
                        "--restricted",
                        "--strict-mcp-config",
                        "--tools",
                        "Read,Glob,Grep,Edit,Write",
                        "--permission-mode",
                        if self.config.workspace.allow_edits {
                            "acceptEdits"
                        } else {
                            "dontAsk"
                        },
                        "--append-system-prompt-file",
                    ])
                    .arg(&self.config.instructions)
                    .arg(if binding.is_some() {
                        "--resume"
                    } else {
                        "--session-id"
                    })
                    .arg(id);
                if !self.config.workspace.allow_edits {
                    process.args(["--disallowedTools", "Edit,Write"]);
                }
            }
        }
        Ok(process)
    }
    async fn codex(
        &self,
        command: &AgentCommand,
        message: &str,
        saved: Option<&ProviderBinding>,
        wire: &mut Wire,
    ) -> Result<String, NodeError> {
        wire.send(json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"cuttlefish","version":env!("CARGO_PKG_VERSION")}}})).await?;
        wire.response(1).await?;
        wire.send(json!({"method":"initialized","params":{}}))
            .await?;
        let mut instructions = String::new();
        std::io::Read::read_to_string(
            &mut std::io::Read::take(std::fs::File::open(&self.config.instructions)?, 65_537),
            &mut instructions,
        )?;
        if instructions.trim().is_empty() || instructions.len() > 65_536 {
            return Err(NodeError::Rejected("invalid specialist instructions"));
        }
        let mut params = json!({"cwd":self.config.directory,"approvalPolicy":"never","sandbox":if self.config.workspace.allow_edits {"workspace-write"} else {"read-only"},"developerInstructions":instructions});
        if let Some(binding) = saved {
            params["threadId"] = json!(binding.provider_session_id.as_str());
        }
        wire.send(json!({"id":2,"method":if saved.is_some(){"thread/resume"}else{"thread/start"},"params":params})).await?;
        let response = wire.response(2).await?;
        let id = response
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or(NodeError::Rejected("provider session missing"))?;
        self.bind(command, id).await?;
        wire.send(json!({"id":3,"method":"turn/start","params":{"threadId":id,"input":crate::attachments::codex_input(&self.config,command,message),"approvalPolicy":"never","sandboxPolicy":if self.config.workspace.allow_edits {json!({"type":"workspaceWrite","writableRoots":[self.config.directory],"networkAccess":false})}else{json!({"type":"readOnly"})}}})).await?;
        let response = wire.response(3).await?;
        self.activity(command, ActivityPhase::Submitted).await?;
        let turn = response
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .ok_or(NodeError::Rejected("provider turn missing"))?;
        let mut text = String::new();
        loop {
            let event = wire.read().await?;
            if event.pointer("/params/threadId").and_then(Value::as_str) != Some(id) {
                continue;
            }
            if let Some(phase) = observed_activity(LocalProvider::Codex, &event) {
                self.activity(command, phase).await?;
            }
            match event.get("method").and_then(Value::as_str) {
                Some("item/completed")
                    if event.pointer("/params/turnId").and_then(Value::as_str) == Some(turn)
                        && event.pointer("/params/item/type").and_then(Value::as_str)
                            == Some("agentMessage") =>
                {
                    event
                        .pointer("/params/item/text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .clone_into(&mut text);
                }
                Some("turn/completed")
                    if event.pointer("/params/turn/id").and_then(Value::as_str) == Some(turn) =>
                {
                    if event.pointer("/params/turn/status").and_then(Value::as_str)
                        != Some("completed")
                    {
                        return Err(NodeError::Rejected(
                            "provider turn failed; inspect workspace before retrying",
                        ));
                    }
                    return Ok(text);
                }
                _ => {}
            }
        }
    }
    async fn claude(&self, command: &AgentCommand, wire: &mut Wire) -> Result<String, NodeError> {
        loop {
            let event = wire.read().await?;
            if let Some(id) = event.get("session_id").and_then(Value::as_str) {
                self.bind(command, id).await?;
            }
            if let Some(phase) = observed_activity(LocalProvider::Claude, &event) {
                self.activity(command, phase).await?;
            }
            if event.get("type").and_then(Value::as_str) == Some("result") {
                if event.get("session_id").and_then(Value::as_str).is_none() {
                    return Err(NodeError::Rejected("provider session missing"));
                }
                if event.get("is_error").and_then(Value::as_bool) != Some(false)
                    || event.get("subtype").and_then(Value::as_str) != Some("success")
                {
                    return Err(NodeError::Rejected(
                        "provider turn failed; inspect workspace before retrying",
                    ));
                }
                return event
                    .get("result")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or(NodeError::Rejected("provider response missing"));
            }
        }
    }
}
impl Agent for LocalAgent {
    type Error = NodeError;
    fn failure_is_uncertain(&self) -> bool {
        true
    }
    async fn execute(&mut self, command: &AgentCommand) -> Result<AgentEvent, NodeError> {
        let result = self.execute_turn(command).await;
        if result.is_err() {
            // The original failure is authoritative even when storage is unavailable.
            let _ = self.activity(command, ActivityPhase::NeedsAttention).await;
        }
        result
    }
}
impl LocalAgent {
    async fn execute_turn(&self, command: &AgentCommand) -> Result<AgentEvent, NodeError> {
        let kind = match &command.action {
            AgentAction::Cancel => AgentEventKind::Cancelled,
            AgentAction::Start { message, .. } | AgentAction::Send { message } => {
                let text = self.run(command, message.text.as_str()).await?;
                AgentEventKind::Replied {
                    message_id: message.id,
                    text: MessageText::try_from(text)
                        .map_err(|_| NodeError::Rejected("invalid provider response"))?,
                }
            }
        };
        let artifacts = if matches!(command.action, AgentAction::Cancel) {
            vec![]
        } else {
            self.activity(command, ActivityPhase::CollectingResults)
                .await?;
            crate::attachments::collect_outputs(&self.journal, &self.config, command).await?
        };
        Ok(AgentEvent {
            artifacts,
            version: command.version,
            command_id: command.command_id,
            group_id: command.group_id,
            node_id: command.node_id,
            target: command.target,
            kind,
        })
    }
}
struct Wire {
    input: Box<dyn AsyncWrite + Unpin + Send>,
    output: Box<dyn AsyncBufRead + Unpin + Send>,
    remaining: usize,
    pending: VecDeque<Value>,
}
impl Wire {
    async fn send(&mut self, value: Value) -> Result<(), NodeError> {
        let mut bytes = value.to_string().into_bytes();
        bytes.push(b'\n');
        self.input.write_all(&bytes).await?;
        self.input.flush().await?;
        Ok(())
    }
    async fn read(&mut self) -> Result<Value, NodeError> {
        match self.pending.pop_front() {
            Some(event) => Ok(event),
            None => self.read_stream().await,
        }
    }
    async fn read_stream(&mut self) -> Result<Value, NodeError> {
        let mut line = Vec::new();
        (&mut self.output)
            .take((MAX_PROVIDER_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .await?;
        if line.is_empty() || line.len() > MAX_PROVIDER_FRAME_BYTES || line.len() > self.remaining {
            return Err(NodeError::Rejected(
                "provider stream closed or exceeded limits",
            ));
        }
        self.remaining -= line.len();
        let event: Value = serde_json::from_slice(&line)?;
        if event.get("id").is_some() && event.get("method").is_some() {
            return Err(NodeError::Rejected(
                "provider requires unsupported interactive approval",
            ));
        }
        Ok(event)
    }
    async fn response(&mut self, id: u64) -> Result<Value, NodeError> {
        loop {
            let event = self.read_stream().await?;
            if event.get("id").and_then(Value::as_u64) == Some(id) {
                return event
                    .get("result")
                    .cloned()
                    .ok_or(NodeError::Rejected("provider rejected request"));
            }
            if self.pending.len() >= 1024 {
                return Err(NodeError::Rejected("too many pending provider events"));
            }
            self.pending.push_back(event);
        }
    }
}

#[cfg(all(test, unix))]
#[path = "../tests/internal/local_provider.rs"]
mod tests;

fn transport(child: &mut tokio::process::Child) -> Result<Wire, NodeError> {
    let input = child
        .stdin
        .take()
        .ok_or(NodeError::Rejected("provider stdin missing"))?;
    let output = child
        .stdout
        .take()
        .ok_or(NodeError::Rejected("provider stdout missing"))?;
    Ok(Wire {
        input: Box::new(input),
        output: Box::new(BufReader::new(output)),
        remaining: 32 * 1024 * 1024,
        pending: VecDeque::new(),
    })
}

fn successful_exit(status: std::io::Result<std::process::ExitStatus>) -> Result<(), NodeError> {
    if status?.success() {
        Ok(())
    } else {
        Err(NodeError::Rejected("provider process failed"))
    }
}
fn spawn(process: &mut Command) -> Result<(tokio::process::Child, Wire), NodeError> {
    let mut child = process.spawn()?;
    let wire = transport(&mut child)?;
    Ok((child, wire))
}

/// Interpret only explicit provider events; never expose raw tool arguments or reasoning.
fn observed_activity(provider: LocalProvider, event: &Value) -> Option<ActivityPhase> {
    match provider {
        LocalProvider::Codex => match event.get("method")?.as_str()? {
            "turn/started" => Some(ActivityPhase::Working),
            "item/started" => match event.pointer("/params/item/type")?.as_str()? {
                "commandExecution" => Some(ActivityPhase::RunningCommand),
                "fileChange" => Some(ActivityPhase::WritingFiles),
                "webSearch" => Some(ActivityPhase::SearchingWeb),
                "imageGeneration" => Some(ActivityPhase::GeneratingImage),
                "collabAgentToolCall" => Some(ActivityPhase::Delegating),
                "mcpToolCall" | "dynamicToolCall" => Some(ActivityPhase::UsingTool),
                "agentMessage" | "reasoning" => Some(ActivityPhase::Working),
                _ => None,
            },
            "item/completed" => Some(ActivityPhase::Waiting),
            _ => None,
        },
        LocalProvider::Claude => match event.get("type")?.as_str()? {
            "system" if event.get("subtype")?.as_str()? == "init" => Some(ActivityPhase::Working),
            "assistant" => {
                let content = event.pointer("/message/content")?.as_array()?;
                let tool = content
                    .iter()
                    .find(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))?;
                Some(match tool.get("name")?.as_str()? {
                    "Read" | "Glob" | "Grep" => ActivityPhase::ReadingFiles,
                    "Write" | "Edit" | "MultiEdit" => ActivityPhase::WritingFiles,
                    "Bash" => ActivityPhase::RunningCommand,
                    "WebSearch" | "WebFetch" => ActivityPhase::SearchingWeb,
                    "Task" | "Agent" => ActivityPhase::Delegating,
                    _ => ActivityPhase::UsingTool,
                })
            }
            "user"
                if event
                    .pointer("/message/content")?
                    .as_array()?
                    .iter()
                    .any(|block| {
                        block.get("type").and_then(Value::as_str) == Some("tool_result")
                    }) =>
            {
                Some(ActivityPhase::Waiting)
            }
            _ => None,
        },
    }
}
