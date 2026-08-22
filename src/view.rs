//! What the screens display.
//!
//! These are the terminal's own types, not the engine's. That is the point of
//! the split: the interface states what it needs to draw, and whoever serves
//! it — the engine in this process, or an inspector across an HTTP
//! connection — maps its own shapes onto these. Neither can quietly reach
//! into the other.
//!
//! Every type here deserializes from the JSON the inspector's API already
//! emits, so the remote adapter is a parse rather than a translation layer.

use serde::{Deserialize, Serialize};

/// Where a target's session is in its lifecycle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum SessionView {
    Idle,
    Connecting,
    Ready { negotiated_version: String },
    Failed { message: String },
}

/// One observed wire frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireRow {
    pub seq: u64,
    pub ts_ms: u64,
    /// `sent` or `received`.
    pub direction: String,
    pub channel: String,
    pub body: String,
}

impl WireRow {
    pub fn is_sent(&self) -> bool {
        self.direction == "sent"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Pass,
    Fail,
    /// Not applicable to the negotiated wire — which is not a failure, and
    /// the screen has to keep the difference.
    Skip,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckRow {
    pub id: String,
    pub description: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckSummary {
    pub protocol_version: String,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub checks: Vec<CheckRow>,
}

/// What the mcpg gateway behind a target says about itself. Mirrors the
/// server's `GatewayReport`; the attached TUI reads it back off the API rather
/// than fetching `/runtime` itself.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GatewayView {
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub uptime_secs: i64,
    #[serde(default)]
    pub readiness: String,
    #[serde(default)]
    pub failing_checks: Vec<GatewayCheckRow>,
    #[serde(default)]
    pub log_level: String,
    #[serde(default)]
    pub plugin_count: usize,
    #[serde(default)]
    pub plugins: Vec<GatewayPluginRow>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GatewayCheckRow {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GatewayPluginRow {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub state: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The remote adapter parses these straight out of the API's responses,
    /// so the field names are a contract with the server, not a local
    /// convenience.
    #[test]
    fn the_view_types_parse_the_api_json_as_it_is_emitted() {
        let session: SessionView =
            serde_json::from_str(r#"{"state":"ready","negotiated_version":"2026-07-28"}"#)
                .expect("ready");
        assert!(matches!(session, SessionView::Ready { .. }));

        let session: SessionView = serde_json::from_str(r#"{"state":"idle"}"#).expect("idle");
        assert_eq!(session, SessionView::Idle);

        let frame: WireRow = serde_json::from_str(
            r#"{"seq":1,"ts_ms":10,"direction":"sent","channel":"http-request","body":"{}"}"#,
        )
        .expect("frame");
        assert!(frame.is_sent());

        let report: CheckSummary = serde_json::from_str(
            r#"{"protocol_version":"2026-07-28","passed":1,"failed":0,"skipped":1,
                "checks":[{"id":"a","description":"d","outcome":"pass"},
                          {"id":"b","description":"e","outcome":"skip","detail":"why"}]}"#,
        )
        .expect("report");
        assert_eq!(report.checks[0].outcome, Outcome::Pass);
        assert_eq!(report.checks[1].outcome, Outcome::Skip);
        assert_eq!(report.checks[1].detail.as_deref(), Some("why"));
    }
}
