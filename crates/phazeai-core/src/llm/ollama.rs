use crate::error::PhazeError;
use crate::llm::traits::*;
use crate::tools::ToolDefinition;
use futures::channel::mpsc;
use futures::StreamExt;
use ollama_rs::generation::chat::{
    ChatMessage as OllamaChatMessage, ChatMessageRequest, ChatToolCall, ChatToolFunction,
    MessageRole,
};
use ollama_rs::Ollama;
use serde_json::Value;

/// Client for local Ollama models using our forked ollama-rs with native tool calling.
pub struct OllamaClient {
    ollama: Ollama,
    model: String,
    base_url: String,
}

impl OllamaClient {
    pub fn new(model: impl Into<String>) -> Self {
        let base_url = "http://localhost:11434".to_string();
        Self {
            ollama: Ollama::try_new(&base_url).expect("Invalid Ollama URL"),
            model: model.into(),
            base_url,
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self.ollama = Ollama::try_new(&self.base_url).expect("Invalid Ollama URL");
        self
    }

    /// Convert PhazeAI messages to ollama-rs ChatMessages
    fn to_ollama_messages(messages: &[Message]) -> Vec<OllamaChatMessage> {
        messages
            .iter()
            .map(|m| {
                if m.tool_call_id.is_some() {
                    // Tool result message
                    OllamaChatMessage::tool(m.content.clone())
                } else if let Some(ref tool_calls) = m.tool_calls {
                    // Assistant message with tool calls — send as assistant with content
                    // The tool_calls are serialized via ChatMessage.tool_calls automatically
                    let mut msg = OllamaChatMessage::assistant(m.content.clone());
                    let tc_vec: Vec<ChatToolCall> = tool_calls
                        .iter()
                        .map(|tc| {
                            let args: Value = serde_json::from_str(&tc.function.arguments)
                                .unwrap_or(Value::Object(Default::default()));
                            ChatToolCall {
                                function: ChatToolFunction {
                                    name: tc.function.name.clone(),
                                    arguments: args,
                                },
                            }
                        })
                        .collect();
                    msg.tool_calls = Some(tc_vec);
                    msg
                } else {
                    let role = match m.role {
                        Role::User => MessageRole::User,
                        Role::Assistant => MessageRole::Assistant,
                        Role::System => MessageRole::System,
                    };
                    OllamaChatMessage::new(role, m.content.clone())
                }
            })
            .collect()
    }

    /// Build tool definitions JSON array from PhazeAI ToolDefinitions
    fn build_tool_defs(tools: &[ToolDefinition]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl LlmClient for OllamaClient {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        let ollama_messages = Self::to_ollama_messages(messages);
        let request = if !tools.is_empty() {
            ChatMessageRequest::new(self.model.clone(), ollama_messages)
                .tools(Self::build_tool_defs(tools))
        } else {
            ChatMessageRequest::new(self.model.clone(), ollama_messages)
        };

        let response = self
            .ollama
            .send_chat_messages(request)
            .await
            .map_err(|e| PhazeError::Llm(format!("Ollama chat error: {e}")))?;

        let chat_msg = response.message.unwrap_or_else(|| {
            OllamaChatMessage::assistant(String::new())
        });

        let content = chat_msg.content.clone();

        // Convert tool calls from ollama-rs types to PhazeAI types
        let tool_calls: Vec<ToolCall> = chat_msg
            .tool_calls
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(i, tc)| ToolCall {
                id: format!("ollama_tool_{}", i),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: tc.function.name.clone(),
                    arguments: serde_json::to_string(&tc.function.arguments)
                        .unwrap_or_default(),
                },
            })
            .collect();

        let message = if tool_calls.is_empty() {
            Message::assistant(content)
        } else {
            Message::assistant_with_tools(content, tool_calls)
        };

        // Extract usage from final_data
        let usage = response.final_data.map(|fd| Usage {
            input_tokens: fd.prompt_eval_count as u32,
            output_tokens: fd.eval_count as u32,
        });

        Ok(LlmResponse { message, usage })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
        // If tools are provided, use non-streaming chat (Ollama streaming doesn't
        // reliably support tool calls) and convert to stream events
        if !tools.is_empty() {
            let response = self.chat(messages, tools).await?;
            let (tx, rx) = mpsc::unbounded();

            if !response.message.content.is_empty() {
                let _ = tx.unbounded_send(StreamEvent::TextDelta(response.message.content.clone()));
            }

            if let Some(ref tool_calls) = response.message.tool_calls {
                for tc in tool_calls {
                    let _ = tx.unbounded_send(StreamEvent::ToolCallStart {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                    });
                    let _ = tx.unbounded_send(StreamEvent::ToolCallDelta {
                        id: tc.id.clone(),
                        arguments_delta: tc.function.arguments.clone(),
                    });
                    let _ = tx.unbounded_send(StreamEvent::ToolCallEnd {
                        id: tc.id.clone(),
                    });
                }
            }

            let _ = tx.unbounded_send(StreamEvent::Done);
            return Ok(rx);
        }

        // No tools — use ollama-rs streaming API
        let ollama_messages = Self::to_ollama_messages(messages);
        let request = ChatMessageRequest::new(self.model.clone(), ollama_messages);

        let stream = self
            .ollama
            .send_chat_messages_stream(request)
            .await
            .map_err(|e| PhazeError::Llm(format!("Ollama stream error: {e}")))?;

        let (tx, rx) = mpsc::unbounded();

        tokio::spawn(async move {
            let mut stream = stream;

            while let Some(result) = stream.next().await {
                match result {
                    Ok(response) => {
                        if let Some(msg) = &response.message {
                            if !msg.content.is_empty() {
                                let _ = tx.unbounded_send(StreamEvent::TextDelta(
                                    msg.content.clone(),
                                ));
                            }
                        }
                        if response.done {
                            let _ = tx.unbounded_send(StreamEvent::Done);
                            return;
                        }
                    }
                    Err(_) => {
                        let _ = tx.unbounded_send(StreamEvent::Error(
                            "Stream deserialization error".to_string(),
                        ));
                        break;
                    }
                }
            }

            let _ = tx.unbounded_send(StreamEvent::Done);
        });

        Ok(rx)
    }
}
