//! What model calls cost, for the token and cost meters.
//!
//! Every price here was read on the provider's own pricing page on
//! November 10, 2026: Claude's on platform.claude.com/docs/en/about-claude/pricing;
//! OpenAI's on developers.openai.com/api/docs/pricing; Gemini's on
//! aistudio.google.com/docs/pricing; Groq's on console.groq.com/docs/models;
//! Together's on together.ai/pricing; Perplexity's on docs.perplexity.ai.
//! DeepSeek, xAI and Mistral Large 4 come from their pages as read for the
//! model table in November 2026. Where a page gives no cached-input price,
//! cached tokens are charged at the full input price, so the estimate errs
//! high, never low. A model not listed has no price ([`None`]) and the meters
//! show its tokens alone; the model table's older prices are not used. Local
//! providers are free, and so are GitHub Models.

use crate::llm::{ProviderId, Usage};

/// USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    /// A prompt token served from the provider's cache.
    pub cache_read: f64,
    /// A prompt token written to the cache (Anthropic's 5-minute writes,
    /// which is what PhazeAI asks for).
    pub cache_write: f64,
}

impl Price {
    const FREE: Price = Price {
        input: 0.0,
        output: 0.0,
        cache_read: 0.0,
        cache_write: 0.0,
    };
}

/// Anthropic's prices: (model id prefix, input, 5m cache write, cache read,
/// output). Dated ids (`claude-sonnet-4-5-20250929`) match their model.
const CLAUDE: &[(&str, f64, f64, f64, f64)] = &[
    ("claude-fable-5-2", 8.0, 10.0, 0.20, 40.0),
    ("claude-mythos-5-2", 8.0, 10.0, 0.20, 40.0),
    ("claude-fable-5-1", 10.0, 12.50, 0.25, 50.0),
    ("claude-mythos-5-1", 10.0, 12.50, 0.25, 50.0),
    ("claude-fable-5", 10.0, 12.50, 1.0, 50.0),
    ("claude-mythos-5", 10.0, 12.50, 1.0, 50.0),
    ("claude-opus-5-5", 4.0, 5.0, 0.20, 20.0),
    ("claude-opus-5", 5.0, 6.25, 0.50, 25.0),
    ("claude-opus-4-8", 5.0, 6.25, 0.50, 25.0),
    ("claude-opus-4-7", 5.0, 6.25, 0.50, 25.0),
    ("claude-opus-4-6", 5.0, 6.25, 0.50, 25.0),
    ("claude-opus-4-5", 5.0, 6.25, 0.50, 25.0),
    ("claude-opus-4-1", 15.0, 18.75, 1.50, 75.0),
    ("claude-opus-4", 15.0, 18.75, 1.50, 75.0),
    ("claude-sonnet-5-5", 2.0, 2.50, 0.10, 10.0),
    ("claude-sonnet-5", 2.0, 2.50, 0.20, 10.0),
    ("claude-sonnet-4-6", 3.0, 3.75, 0.30, 15.0),
    ("claude-sonnet-4-5", 3.0, 3.75, 0.30, 15.0),
    ("claude-sonnet-4", 3.0, 3.75, 0.30, 15.0),
    ("claude-haiku-4-5", 1.0, 1.25, 0.10, 5.0),
];

/// Claude Haiku 5.5 costs five times as much for a prompt over this many
/// tokens.
const HAIKU_5_5_LONG_PROMPT: u64 = 100_000;

