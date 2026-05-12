use super::*;
use crate::provider::{ChunkStream, ToolCallDelta};
use crate::tools::Tool;
use async_trait::async_trait;
use futures_util::stream;
use serde_json::json;
use std::collections::VecDeque;
use std::path::PathBuf;
use tokio::sync::Mutex;

/// In-memory provider that replays a queue of pre-canned chunk lists,
/// one per call to `complete`. Panics if drained - tests should
/// supply enough responses to cover every expected provider call.
struct MockProvider {
    responses: Mutex<VecDeque<Vec<Chunk>>>,
}

impl MockProvider {
    fn new(responses: Vec<Vec<Chunk>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn complete<'a>(&self, _req: CompletionRequest<'a>) -> Result<ChunkStream> {
        let mut g = self.responses.lock().await;
        let resp = g
            .pop_front()
            .expect("MockProvider: exhausted response queue");
        let resp: Vec<Result<Chunk>> = resp.into_iter().map(Ok).collect();
        Ok(Box::pin(stream::iter(resp)))
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        // Mock returns an empty list; tests that need a real model
        // list should mock it explicitly.
        Ok(Vec::new())
    }
}

/// Echoes back its own arguments string. Lets us test tool dispatch
/// without touching the filesystem.
#[derive(Debug, Default, Clone, Copy)]
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }
    fn description(&self) -> &'static str {
        "echo arguments back"
    }
    fn parameters(&self) -> serde_json::Value {
        json!({ "type": "object", "additionalProperties": true })
    }
    async fn invoke(&self, _ctx: &ToolContext, args: &str) -> Result<String> {
        Ok(args.to_string())
    }
}

fn build_agent(provider: Arc<dyn Provider>, tools: ToolRegistry) -> Agent {
    Agent::new(
        provider,
        tools,
        Session::ephemeral(),
        ToolContext::new(PathBuf::from(".")),
        AgentOptions::default(),
    )
}

fn delta(
    index: u32,
    id: Option<&str>,
    name: Option<&str>,
    args: Option<&str>,
) -> Chunk {
    Chunk::ToolCallDelta(ToolCallDelta {
        index,
        id: id.map(String::from),
        name: name.map(String::from),
        arguments: args.map(String::from),
    })
}

#[tokio::test]
async fn single_turn_no_tools_streams_text_and_ends() {
    let mp = MockProvider::new(vec![vec![
        Chunk::Content("hello ".into()),
        Chunk::Content("world".into()),
        Chunk::Done(FinishReason::Stop),
    ]]);
    let mut a = build_agent(Arc::new(mp), ToolRegistry::new());
    let mut events = Vec::new();
    let reason = a.turn("hi".into(), |e| events.push(e)).await.unwrap();
    assert_eq!(reason, FinishReason::Stop);

    // 2 text events + 1 turn end
    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0], AgentEvent::AssistantText(s) if s == "hello "));
    assert!(matches!(&events[2], AgentEvent::TurnEnd { reason: FinishReason::Stop }));

    let msgs = a.session().messages();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, Role::User);
    assert_eq!(msgs[1].role, Role::Assistant);
    assert_eq!(msgs[1].content, "hello world");
    assert!(msgs[1].tool_calls.is_none());
}

