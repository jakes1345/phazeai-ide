use crate::error::PhazeError;
use crate::tools::sandbox;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

pub struct ReadFileTool;

#[async_trait::async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Supports optional line range with offset and limit parameters."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("read_file", "Missing required parameter: path"))?;

        let resolved = sandbox::resolve_within_workspace("read_file", path)?;
        crate::privacy::tool_check("read_file", &resolved, path)?;
        let content = tokio::fs::read_to_string(&resolved).await.map_err(|e| {
            PhazeError::tool("read_file", format!("Failed to read '{}': {}", path, e))
        })?;
        crate::privacy::audit::note_file(&resolved);

        let offset = params
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|v| v.saturating_sub(1) as usize)
            .unwrap_or(0);

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let selected: Vec<String> = lines
            .into_iter()
            .skip(offset)
            .take(limit.unwrap_or(usize::MAX))
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", offset + i + 1, line))
            .collect();

        Ok(serde_json::json!({
            "content": selected.join("\n"),
            "path": path,
            "total_lines": total_lines,
            "lines_shown": selected.len(),
        }))
    }
}

pub struct WriteFileTool;

#[async_trait::async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        plan_write_file(&params).await?.commit().await
    }

    async fn plan_write(&self, params: &Value) -> Option<Result<FileWrite, PhazeError>> {
        Some(plan_write_file(params).await)
    }
}

/// What `write_file` with `params` would write, worked out without writing.
async fn plan_write_file(params: &Value) -> Result<FileWrite, PhazeError> {
    const TOOL: &str = "write_file";
    let path = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PhazeError::tool(TOOL, "Missing required parameter: path"))?;
    let content = params
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PhazeError::tool(TOOL, "Missing required parameter: content"))?;

    let target = writable_target(TOOL, path)?;
    let current = read_current(TOOL, path, &target).await?;
    let result = serde_json::json!({
        "path": path,
        "bytes_written": content.len(),
        "success": true,
    });
    Ok(FileWrite::new(
        TOOL,
        path,
        target,
        current,
        content.to_string(),
        result,
    ))
}

/// One file write worked out in full before anything is written: which file,
/// what it holds now, and exactly what it will hold.
///
/// `write_file` and `edit_file` are each "plan a `FileWrite`, then
/// [`commit`](Self::commit) it" (see [`Tool::plan_write`]). An agent with a
/// diff hook shows the plan for review and commits that same plan, so what
/// the reviewer approves is what is written.
#[derive(Debug)]
pub struct FileWrite {
    tool: &'static str,
    /// The path as the model gave it (echoed in errors and the result).
    path: String,
    /// Where the bytes go: `path` resolved inside the sandbox.
    target: PathBuf,
    /// The file's bytes when the write was planned; `None`: no file.
    current: Option<Vec<u8>>,
    /// The file's whole content once written.
    after: String,
    /// What the tool returns once the write is done.
    result: Value,
}

impl FileWrite {
    pub(crate) fn new(
        tool: &'static str,
        path: &str,
        target: PathBuf,
        current: Option<Vec<u8>>,
        after: String,
        result: Value,
    ) -> Self {
        Self {
            tool,
            path: path.to_string(),
            target,
            current,
            after,
            result,
        }
    }

