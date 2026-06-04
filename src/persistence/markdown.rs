//! Markdown-backed review session persistence.
//!
//! The visible format follows revdiff's "heading + body" style so the file is
//! useful to humans and agents. A hidden JSON block keeps tuicr-specific state
//! lossless for the MVP.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Result, TuicrError};
use crate::model::{LineRange, LineSide, ReviewSession};
use crate::persistence::storage;

const STATE_BEGIN: &str = "<!-- tuicr:session-json";
const STATE_END: &str = "-->";

pub fn load_session(path: &Path) -> Result<ReviewSession> {
    let contents = fs::read_to_string(path)?;
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
    let _ = writeln!(md, "# tuicr review");
    let _ = writeln!(md);
    if let Some(slug) = slug.as_deref() {
        let _ = writeln!(md, "session: {slug}");
    }
    let _ = writeln!(md, "source: {:?}", session.diff_source);
    let _ = writeln!(md, "repo: {}", session.repo_path.display());
    let _ = writeln!(md, "base: {}", session.base_commit);
    if let Some(key) = session.pr_session_key.as_ref() {
        let _ = writeln!(
            md,
            "pull_request: {}#{}",
            key.repository.display_name(),
            key.number
        );
        let _ = writeln!(md, "head: {}", key.short_head());
    }
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

    let _ = writeln!(md);
    let _ = writeln!(md, "## tuicr State");
    let _ = writeln!(md);
    let _ = writeln!(
        md,
        "This hidden block is used by tuicr to preserve review state."
    );
    let _ = writeln!(md);
    let _ = writeln!(md, "{STATE_BEGIN}");
    let _ = writeln!(md, "{}", serde_json::to_string_pretty(session)?);
    let _ = writeln!(md, "{STATE_END}");
    Ok(md)
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
}