#[tokio::test]
async fn one_tool_round_trip_completes_and_persists_correctly() {
    // First call: streamed tool call, split across two argument deltas.
    let r1 = vec![
        delta(0, Some("c1"), Some("echo"), Some(r#"{"x":"#)),
        delta(0, None, None, Some(r"1}")),
        Chunk::Done(FinishReason::ToolCalls),
    ];
    // Second call: model wraps up with text after seeing the tool result.
    let r2 = vec![
        Chunk::Content("done".into()),
        Chunk::Done(FinishReason::Stop),
    ];
    let mp = MockProvider::new(vec![r1, r2]);

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool));
    let mut a = build_agent(Arc::new(mp), tools);

    let mut events = Vec::new();
    let reason = a.turn("trigger".into(), |e| events.push(e)).await.unwrap();
    assert_eq!(reason, FinishReason::Stop);

    assert!(events.iter().any(
        |e| matches!(e, AgentEvent::ToolCallStart { name, arguments, .. }
            if name == "echo" && arguments == r#"{"x":1}"#)
    ));
    assert!(events.iter().any(
        |e| matches!(e, AgentEvent::ToolCallEnd { result, is_error: false, .. }
            if result == r#"{"x":1}"#)
    ));

    let msgs = a.session().messages();
    // user -> assistant(tool_calls) -> tool -> assistant(text)
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].role, Role::User);
    assert_eq!(msgs[1].role, Role::Assistant);
    assert!(msgs[1].tool_calls.is_some());
    assert_eq!(msgs[2].role, Role::Tool);
    assert_eq!(msgs[2].tool_call_id.as_deref(), Some("c1"));
    assert_eq!(msgs[2].content, r#"{"x":1}"#);
    assert_eq!(msgs[3].role, Role::Assistant);
    assert_eq!(msgs[3].content, "done");
}

#[tokio::test]
async fn parallel_tool_calls_dispatch_in_index_order() {
    let r1 = vec![
        delta(1, Some("b"), Some("echo"), Some(r#"{"k":"second"}"#)),
        delta(0, Some("a"), Some("echo"), Some(r#"{"k":"first"}"#)),
        Chunk::Done(FinishReason::ToolCalls),
    ];
    let r2 = vec![
        Chunk::Content("ok".into()),
        Chunk::Done(FinishReason::Stop),
    ];
    let mp = MockProvider::new(vec![r1, r2]);
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool));
    let mut a = build_agent(Arc::new(mp), tools);

    let mut starts = Vec::new();
    a.turn("x".into(), |e| {
        if let AgentEvent::ToolCallStart { id, .. } = e {
            starts.push(id);
        }
    })
    .await
    .unwrap();

    // Dispatch order matches index, not arrival order.
    assert_eq!(starts, vec!["a", "b"]);
}

#[tokio::test]
async fn tool_failure_is_fed_back_as_error_message() {
    let r1 = vec![
        delta(0, Some("c1"), Some("nope"), Some("{}")),
        Chunk::Done(FinishReason::ToolCalls),
    ];
    let r2 = vec![
        Chunk::Content("recovered".into()),
        Chunk::Done(FinishReason::Stop),
    ];
    let mp = MockProvider::new(vec![r1, r2]);
    let mut a = build_agent(Arc::new(mp), ToolRegistry::new());

    let mut events = Vec::new();
    a.turn("x".into(), |e| events.push(e)).await.unwrap();

    let tool_msg = a
        .session()
        .messages()
        .iter()
        .find(|m| m.role == Role::Tool)
        .expect("expected a tool result message");
    assert!(tool_msg.content.contains("unknown tool"));
    assert!(events.iter().any(
        |e| matches!(e, AgentEvent::ToolCallEnd { is_error: true, .. })
    ));
}

// --- seed_system_prompt -----------------------------------------

fn empty_provider() -> Arc<dyn Provider> {
    // Never called - tests below assert seeding without invoking turn().
    Arc::new(MockProvider::new(vec![]))
}

#[tokio::test]
async fn seed_system_prompt_pushes_one_system_message_on_fresh_session() {
    let mut a = build_agent(empty_provider(), ToolRegistry::new());
    a.seed_system_prompt("you are lumen").await.unwrap();
    let msgs = a.session().messages();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, Role::System);
    assert_eq!(msgs[0].content, "you are lumen");
}

#[tokio::test]
async fn seed_system_prompt_is_noop_when_session_non_empty() {
    // Resumed-session shape: messages already present. Seeding must
    // not duplicate the system prompt and degrade cache hit rate.
    let mut a = build_agent(empty_provider(), ToolRegistry::new());
    a.session_mut()
        .push(Message {
            role: Role::User,
            content: "hi".into(),
            tool_calls: None,
            tool_call_id: None,
        })
        .await
        .unwrap();
    a.seed_system_prompt("you are lumen").await.unwrap();
    let msgs = a.session().messages();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, Role::User);
}

#[tokio::test]
async fn seed_system_prompt_called_twice_only_seeds_once() {
    let mut a = build_agent(empty_provider(), ToolRegistry::new());
    a.seed_system_prompt("first").await.unwrap();
    a.seed_system_prompt("second").await.unwrap();
    let msgs = a.session().messages();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "first");
}