fn claude_price(model: &str, prompt_tokens: u64) -> Option<Price> {
    // OpenRouter-style "anthropic/claude-..." names the same model.
    let model = model.strip_prefix("anthropic/").unwrap_or(model);
    if model.starts_with("claude-haiku-5-5") {
        let short = prompt_tokens <= HAIKU_5_5_LONG_PROMPT;
        return Some(if short {
            Price {
                input: 0.10,
                output: 0.50,
                cache_read: 0.01,
                cache_write: 0.125,
            }
        } else {
            Price {
                input: 0.50,
                output: 2.50,
                cache_read: 0.05,
                cache_write: 0.625,
            }
        });
    }
    // The id itself, or the id with a date (`-20250929`).
    let matches = |prefix: &str| {
        model.strip_prefix(prefix).is_some_and(|rest| {
            rest.is_empty()
                || rest
                    .strip_prefix('-')
                    .is_some_and(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()))
        })
    };
    CLAUDE.iter().find(|(prefix, ..)| matches(prefix)).map(
        |&(_, input, cache_write, cache_read, output)| Price {
            input,
            output,
            cache_read,
            cache_write,
        },
    )
}

/// Other providers' prices: (provider, model id, input, cached input, output,
/// and, for a price the provider has announced will change, the day it
/// changes as YYYYMMDD and the new input, cached input and output).
type Listed = (
    ProviderId,
    &'static str,
    f64,
    f64,
    f64,
    Option<(u32, f64, f64, f64)>,
);

fn listed() -> Vec<Listed> {
    use ProviderId::*;
    vec![
        (OpenAI, "gpt-6-astra", 10.0, 1.0, 50.0, None),
        (OpenAI, "gpt-6.1-sol", 2.0, 0.10, 10.0, None),
        (OpenAI, "gpt-6-sol", 2.0, 0.20, 10.0, None),
        (OpenAI, "gpt-6.1-terra", 1.0, 0.05, 5.0, None),
        (OpenAI, "gpt-6-luna", 0.10, 0.01, 0.50, None),
        (
            Gemini,
            "gemini-4-argon",
            2.0,
            0.10,
            10.0,
            Some((20261201, 4.0, 0.20, 20.0)),
        ),
        (
            Gemini,
            "gemini-3.8-flash",
            0.75,
            0.075,
            3.75,
            Some((20270101, 1.50, 0.15, 7.50)),
        ),
        (
            Gemini,
            "gemini-3.6-flash",
            0.75,
            0.075,
            3.75,
            Some((20270101, 1.50, 0.15, 7.50)),
        ),
        (Gemini, "gemini-3.5-flash-lite", 0.30, 0.03, 2.50, None),
        (Gemini, "gemini-3.1-flash-lite", 0.25, 0.025, 1.50, None),
        (Gemini, "gemini-2.5-flash", 0.30, 0.03, 2.50, None),
        (Gemini, "gemini-2.5-flash-lite", 0.10, 0.01, 0.40, None),
        (Groq, "openai/gpt-oss-120b", 0.15, 0.075, 0.60, None),
        (Groq, "openai/gpt-oss-20b", 0.075, 0.075, 0.30, None),
        (Groq, "qwen/qwen3.8-27b", 0.80, 0.80, 4.0, None),
        (Together, "moonshotai/Kimi-K3", 3.0, 0.30, 15.0, None),
        (Together, "moonshotai/Kimi-K3.1-Pro", 2.0, 0.20, 10.0, None),
        (Together, "zai-org/GLM-5.3", 1.40, 0.26, 4.40, None),
        (
            Together,
            "deepseek-ai/DeepSeek-V4-Pro-0813",
            1.32,
            0.13,
            3.96,
            None,
        ),
        (
            Together,
            "deepseek-ai/DeepSeek-V4.1-Flash",
            0.30,
            0.006,
            1.20,
            None,
        ),
        (Together, "Qwen/Qwen3.7-Plus", 0.32, 0.32, 1.28, None),
        (Together, "Qwen/Qwen3.7-Max", 2.50, 0.50, 7.50, None),
        (Together, "openai/gpt-oss-120b", 0.15, 0.15, 0.60, None),
        (Perplexity, "sonar", 0.25, 0.0625, 2.50, None),
        // DeepSeek's peak-hour rates (off-peak is half).
        (DeepSeek, "deepseek-flash", 0.30, 0.30, 1.20, None),
        (DeepSeek, "deepseek-v4-pro", 1.32, 1.32, 3.96, None),
        (Xai, "grok-build-0.1", 1.0, 1.0, 2.0, None),
        (Xai, "grok-4.7", 2.0, 2.0, 6.0, None),
        (Xai, "grok-4.3", 1.25, 1.25, 2.50, None),
        (Mistral, "mistral-large-4", 0.70, 0.70, 2.10, None),
    ]
}

