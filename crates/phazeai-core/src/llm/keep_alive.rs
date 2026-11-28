//! Keeping Claude's prompt cache warm while the user reads an answer.
//!
//! Anthropic keeps a cached prompt prefix for five minutes from the start of
//! the last request that read or wrote it. A user who reads Claude's answer,
//! tries it and replies six minutes later finds the cache gone: the next
//! request writes the whole conversation again at 1.25x the input price
//! instead of reading it at a tenth (a twentieth on the newest models).
//!
//! So once an agent turn ends, the request that produced the answer is sent
//! again every four minutes with `max_tokens: 0`, for as long as the window
//! the settings give (`[llm] cache_keep_alive_minutes`). The API reads the
//! prefix, which restarts its timer, writes nothing and generates nothing, so
//! each refresh bills only a cache read. A new request in the same
//! conversation ends the refreshes, since it refreshes the cache itself.
//! A refresh that fails ends them too: it costs nothing to stop.
//!
//! Refreshes are wasted when the user comes back after the window anyway.
//! So once the last [`LATE_REPLIES_TO_PAUSE`] replies all came after it,
//! answers are not kept warm until a reply comes inside the window again.
//!
//! This is cheaper than the one-hour TTL, which charges 2x for every write
//! in the conversation to keep the few that matter alive.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

use crate::llm::traits::{Message, Role};

/// Anthropic's default cache lifetime, from the start of a request.
pub(super) const CACHE_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// How often a refresh is sent: well inside [`CACHE_LIFETIME`], so a slow
/// connection or a busy machine never lets the entry lapse.
pub(super) const REFRESH_EVERY: Duration = Duration::from_secs(4 * 60);

/// How long the cache is kept warm after an answer when the settings say
/// nothing (`[llm] cache_keep_alive_minutes`).
pub const DEFAULT_KEEP_ALIVE_MINUTES: u32 = 15;

/// After this many replies in a row that came after the keep-alive window,
/// answers are not kept warm until one comes inside it again.
pub const LATE_REPLIES_TO_PAUSE: usize = 3;

/// Conversations remembered as answered and waiting for a reply; past this,
/// the oldest are forgotten (a reply to one is then not counted).
const ANSWERED_KEPT: usize = 64;

/// What the keep-alive knows across clients: the IDE builds a new agent and
/// client for every chat message, so this is kept for the whole process.
#[derive(Default)]
struct State {
    /// The refreshes running, by conversation ([`conversation_key`]), each
    /// with an id so a finished task removes only its own entry.
    running: HashMap<u64, (u64, tokio::task::AbortHandle)>,
    /// When each conversation was last answered and how long it was to be
    /// kept warm, until its next request.
    answered: HashMap<u64, (Instant, Duration)>,
    /// Whether each recent reply came after its window, oldest first.
    late: std::collections::VecDeque<bool>,
}

impl State {
    /// A request in conversation `key` at `now`: its refreshes stop, and if
    /// it replies to an answer, whether the reply was late is remembered.
    fn next_request(&mut self, key: u64, now: Instant) {
        if let Some((_, handle)) = self.running.remove(&key) {
            handle.abort();
        }
        if let Some((at, window)) = self.answered.remove(&key) {
            self.late
                .push_back(now.saturating_duration_since(at) > window);
            while self.late.len() > LATE_REPLIES_TO_PAUSE {
                self.late.pop_front();
            }
        }
    }

    /// Conversation `key` was answered at `at`, to be kept warm for `window`.
    fn answered(&mut self, key: u64, at: Instant, window: Duration) {
        if self.answered.len() >= ANSWERED_KEPT {
            if let Some(oldest) = self
                .answered
                .iter()
                .min_by_key(|(_, (t, _))| *t)
                .map(|(k, _)| *k)
            {
                self.answered.remove(&oldest);
            }
        }
        self.answered.insert(key, (at, window));
    }

    /// Whether keeping an answer warm is likely to pay: not while every one
    /// of the last few replies came after the window.
    fn worth_it(&self) -> bool {
        self.late.len() < LATE_REPLIES_TO_PAUSE || self.late.iter().any(|late| !late)
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(state.get_or_insert_with(State::default))
}

/// Which conversation `messages` belong to: their system prompt and first
/// message, which every later request of the conversation repeats.
pub(super) fn conversation_key(messages: &[Message]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for m in messages.iter().filter(|m| m.role == Role::System) {
        m.content.hash(&mut h);
    }
    if let Some(first) = messages.iter().find(|m| m.role != Role::System) {
        format!("{:?}", first.role).hash(&mut h);
        first.content.hash(&mut h);
    }
    h.finish()
}

/// A new request in conversation `key` is about to be sent: it keeps the
/// cache warm by itself, so the refreshes stop.
pub(super) fn cancel(key: u64) {
    with_state(|state| state.next_request(key, Instant::now()));
}

/// Whether the cache of conversation `key` is being refreshed now.
#[cfg(test)]
pub(super) fn is_running(key: u64) -> bool {
    with_state(|state| state.running.contains_key(&key))
}

/// The request to send again to refresh the cache: the answered request with
/// no reply asked for, and not streamed (`max_tokens: 0` refuses `stream`).
/// Everything the cache is keyed on (tools, system, messages, thinking and
/// effort settings, breakpoints) is left as it was.
pub(super) fn refresh_body(answered: &Value) -> Value {
    let mut body = answered.clone();
    if let Some(fields) = body.as_object_mut() {
        fields.remove("stream");
    }
    body["max_tokens"] = Value::from(0);
    body
}

/// Everything needed to refresh one conversation's cache after its turn
/// ends, made before the request is sent.
pub(super) struct KeepAlive {
    pub(super) key: u64,
    /// When the answered request started: the cache entry's timer runs from
    /// then.
    pub(super) started: Instant,
    /// How long after the answer the cache should stay warm.
    pub(super) window: Duration,
    pub(super) every: Duration,
    /// Sends one refresh and says whether the API took it.
    pub(super) send: Box<dyn Fn() -> futures::future::BoxFuture<'static, bool> + Send + Sync>,
}

impl KeepAlive {
    /// How many refreshes keep the cache warm for [`Self::window`] after
    /// the answer: one every [`Self::every`] while the entry would lapse
    /// before the window ends.
    pub(super) fn refreshes(window: Duration, every: Duration, lifetime: Duration) -> u32 {
        let mut warm_until = lifetime;
        let mut sent_at = Duration::ZERO;
        let mut count = 0;
        while warm_until < window && !every.is_zero() {
            sent_at += every;
            warm_until = sent_at + lifetime;
            count += 1;
        }
        count
    }

