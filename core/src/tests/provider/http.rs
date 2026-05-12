use super::*;
use crate::provider::ToolCall;
use serde_json::json;

// --- request serialization ------------------------------------- //

fn wire_json(req: &CompletionRequest<'_>) -> serde_json::Value {
    serde_json::to_value(build_wire_request(req)).unwrap()
}

/// Build a request from owned messages. Returns the messages vec
/// alongside the request because `req.messages` borrows from it -
/// callers keep both alive together for the duration of the test.
fn req_with(messages: &[Message], tools: Vec<ToolSchema>) -> CompletionRequest<'_> {
    CompletionRequest {
        model: "m".into(),
        messages,
        tools,
        temperature: None,
        max_tokens: None,
    }
}

#[test]
fn tools_get_function_envelope() {
    let msgs: Vec<Message> = vec![];
    let req = req_with(
        &msgs,
        vec![ToolSchema {
            name: "read".into(),
            description: "read a file".into(),
            parameters: json!({"type": "object"}),
        }],
    );
    let v = wire_json(&req);
    assert_eq!(v["tools"][0]["type"], "function");
    assert_eq!(v["tools"][0]["function"]["name"], "read");
    assert_eq!(v["tools"][0]["function"]["description"], "read a file");
    assert_eq!(v["tools"][0]["function"]["parameters"]["type"], "object");
}

#[test]
fn tools_field_omitted_when_empty() {
    let msgs: Vec<Message> = vec![];
    let req = req_with(&msgs, vec![]);
    let v = wire_json(&req);
    assert!(v.get("tools").is_none(), "expected `tools` to be omitted");
}

#[test]
fn assistant_with_tool_calls_uses_null_content_and_function_envelope() {
    let msgs = vec![Message {
        role: Role::Assistant,
        content: String::new(), // empty + tool_calls => content: null
        tool_calls: Some(vec![ToolCall {
            id: "call_1".into(),
            name: "read".into(),
            arguments: r#"{"path":"a"}"#.into(),
        }]),
        tool_call_id: None,
    }];
    let req = req_with(&msgs, vec![]);
    let v = wire_json(&req);
    assert!(v["messages"][0]["content"].is_null());
    assert_eq!(v["messages"][0]["tool_calls"][0]["id"], "call_1");
    assert_eq!(v["messages"][0]["tool_calls"][0]["type"], "function");
    assert_eq!(v["messages"][0]["tool_calls"][0]["function"]["name"], "read");
    assert_eq!(
        v["messages"][0]["tool_calls"][0]["function"]["arguments"],
        r#"{"path":"a"}"#
    );
}

#[test]
fn user_message_keeps_content_string() {
    let msgs = vec![Message {
        role: Role::User,
        content: "hi".into(),
        tool_calls: None,
        tool_call_id: None,
    }];
    let req = req_with(&msgs, vec![]);
    let v = wire_json(&req);
    assert_eq!(v["messages"][0]["role"], "user");
    assert_eq!(v["messages"][0]["content"], "hi");
    assert!(v["messages"][0].get("tool_calls").is_none());
}

#[test]
fn tool_message_carries_tool_call_id() {
    let msgs = vec![Message {
        role: Role::Tool,
        content: "result text".into(),
        tool_calls: None,
        tool_call_id: Some("call_1".into()),
    }];
    let req = req_with(&msgs, vec![]);
    let v = wire_json(&req);
    assert_eq!(v["messages"][0]["role"], "tool");
    assert_eq!(v["messages"][0]["content"], "result text");
    assert_eq!(v["messages"][0]["tool_call_id"], "call_1");
}

#[test]
fn stream_field_is_always_true() {
    let msgs: Vec<Message> = vec![];
    let req = req_with(&msgs, vec![]);
    let v = wire_json(&req);
    assert_eq!(v["stream"], true);
}

// --- response parsing ------------------------------------------ //

#[test]
fn done_sentinel_yields_done_chunk() {
    let chunks = parse_event("[DONE]");
    assert!(matches!(chunks[..], [Ok(Chunk::Done(FinishReason::Stop))]));
}

#[test]
fn content_delta_yields_content_chunk() {
    let json = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
    let chunks = parse_event(json);
    assert_eq!(chunks.len(), 1);
    match &chunks[0] {
        Ok(Chunk::Content(s)) => assert_eq!(s, "Hello"),
        other => panic!("expected Content, got {other:?}"),
    }
}