/// Gemini 2.5 Pro costs more for a prompt over 200K tokens.
const GEMINI_PRO_LONG_PROMPT: u64 = 200_000;

/// Today as YYYYMMDD (UTC).
fn today() -> u32 {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400) as i64;
    ymd(days)
}

/// Days since 1970-01-01 as YYYYMMDD (Howard Hinnant's civil_from_days).
fn ymd(days: i64) -> u32 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y * 10_000 + m * 100 + d) as u32
}

fn listed_price(provider: &ProviderId, model: &str, prompt_tokens: u64, on: u32) -> Option<Price> {
    if *provider == ProviderId::Gemini && model == "gemini-2.5-pro" {
        let short = prompt_tokens <= GEMINI_PRO_LONG_PROMPT;
        let (input, cache_read, output) = if short {
            (1.25, 0.125, 10.0)
        } else {
            (2.50, 0.25, 15.0)
        };
        return Some(Price {
            input,
            output,
            cache_read,
            cache_write: input,
        });
    }
    listed()
        .into_iter()
        .find(|(p, id, ..)| p == provider && *id == model)
        .map(|(_, _, input, cache_read, output, change)| {
            let (input, cache_read, output) = match change {
                Some((day, i, c, o)) if on >= day => (i, c, o),
                _ => (input, cache_read, output),
            };
            Price {
                input,
                output,
                cache_read,
                cache_write: input,
            }
        })
}

/// What `model` at `provider` costs for a call whose prompt is
/// `prompt_tokens` long (some prices depend on it). `None`: the price is not
/// known.
pub fn price(provider: &ProviderId, model: &str, prompt_tokens: u64) -> Option<Price> {
    price_on(provider, model, prompt_tokens, today())
}

fn price_on(provider: &ProviderId, model: &str, prompt_tokens: u64, on: u32) -> Option<Price> {
    if provider.is_local() || *provider == ProviderId::GithubModels {
        return Some(Price::FREE);
    }
    if matches!(provider, ProviderId::Claude | ProviderId::OpenRouter) {
        if let Some(price) = claude_price(model, prompt_tokens) {
            return Some(price);
        }
    }
    listed_price(provider, model, prompt_tokens, on)
}

/// The USD cost of one call's `usage` (whose `input_tokens` include its cache
/// reads and writes), or `None` when the price is not known.
pub fn call_cost(provider: &ProviderId, model: &str, usage: &Usage) -> Option<f64> {
    let input = u64::from(usage.input_tokens);
    let price = price(provider, model, input)?;
    let read = u64::from(usage.cache_read_tokens).min(input);
    let write = u64::from(usage.cache_write_tokens).min(input - read);
    let fresh = input - read - write;
    let dollars = fresh as f64 * price.input
        + read as f64 * price.cache_read
        + write as f64 * price.cache_write
        + f64::from(usage.output_tokens) * price.output;
    Some(dollars / 1_000_000.0)
}

/// Token and cost totals over many calls, for a meter.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Meter {
    /// Every prompt token, cached ones included.
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// The cost of the calls whose price is known.
    pub cost: f64,
    /// Calls whose price is not known (not in `cost`).
    pub unpriced_calls: u64,
}

impl Meter {
    /// Count one call of `model` at `provider`; `None`: the model is not
    /// known, so the call is counted without a price.
    pub fn add(&mut self, billed: Option<(&ProviderId, &str)>, usage: &Usage) {
        self.input += u64::from(usage.input_tokens);
        self.output += u64::from(usage.output_tokens);
        self.cache_read += u64::from(usage.cache_read_tokens);
        self.cache_write += u64::from(usage.cache_write_tokens);
        match billed.and_then(|(provider, model)| call_cost(provider, model, usage)) {
            Some(c) => self.cost += c,
            None => self.unpriced_calls += 1,
        }
    }