// --- emit_text_chunks -------------------------------------------

fn collect_text_chunks(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    emit_text_chunks(input, &mut |e| {
        if let AgentEvent::AssistantText(s) = e {
            out.push(s);
        }
    });
    out
}

#[test]
fn small_text_emits_one_chunk_verbatim() {
    let chunks = collect_text_chunks("hello world");
    assert_eq!(chunks, vec!["hello world".to_string()]);
}

#[test]
fn empty_text_emits_nothing() {
    assert!(collect_text_chunks("").is_empty());
}

#[test]
fn oversized_text_splits_into_multiple_events() {
    let big = "x".repeat(MAX_TEXT_CHUNK * 2 + 100);
    let chunks = collect_text_chunks(&big);
    assert!(chunks.len() >= 3, "expected 3+ chunks, got {}", chunks.len());
    for c in &chunks {
        assert!(c.len() <= MAX_TEXT_CHUNK, "chunk over cap: {}", c.len());
    }
    let rejoined: String = chunks.concat();
    assert_eq!(rejoined, big, "concat must reproduce input losslessly");
}

#[test]
fn split_respects_utf8_char_boundaries() {
    // Multi-byte chars straddling the cap. '€' is 3 bytes; we
    // pack just enough to land the cap mid-codepoint and confirm
    // the splitter walks back to the boundary.
    let codepoints_to_overflow = (MAX_TEXT_CHUNK / 3) + 5;
    let big: String = "€".repeat(codepoints_to_overflow);
    let chunks = collect_text_chunks(&big);
    assert!(chunks.len() >= 2);
    // Every chunk must be valid UTF-8 (it's already a String, so
    // this is trivially true) and the concat must equal the input.
    let rejoined: String = chunks.concat();
    assert_eq!(rejoined, big);
    // Each chunk should end on a codepoint boundary - the rejoin
    // check above effectively asserts this, but be explicit.
    for c in &chunks {
        assert_eq!(c.len() % 3, 0, "€ is 3 bytes; chunk len must be divisible by 3");
    }
}

#[tokio::test]
async fn seeded_system_prompt_appears_in_provider_request_prefix() {
    // The whole point of seeding: the system message lands as the
    // first wire-message every turn. Verify by running one turn
    // and inspecting the message slice the agent would have sent.
    let mp = MockProvider::new(vec![vec![
        Chunk::Content("ok".into()),
        Chunk::Done(FinishReason::Stop),
    ]]);
    let mut a = build_agent(Arc::new(mp), ToolRegistry::new());
    a.seed_system_prompt(CORE_SYSTEM_PROMPT).await.unwrap();
    a.turn("hi".into(), |_| {}).await.unwrap();
    let msgs = a.session().messages();
    assert_eq!(msgs[0].role, Role::System);
    assert_eq!(msgs[0].content, CORE_SYSTEM_PROMPT);
    assert_eq!(msgs[1].role, Role::User);
}

#[tokio::test]
async fn max_iterations_bounds_runaway_tool_loop() {
    // Endless tool-request loop on the provider side; agent should
    // bail after `max_tool_iterations` rather than spin forever.
    let make_resp = || {
        vec![
            delta(0, Some("c"), Some("echo"), Some("{}")),
            Chunk::Done(FinishReason::ToolCalls),
        ]
    };
    let many = (0..10).map(|_| make_resp()).collect::<Vec<_>>();
    let mp = MockProvider::new(many);

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool));

    let opts = AgentOptions {
        max_tool_iterations: 3,
        ..AgentOptions::default()
    };
    let mut a = Agent::new(
        Arc::new(mp),
        tools,
        Session::ephemeral(),
        ToolContext::new(PathBuf::from(".")),
        opts,
    );
    let err = a.turn("x".into(), |_| {}).await.unwrap_err();
    assert!(matches!(err, Error::Tool(ref m) if m.contains("max_tool_iterations")));
}
