//! Markdown-backed review session persistence.
//!
//! The visible format follows revdiff's "heading + body" style so the file is
//! useful to humans and agents. YAML frontmatter keeps tuicr-specific state
//! lossless.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Result, TuicrError};
use crate::model::{LineRange, LineSide, ReviewSession};
use crate::persistence::storage;

const FORMAT_ID: &str = "tuicr-review-md/v2";
const FRONTMATTER_DELIM: &str = "---";
const STATE_KEY: &str = "tuicr_session_json";
const STATE_BEGIN: &str = "<!-- tuicr:session-json";
const STATE_END: &str = "-->";
const COMPACT_JSON_WIDTH: usize = 100;

pub fn load_session(path: &Path) -> Result<ReviewSession> {
    let contents = fs::read_to_string(path)?;
    if let Some(json) = read_frontmatter_session_json(&contents, path)? {
        return serde_json::from_str(json.trim())
            .map_err(|err| TuicrError::CorruptedSession(err.to_string()));
    }

    read_legacy_hidden_session_json(&contents, path)
}

fn read_frontmatter_session_json(contents: &str, path: &Path) -> Result<Option<String>> {
    let mut lines = contents.lines();
    if lines.next() != Some(FRONTMATTER_DELIM) {
        return Ok(None);
    }

    let mut in_state = false;
    let mut state = Vec::new();
    let mut found_end = false;

    for line in lines {
        if line == FRONTMATTER_DELIM {
            found_end = true;
            break;
        }
        if in_state {
            if let Some(stripped) = line.strip_prefix("  ") {
                state.push(stripped.to_string());
                continue;
            }
            in_state = false;
        }
        if line.starts_with(&format!("{STATE_KEY}:")) {
            if !line.contains('|') {
                return Err(TuicrError::CorruptedSession(format!(
                    "markdown review file {} has non-block {STATE_KEY}",
                    path.display()
                )));
            }
            in_state = true;
        }
    }

    if !found_end {
        return Err(TuicrError::CorruptedSession(format!(
            "markdown review file {} has unterminated YAML frontmatter",
            path.display()
        )));
    }
    if state.is_empty() {
        return Err(TuicrError::CorruptedSession(format!(
            "markdown review file {} is missing {STATE_KEY}",
            path.display()
        )));
    }

    Ok(Some(state.join("\n")))
}

fn read_legacy_hidden_session_json(contents: &str, path: &Path) -> Result<ReviewSession> {
    let start = contents.find(STATE_BEGIN).ok_or_else(|| {
        TuicrError::CorruptedSession(format!(
            "markdown review file {} is missing tuicr session state",
            path.display()
        ))
    })?;
    let json_start = contents[start + STATE_BEGIN.len()..]
        .find('\n')
        .map(|offset| start + STATE_BEGIN.len() + offset + 1)
        .ok_or_else(|| {
            TuicrError::CorruptedSession(format!(
                "markdown review file {} has malformed tuicr session state",
                path.display()
            ))
        })?;
    let json_end = contents[json_start..]
        .find(STATE_END)
        .map(|offset| json_start + offset)
        .ok_or_else(|| {
            TuicrError::CorruptedSession(format!(
                "markdown review file {} has unterminated tuicr session state",
                path.display()
            ))
        })?;
    serde_json::from_str(contents[json_start..json_end].trim())
        .map_err(|err| TuicrError::CorruptedSession(err.to_string()))
}

pub fn save_session(path: &Path, session: &ReviewSession) -> Result<PathBuf> {
    let markdown = format_session(session)?;
    write_atomic(path, markdown.as_bytes())?;
    Ok(path.to_path_buf())
}

pub fn session_slug(session: &ReviewSession) -> Option<String> {
    storage::slug_for_session(session)
        .ok()
        .map(|slug| slug.to_string())
}

