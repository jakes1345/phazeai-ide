//! A model call that can only produce text.
//!
//! For helpers that want a reply and nothing else, such as the IDE's Ctrl+K
//! inline edit, commit-message button and terminal command bar. The model is
//! offered no tools and a tool call it streams anyway is dropped, so text in
//! the prompt (an open file, a staged diff) can't get anything run. An
//! [`crate::Agent`], by contrast, runs every tool call it gets unless an
//! approval function says no.

use super::{LlmClient, Message, StreamEvent};
use futures::StreamExt;

/// What a [`text_only_reply`] call produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextReply {
    /// The text the model streamed: all of it, or what arrived before `error`.
    pub text: String,
    /// Why the call failed, if it did.
    pub error: Option<String>,
}

/// Send `prompt` as the only message (no system prompt, no history), offer
/// no tools, and collect the text the model streams back.
pub async fn text_only_reply(client: &dyn LlmClient, prompt: &str) -> TextReply {
    let mut reply = TextReply::default();
    let mut stream = match client.chat_stream(&[Message::user(prompt)], &[]).await {
        Ok(stream) => stream,
        Err(e) => {
            reply.error = Some(e.to_string());
            return reply;
        }
    };
    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::TextDelta(delta) => reply.text.push_str(&delta),
            StreamEvent::Done => break,
            StreamEvent::Error(e) => {
                reply.error = Some(e);
                break;
            }
            // No tools were offered, so there is nothing to call; usage is not text.
            StreamEvent::ToolCallStart { .. }
            | StreamEvent::ToolCallDelta { .. }
            | StreamEvent::ToolCallEnd { .. }
            | StreamEvent::Usage(_) => {}
        }
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::PhazeError;
    use crate::llm::{LlmResponse, Role};
    use crate::tools::ToolDefinition;
    use futures::channel::mpsc;
    use std::sync::Mutex;

    /// Streams `events` for every request and records what each one carried.
    struct Scripted {
        events: Vec<StreamEvent>,
        /// (messages, names of the tools offered) per request.
        requests: Mutex<Vec<(Vec<Message>, Vec<String>)>>,
    }

    impl Scripted {
        fn new(events: Vec<StreamEvent>) -> Self {
            Self {
                events,
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for Scripted {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
        ) -> Result<LlmResponse, PhazeError> {
            Err(PhazeError::Other("only chat_stream is scripted".into()))
        }

        async fn chat_stream(
            &self,
            messages: &[Message],
            tools: &[ToolDefinition],
        ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
            let names = tools.iter().map(|t| t.name.clone()).collect();
            self.requests
                .lock()
                .unwrap()
                .push((messages.to_vec(), names));
            let (tx, rx) = mpsc::unbounded();
            for event in &self.events {
                tx.unbounded_send(event.clone()).unwrap();
            }
            Ok(rx)
        }
    }

    fn tool_call(id: &str, name: &str, arguments: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::ToolCallStart {
                id: id.into(),
                name: name.into(),
            },
            StreamEvent::ToolCallDelta {
                id: id.into(),
                arguments_delta: arguments.into(),
            },
            StreamEvent::ToolCallEnd { id: id.into() },
        ]
    }

    #[tokio::test]
    async fn offers_no_tools_and_keeps_only_the_text() {
        let mut events = vec![StreamEvent::TextDelta("fn a() ".into())];
        events.extend(tool_call("c1", "bash", r#"{"command":"touch ran"}"#));
        events.push(StreamEvent::TextDelta("{}".into()));
        events.push(StreamEvent::Done);
        let client = Scripted::new(events);

        let reply = text_only_reply(&client, "add a fn").await;

        {
            let requests = client.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let (messages, tools) = &requests[0];
            assert!(tools.is_empty(), "offered tools: {tools:?}");
            assert_eq!(messages.len(), 1, "{messages:?}");
            assert_eq!(messages[0].role, Role::User);
            assert_eq!(messages[0].content, "add a fn");
        }
        assert_eq!(
            reply,
            TextReply {
                text: "fn a() {}".into(),
                error: None
            }
        );

        // A failure ends the reply and keeps the text that arrived before it.
        let client = Scripted::new(vec![
            StreamEvent::TextDelta("partial".into()),
            StreamEvent::Error("connection reset".into()),
            StreamEvent::TextDelta("never read".into()),
        ]);
        assert_eq!(
            text_only_reply(&client, "x").await,
            TextReply {
                text: "partial".into(),
                error: Some("connection reset".into())
            }
        );
    }
}
