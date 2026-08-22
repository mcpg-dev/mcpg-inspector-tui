//! What the TUI can ask for, and the two places it can ask.
//!
//! The screens do not care whether the engine is in this process or behind
//! an HTTP API — but the difference matters to the user, because a running
//! inspector has state the terminal cannot otherwise see. A gateway's
//! supervised sidecar holds the wire log of the gateway's own traffic; a TUI
//! that dials the gateway directly opens its own session and sees none of
//! it. `--attach` is how the terminal looks at what is already there.
//!
//! Every remote call goes to an endpoint the web UI already uses, so there
//! is no second API to keep in step.

use futures::Stream;
use serde_json::{Value, json};
use std::pin::Pin;

use super::state::{PendingRow, PromptRow, ResourceRow, TargetRow, ToolRow};
use super::view::{CheckSummary, GatewayView, SessionView, WireRow};

/// Everything one target advertises, plus what could not be read.
///
/// Collected in one call because a refresh is one user action: four
/// round-trips that can each fail separately still have to produce a single
/// coherent screen, and a surface a server does not implement must not blank
/// the others.
#[derive(Debug, Default)]
pub struct Catalog {
    pub tools: Vec<ToolRow>,
    pub resources: Vec<ResourceRow>,
    pub prompts: Vec<PromptRow>,
    /// Per-surface failures, for the status line. Empty is not the same as
    /// "there are none" and must not read that way.
    pub errors: Vec<String>,
}

pub type PushStream = Pin<Box<dyn Stream<Item = Value> + Send>>;

/// The operations the TUI drives.
#[async_trait::async_trait]
pub trait InspectorApi: Send + Sync {
    async fn targets(&self) -> Vec<TargetRow>;
    async fn connect(&self, id: &str) -> Result<(), String>;
    async fn disconnect(&self, id: &str) -> Result<(), String>;
    async fn catalog(&self, id: &str) -> Catalog;
    async fn call_tool(&self, id: &str, name: &str, args: &Value) -> Result<Value, String>;
    async fn read_resource(&self, id: &str, uri: &str) -> Result<Value, String>;
    async fn get_prompt(&self, id: &str, name: &str, args: &Value) -> Result<Value, String>;
    async fn wire(&self, id: &str) -> Vec<WireRow>;
    async fn checks(&self, id: &str) -> Result<CheckSummary, String>;
    /// The gateway behind this target, when there is one.
    async fn gateway(&self, id: &str) -> Result<GatewayView, String>;
    async fn pending(&self, id: &str) -> Vec<PendingRow>;
    /// `Ok` fulfils the request; `Err` declines it, which is a legitimate
    /// answer rather than a failure of this call.
    async fn respond(
        &self,
        id: &str,
        request: u64,
        answer: Result<Value, String>,
    ) -> Result<(), String>;
    async fn subscribe(&self, id: &str, uris: &[String]) -> Result<PushStream, String>;
    /// This target's exchange as a recording — the text of the file, not a
    /// path, because whoever asked decides where it goes.
    async fn recording(&self, id: &str) -> Result<String, String>;
    /// What the server suggests for one argument, given what is typed so far.
    ///
    /// `reference` names a prompt or a resource template, the two surfaces
    /// MCP defines completions for; a tool's arguments have no equivalent.
    async fn complete(
        &self,
        id: &str,
        reference: &Value,
        argument: &str,
        typed: &str,
    ) -> Result<Vec<String>, String>;
}