fn format_session(session: &ReviewSession) -> Result<String> {
    let mut md = String::new();
    let slug = session_slug(session);
    write_frontmatter(&mut md, session, slug.as_deref())?;
    let _ = writeln!(md, "# tuicr review");
    let _ = writeln!(md);

    if let Some(notes) = session.session_notes.as_deref().filter(|s| !s.is_empty()) {
        let _ = writeln!(md, "## Summary");
        let _ = writeln!(md);
        let _ = writeln!(md, "{notes}");
        let _ = writeln!(md);
    }

    let _ = writeln!(md, "## Comments");
    let mut wrote_comment = false;

    for comment in &session.review_comments {
        write_comment_record(&mut md, "Review", None, None, comment)?;
        wrote_comment = true;
    }

    let mut files: Vec<_> = session.files.iter().collect();
    files.sort_by_key(|(path, _)| path.to_string_lossy().to_string());
    for (path, review) in files {
        let path = path.to_string_lossy();
        for comment in &review.file_comments {
            write_comment_record(&mut md, &path, None, None, comment)?;
            wrote_comment = true;
        }

        let mut line_comments: Vec<_> = review.line_comments.iter().collect();
        line_comments.sort_by_key(|(line, _)| **line);
        for (line, comments) in line_comments {
            for comment in comments {
                let range = comment.line_range.or(Some(LineRange::single(*line)));
                write_comment_record(&mut md, &path, range, comment.side, comment)?;
                wrote_comment = true;
            }
        }
    }

    if !wrote_comment {
        let _ = writeln!(md);
        let _ = writeln!(md, "_No comments yet._");
    }

    Ok(md)
}

fn write_frontmatter(md: &mut String, session: &ReviewSession, slug: Option<&str>) -> Result<()> {
    let _ = writeln!(md, "{FRONTMATTER_DELIM}");
    write_yaml_string(md, "tuicr_format", FORMAT_ID);
    if let Some(slug) = slug {
        write_yaml_string(md, "session", slug);
    }
    write_yaml_string(
        md,
        "source",
        &serde_json::to_value(session.diff_source)?
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
    );
    write_yaml_string(md, "repo", &session.repo_path.display().to_string());
    write_yaml_string(md, "base", &session.base_commit);
    if let Some(key) = session.pr_session_key.as_ref() {
        write_yaml_string(
            md,
            "pull_request",
            &format!("{}#{}", key.repository.display_name(), key.number),
        );
        write_yaml_string(md, "head", &key.short_head());
    }
    let _ = writeln!(md, "{STATE_KEY}: |-");
    let compact = compact_pretty_json(&serde_json::to_value(session)?, 0);
    for line in compact.lines() {
        let _ = writeln!(md, "  {line}");
    }
    let _ = writeln!(md, "{FRONTMATTER_DELIM}");
    let _ = writeln!(md);
    Ok(())
}

fn write_yaml_string(md: &mut String, key: &str, value: &str) {
    let quoted = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
    let _ = writeln!(md, "{key}: {quoted}");
}

fn compact_pretty_json(value: &serde_json::Value, indent: usize) -> String {
    let compact = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    if compact.len() + indent <= COMPACT_JSON_WIDTH || value_is_scalar(value) {
        return compact;
    }

    match value {
        serde_json::Value::Array(values) => compact_pretty_array(values, indent),
        serde_json::Value::Object(entries) => compact_pretty_object(entries, indent),
        _ => compact,
    }
}

