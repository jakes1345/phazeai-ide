//! Reading a GGUF file's header: the few numbers that say how much memory a
//! model needs per token of context.
//!
//! A GGUF file starts with `GGUF`, a version, a tensor count and a list of
//! typed key/value pairs. Only the header is read: big values (the tokenizer's
//! vocabulary arrays) are skipped without being loaded, so this takes a few
//! milliseconds even on a 40 GB file.

use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// What the header says about the model's shape.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GgufMeta {
    /// `general.architecture`, e.g. `llama`, `qwen2`.
    pub architecture: String,
    /// `general.name`, when set.
    pub name: Option<String>,
    pub block_count: Option<u64>,
    pub head_count: Option<u64>,
    /// Largest per-layer KV head count (some models list one per layer).
    pub head_count_kv: Option<u64>,
    pub embedding_length: Option<u64>,
    pub key_length: Option<u64>,
    pub value_length: Option<u64>,
    /// The context the model was trained for.
    pub context_length: Option<u64>,
}

impl GgufMeta {
    /// f16 KV-cache bytes per token of context:
    /// `layers × kv_heads × (key_len + value_len) × 2 bytes`. `None` when
    /// the header lacks the numbers.
    pub fn kv_bytes_per_token(&self) -> Option<u64> {
        let layers = self.block_count?;
        let heads = self.head_count?.max(1);
        let kv_heads = self.head_count_kv.unwrap_or(heads);
        let head_dim = self.embedding_length.map(|e| e / heads).filter(|d| *d > 0);
        let k = self.key_length.or(head_dim)?;
        let v = self.value_length.or(head_dim).unwrap_or(k);
        Some(layers * kv_heads * (k + v) * 2)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GgufError {
    #[error("not a GGUF file")]
    NotGguf,
    #[error("GGUF version {0} is not supported")]
    Version(u32),
    #[error("the GGUF header is damaged: {0}")]
    Damaged(&'static str),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

const MAX_KEY: u64 = 64 * 1024;
const MAX_STRING: u64 = 64 * 1024 * 1024;

struct Reader<R> {
    r: R,
}

impl<R: Read + Seek> Reader<R> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], GgufError> {
        let mut b = [0u8; N];
        self.r.read_exact(&mut b)?;
        Ok(b)
    }
    fn u32(&mut self) -> Result<u32, GgufError> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    fn u64(&mut self) -> Result<u64, GgufError> {
        Ok(u64::from_le_bytes(self.bytes()?))
    }
    fn string(&mut self, max: u64) -> Result<String, GgufError> {
        let len = self.u64()?;
        if len > max {
            return Err(GgufError::Damaged("a string is impossibly long"));
        }
        let mut b = vec![0u8; len as usize];
        self.r.read_exact(&mut b)?;
        Ok(String::from_utf8_lossy(&b).into_owned())
    }
    fn skip(&mut self, n: u64) -> Result<(), GgufError> {
        let n = i64::try_from(n).map_err(|_| GgufError::Damaged("a value is impossibly long"))?;
        self.r.seek(SeekFrom::Current(n))?;
        Ok(())
    }
}

/// Size in bytes of a fixed-size value type, `None` for strings and arrays.
fn fixed_size(ty: u32) -> Option<u64> {
    match ty {
        0 | 1 | 7 => Some(1),
        2 | 3 => Some(2),
        4..=6 => Some(4),
        10..=12 => Some(8),
        _ => None,
    }
}

/// A value read as an unsigned integer, when it is one (an array gives its
/// largest element).
enum Value {
    Int(u64),
    Text(String),
    Other,
}

fn read_int<R: Read + Seek>(r: &mut Reader<R>, ty: u32) -> Result<Option<u64>, GgufError> {
    Ok(match ty {
        0 => Some(r.bytes::<1>()?[0] as u64),
        1 => Some((r.bytes::<1>()?[0] as i8).max(0) as u64),
        2 => Some(u16::from_le_bytes(r.bytes()?) as u64),
        3 => Some(i16::from_le_bytes(r.bytes()?).max(0) as u64),
        4 => Some(r.u32()? as u64),
        5 => Some(i32::from_le_bytes(r.bytes()?).max(0) as u64),
        10 => Some(r.u64()?),
        11 => Some(i64::from_le_bytes(r.bytes()?).max(0) as u64),
        _ => None,
    })
}

fn read_value<R: Read + Seek>(r: &mut Reader<R>, ty: u32, want: bool) -> Result<Value, GgufError> {
    match ty {
        8 => {
            if want {
                Ok(Value::Text(r.string(MAX_STRING)?))
            } else {
                let len = r.u64()?;
                r.skip(len)?;
                Ok(Value::Other)
            }
        }
        9 => {
            let item = r.u32()?;
            let n = r.u64()?;
            if let Some(size) = fixed_size(item) {
                if want && matches!(item, 0..=5 | 10 | 11) && n <= 4096 {
                    let mut max = 0;
                    for _ in 0..n {
                        max = max.max(read_int(r, item)?.unwrap_or(0));
                    }
                    return Ok(Value::Int(max));
                }
                r.skip(
                    size.checked_mul(n)
                        .ok_or(GgufError::Damaged("an array is impossibly long"))?,
                )?;
            } else {
                for _ in 0..n {
                    read_value(r, item, false)?;
                }
            }
            Ok(Value::Other)
        }
        _ => {
            if want {
                if let Some(v) = read_int(r, ty)? {
                    return Ok(Value::Int(v));
                }
                r.skip(fixed_size(ty).ok_or(GgufError::Damaged("unknown value type"))?)?;
                Ok(Value::Other)
            } else {
                r.skip(fixed_size(ty).ok_or(GgufError::Damaged("unknown value type"))?)?;
                Ok(Value::Other)
            }
        }
    }
}

/// Read the header of the GGUF file at `path`.
pub fn read_meta(path: &Path) -> Result<GgufMeta, GgufError> {
    let file = std::fs::File::open(path)?;
    read_meta_from(BufReader::with_capacity(64 * 1024, file))
}

pub fn read_meta_from<R: Read + Seek>(r: R) -> Result<GgufMeta, GgufError> {
    let mut r = Reader { r };
    if &r.bytes::<4>()? != b"GGUF" {
        return Err(GgufError::NotGguf);
    }
    let version = r.u32()?;
    if !(2..=3).contains(&version) {
        return Err(GgufError::Version(version));
    }
    let _tensors = r.u64()?;
    let kv_count = r.u64()?;
    if kv_count > 1_000_000 {
        return Err(GgufError::Damaged("impossibly many keys"));
    }
    let mut meta = GgufMeta::default();
    // Arch-specific keys (`<arch>.block_count`) are matched by suffix, since
    // `general.architecture` may come after them.
    let mut by_suffix: Vec<(String, u64)> = Vec::new();
    for _ in 0..kv_count {
        let key = r.string(MAX_KEY)?;
        let ty = r.u32()?;
        let wanted = key == "general.architecture"
            || key == "general.name"
            || [
                ".block_count",
                ".attention.head_count",
                ".attention.head_count_kv",
                ".embedding_length",
                ".attention.key_length",
                ".attention.value_length",
                ".context_length",
            ]
            .iter()
            .any(|s| key.ends_with(s));
        match read_value(&mut r, ty, wanted)? {
            Value::Text(t) if key == "general.architecture" => meta.architecture = t,
            Value::Text(t) if key == "general.name" => meta.name = Some(t),
            Value::Int(v) => by_suffix.push((key, v)),
            _ => {}
        }
    }
    let arch = meta.architecture.clone();
    let get = |suffix: &str| {
        by_suffix
            .iter()
            .find(|(k, _)| *k == format!("{arch}{suffix}"))
            .map(|(_, v)| *v)
    };
    meta.block_count = get(".block_count");
    meta.head_count = get(".attention.head_count");
    meta.head_count_kv = get(".attention.head_count_kv");
    meta.embedding_length = get(".embedding_length");
    meta.key_length = get(".attention.key_length");
    meta.value_length = get(".attention.value_length");
    meta.context_length = get(".context_length");
    Ok(meta)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A GGUF header with `kvs` as (key, type, encoded value) entries.
    pub(crate) fn header(kvs: &[(&str, u32, Vec<u8>)]) -> Vec<u8> {
        let mut b = b"GGUF".to_vec();
        b.extend(3u32.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        b.extend((kvs.len() as u64).to_le_bytes());
        for (k, ty, v) in kvs {
            b.extend((k.len() as u64).to_le_bytes());
            b.extend(k.as_bytes());
            b.extend(ty.to_le_bytes());
            b.extend(v);
        }
        b
    }

    pub(crate) fn s(text: &str) -> Vec<u8> {
        let mut v = (text.len() as u64).to_le_bytes().to_vec();
        v.extend(text.as_bytes());
        v
    }

    pub(crate) fn u32v(n: u32) -> Vec<u8> {
        n.to_le_bytes().to_vec()
    }

    /// A Qwen2.5-7B-shaped header: 28 layers, 28 heads, 4 KV heads, 3584 wide.
    pub(crate) fn qwen7b() -> Vec<u8> {
        // A 3-entry string array (the vocabulary) and a float array to skip.
        let mut vocab = 8u32.to_le_bytes().to_vec();
        vocab.extend(3u64.to_le_bytes());
        for t in ["a", "bb", "ccc"] {
            vocab.extend(s(t));
        }
        let mut scores = 6u32.to_le_bytes().to_vec();
        scores.extend(3u64.to_le_bytes());
        for f in [0.5f32, 1.0, 2.0] {
            scores.extend(f.to_le_bytes());
        }
        header(&[
            ("general.name", 8, s("Qwen2.5 7B Instruct")),
            ("qwen2.block_count", 4, u32v(28)),
            ("tokenizer.ggml.tokens", 9, vocab),
            ("tokenizer.ggml.scores", 9, scores),
            ("qwen2.context_length", 4, u32v(32768)),
            ("qwen2.embedding_length", 4, u32v(3584)),
            ("qwen2.attention.head_count", 4, u32v(28)),
            ("qwen2.attention.head_count_kv", 4, u32v(4)),
            ("general.architecture", 8, s("qwen2")),
            ("general.file_type", 4, u32v(15)),
            (
                "qwen2.rope.freq_base",
                6,
                1_000_000f32.to_le_bytes().to_vec(),
            ),
            ("general.flag", 7, vec![1]),
            ("general.big", 10, 7u64.to_le_bytes().to_vec()),
        ])
    }

    #[test]
    fn the_header_gives_the_kv_cache_size() {
        let meta = read_meta_from(std::io::Cursor::new(qwen7b())).unwrap();
        assert_eq!(meta.architecture, "qwen2");
        assert_eq!(meta.name.as_deref(), Some("Qwen2.5 7B Instruct"));
        assert_eq!(meta.block_count, Some(28));
        assert_eq!(meta.head_count_kv, Some(4));
        assert_eq!(meta.context_length, Some(32768));
        // The same figure the curated catalog uses for this model.
        assert_eq!(meta.kv_bytes_per_token(), Some(2 * 28 * 4 * 128 * 2));
    }

    #[test]
    fn per_layer_kv_heads_and_explicit_key_lengths_are_used() {
        let mut per_layer = 4u32.to_le_bytes().to_vec();
        per_layer.extend(3u64.to_le_bytes());
        for n in [0u32, 8, 8] {
            per_layer.extend(n.to_le_bytes());
        }
        let bytes = header(&[
            ("general.architecture", 8, s("gemma3")),
            ("gemma3.block_count", 4, u32v(3)),
            ("gemma3.attention.head_count", 4, u32v(16)),
            ("gemma3.attention.head_count_kv", 9, per_layer),
            ("gemma3.attention.key_length", 4, u32v(256)),
            ("gemma3.attention.value_length", 4, u32v(256)),
        ]);
        let meta = read_meta_from(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(meta.head_count_kv, Some(8));
        assert_eq!(meta.kv_bytes_per_token(), Some(3 * 8 * 512 * 2));
    }

    #[test]
    fn other_files_and_damaged_headers_are_refused() {
        assert!(matches!(
            read_meta_from(std::io::Cursor::new(b"PK\x03\x04rest".to_vec())),
            Err(GgufError::NotGguf)
        ));
        let mut cut = qwen7b();
        cut.truncate(60);
        assert!(read_meta_from(std::io::Cursor::new(cut)).is_err());
        let mut huge = b"GGUF".to_vec();
        huge.extend(3u32.to_le_bytes());
        huge.extend(0u64.to_le_bytes());
        huge.extend(1u64.to_le_bytes());
        huge.extend(u64::MAX.to_le_bytes());
        assert!(matches!(
            read_meta_from(std::io::Cursor::new(huge)),
            Err(GgufError::Damaged(_))
        ));
        assert_eq!(GgufMeta::default().kv_bytes_per_token(), None);
    }
}
