use crate::llm::Message;
use std::collections::VecDeque;

pub struct ConversationHistory {
    messages: VecDeque<Message>,
    max_messages: usize,
    system_prompt: Option<String>,
}

impl ConversationHistory {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            max_messages: 100,
            system_prompt: None,
        }
    }

    pub fn with_max_messages(mut self, max: usize) -> Self {
        self.max_messages = max;
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.system_prompt = Some(prompt.into());
    }

    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.messages.push_back(Message::user(content));
        self.trim_if_needed();
    }

    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.messages.push_back(Message::assistant(content));
        self.trim_if_needed();
    }

    pub fn add_tool_result(&mut self, tool_call_id: impl Into<String>, result: impl Into<String>) {
        self.messages
            .push_back(Message::tool_result(tool_call_id, result));
        self.trim_if_needed();
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push_back(message);
        self.trim_if_needed();
    }

    /// Get all messages including system prompt as a system message.
    pub fn get_messages(&self) -> Vec<Message> {
        let mut messages = Vec::new();
        if let Some(ref system) = self.system_prompt {
            messages.push(Message::system(system));
        }
        messages.extend(self.messages.iter().cloned());
        messages
    }

    /// Get only conversation messages (no system prompt).
    pub fn get_conversation_messages(&self) -> Vec<Message> {
        self.messages.iter().cloned().collect()
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn last_message(&self) -> Option<&Message> {
        self.messages.back()
    }

    fn trim_if_needed(&mut self) {
        while self.messages.len() > self.max_messages {
            self.messages.pop_front();
        }
    }

    pub fn estimate_tokens(&self) -> usize {
        self.messages
            .iter()
            .map(|m| m.content.len() / 4)
            .sum()
    }
}

impl Default for ConversationHistory {
    fn default() -> Self {
        Self::new()
    }
}
