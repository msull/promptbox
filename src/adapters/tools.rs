//! Tool discovery and execution. Each tool is a folder under the data
//! directory's `tools/` holding a `tool.json` manifest and whatever it
//! runs. Scripts get [`ToolInput`] as JSON on stdin and report with JSON
//! (or plain text) on stdout; a non-zero exit is an error.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::ports::tools::{ToolInput, ToolManifest, ToolOutcome, ToolRunner};

pub const MANIFEST_FILE: &str = "tool.json";
const TIMEOUT: Duration = Duration::from_secs(30);

/// Loads every `<dir>/*/tool.json`, sorted by folder name. Folders with a
/// broken manifest are reported, not fatal.
pub fn load_manifests(dir: &Path) -> (Vec<ToolManifest>, Vec<String>) {
    let mut tools = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (tools, problems);
    };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join(MANIFEST_FILE).is_file())
        .collect();
    folders.sort();
    for folder in folders {
        match load_manifest(&folder) {
            Ok(m) => tools.push(m),
            Err(e) => problems.push(e),
        }
    }
    (tools, problems)
}

fn load_manifest(folder: &Path) -> Result<ToolManifest, String> {
    let path = folder.join(MANIFEST_FILE);
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut m: ToolManifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if m.name.trim().is_empty() || !m.name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(format!(
            "{}: name must be letters, digits, and underscores",
            path.display()
        ));
    }
    if m.command.is_empty() {
        return Err(format!("{}: command is empty", path.display()));
    }
    if !m.parameters.is_object() {
        return Err(format!(
            "{}: parameters must be a JSON object",
            path.display()
        ));
    }
    m.dir = folder.to_path_buf();
    Ok(m)
}

/// Runs the manifest's command as a child process.
#[derive(Default)]
pub struct ProcessToolRunner;

impl ToolRunner for ProcessToolRunner {
    fn run(&self, tool: &ToolManifest, input: &ToolInput) -> Result<ToolOutcome, String> {
        let program = resolve_program(&tool.dir, &tool.command[0]);
        let mut child = Command::new(&program)
            .args(&tool.command[1..])
            .current_dir(&tool.dir)
            .env("PROMPTBOX_PROMPT", &input.prompt)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", program.display()))?;
        let payload = serde_json::to_vec(input).map_err(|e| e.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            // A script that never reads stdin closes the pipe; that is fine.
            let _ = stdin.write_all(&payload);
        }
        let output = wait_with_timeout(child, TIMEOUT)?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if !output.status.success() {
            let detail = if stderr.is_empty() { stdout } else { stderr };
            return Err(format!(
                "{} failed ({}): {detail}",
                tool.name, output.status
            ));
        }
        Ok(parse_outcome(&stdout))
    }
}

/// A bare name is looked up on PATH; anything with a separator is relative
/// to the tool folder.
fn resolve_program(dir: &Path, program: &str) -> PathBuf {
    let p = Path::new(program);
    if p.is_absolute()
        || p.components().count() == 1 && !program.contains(std::path::MAIN_SEPARATOR)
    {
        if p.components().count() == 1 && dir.join(p).is_file() {
            return dir.join(p);
        }
        return p.to_path_buf();
    }
    dir.join(p)
}

/// JSON with the outcome's fields, or any other text as the message.
fn parse_outcome(stdout: &str) -> ToolOutcome {
    if stdout.starts_with('{')
        && let Ok(o) = serde_json::from_str::<ToolOutcome>(stdout)
    {
        return o;
    }
    ToolOutcome {
        message: stdout.to_owned(),
        replace_prompt: None,
    }
}

fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    // Drain the pipes on threads so a chatty script cannot deadlock.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let read = |pipe: Option<std::process::ChildStdout>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = std::io::Read::read_to_end(&mut p, &mut buf);
            }
            buf
        })
    };
    let read_err = |pipe: Option<std::process::ChildStderr>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = std::io::Read::read_to_end(&mut p, &mut buf);
            }
            buf
        })
    };
    let out_thread = read(stdout);
    let err_thread = read_err(stderr);
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    };
    Ok(std::process::Output {
        status,
        stdout: out_thread.join().unwrap_or_default(),
        stderr: err_thread.join().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tool(dir: &Path, name: &str, script: &str, extra: &str) {
        let folder = dir.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join(MANIFEST_FILE),
            format!(
                r#"{{"name":"{name}","description":"test tool","command":["sh","run.sh"]{extra}}}"#
            ),
        )
        .unwrap();
        std::fs::write(folder.join("run.sh"), script).unwrap();
    }

    #[test]
    fn loads_manifests_and_reports_broken_ones() {
        let dir = tempfile::tempdir().unwrap();
        write_tool(dir.path(), "b_tool", "true", r#","review":true"#);
        write_tool(dir.path(), "a_tool", "true", "");
        std::fs::create_dir_all(dir.path().join("broken")).unwrap();
        std::fs::write(dir.path().join("broken").join(MANIFEST_FILE), "{").unwrap();
        std::fs::create_dir_all(dir.path().join("not_a_tool")).unwrap();
        let (tools, problems) = load_manifests(dir.path());
        assert_eq!(
            tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["a_tool", "b_tool"]
        );
        assert!(tools[1].review);
        assert_eq!(tools[0].parameters, empty_parameters());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("broken"));
    }

    fn empty_parameters() -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    #[test]
    fn runs_a_script_with_stdin_json_env_and_parses_the_reply() {
        let dir = tempfile::tempdir().unwrap();
        write_tool(
            dir.path(),
            "echo",
            r#"input=$(cat); printf '{"message":"got %s in %s","replace_prompt":"new"}' "$(printf '%s' "$input" | tr -d '\n' | wc -c | tr -d ' ')" "$PROMPTBOX_PROMPT""#,
            "",
        );
        let (tools, _) = load_manifests(dir.path());
        let input = ToolInput {
            arguments: serde_json::json!({"quote": "Be kind."}),
            prompt: "hello".into(),
        };
        let out = ProcessToolRunner.run(&tools[0], &input).unwrap();
        let expected_len = serde_json::to_vec(&input).unwrap().len();
        assert_eq!(out.message, format!("got {expected_len} in hello"));
        assert_eq!(out.replace_prompt.as_deref(), Some("new"));
    }

    #[test]
    fn plain_text_stdout_and_failures() {
        let dir = tempfile::tempdir().unwrap();
        write_tool(dir.path(), "plain", "echo saved it", "");
        write_tool(dir.path(), "bad", "echo oops >&2; exit 3", "");
        let (tools, _) = load_manifests(dir.path());
        let input = ToolInput {
            arguments: serde_json::json!({}),
            prompt: String::new(),
        };
        let ok = ProcessToolRunner.run(&tools[1], &input).unwrap();
        assert_eq!(ok.message, "saved it");
        assert_eq!(ok.replace_prompt, None);
        let err = ProcessToolRunner.run(&tools[0], &input).unwrap_err();
        assert!(err.contains("oops") && err.contains("bad"), "{err}");
    }
}
