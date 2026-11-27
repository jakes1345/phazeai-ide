//! Inline completion bench: real code typed one character at a time,
//! counting the completion requests the editor sends and their input tokens,
//! before and after `fim_memory` reuses answers.
//!
//! `cargo test -p phazeai-cli --test fim_bench -- --ignored --nocapture`
//!
//! - **The typing.** The token bench fixture's seven source files (411 lines
//!   of Rust) are typed from the top, one character at a time. The editor
//!   asks after a 300 ms pause in typing; a pause is counted after each
//!   newline and after the first character that ends a word, which is about
//!   where people pause.
//! - **Before.** Every pause with code before the cursor sends a request,
//!   as the editor did.
//! - **After.** Each pause goes through `SuggestionMemory::at_pause`; only
//!   `Ask` sends a request.
//! - **The model.** Scripted, so every run makes the same requests:
//!   - *right*: it always suggests the rest of the line that is really
//!     typed next (or the next line, at the start of one). Best case.
//!   - *wrong*: it always suggests something that is never typed. Worst
//!     case: typing through never applies.
//! - **Typos.** In the second pair of runs, every eighth word starts with
//!   two wrong letters, a pause, two backspaces and a pause, then the word.
//! - **Tokens** are the chat prompt's system and user messages (the prompt
//!   Claude gets, which has no fill-in-the-middle API), counted with `o200k`.

use phazeai_core::fim::{chat_prompt, window};
use phazeai_core::fim_memory::{AtPause, SuggestionMemory};

const FILES: &[&str] = &[
    include_str!("fixtures/token_bench/src/config.rs"),
    include_str!("fixtures/token_bench/src/lib.rs"),
    include_str!("fixtures/token_bench/src/log.rs"),
    include_str!("fixtures/token_bench/src/main.rs"),
    include_str!("fixtures/token_bench/src/routes.rs"),
    include_str!("fixtures/token_bench/src/server.rs"),
    include_str!("fixtures/token_bench/src/store.rs"),
];

#[derive(Clone, Copy)]
enum Model {
    Right,
    Wrong,
}

impl Model {
    /// The answer for a pause with `typed` before the cursor, while typing
    /// `text`. A typo makes the right model unsure: it answers nothing.
    fn answer(self, text: &str, typed: &str) -> String {
        match self {
            Model::Wrong => "todo!()".to_string(),
            Model::Right => {
                let Some(rest) = text.strip_prefix(typed) else {
                    return String::new();
                };
                let line = rest.split('\n').next().unwrap_or_default();
                let answer = if line.trim().is_empty() {
                    rest.split('\n').nth(1).unwrap_or_default()
                } else {
                    line
                };
                if answer.trim().is_empty() {
                    String::new()
                } else {
                    answer.to_string()
                }
            }
        }
    }
}

#[derive(Default, Debug)]
struct Count {
    requests: usize,
    tokens: usize,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Type `text` from the top and return what the editor sent, without and
/// with reuse.
fn replay(text: &str, model: Model, typos: bool) -> (Count, Count) {
    let bpe = tiktoken_rs::o200k_base().unwrap();
    let cost = |prefix: &str| {
        let (p, s) = window(prefix, "");
        let (system, user) = chat_prompt(p, s, "rs");
        bpe.encode_ordinary(&system).len() + bpe.encode_ordinary(&user).len()
    };
    let mut before = Count::default();
    let mut after = Count::default();
    let mut memory = SuggestionMemory::new();
    let mut pause = |prefix: &str, before: &mut Count, after: &mut Count| {
        if prefix.trim().is_empty() {
            return;
        }
        let tokens = cost(prefix);
        before.requests += 1;
        before.tokens += tokens;
        if memory.at_pause(prefix, "") == AtPause::Ask {
            after.requests += 1;
            after.tokens += tokens;
            let answer = model.answer(text, prefix);
            memory.remember(prefix, "", &answer, true);
        }
    };

    let mut typed = String::new();
    let mut words = 0usize;
    let mut previous = '\n';
    for c in text.chars() {
        if typos && is_word(c) && !is_word(previous) {
            words += 1;
            if words % 8 == 0 {
                let at = typed.len();
                typed.push_str("zq");
                pause(&typed, &mut before, &mut after);
                typed.truncate(at);
                pause(&typed, &mut before, &mut after);
            }
        }
        typed.push(c);
        if c == '\n' || (!is_word(c) && is_word(previous)) {
            pause(&typed, &mut before, &mut after);
        }
        previous = c;
    }
    (before, after)
}

#[test]
#[ignore = "a measurement, run with --ignored --nocapture"]
fn fim_bench() {
    let text = FILES.concat();
    println!("\n## Inline completion bench\n");
    println!(
        "{} lines of Rust typed one character at a time.\n",
        text.lines().count()
    );
    println!("| Typing | Model | Requests before | after | Input tokens before | after | Change |");
    println!("|---|---|--:|--:|--:|--:|--:|");
    for (typos, label) in [(false, "clean"), (true, "with typos")] {
        for (model, name) in [(Model::Right, "right"), (Model::Wrong, "wrong")] {
            let (b, a) = replay(&text, model, typos);
            println!(
                "| {label} | {name} | {} | {} | {} | {} | {:+.1}% |",
                b.requests,
                a.requests,
                b.tokens,
                a.tokens,
                100.0 * (a.tokens as f64 - b.tokens as f64) / b.tokens as f64
            );
        }
    }
}

/// The replay itself is sound: nothing is reused when nothing matches,
/// typing what the right model shows saves requests, and some backspaced
/// typos are answered from memory.
#[test]
fn the_replay_counts_what_it_should() {
    let text = FILES[0];
    let (b, a) = replay(text, Model::Wrong, false);
    assert_eq!(
        a.requests, b.requests,
        "nothing to reuse when nothing matches"
    );
    let (b, a) = replay(text, Model::Right, false);
    assert!(a.requests < b.requests / 2, "{b:?} {a:?}");
    let (b_clean, a_clean) = replay(text, Model::Wrong, false);
    let (b_typos, a_typos) = replay(text, Model::Wrong, true);
    let extra_pauses = b_typos.requests - b_clean.requests;
    // Each typo adds two pauses. Backspacing returns to the text of the
    // pause before it, when there was one, and that answer is reused.
    let extra_requests = a_typos.requests - a_clean.requests;
    assert!(
        extra_requests < extra_pauses,
        "{extra_requests} of {extra_pauses}"
    );
    assert!(
        extra_requests >= extra_pauses / 2,
        "{extra_requests} of {extra_pauses}"
    );
}