    /// The share of prompt tokens served from the cache, 0 to 100.
    pub fn cached_percent(&self) -> u64 {
        (self.cache_read * 100).checked_div(self.input).unwrap_or(0)
    }

    /// One line for a status bar, e.g. "12.3k in · 61% cached · 1.1k out ·
    /// $0.042". The cost is left out when no call had a known price, and
    /// marked "+" when some calls had none.
    pub fn summary(&self) -> String {
        let tokens = |n: u64| {
            if n >= 1_000_000 {
                format!("{:.1}M", n as f64 / 1_000_000.0)
            } else if n >= 1_000 {
                format!("{:.1}k", n as f64 / 1_000.0)
            } else {
                n.to_string()
            }
        };
        let mut out = format!("{} in", tokens(self.input));
        if self.cache_read > 0 {
            out.push_str(&format!(" · {}% cached", self.cached_percent()));
        }
        out.push_str(&format!(" · {} out", tokens(self.output)));
        let priced = self.unpriced_calls == 0 || self.cost > 0.0;
        if priced {
            let more = if self.unpriced_calls > 0 { "+" } else { "" };
            out.push_str(&format!(" · {}{more}", dollars(self.cost)));
        }
        out
    }
}

/// `$0.0042`, `$0.042`, `$1.23`: enough digits to see a small spend move.
pub fn dollars(amount: f64) -> String {
    if amount == 0.0 {
        "$0".to_string()
    } else if amount < 0.01 {
        format!("${amount:.4}")
    } else if amount < 1.0 {
        format!("${amount:.3}")
    } else {
        format!("${amount:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32, read: u32, write: u32) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: read,
            cache_write_tokens: write,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn claude_prices_come_from_anthropics_table() {
        let p = price(&ProviderId::Claude, "claude-opus-5-5", 1_000).unwrap();
        assert_eq!(
            (p.input, p.output, p.cache_read, p.cache_write),
            (4.0, 20.0, 0.20, 5.0)
        );
        // Dated ids match; a longer prefix wins over a shorter one.
        let s = price(&ProviderId::Claude, "claude-sonnet-4-5-20250929", 0).unwrap();
        assert_eq!(s.input, 3.0);
        assert_eq!(
            price(&ProviderId::Claude, "claude-opus-4-6", 0)
                .unwrap()
                .input,
            5.0
        );
        assert_eq!(
            price(&ProviderId::Claude, "claude-opus-4-1-20250805", 0)
                .unwrap()
                .input,
            15.0
        );
        assert_eq!(
            price(&ProviderId::Claude, "claude-opus-5", 0)
                .unwrap()
                .input,
            5.0
        );
        assert_eq!(
            price(&ProviderId::Claude, "claude-sonnet-5", 0)
                .unwrap()
                .cache_read,
            0.20
        );
        // Through OpenRouter, the same model.
        assert_eq!(
            price(&ProviderId::OpenRouter, "anthropic/claude-sonnet-5-5", 0)
                .unwrap()
                .input,
            2.0
        );
        assert!(price(&ProviderId::Claude, "claude-unknown-9", 0).is_none());
        // A newer model is not priced as an older one that shares a prefix.
        assert!(price(&ProviderId::Claude, "claude-opus-4-9", 0).is_none());
    }

    #[test]
    fn haiku_5_5_costs_more_past_100k_prompt_tokens() {
        let short = price(&ProviderId::Claude, "claude-haiku-5-5", 100_000).unwrap();
        let long = price(&ProviderId::Claude, "claude-haiku-5-5", 100_001).unwrap();
        assert_eq!((short.input, short.output), (0.10, 0.50));
        assert_eq!((long.input, long.output), (0.50, 2.50));
    }

    #[test]
    fn cache_reads_and_writes_are_priced_apart() {
        // 10k prompt: 8k read from cache, 1k written, 1k fresh; 500 out.
        let c = call_cost(
            &ProviderId::Claude,
            "claude-sonnet-5-5",
            &usage(10_000, 500, 8_000, 1_000),
        )
        .unwrap();
        let expected = (1_000.0 * 2.0 + 8_000.0 * 0.10 + 1_000.0 * 2.50 + 500.0 * 10.0) / 1e6;
        assert!(close(c, expected), "{c} vs {expected}");
    }

    #[test]
    fn other_providers_use_their_cached_price_or_else_the_full_input_price() {
        // OpenAI publishes one: $2 in, $0.10 cached.
        let c = call_cost(
            &ProviderId::OpenAI,
            "gpt-6.1-sol",
            &usage(1_000_000, 0, 900_000, 0),
        )
        .unwrap();
        assert!(close(c, 0.1 * 2.0 + 0.9 * 0.10), "{c}");
        // DeepSeek's page gives none here, so a cache read costs full price.
        let c = call_cost(
            &ProviderId::DeepSeek,
            "deepseek-flash",
            &usage(1_000_000, 0, 900_000, 0),
        )
        .unwrap();
        assert!(close(c, 0.30));
    }

    #[test]
    fn announced_price_changes_apply_from_their_day() {
        let before = price_on(&ProviderId::Gemini, "gemini-4-argon", 0, 20261130).unwrap();
        let after = price_on(&ProviderId::Gemini, "gemini-4-argon", 0, 20261201).unwrap();
        assert_eq!((before.input, before.output), (2.0, 10.0));
        assert_eq!((after.input, after.output), (4.0, 20.0));
        let pro = |n| {
            price_on(&ProviderId::Gemini, "gemini-2.5-pro", n, 20261110)
                .unwrap()
                .input
        };
        assert_eq!((pro(200_000), pro(200_001)), (1.25, 2.50));
    }

    #[test]
    fn days_become_dates() {
        assert_eq!(ymd(0), 19700101);
        assert_eq!(ymd(20_400), 20251108);
        assert_eq!(ymd(11_016), 20000229);
        assert!(today() >= 20261110);
    }

    #[test]
    fn stale_model_table_prices_are_not_used() {
        // gpt-4o is in the model table, but not on OpenAI's pricing page now.
        assert!(price(&ProviderId::OpenAI, "gpt-4o", 0).is_none());
        assert_eq!(
            price(&ProviderId::GithubModels, "gpt-4o", 0),
            Some(Price::FREE)
        );
    }

    #[test]
    fn local_models_are_free_and_unknown_ones_unpriced() {
        assert_eq!(
            call_cost(&ProviderId::Ollama, "anything", &usage(5_000, 5_000, 0, 0)),
            Some(0.0)
        );
        assert_eq!(
            call_cost(&ProviderId::OpenAI, "not-a-model", &usage(1, 1, 0, 0)),
            None
        );
    }

    #[test]
    fn the_meter_sums_calls_and_says_when_some_had_no_price() {
        let mut m = Meter::default();
        m.add(
            Some((&ProviderId::Claude, "claude-opus-5-5")),
            &usage(20_000, 1_000, 15_000, 0),
        );
        assert_eq!(m.cached_percent(), 75);
        assert_eq!(m.summary(), "20.0k in · 75% cached · 1.0k out · $0.043");
        m.add(None, &usage(1_000, 10, 0, 0));
        assert!(m.summary().ends_with("$0.043+"), "{}", m.summary());

        let mut unknown = Meter::default();
        unknown.add(
            Some((&ProviderId::OpenAI, "not-a-model")),
            &usage(900, 20, 0, 0),
        );
        assert_eq!(unknown.summary(), "900 in · 20 out");
    }

    #[test]
    fn dollars_keep_small_spends_visible() {
        assert_eq!(dollars(0.0), "$0");
        assert_eq!(dollars(0.00042), "$0.0004");
        assert_eq!(dollars(0.042), "$0.042");
        assert_eq!(dollars(3.456), "$3.46");
    }
}
