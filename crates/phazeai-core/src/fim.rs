/// Fill-in-Middle (FIM) inline code completion.
///
/// Uses the active LLM with a tight chat prompt that emulates FIM behaviour.
/// The model sees the code before and after the cursor and must output ONLY
/// the text to insert at the cursor — no explanation, no markdown, no repeating
/// the surrounding code.

use crate::config::Settings;
use crate::llm::Message;

/// Request a FIM completion. Returns the suggested insertion text (may be
/// empty if the model returns nothing useful) or an error string.
///
/// `prefix` — everything in the file before the cursor.
/// `suffix` — everything in the file after the cursor.
/// `language` — language hint for the prompt ("rust", "python", …).
///
/// This is intentionally synchronous-from-a-thread: callers spawn a thread,
/// call this, then write the result back to a signal via `create_ext_action`.
pub async fn fim_complete(
    prefix: &str,
    suffix: &str,
    language: &str,
) -> Result<String, String> {
    let settings = Settings::load();
    let client = settings
        .build_llm_client()
        .map_err(|e| e.to_string())?;

    // Trim prefix to last ~40 lines and suffix to next ~20 lines so we
    // don't blow the context window on large files.
    let prefix_trimmed = last_n_lines(prefix, 40);
    let suffix_trimmed = first_n_lines(suffix, 20);

    let system = format!(
        "You are an expert {language} code completion engine. \
         The user will show you code with a <CURSOR> marker. \
         Output ONLY the text that should be inserted at <CURSOR> — \
         no explanation, no markdown, no code fences, no repetition of \
         surrounding lines. If no completion is appropriate, output nothing."
    );

    let user = format!(
        "Complete the {language} code at <CURSOR>:\n\n\
         ```{language}\n{prefix_trimmed}<CURSOR>{suffix_trimmed}\n```"
    );

    let messages = vec![
        Message::system(system),
        Message::user(user),
    ];

    let resp = client
        .chat(&messages, &[])
        .await
        .map_err(|e| e.to_string())?;

    let raw = resp.message.content.trim().to_string();

    // Strip any accidental markdown fences the model might output.
    let cleaned = raw
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    // Drop if the model repeated the prefix (common failure mode).
    if cleaned.is_empty() || prefix_trimmed.contains(cleaned) {
        return Ok(String::new());
    }

    // Limit to 3 lines so ghost text stays readable.
    let limited: String = cleaned.lines().take(3).collect::<Vec<_>>().join("\n");
    Ok(limited)
}

fn last_n_lines(s: &str, n: usize) -> &str {
    let mut count = 0;
    let mut pos = s.len();
    for (i, b) in s.bytes().enumerate().rev() {
        if b == b'\n' {
            count += 1;
            if count == n {
                pos = i + 1;
                break;
            }
        }
    }
    &s[pos..]
}

fn first_n_lines(s: &str, n: usize) -> &str {
    let mut count = 0;
    for (i, b) in s.bytes().enumerate() {
        if b == b'\n' {
            count += 1;
            if count == n {
                return &s[..i];
            }
        }
    }
    s
}