    /// Start refreshing in the background, replacing any refreshes already
    /// running for the same conversation.
    pub(super) fn start(self, lifetime: Duration) {
        let count = Self::refreshes(self.window, self.every, lifetime);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let key = self.key;
        // Held until the task is registered, so a task that finishes at
        // once cannot remove its entry before it is there.
        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let state = guard.get_or_insert_with(State::default);
        state.answered(key, Instant::now(), self.window);
        if count == 0 || !state.worth_it() {
            return;
        }
        let task = tokio::spawn(async move {
            let mut at = self.started;
            for n in 1..=count {
                at += self.every;
                tokio::time::sleep_until(at).await;
                if !(self.send)().await {
                    tracing::debug!("cache keep-alive {n} of {count} refused; stopping");
                    break;
                }
            }
            with_state(|state| {
                if state
                    .running
                    .get(&key)
                    .is_some_and(|(owner, _)| *owner == id)
                {
                    state.running.remove(&key);
                }
            });
        });
        if let Some((_, old)) = state.running.insert(key, (id, task.abort_handle())) {
            old.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_sets_how_many_refreshes_are_sent() {
        let m = |n: u64| Duration::from_secs(n * 60);
        // The answer itself keeps the cache for five minutes.
        assert_eq!(KeepAlive::refreshes(m(0), REFRESH_EVERY, CACHE_LIFETIME), 0);
        assert_eq!(KeepAlive::refreshes(m(5), REFRESH_EVERY, CACHE_LIFETIME), 0);
        // At 4, 8 and 12 minutes: warm until 17.
        assert_eq!(
            KeepAlive::refreshes(m(15), REFRESH_EVERY, CACHE_LIFETIME),
            3
        );
        assert_eq!(
            KeepAlive::refreshes(m(60), REFRESH_EVERY, CACHE_LIFETIME),
            14
        );
    }

    #[test]
    fn answers_are_not_kept_warm_while_replies_keep_coming_late() {
        let mut state = State::default();
        let window = Duration::from_secs(15 * 60);
        let start = Instant::now();
        let mut now = start;
        let mut reply_after = |state: &mut State, key: u64, minutes: u64| {
            state.answered(key, now, window);
            now += Duration::from_secs(minutes * 60);
            state.next_request(key, now);
        };
        assert!(state.worth_it(), "with no replies yet, keep warm");
        reply_after(&mut state, 1, 30);
        reply_after(&mut state, 1, 40);
        assert!(state.worth_it(), "two late replies are not a pattern");
        reply_after(&mut state, 2, 20);
        assert!(!state.worth_it(), "the last three came after the window");
        // A reply inside the window, even one that needed no refresh,
        // turns it back on.
        reply_after(&mut state, 2, 2);
        assert!(state.worth_it());
        // A new request with no answer before it counts as no reply.
        state.next_request(3, now);
        assert_eq!(state.late.len(), LATE_REPLIES_TO_PAUSE);
    }

    #[test]
    fn a_refresh_asks_for_nothing_and_keeps_the_cached_prompt() {
        let answered = serde_json::json!({
            "model": "claude-sonnet-5-5", "max_tokens": 32000, "stream": true,
            "thinking": { "type": "adaptive" },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi",
                "cache_control": { "type": "ephemeral" } }] }],
        });
        let body = refresh_body(&answered);
        assert_eq!(body["max_tokens"], 0);
        assert!(body.get("stream").is_none());
        assert_eq!(body["messages"], answered["messages"]);
        assert_eq!(body["thinking"], answered["thinking"]);
    }

    #[test]
    fn a_conversation_is_known_by_its_prompt_and_first_message() {
        let a = [
            Message::system("You are Phaze."),
            Message::user("Fix the port"),
        ];
        let mut later = a.to_vec();
        later.push(Message::assistant("Done."));
        later.push(Message::user("Also log it"));
        assert_eq!(conversation_key(&a), conversation_key(&later));
        let other = [
            Message::system("You are Phaze."),
            Message::user("Add a test"),
        ];
        assert_ne!(conversation_key(&a), conversation_key(&other));
    }
}
