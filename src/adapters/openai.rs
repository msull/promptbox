//! `OpenAI` chat-completions client for prompt rewrites, plus a tiny `.env`
//! reader so a developer's `OPENAI_API_KEY` is picked up without exporting.

use std::path::Path;

use serde::Deserialize;

use crate::ports::ai::{
    RewriteRequest, RewriteResponse, Rewriter, SYSTEM_PROMPT, TOOL_SYSTEM_PROMPT, ToolChoice,
    ToolChoiceRequest,
};
use crate::ports::tools::ToolCall;

pub const DEFAULT_MODEL: &str = "gpt-5.6-luna";
const ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";

pub struct OpenAiRewriter {
    api_key: String,
    model: String,
}

impl OpenAiRewriter {
    #[must_use]
    pub fn new(api_key: String, model: String) -> Self {
        Self { api_key, model }
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ApiToolCall>,
}

#[derive(Deserialize)]
struct ApiToolCall {
    function: ApiFunction,
}

#[derive(Deserialize)]
struct ApiFunction {
    name: String,
    /// JSON text, per the API.
    #[serde(default)]
    arguments: String,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

impl Rewriter for OpenAiRewriter {
    fn rewrite(&self, request: &RewriteRequest) -> Result<RewriteResponse, String> {
        let user = format!(
            "Instruction: {}\n\nText:\n{}",
            request.instruction.trim(),
            request.content
        );
        let system = if request.context.trim().is_empty() {
            SYSTEM_PROMPT.to_owned()
        } else {
            format!(
                "{SYSTEM_PROMPT}\n\nAbout the project the text is for (use it to resolve \
                 misheard names and technical terms):\n{}",
                request.context.trim()
            )
        };
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
        });
        let (choice, usage) = self.complete(&body)?;
        let content = choice.message.content.unwrap_or_default();
        let content = strip_fences(content.trim()).to_owned();
        if content.is_empty() {
            return Err(format!(
                "empty reply (finish_reason {})",
                choice.finish_reason.unwrap_or_default()
            ));
        }
        Ok(RewriteResponse {
            text: content,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
        })
    }

    fn choose_tool(&self, request: &ToolChoiceRequest) -> Result<ToolChoice, String> {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();
        let mut system = TOOL_SYSTEM_PROMPT.to_owned();
        if !request.context.trim().is_empty() {
            system.push_str("\n\nAbout the project:\n");
            system.push_str(request.context.trim());
        }
        let user = format!(
            "Request: {}\n\nCurrent prompt text:\n{}",
            request.request.trim(),
            request.prompt
        );
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "tools": tools,
            "tool_choice": "auto",
            // gpt-5.6-luna refuses function tools on chat completions
            // unless reasoning is off; routing needs no reasoning anyway.
            "reasoning_effort": "none",
        });
        let (choice, usage) = self.complete(&body)?;
        let call = match choice.message.tool_calls.into_iter().next() {
            Some(c) => {
                let arguments = if c.function.arguments.trim().is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&c.function.arguments)
                        .map_err(|e| format!("tool arguments were not JSON: {e}"))?
                };
                Some(ToolCall {
                    name: c.function.name,
                    arguments,
                })
            }
            None => None,
        };
        Ok(ToolChoice {
            call,
            message: choice.message.content.unwrap_or_default().trim().to_owned(),
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
        })
    }
}

impl OpenAiRewriter {
    /// One chat-completions round trip; returns the first choice and usage.
    fn complete(&self, body: &serde_json::Value) -> Result<(Choice, Usage), String> {
        let payload = serde_json::to_string(body).map_err(|e| e.to_string())?;
        let mut response = ureq::post(ENDPOINT)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .config()
            .http_status_as_error(false)
            .build()
            .send(payload.as_bytes())
            .map_err(|e| format!("request failed: {e}"))?;
        let status = response.status();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("read response: {e}"))?;
        let parsed: ChatResponse = serde_json::from_str(&text)
            .map_err(|e| format!("HTTP {status}: unparseable reply ({e})"))?;
        if let Some(err) = parsed.error {
            return Err(format!("OpenAI: {}", err.message));
        }
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| format!("HTTP {status}: no choices in reply"))?;
        Ok((choice, parsed.usage.unwrap_or_default()))
    }
}

/// Models sometimes wrap output in triple-backtick fences despite instructions.
fn strip_fences(s: &str) -> &str {
    let t = s.trim();
    if let Some(inner) = t.strip_prefix("```")
        && let Some(inner) = inner.strip_suffix("```")
    {
        let inner = inner.split_once('\n').map_or(inner, |(first, rest)| {
            if first.chars().all(char::is_alphanumeric) {
                rest
            } else {
                inner
            }
        });
        return inner.trim();
    }
    t
}

/// Reads `KEY=value` lines from a `.env` file; returns the requested key.
/// Quotes around the value are removed. Missing file is not an error.
#[must_use]
pub fn read_dotenv_key(path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("export ").or(Some(l)))
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().trim_matches('"').trim_matches('\'').to_owned())
        .filter(|v| !v.is_empty())
}

/// Test double with a canned reply or failure.
pub struct FakeRewriter {
    pub reply: Result<String, String>,
    /// What `choose_tool` answers; `None` means "no tool fits".
    pub tool: Option<ToolCall>,
}

impl FakeRewriter {
    #[must_use]
    pub fn replying(reply: Result<String, String>) -> Self {
        Self { reply, tool: None }
    }
}

impl Rewriter for FakeRewriter {
    fn rewrite(&self, _request: &RewriteRequest) -> Result<RewriteResponse, String> {
        self.reply.clone().map(|text| RewriteResponse {
            text,
            prompt_tokens: 10,
            completion_tokens: 5,
        })
    }

    fn choose_tool(&self, _request: &ToolChoiceRequest) -> Result<ToolChoice, String> {
        Ok(ToolChoice {
            call: self.tool.clone(),
            message: if self.tool.is_some() {
                String::new()
            } else {
                "No registered tool does that.".to_owned()
            },
            prompt_tokens: 10,
            completion_tokens: 5,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_code_fences_with_and_without_language() {
        assert_eq!(strip_fences("```\nhello\n```"), "hello");
        assert_eq!(strip_fences("```text\nhello\n```"), "hello");
        assert_eq!(strip_fences("plain"), "plain");
        assert_eq!(strip_fences("```not closed"), "```not closed");
    }

    #[test]
    fn dotenv_reader_handles_quotes_exports_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".env");
        std::fs::write(&p, "# c\nexport OTHER='x'\nOPENAI_API_KEY=\"sk-test\"\n").unwrap();
        assert_eq!(
            read_dotenv_key(&p, "OPENAI_API_KEY").as_deref(),
            Some("sk-test")
        );
        assert_eq!(read_dotenv_key(&p, "OTHER").as_deref(), Some("x"));
        assert_eq!(read_dotenv_key(&p, "MISSING"), None);
        assert_eq!(
            read_dotenv_key(Path::new("/nonexistent/.env"), "OPENAI_API_KEY"),
            None
        );
    }
}