    /// The file that will be written (resolved: no `..`, no symlinks).
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// What the file holds now, as text for a diff: empty when there is no
    /// file; bytes that are not UTF-8 show as U+FFFD.
    pub fn before(&self) -> Cow<'_, str> {
        match &self.current {
            Some(bytes) => String::from_utf8_lossy(bytes),
            None => Cow::Borrowed(""),
        }
    }

    /// Exactly what the file will hold.
    pub fn after(&self) -> &str {
        &self.after
    }

    /// Whether there is no file yet, so committing creates one.
    pub fn creates_file(&self) -> bool {
        self.current.is_none()
    }

    /// Whether committing changes anything on disk. Creating a file does,
    /// even an empty one.
    pub fn changes_file(&self) -> bool {
        self.current.as_deref() != Some(self.after.as_bytes())
    }

    /// Write [`after`](Self::after) to the file, but only if it still holds
    /// what it held when this write was planned. Otherwise nothing is written
    /// and the error says so: applying the change to what is there now would
    /// write something nobody reviewed, and writing `after` anyway would undo
    /// whatever changed the file (often the user, saving in the editor while
    /// the review was open).
    ///
    /// The path and the privacy policy are checked again too, as the tool
    /// checks them when it writes.
    pub async fn commit(self) -> ToolResult {
        let Self {
            tool,
            path,
            target,
            current,
            after,
            result,
        } = self;
        let now = writable_target(tool, &path)?;
        if now != target {
            return Err(PhazeError::tool(
                tool,
                format!(
                    "'{path}' now resolves to '{}', not '{}' as when this change was \
                     prepared; nothing was written",
                    now.display(),
                    target.display()
                ),
            ));
        }
        if read_current(tool, &path, &target).await? != current {
            return Err(changed(tool, &path));
        }
        if current.is_some() {
            tokio::fs::write(&target, &after)
                .await
                .map_err(|e| write_failed(tool, &path, e))?;
        } else {
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    PhazeError::tool(
                        tool,
                        format!("Failed to create directory '{}': {}", parent.display(), e),
                    )
                })?;
            }
            create_new(&target, after.as_bytes())
                .await
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::AlreadyExists => changed(tool, &path),
                    _ => write_failed(tool, &path, e),
                })?;
        }
        Ok(result)
    }
}

/// `path` resolved inside the sandbox and cleared by the privacy policy for
/// `tool` to write: the checks a file write passes when it is planned and
/// again when it is committed.
pub(crate) fn writable_target(tool: &str, path: &str) -> Result<PathBuf, PhazeError> {
    let target = sandbox::resolve_target_path(tool, path)?;
    refuse_dangling_link(tool, path, &target)?;
    crate::privacy::tool_check(tool, &target, path)?;
    crate::privacy::tool_check_protected(tool, &target, path)?;
    Ok(target)
}

/// A symbolic link whose destination does not exist. Writing there would
/// have to follow the link to a file nobody reviewed (possibly outside the
/// workspace), and the exclusive create used for new files fails on it, so
/// it is refused up front with the real reason.
fn refuse_dangling_link(tool: &str, path: &str, target: &Path) -> Result<(), PhazeError> {
    let is_link = std::fs::symlink_metadata(target).is_ok_and(|m| m.file_type().is_symlink());
    if !is_link || std::fs::metadata(target).is_ok() {
        return Ok(());
    }
    let points_to = std::fs::read_link(target)
        .map(|d| d.display().to_string())
        .unwrap_or_else(|_| "an unreadable destination".into());
    Err(PhazeError::tool(
        tool,
        format!(
            "'{path}' is a symbolic link to '{points_to}', which does not exist; nothing \
             was written. Write to the file the link should point to instead."
        ),
    ))
}

/// The bytes of the file at `target`, or `None` when there is none.
async fn read_current(
    tool: &str,
    path: &str,
    target: &Path,
) -> Result<Option<Vec<u8>>, PhazeError> {
    match tokio::fs::read(target).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(PhazeError::tool(
            tool,
            format!("Failed to read '{path}': {e}"),
        )),
    }
}

/// Create the file at `target` holding `bytes`; fails if it already exists.
async fn create_new(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .await?;
    file.write_all(bytes).await?;
    file.flush().await
}

fn changed(tool: &str, path: &str) -> PhazeError {
    PhazeError::tool(
        tool,
        format!(
            "'{path}' changed after this change to it was prepared, so nothing was written \
             (writing now would undo that change). Read the file again and redo the change."
        ),
    )
}

fn write_failed(tool: &str, path: &str, e: std::io::Error) -> PhazeError {
    PhazeError::tool(tool, format!("Failed to write '{path}': {e}"))
}