#[test]
fn empty_content_is_dropped() {
    let json = r#"{"choices":[{"delta":{"content":""}}]}"#;
    let chunks = parse_event(json);
    assert!(chunks.is_empty());
}

#[test]
fn finish_reason_yields_done() {
    let json = r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#;
    let chunks = parse_event(json);
    assert!(matches!(
        chunks[..],
        [Ok(Chunk::Done(FinishReason::Length))]
    ));
}

#[test]
fn tool_call_delta_yields_tool_chunk() {
    let json = r#"{"choices":[{"delta":{"tool_calls":[
        {"index":0,"id":"call_1","function":{"name":"read","arguments":"{\""}}
    ]}}]}"#;
    let chunks = parse_event(json);
    assert_eq!(chunks.len(), 1);
    match &chunks[0] {
        Ok(Chunk::ToolCallDelta(d)) => {
            assert_eq!(d.index, 0);
            assert_eq!(d.id.as_deref(), Some("call_1"));
            assert_eq!(d.name.as_deref(), Some("read"));
            assert_eq!(d.arguments.as_deref(), Some("{\""));
        }
        other => panic!("expected ToolCallDelta, got {other:?}"),
    }
}

#[test]
fn parallel_tool_calls_yield_multiple_chunks() {
    let json = r#"{"choices":[{"delta":{"tool_calls":[
        {"index":0,"id":"a","function":{"name":"foo","arguments":""}},
        {"index":1,"id":"b","function":{"name":"bar","arguments":""}}
    ]}}]}"#;
    let chunks = parse_event(json);
    assert_eq!(chunks.len(), 2);
}

#[test]
fn invalid_json_yields_error_chunk() {
    let chunks = parse_event("not json");
    assert_eq!(chunks.len(), 1);
    assert!(matches!(chunks[0], Err(Error::Json(_))));
}

// --- prompt-cache prefix stability ----------------------------- //

#[test]
fn wire_request_serializes_byte_identical_on_repeat() {
    // Prompt-cache hit rate depends on `build_wire_request +
    // serde_json` producing identical bytes for identical input
    // on every turn. A HashMap somewhere in the schema path, a
    // timestamp injection, or any other nondeterminism would
    // silently degrade caching. Catch it here before the next
    // provider PR sneaks it in.
    let msgs = vec![Message {
        role: Role::User,
        content: "hi".into(),
        tool_calls: None,
        tool_call_id: None,
    }];
    let tools = vec![
        ToolSchema {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: json!({"type": "object", "additionalProperties": false}),
        },
        ToolSchema {
            name: "write".into(),
            description: "Write a file".into(),
            parameters: json!({"type": "object", "additionalProperties": false}),
        },
    ];
    let req = req_with(&msgs, tools);
    let b1 = serde_json::to_vec(&build_wire_request(&req)).unwrap();
    let b2 = serde_json::to_vec(&build_wire_request(&req)).unwrap();
    assert_eq!(b1, b2, "wire request must serialize byte-identical on repeat");
}

#[test]
fn wire_request_prefix_stable_when_only_user_message_differs() {
    // The real prompt-cache scenario across turns: system +
    // tools are constant, the tail (user/assistant/tool
    // messages) grows. Bytes up to the first divergence must
    // be identical so the provider hits cache. Verify by
    // computing the longest common prefix of two serializations
    // and asserting the system content is fully inside it.
    let tools = vec![ToolSchema {
        name: "read".into(),
        description: "Read".into(),
        parameters: json!({"type": "object"}),
    }];
    let make = |user: &str| -> Vec<u8> {
        let msgs = vec![
            Message {
                role: Role::System,
                content: "you are lumen".into(),
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: Role::User,
                content: user.into(),
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        let req = req_with(&msgs, tools.clone());
        serde_json::to_vec(&build_wire_request(&req)).unwrap()
    };
    let a = make("hello");
    let b = make("a different question");
    let common = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    let prefix = std::str::from_utf8(&a[..common]).expect("ASCII prefix");
    assert!(
        prefix.contains("you are lumen"),
        "system content must land inside the prompt-cache prefix; got: {prefix:?}"
    );
}
