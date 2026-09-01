//! End-to-end test of the OpenAI-compatible transport against a local socket.
//!
//! Everything between the Anthropic-shaped history and the Anthropic-shaped
//! answer is exercised for real: the translated request goes out over HTTP, a
//! stub endpoint replies with a streamed chat-completions response carrying a
//! tool call, and the turn is read back. The unit tests cover the translation
//! rules; this one covers that they are actually what goes on the wire.

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A streamed chat-completions reply: two content deltas, one tool call split
/// across chunks, a finish reason, and a usage-only final chunk — the shape a
/// real endpoint sends.
const SSE_BODY: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"looking\"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" now\"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",",
    "\"function\":{\"name\":\"get_source_info\",\"arguments\":\"{\\\"depth\\\"\"}}]}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,",
    "\"function\":{\"arguments\":\":2}\"}}]}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1234}}\n\n",
    "data: [DONE]\n\n",
);

/// Serve exactly one request, hand back the SSE body, and return what the
/// client sent (headers and JSON body).
async fn serve_one(listener: TcpListener) -> (String, Value) {
    let (mut socket, _) = listener.accept().await.expect("a connection");

    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    let (head_end, content_length) = loop {
        let n = socket.read(&mut buf).await.expect("request bytes");
        assert!(n > 0, "the client closed before sending a request");
        raw.extend_from_slice(&buf[..n]);
        if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
            let len: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse().ok())
                .expect("a content-length");
            break (end + 4, len);
        }
    };
    while raw.len() < head_end + content_length {
        let n = socket.read(&mut buf).await.expect("body bytes");
        assert!(n > 0, "the client closed mid-body");
        raw.extend_from_slice(&buf[..n]);
    }

    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let body: Value =
        serde_json::from_slice(&raw[head_end..head_end + content_length]).expect("a JSON body");

    socket
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{SSE_BODY}"
            )
            .as_bytes(),
        )
        .await
        .expect("the response is written");
    socket.shutdown().await.expect("the response ends");
    (head, body)
}

#[tokio::test]
async fn a_streamed_turn_round_trips_through_an_openai_compatible_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(serve_one(listener));

    let mut history = vec![
        json!({"role": "user", "content": [{"type": "text", "text": "convert this"}]}),
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "t0", "name": "get_xfa", "input": {"path": "/a"}},
        ]}),
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t0",
             "content": [{"type": "text", "text": "<xfa/>"}]},
        ]}),
    ];
    let tools = vec![json!({
        "name": "get_source_info",
        "description": "read the source",
        "input_schema": {"type": "object", "properties": {"depth": {"type": "integer"}}},
    })];
    let endpoint = runner::LlmEndpoint::openai(&base_url, "sk-test", "vendor/some-model");

    let turn = runner::openai::openai_stream_turn(
        &mut history,
        &tools,
        &endpoint,
        4096,
        Some("be careful"),
        &pipeline::AbortFlag::default(),
    )
    .await
    .expect("the turn completes");

    // What came back: text, the tool call reassembled from its fragments, and a
    // stop reason the controller reads as "keep going".
    assert_eq!(turn.text, "looking now");
    assert_eq!(turn.stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(turn.prompt_tokens, 1234);
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].id, "call_a");
    assert_eq!(turn.tool_calls[0].name, "get_source_info");
    assert_eq!(turn.tool_calls[0].input, json!({"depth": 2}));

    // The history stays Anthropic-shaped, so the next turn, the eviction ladder
    // and a resumed session all read it the same way whichever endpoint served it.
    let appended = history.last().expect("the assistant message is appended");
    assert_eq!(appended["role"], "assistant");
    assert_eq!(appended["content"][0]["type"], "text");
    assert_eq!(appended["content"][1]["type"], "tool_use");
    assert_eq!(appended["content"][1]["name"], "get_source_info");

    // What went out: the OpenAI dialect, authenticated, at the endpoint's path.
    let (head, body) = server.await.expect("the stub endpoint finishes");
    assert!(head.starts_with("POST /v1/chat/completions "), "{head}");
    assert!(
        head.to_ascii_lowercase()
            .contains("authorization: bearer sk-test"),
        "{head}"
    );
    assert_eq!(body["model"], "vendor/some-model");
    assert_eq!(body["stream"], true);
    assert_eq!(body["tools"][0]["function"]["name"], "get_source_info");
    let roles: Vec<&str> = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["system", "user", "assistant", "tool"]);
    assert_eq!(body["messages"][2]["tool_calls"][0]["id"], "t0");
    assert_eq!(body["messages"][3]["tool_call_id"], "t0");
}
