use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

#[derive(Clone)]
pub struct SessionRef {
    pub id: String,
    pub file: PathBuf,
    pub project_dir: PathBuf,
    pub mtime: SystemTime,
    /// Some(basename) when the session ran in a worktree rather than the main cwd.
    pub worktree: Option<String>,
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

/// Claude Code encodes a cwd into a project dir name by replacing every
/// non-alphanumeric character with '-'.
pub fn encode_path(p: &Path) -> String {
    p.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// All session transcripts for this folder, newest first, including sessions
/// running in git worktrees of this folder and in ~/.claude-worktrees clones.
pub fn discover_sessions(cwd: &Path) -> Vec<SessionRef> {
    let mut roots: Vec<PathBuf> = vec![cwd.to_path_buf()];

    if let Ok(out) = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(cwd)
        .output()
    {
        if out.status.success() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    roots.push(PathBuf::from(p.trim()));
                }
            }
        }
    }

    if let Some(base) = cwd.file_name().map(|s| s.to_string_lossy().to_string()) {
        if let Ok(rd) = fs::read_dir(home().join(".claude-worktrees")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with(&format!("{base}-")) && e.path().is_dir() {
                    roots.push(e.path());
                }
            }
        }
    }

    roots.sort();
    roots.dedup();

    let projects = home().join(".claude").join("projects");
    let mut out = Vec::new();
    for root in &roots {
        let pd = projects.join(encode_path(root));
        let Ok(rd) = fs::read_dir(&pd) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(md) = e.metadata() else { continue };
            if !md.is_file() || md.len() == 0 {
                continue;
            }
            let id = p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let worktree = if root.as_path() == cwd {
                None
            } else {
                root.file_name().map(|s| s.to_string_lossy().to_string())
            };
            out.push(SessionRef {
                id,
                file: p,
                project_dir: pd.clone(),
                mtime: md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                worktree,
            });
        }
    }
    out.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_paths_like_claude_code() {
        assert_eq!(
            encode_path(Path::new("/Users/rob/rob-workspace/claude-watch")),
            "-Users-rob-rob-workspace-claude-watch"
        );
        // dots and slashes both become dashes (real worktree example)
        assert_eq!(
            encode_path(Path::new("/Users/rob/.claude-worktrees/work-stupefied-easley")),
            "-Users-rob--claude-worktrees-work-stupefied-easley"
        );
    }

    #[test]
    fn worktree_prefix_matches_basename() {
        let base = "work";
        assert!("work-stupefied-easley".starts_with(&format!("{base}-")));
        assert!(!"workspace-other".starts_with(&format!("{base}-")));
    }
}