/// The suggestion list out of a `completion/complete` result.
pub fn completion_values(result: &Value) -> Vec<String> {
    result
        .pointer("/result/completion/values")
        .or_else(|| result.pointer("/completion/values"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Somewhere else
// ---------------------------------------------------------------------------

/// A remote inspector, over the API its own web UI uses.
pub struct RemoteApi {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl RemoteApi {
    /// `url` is what the inspector printed, `?token=…` and all. The token may
    /// also come from the environment, which is the form that does not land
    /// in `ps` output for every process on the box to read.
    pub fn new(url: &str, env_token: Option<String>) -> Result<Self, String> {
        let mut parsed = url::Url::parse(url).map_err(|e| format!("--attach needs a URL: {e}"))?;
        let from_url = parsed
            .query_pairs()
            .find(|(k, _)| k == "token")
            .map(|(_, v)| v.into_owned());
        // The token is a credential, not part of the address: strip it so it
        // cannot be echoed back out in an error message or a status line.
        parsed.set_query(None);
        let base = parsed.as_str().trim_end_matches('/').to_owned();
        Ok(Self {
            base,
            token: env_token.or(from_url),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|e| format!("http client: {e}"))?,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1{path}", self.base)
    }

    fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    async fn get(&self, path: &str) -> Result<Value, String> {
        let response = self
            .auth(self.http.get(self.url(path)))
            .send()
            .await
            .map_err(|e| format!("{}: {e}", self.base))?;
        Self::body(response).await
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let response = self
            .auth(self.http.post(self.url(path)).json(body))
            .send()
            .await
            .map_err(|e| format!("{}: {e}", self.base))?;
        Self::body(response).await
    }

    /// The API's own error shape, so a remote failure reads like a local one
    /// rather than as an HTTP status code.
    async fn body(response: reqwest::Response) -> Result<Value, String> {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let parsed: Option<Value> = serde_json::from_str(&text).ok();
        if status.is_success() {
            return Ok(parsed.unwrap_or(Value::Null));
        }
        let message = parsed
            .as_ref()
            .and_then(|v| v.pointer("/error/message"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("HTTP {status}"));
        Err(message)
    }

    async fn op(&self, id: &str, op: &str, params: &Value) -> Result<Value, String> {
        self.post(&format!("/targets/{}/ops/{op}", encode(id)), params)
            .await
    }
}

fn encode(segment: &str) -> String {
    // Target ids are operator-chosen and may contain anything a path segment
    // may not.
    percent_encode(segment)
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[async_trait::async_trait]
impl InspectorApi for RemoteApi {
    async fn targets(&self) -> Vec<TargetRow> {
        let Ok(body) = self.get("/targets").await else {
            return Vec::new();
        };
        body.get("targets")
            .and_then(Value::as_array)
            .map(|list| list.iter().map(remote_target).collect())
            .unwrap_or_default()
    }

    async fn connect(&self, id: &str) -> Result<(), String> {
        self.post(&format!("/targets/{}/connect", encode(id)), &json!({}))
            .await
            .map(|_| ())
    }

    async fn disconnect(&self, id: &str) -> Result<(), String> {
        self.post(&format!("/targets/{}/disconnect", encode(id)), &json!({}))
            .await
            .map(|_| ())
    }

    async fn catalog(&self, id: &str) -> Catalog {
        let mut catalog = Catalog::default();
        match self.op(id, "tools.list", &json!({})).await {
            Ok(body) => catalog.tools = parse_tools(&body),
            Err(e) => catalog.errors.push(format!("tools/list: {e}")),
        }
        match self.op(id, "resources.list", &json!({})).await {
            Ok(body) => catalog.resources = parse_resources(&body),
            Err(e) => catalog.errors.push(format!("resources/list: {e}")),
        }
        if let Ok(body) = self.op(id, "resources.templates.list", &json!({})).await {
            catalog.resources.extend(parse_templates(&body));
        }
        match self.op(id, "prompts.list", &json!({})).await {
            Ok(body) => catalog.prompts = parse_prompts(&body),
            Err(e) => catalog.errors.push(format!("prompts/list: {e}")),
        }
        catalog
    }

    async fn call_tool(&self, id: &str, name: &str, args: &Value) -> Result<Value, String> {
        self.op(
            id,
            "tools.call",
            &json!({ "name": name, "arguments": args }),
        )
        .await
    }

    async fn read_resource(&self, id: &str, uri: &str) -> Result<Value, String> {
        self.op(id, "resources.read", &json!({ "uri": uri })).await
    }

    async fn get_prompt(&self, id: &str, name: &str, args: &Value) -> Result<Value, String> {
        self.op(
            id,
            "prompts.get",
            &json!({ "name": name, "arguments": args }),
        )
        .await
    }

    async fn wire(&self, id: &str) -> Vec<WireRow> {
        // NDJSON, not the SSE stream: the wire screen is a snapshot the user
        // refreshes, and a held-open stream would be a second thing to
        // reconnect.
        let Ok(response) = self
            .auth(self.http.get(format!(
                "{}/api/v1/targets/{}/export",
                self.base,
                encode(id)
            )))
            .send()
            .await
        else {
            return Vec::new();
        };
        let ndjson = response.text().await.unwrap_or_default();
        ndjson
            .lines()
            .filter_map(|line| serde_json::from_str::<WireRow>(line).ok())
            .collect::<Vec<_>>()
    }

    async fn checks(&self, id: &str) -> Result<CheckSummary, String> {
        let body = self.get(&format!("/targets/{}/checks", encode(id))).await?;
        serde_json::from_value(body).map_err(|e| format!("unreadable check report: {e}"))
    }

    async fn gateway(&self, id: &str) -> Result<GatewayView, String> {
        let body = self
            .get(&format!("/targets/{}/gateway", encode(id)))
            .await?;
        serde_json::from_value(body).map_err(|e| format!("unreadable gateway report: {e}"))
    }

    async fn pending(&self, id: &str) -> Vec<PendingRow> {
        let Ok(body) = self.get(&format!("/targets/{}/pending", encode(id))).await else {
            return Vec::new();
        };
        body.get("pending")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|request| {
                        Some(PendingRow {
                            id: request.get("id")?.as_u64()?,
                            method: request.get("method")?.as_str()?.to_owned(),
                            params: serde_json::to_string_pretty(
                                request.get("params").unwrap_or(&Value::Null),
                            )
                            .unwrap_or_default(),
                            regime: request
                                .get("regime")
                                .and_then(Value::as_str)
                                .unwrap_or("?")
                                .to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn respond(
        &self,
        id: &str,
        request: u64,
        answer: Result<Value, String>,
    ) -> Result<(), String> {
        let body = match answer {
            Ok(result) => json!({ "result": result }),
            Err(message) => json!({ "error": { "code": -32601, "message": message } }),
        };
        self.post(&format!("/targets/{}/pending/{request}", encode(id)), &body)
            .await
            .map(|_| ())
    }

    async fn subscribe(&self, id: &str, uris: &[String]) -> Result<PushStream, String> {
        let mut url = format!(
            "{}/api/v1/targets/{}/subscribe?lists=true",
            self.base,
            encode(id)
        );
        if !uris.is_empty() {
            url.push_str(&format!("&uris={}", percent_encode(&uris.join(","))));
        }
        let response = self
            .auth(self.http.get(url))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("subscribe failed: HTTP {}", response.status()));
        }
        Ok(Box::pin(sse_frames(response)))
    }

    async fn recording(&self, id: &str) -> Result<String, String> {
        let response = self
            .auth(self.http.get(format!(
                "{}/api/v1/targets/{}/export",
                self.base,
                encode(id)
            )))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("export failed: HTTP {}", response.status()));
        }
        response.text().await.map_err(|e| e.to_string())
    }

    async fn complete(
        &self,
        id: &str,
        reference: &Value,
        argument: &str,
        typed: &str,
    ) -> Result<Vec<String>, String> {
        let body = self
            .op(
                id,
                "completion.complete",
                &json!({
                    "ref": reference,
                    "argument": { "name": argument, "value": typed },
                }),
            )
            .await?;
        Ok(completion_values(&body))
    }
}

/// Server-sent events, reduced to the JSON payloads.
///
/// Hand-rolled rather than pulled in as a dependency: the inspector emits one
/// `data:` line per event and nothing else, so the whole parser is the
/// blank-line boundary and the field prefix.
fn sse_frames(response: reqwest::Response) -> impl Stream<Item = Value> + Send {
    async_stream::stream! {
        use futures::StreamExt;
        let mut bytes = response.bytes_stream();
        let mut buffer = String::new();
        while let Some(Ok(chunk)) = bytes.next().await {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(end) = buffer.find("\n\n") {
                let event: String = buffer.drain(..end + 2).collect();
                for line in event.lines() {
                    if let Some(data) = line.strip_prefix("data:")
                        && let Ok(value) = serde_json::from_str::<Value>(data.trim())
                    {
                        yield value;
                    }
                }
            }
        }
    }
}

fn remote_target(value: &Value) -> TargetRow {
    let spec = value.get("spec");
    TargetRow {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        endpoint: spec
            .and_then(|s| s.get("url"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                spec.and_then(|s| s.get("command"))
                    .and_then(Value::as_str)
                    .map(|command| format!("stdio:{command}"))
            })
            .unwrap_or_default(),
        session: remote_session(value.get("session")),
    }
}

fn remote_session(value: Option<&Value>) -> SessionView {
    let state = value
        .and_then(|s| s.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("idle");
    match state {
        "ready" => SessionView::Ready {
            negotiated_version: value
                .and_then(|s| s.get("negotiated_version"))
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned(),
        },
        "connecting" => SessionView::Connecting,
        "failed" => SessionView::Failed {
            message: value
                .and_then(|s| s.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("failed")
                .to_owned(),
        },
        _ => SessionView::Idle,
    }
}

fn parse_tools(body: &Value) -> Vec<ToolRow> {
    array(body, "tools")
        .iter()
        .filter_map(|tool| {
            Some(ToolRow {
                name: tool.get("name")?.as_str()?.to_owned(),
                description: tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                input_schema: tool.get("inputSchema").cloned(),
                output_schema: tool.get("outputSchema").cloned(),
                app_uri: tool.get("_meta").and_then(app_uri_of),
            })
        })
        .collect()
}

fn parse_resources(body: &Value) -> Vec<ResourceRow> {
    array(body, "resources")
        .iter()
        .filter_map(|resource| {
            Some(ResourceRow {
                uri: resource.get("uri")?.as_str()?.to_owned(),
                name: resource
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                description: resource
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                is_template: false,
            })
        })
        .collect()
}

fn parse_templates(body: &Value) -> Vec<ResourceRow> {
    array(body, "resourceTemplates")
        .iter()
        .filter_map(|template| {
            Some(ResourceRow {
                uri: template.get("uriTemplate")?.as_str()?.to_owned(),
                name: template
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                description: template
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                is_template: true,
            })
        })
        .collect()
}

fn parse_prompts(body: &Value) -> Vec<PromptRow> {
    array(body, "prompts")
        .iter()
        .filter_map(|prompt| {
            Some(PromptRow {
                name: prompt.get("name")?.as_str()?.to_owned(),
                description: prompt
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                arguments: prompt
                    .get("arguments")
                    .and_then(Value::as_array)
                    .map(|args| {
                        args.iter()
                            .filter_map(|a| {
                                Some(crate::schema::PromptArgument {
                                    name: a.get("name")?.as_str()?.to_owned(),
                                    description: a
                                        .get("description")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned),
                                    required: a
                                        .get("required")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// SEP-1865's `_meta.ui.resourceUri`, with the deprecated flat alias.
///
/// Read here rather than taken from the wire crate because the terminal
/// depends on no engine type; it is four lines and a comment.
fn app_uri_of(meta: &Value) -> Option<String> {
    meta.get("ui")
        .and_then(|ui| ui.get("resourceUri"))
        .and_then(Value::as_str)
        .or_else(|| meta.get("ui/resourceUri").and_then(Value::as_str))
        .filter(|uri| uri.starts_with("ui://"))
        .map(str::to_owned)
}

fn array<'a>(body: &'a Value, key: &str) -> &'a [Value] {
    body.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A token on the command line is readable by every process on the box,
    /// so the URL's copy is lifted out of the address it is stored beside.
    #[test]
    fn the_token_is_taken_out_of_the_url() {
        let api = RemoteApi::new("http://127.0.0.1:7846/?token=abc123", None).unwrap();
        assert_eq!(api.base, "http://127.0.0.1:7846");
        assert_eq!(api.token.as_deref(), Some("abc123"));
        assert_eq!(api.url("/targets"), "http://127.0.0.1:7846/api/v1/targets");
    }

    /// The environment wins: it is the form that does not reach `ps`, so a
    /// caller who took the trouble should not be overridden by a stale URL.
    #[test]
    fn the_environment_token_wins() {
        let api =
            RemoteApi::new("http://host:7846/?token=from-url", Some("from-env".into())).unwrap();
        assert_eq!(api.token.as_deref(), Some("from-env"));

        let api = RemoteApi::new("http://host:7846/", None).unwrap();
        assert!(api.token.is_none(), "no token is a valid configuration");
    }

    /// A trailing slash would produce `//api/v1`, which some proxies redirect
    /// and some reject.
    #[test]
    fn the_base_url_is_normalised() {
        for given in [
            "http://host:7846",
            "http://host:7846/",
            "http://host:7846/?token=x",
        ] {
            let api = RemoteApi::new(given, None).unwrap();
            assert_eq!(api.url("/meta"), "http://host:7846/api/v1/meta", "{given}");
        }
    }

    #[test]
    fn target_ids_are_escaped_into_the_path() {
        assert_eq!(encode("my target/1"), "my%20target%2F1");
        assert_eq!(encode("gateway"), "gateway");
    }

    /// Not a URL at all is a usage error, reported as one.
    #[test]
    fn a_bad_attach_url_is_refused() {
        let err = match RemoteApi::new("not a url", None) {
            Ok(_) => panic!("a non-URL must not be accepted"),
            Err(message) => message,
        };
        assert!(err.contains("--attach needs a URL"), "{err}");
    }

    #[test]
    fn remote_catalog_shapes_are_read_off_the_api_responses() {
        let tools = parse_tools(&json!({
            "tools": [{ "name": "echo", "description": "d" }, { "no": "name" }]
        }));
        assert_eq!(tools.len(), 1, "an entry with no name is not a tool");
        assert_eq!(tools[0].name, "echo");

        let prompts = parse_prompts(&json!({
            "prompts": [{
                "name": "greet",
                "arguments": [
                    { "name": "who", "required": true },
                    { "name": "tone" },
                ],
            }]
        }));
        assert_eq!(
            prompts[0].required_args(),
            vec!["who"],
            "optional args excluded"
        );

        let templates = parse_templates(&json!({
            "resourceTemplates": [{ "uriTemplate": "orders://{id}", "name": "order" }]
        }));
        assert!(templates[0].is_template);
        assert_eq!(templates[0].uri, "orders://{id}");
    }

    #[test]
    fn a_remote_session_state_survives_the_round_trip() {
        let row = remote_target(&json!({
            "id": "gateway",
            "spec": { "url": "http://127.0.0.1:8787/mcp" },
            "session": { "state": "ready", "negotiated_version": "2026-07-28" },
        }));
        assert_eq!(row.id, "gateway");
        assert_eq!(row.endpoint, "http://127.0.0.1:8787/mcp");
        assert!(matches!(
            row.session,
            SessionView::Ready { ref negotiated_version } if negotiated_version == "2026-07-28"
        ));

        let stdio = remote_target(&json!({
            "id": "local",
            "spec": { "command": "python3" },
            "session": { "state": "failed", "message": "boom" },
        }));
        assert_eq!(stdio.endpoint, "stdio:python3");
        assert!(matches!(stdio.session, SessionView::Failed { .. }));
    }
}
