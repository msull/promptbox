//! Real `OpenAI` call. Ignored by default; needs `OPENAI_API_KEY` (env or .env)
//! and spends a few hundred tokens:
//!
//! ```sh
//! cargo test --test openai_live -- --ignored --nocapture
//! ```

use std::path::Path;

use promptbox::adapters::openai::{DEFAULT_MODEL, OpenAiRewriter, read_dotenv_key};
use promptbox::ports::ai::{CLEAN_UP_INSTRUCTION, RewriteRequest, Rewriter};

#[test]
#[ignore = "calls the OpenAI API and spends tokens"]
fn cleans_up_a_dictated_sentence() {
    let key = std::env::var("OPENAI_API_KEY")
        .ok()
        .or_else(|| read_dotenv_key(Path::new(".env"), "OPENAI_API_KEY"))
        .expect("OPENAI_API_KEY");
    let rewriter = OpenAiRewriter::new(key, DEFAULT_MODEL.into());
    let reply = rewriter
        .rewrite(&RewriteRequest {
            id: 1,
            instruction: CLEAN_UP_INSTRUCTION.into(),
            content: "um so add a a pedantic model for the dynamo db item and and use a conditional expression so the the write isn't overwritten".into(),
            context: String::new(),
        })
        .unwrap();
    println!(
        "reply: {:?}\ntokens: {} prompt + {} completion",
        reply.text, reply.prompt_tokens, reply.completion_tokens
    );
    let lower = reply.text.to_lowercase();
    assert!(lower.contains("pydantic") || lower.contains("pedantic"));
    assert!(lower.contains("dynamodb") || lower.contains("dynamo"));
    assert!(!lower.starts_with("here"), "no preamble: {}", reply.text);
    assert!(reply.prompt_tokens > 0 && reply.completion_tokens > 0);
}

#[test]
#[ignore = "calls the OpenAI API and spends tokens"]
fn chooses_the_quote_tool() {
    use promptbox::ports::ai::ToolChoiceRequest;
    let key = std::env::var("OPENAI_API_KEY")
        .ok()
        .or_else(|| read_dotenv_key(Path::new(".env"), "OPENAI_API_KEY"))
        .expect("OPENAI_API_KEY");
    let rewriter = OpenAiRewriter::new(key, DEFAULT_MODEL.into());
    let (tools, problems) = promptbox::adapters::tools::load_manifests(Path::new("examples/tools"));
    assert!(problems.is_empty(), "{problems:?}");
    let choice = rewriter
        .choose_tool(&ToolChoiceRequest {
            id: 1,
            request: "save that quote".into(),
            prompt: "Be kind, for everyone you meet is fighting a hard battle. Ian Maclaren".into(),
            context: String::new(),
            tools,
        })
        .unwrap();
    println!("{choice:?}");
    let call = choice.call.expect("a tool call");
    assert_eq!(call.name, "save_quote");
    assert!(
        call.arguments["quote"]
            .as_str()
            .unwrap()
            .contains("hard battle")
    );
}