fn compact_pretty_array(values: &[serde_json::Value], indent: usize) -> String {
    if values.is_empty() {
        return "[]".to_string();
    }

    let child_indent = indent + 2;
    let mut out = String::from("[\n");
    for (idx, value) in values.iter().enumerate() {
        out.push_str(&" ".repeat(child_indent));
        out.push_str(&compact_pretty_json(value, child_indent));
        if idx + 1 < values.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(&" ".repeat(indent));
    out.push(']');
    out
}

fn compact_pretty_object(
    entries: &serde_json::Map<String, serde_json::Value>,
    indent: usize,
) -> String {
    if entries.is_empty() {
        return "{}".to_string();
    }

    let child_indent = indent + 2;
    let mut out = String::from("{\n");
    for (idx, (key, value)) in entries.iter().enumerate() {
        let key = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
        out.push_str(&" ".repeat(child_indent));
        out.push_str(&key);
        out.push_str(": ");
        out.push_str(&compact_pretty_json(value, child_indent));
        if idx + 1 < entries.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(&" ".repeat(indent));
    out.push('}');
    out
}

fn value_is_scalar(value: &serde_json::Value) -> bool {
    matches!(
        value,
        serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_)
    )
}

fn write_comment_record(
    md: &mut String,
    target: &str,
    range: Option<LineRange>,
    side: Option<LineSide>,
    comment: &crate::model::Comment,
) -> Result<()> {
    let side_label = match side {
        Some(LineSide::Old) => ", old",
        Some(LineSide::New) => ", new",
        None => "",
    };
    let ty = comment.comment_type.as_str();
    let location = match range {
        Some(range) if range.is_single() => format!("{target}:{}", range.start),
        Some(range) => format!("{target}:{}-{}", range.start, range.end),
        None => target.to_string(),
    };
    let _ = writeln!(md);
    let _ = writeln!(md, "## {location} ({ty}{side_label})");
    let _ = writeln!(md);
    let _ = writeln!(md, "{}", escape_body(&comment.content));
    Ok(())
}

fn escape_body(body: &str) -> String {
    if !body.contains("## ") {
        return body.to_string();
    }
    body.lines()
        .map(|line| {
            if line.trim_start().starts_with("## ") {
                format!(" {line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("review.md");
    let tmp_path = path.with_file_name(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&tmp_path, bytes)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Comment, CommentType, FileStatus, SessionDiffSource};

    #[test]
    fn should_round_trip_session_through_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.md");
        let mut session = ReviewSession::new(
            dir.path().to_path_buf(),
            "abc123".to_string(),
            Some("main".to_string()),
            SessionDiffSource::WorkingTree,
        );
        session.add_file(PathBuf::from("src/lib.rs"), FileStatus::Modified, 1);
        session
            .files
            .get_mut(&PathBuf::from("src/lib.rs"))
            .unwrap()
            .add_line_comment(
                42,
                Comment::new("fix this".to_string(), CommentType::Issue, None),
            );

        save_session(&path, &session).unwrap();
        let markdown = fs::read_to_string(&path).unwrap();
        assert!(markdown.starts_with("---\n"));
        assert!(markdown.contains("tuicr_format: \"tuicr-review-md/v2\""));
        assert!(markdown.contains("tuicr_session_json: |-\n"));
        assert!(!markdown.contains(STATE_BEGIN));
        assert!(markdown.contains("## src/lib.rs:42 (ISSUE)"));
        assert!(markdown.contains("fix this"));

        let restored = load_session(&path).unwrap();
        assert_eq!(
            restored
                .files
                .get(&PathBuf::from("src/lib.rs"))
                .unwrap()
                .line_comments
                .get(&42)
                .unwrap()[0]
                .content,
            "fix this"
        );
    }

    #[test]
    fn compact_pretty_json_keeps_small_containers_on_one_line() {
        let value: serde_json::Value =
            serde_json::from_str(r#"{"multi":"keys","fit":"same_line"}"#).unwrap();
        let formatted = compact_pretty_json(&value, 0);

        assert!(!formatted.contains('\n'));
        assert!(formatted.len() <= COMPACT_JSON_WIDTH);
        assert!(formatted.contains(r#""fit":"same_line""#));
        assert!(formatted.contains(r#""multi":"keys""#));
    }

    #[test]
    fn should_load_legacy_hidden_state_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.md");
        let session = ReviewSession::new(
            dir.path().to_path_buf(),
            "abc123".to_string(),
            Some("main".to_string()),
            SessionDiffSource::WorkingTree,
        );
        fs::write(
            &path,
            format!(
                "# tuicr review\n\n{STATE_BEGIN}\n{}\n{STATE_END}\n",
                serde_json::to_string_pretty(&session).unwrap()
            ),
        )
        .unwrap();

        let restored = load_session(&path).unwrap();
        assert_eq!(restored.base_commit, "abc123");
        assert_eq!(restored.branch_name.as_deref(), Some("main"));
    }
}
