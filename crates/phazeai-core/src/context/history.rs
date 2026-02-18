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
        let mut result_str = result.into();
        
        // Truncate huge tool results to save context
        const MAX_TOOL_RESULT_LEN: usize = 12000;
        if result_str.len() > MAX_TOOL_RESULT_LEN {
            result_str = format!(
                "{}... [Truncated {} bytes]", 
                &result_str[..MAX_TOOL_RESULT_LEN],
                result_str.len() - MAX_TOOL_RESULT_LEN
            );
        }

        self.messages
            .push_back(Message::tool_result(tool_call_id, result_str));
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
        // Keep message count within limits
        while self.messages.len() > self.max_messages {
            self.messages.pop_front();
        }
    }

    /// Trim conversation to stay within a token budget while preserving the system prompt and recent context.
    pub fn trim_to_token_budget(&mut self, max_tokens: usize) {
        while self.estimate_tokens() > max_tokens && !self.messages.is_empty() {
            // Remove the oldest message
            self.messages.pop_front();
        }
    }

    pub fn estimate_tokens(&self) -> usize {
        let mut total = 0;
        
        // System prompt tokens
        if let Some(ref system) = self.system_prompt {
            total += (system.len() + 3) / 4;
        }

        // Message tokens
        total += self.messages
            .iter()
            .map(|m| (m.content.len() + 3) / 4)
            .sum::<usize>();
            
        total
    }
}


impl Default for ConversationHistory {
    fn default() -> Self {
        Self::new()
    }
}
