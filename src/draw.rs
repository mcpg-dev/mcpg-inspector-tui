//! Rendering. A pure function of [`AppState`], so every screen can be
//! asserted against a `TestBackend` with no terminal and no server.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use super::state::{AppState, Screen};
use super::view::{Outcome, SessionView};

pub fn draw(frame: &mut Frame, state: &AppState) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_header(frame, areas[0], state);
    match state.screen() {
        Screen::Targets => draw_targets(frame, areas[1], state),
        Screen::Tools => draw_tools(frame, areas[1], state),
        Screen::Resources => draw_resources(frame, areas[1], state),
        Screen::Prompts => draw_prompts(frame, areas[1], state),
        Screen::Subs => draw_subs(frame, areas[1], state),
        Screen::History => draw_history(frame, areas[1], state),
        Screen::Pending => draw_pending(frame, areas[1], state),
        Screen::Diagnose => draw_diagnose(frame, areas[1], state),
        Screen::Wire => draw_wire(frame, areas[1], state),
    }
    draw_footer(frame, areas[2], state);

    if state.show_help {
        draw_help(frame, frame.area());
    }
}

fn draw_header(frame: &mut Frame, area: Rect, state: &AppState) {
    let mut spans = vec![Span::styled(
        "mcpg-inspector",
        Style::default().add_modifier(Modifier::BOLD),
    )];
    for screen in Screen::ALL {
        let selected = state.screen() == screen;
        spans.push(Span::raw("  "));
        // A waiting request is holding a call open, so the tab says so from
        // wherever the user is standing.
        let waiting = screen == Screen::Pending && !state.pending.is_empty();
        // A push that arrived while you were elsewhere is only noticeable if
        // the tab says so.
        let pushed = screen == Screen::Subs && state.unseen_pushes > 0;
        let label = if waiting {
            format!("{}({})", screen.title(), state.pending.len())
        } else if pushed {
            format!("{}({})", screen.title(), state.unseen_pushes)
        } else {
            screen.title().to_owned()
        };
        spans.push(Span::styled(
            label,
            if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if waiting {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if pushed {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ));
    }
    if let Some(target) = state.selected_target() {
        spans.push(Span::raw("   "));
        spans.push(session_span(&target.session));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn session_span(session: &SessionView) -> Span<'static> {
    match session {
        SessionView::Ready { negotiated_version } => Span::styled(
            negotiated_version.clone(),
            Style::default().fg(Color::Green),
        ),
        SessionView::Failed { .. } => Span::styled("failed", Style::default().fg(Color::Red)),
        SessionView::Connecting => Span::styled("connecting…", Style::default().fg(Color::Yellow)),
        SessionView::Idle => Span::styled("idle", Style::default().fg(Color::DarkGray)),
    }
}

fn draw_targets(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.targets.is_empty() {
        return frame.render_widget(
            Paragraph::new("No targets. Pass --target when starting the TUI.")
                .block(bordered("targets")),
            area,
        );
    }
    let items: Vec<ListItem> = state
        .targets
        .iter()
        .enumerate()
        .map(|(i, target)| {
            let marker = if i == state.selected_target {
                "▸ "
            } else {
                "  "
            };
            ListItem::new(Line::from(vec![
                Span::raw(marker),
                Span::styled(
                    target.id.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    target.endpoint.clone(),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw("  "),
                session_span(&target.session),
            ]))
        })
        .collect();
    frame.render_widget(List::new(items).block(bordered("targets")), area);
}

fn draw_tools(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.tools.is_empty() {
        return frame.render_widget(
            Paragraph::new("No tools listed. Connect a target (c), then refresh (r).")
                .block(bordered("tools")),
            area,
        );
    }
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let items: Vec<ListItem> = state
        .tools
        .iter()
        .enumerate()
        .map(|(i, tool)| {
            let marker = if i == state.selected_tool {
                "▸ "
            } else {
                "  "
            };
            ListItem::new(Line::from(vec![
                Span::raw(marker),
                Span::raw(tool.name.clone()),
            ]))
        })
        .collect();
    frame.render_widget(List::new(items).block(bordered("tools")), columns[0]);

    // The detail column carries the tool, the arguments that will be sent,
    // and the last result — the whole call cycle, because a terminal has no
    // second place to put it.
    let mut detail = state
        .tools
        .get(state.selected_tool)
        .map(|tool| {
            format!(
                "{}\n\n{}",
                tool.name,
                tool.description.clone().unwrap_or_else(|| "—".to_owned())
            )
        })
        .unwrap_or_default();
    // A tool that ships an app: a terminal cannot run the page, and saying so
    // beats pretending there is nothing there. The URI is readable with the
    // resources screen, which is where the HTML and its declared CSP live.
    if let Some(uri) = state
        .tools
        .get(state.selected_tool)
        .and_then(|tool| tool.app_uri.as_ref())
    {
        detail.push_str(&format!(
            "\n\nMCP App  {uri}\nan HTML app — read it on the resources screen; \
             the web UI renders it sandboxed"
        ));
    }
    push_arguments(&mut detail, state);
    push_result(
        &mut detail,
        state,
        state.call_result.as_deref(),
        state
            .tools
            .get(state.selected_tool)
            .and_then(|tool| tool.output_schema.as_ref()),
    );
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .block(bordered("detail")),
        columns[1],
    );
}

/// The arguments, as a form when the schema allows one and as JSON otherwise.
///
/// A form says what a server will accept without reading a schema; raw JSON
/// is the only way to send what the schema does not describe, which is half
/// of what an inspector is for. Neither is a mode to be stuck in, so `f`
/// moves between them and both edit the same value.
fn push_arguments(detail: &mut String, state: &AppState) {
    let fields = state.fields();
    let args = if state.args.is_empty() {
        "{}"
    } else {
        &state.args
    };
    if fields.is_empty() {
        detail.push_str("\n\narguments (a to edit, enter to call)");
        // A caret marks the edit point; without it an empty buffer in edit
        // mode is indistinguishable from one that is not being edited.
        detail.push_str(&format!("\n{args}"));
        if state.editing_args {
            detail.push('▎');
        }
        return;
    }
    if !state.form {
        detail.push_str("\n\narguments — JSON (f for the form, a to edit, enter to call)");
        detail.push_str(&format!("\n{args}"));
        if state.editing_args {
            detail.push('▎');
        }
        return;
    }

    detail.push_str("\n\narguments — form (f JSON · j/k move · a edit · s suggest · enter run)\n");
    let width = fields
        .iter()
        .map(|field| field.name.chars().count())
        .max()
        .unwrap_or(0);
    for (i, field) in fields.iter().enumerate() {
        let marker = if i == state.selected_field {
            "▸"
        } else {
            " "
        };
        let required = if field.required { "*" } else { " " };
        let value = state.field_text(&field.name);
        let shown = if value.is_empty() {
            match field.kind {
                crate::schema::Kind::Enum => field.options.join(" | "),
                _ => "—".to_owned(),
            }
        } else {
            value
        };
        let editing = if i == state.selected_field && state.editing_args {
            "▎"
        } else {
            ""
        };
        detail.push_str(&format!(
            "{marker}{required} {:<width$}  {:<8}  {shown}{editing}\n",
            field.name,
            field.kind.label(),
        ));
    }
    if let Some(field) = fields.get(state.selected_field)
        && let Some(description) = &field.description
    {
        detail.push_str(&format!("\n{description}\n"));
    }
    // What the server suggests for this field, when it was asked. Suggestions
    // are the server's, not a guess made here, which is why they are only
    // shown after `c` fetches them.
    if !state.suggestions.is_empty() {
        detail.push_str(&format!(
            "\nsuggestions: {}\n",
            state.suggestions.join(" · ")
        ));
    }
    // Reported, never enforced: sending what the schema disagrees with is a
    // legitimate thing to do from an inspector.
    let complaints = state.field_problems();
    if !complaints.is_empty() {
        detail.push_str(&format!(
            "\n{} — the call is still allowed\n",
            complaints.join(" · ")
        ));
    }
}

/// The result, read rather than decoded — with the envelope one key away.
///
/// MCP returns two things at once: `content` blocks meant for a person, and,
/// when a tool declares an output schema, `structuredContent` meant for a
/// program. Showing only the envelope makes the reader do the decoding;
/// showing only the reading hides what actually arrived.
fn push_result(
    detail: &mut String,
    state: &AppState,
    body: Option<&str>,
    output_schema: Option<&serde_json::Value>,
) {
    let Some(body) = body else { return };
    let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
    let structured = parsed
        .as_ref()
        .and_then(|value| value.get("structuredContent"));
    let rows = structured
        .map(|value| crate::schema::result_rows(output_schema, value))
        .unwrap_or_default();
    let texts: Vec<&str> = parsed
        .as_ref()
        .and_then(|value| value.get("content"))
        .and_then(serde_json::Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    let readable = !rows.is_empty() || !texts.is_empty();

    if state.raw_result || !readable {
        detail.push_str(if readable {
            "\n\nresult — JSON (J for the reading)\n"
        } else {
            "\n\nresult\n"
        });
        detail.push_str(body);
        return;
    }

    detail.push_str("\n\nresult (J for the raw JSON)\n");
    if parsed
        .as_ref()
        .and_then(|value| value.get("isError"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        detail.push_str("isError\n");
    }
    let width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    for (label, value) in &rows {
        detail.push_str(&format!("{label:<width$}  {value}\n"));
    }
    for text in texts {
        detail.push_str(&format!("\n{text}\n"));
    }
}

/// Resources and templates in one list, flagged. They are the same entity at
/// different degrees of resolution, and a terminal has no room to separate
/// them into panes.
fn draw_resources(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.resources.is_empty() {
        return frame.render_widget(
            Paragraph::new("No resources listed. Connect a target (c), then refresh (r).")
                .block(bordered("resources")),
            area,
        );
    }
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let items: Vec<ListItem> = state
        .resources
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let marker = if i == state.selected_resource {
                "▸ "
            } else {
                "  "
            };
            let kind = if row.is_template { " [template]" } else { "" };
            ListItem::new(Line::from(vec![
                Span::raw(marker),
                Span::raw(row.name.clone().unwrap_or_else(|| row.uri.clone())),
                Span::styled(kind, Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    frame.render_widget(List::new(items).block(bordered("resources")), columns[0]);

    let mut detail = String::new();
    if let Some(row) = state.resources.get(state.selected_resource) {
        detail.push_str(&row.uri);
        detail.push_str("\n\n");
        detail.push_str(row.description.as_deref().unwrap_or("—"));
        detail.push_str("\n\n");
        if row.is_template {
            detail.push_str("uri (a to fill the variables, enter to read)\n");
        } else {
            detail.push_str("uri (a to override, enter to read)\n");
        }
        let shown = if state.args.trim().is_empty() || state.args.trim() == "{}" {
            row.uri.clone()
        } else {
            state.args.clone()
        };
        detail.push_str(&shown);
        if state.editing_args {
            detail.push('▎');
        }
        push_result(&mut detail, state, state.entity_result.as_deref(), None);
    }
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .block(bordered("detail")),
        columns[1],
    );
}

/// Prompts, with the arguments a call needs and what one expands to.
fn draw_prompts(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.prompts.is_empty() {
        return frame.render_widget(
            Paragraph::new("No prompts listed. Connect a target (c), then refresh (r).")
                .block(bordered("prompts")),
            area,
        );
    }
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let items: Vec<ListItem> = state
        .prompts
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let marker = if i == state.selected_prompt {
                "▸ "
            } else {
                "  "
            };
            ListItem::new(Line::from(vec![
                Span::raw(marker),
                Span::raw(row.name.clone()),
            ]))
        })
        .collect();
    frame.render_widget(List::new(items).block(bordered("prompts")), columns[0]);

    let mut detail = String::new();
    if let Some(row) = state.prompts.get(state.selected_prompt) {
        detail.push_str(&row.name);
        detail.push_str("\n\n");
        detail.push_str(row.description.as_deref().unwrap_or("—"));
        let required = row.required_args();
        if !required.is_empty() {
            detail.push_str("\n\nrequired: ");
            detail.push_str(&required.join(", "));
        }
        push_arguments(&mut detail, state);
        push_result(&mut detail, state, state.entity_result.as_deref(), None);
    }
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .block(bordered("detail")),
        columns[1],
    );
}

/// What the server is asking the client, and the answer going back.
///
/// This screen exists because the request on it is blocking a call: the
/// server will not finish until something answers. That is also why an
/// arriving request pulls the user here rather than waiting to be found.
fn draw_pending(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.pending.is_empty() {
        return frame.render_widget(
            Paragraph::new(
                "Nothing is waiting.\n\n\
                 When a server asks this client for something — to sample a model, to \
                 elicit a value, to list roots — the request lands here and the call \
                 that triggered it waits for your answer. Under any responder policy \
                 other than interactive it is answered without you, and this stays empty.",
            )
            .wrap(Wrap { trim: false })
            .block(bordered("pending")),
            area,
        );
    }
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(area);

    let items: Vec<ListItem> = state
        .pending
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let marker = if i == state.selected_pending {
                "▸ "
            } else {
                "  "
            };
            ListItem::new(Line::from(vec![
                Span::raw(marker),
                Span::styled(row.method.clone(), Style::default().fg(Color::Yellow)),
            ]))
        })
        .collect();
    frame.render_widget(
        List::new(items).block(bordered(&format!("pending ({})", state.pending.len()))),
        columns[0],
    );

    let mut detail = String::new();
    if let Some(row) = state.pending.get(state.selected_pending) {
        detail.push_str(&row.method);
        detail.push_str("  (");
        detail.push_str(&row.regime);
        detail.push_str(" wire)\n\nthe server sent\n");
        detail.push_str(&row.params);
        detail.push_str("\n\nyour answer (a to edit, enter to send, x to decline)\n");
        detail.push_str(&state.answer);
        if state.editing_args {
            detail.push('▎');
        }
    }
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .block(bordered("request")),
        columns[1],
    );
}

/// Everything asked of this server, and what came back.
///
/// The value is comparison: two calls that differ in one argument, and which
/// of them returned the thing you wanted. So the list keeps both and `enter`
/// puts an old call's arguments back in the screen that sent it — rather than
/// re-sending, which would lose the difference being chased.
fn draw_history(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.history.is_empty() {
        return frame.render_widget(
            Paragraph::new(
                "Nothing called yet.\n\n\
                 Every tool call, resource read and prompt render lands here — with what \
                 was sent, what came back, and how long it took. `enter` loads one back \
                 into the screen that sent it.",
            )
            .wrap(Wrap { trim: false })
            .block(bordered("history")),
            area,
        );
    }
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    let width = state
        .history
        .iter()
        .map(|row| row.subject.chars().count())
        .max()
        .unwrap_or(0)
        .min(32);
    let items: Vec<ListItem> = state
        .history
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let marker = if i == state.selected_history {
                "▸ "
            } else {
                "  "
            };
            ListItem::new(Line::from(vec![
                Span::raw(marker),
                Span::styled(
                    if row.ok { "ok  " } else { "err " },
                    Style::default().fg(if row.ok { Color::Green } else { Color::Red }),
                ),
                Span::raw(format!("{:<width$}  ", row.subject)),
                Span::styled(
                    format!("{}ms", row.took_ms),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    frame.render_widget(
        List::new(items).block(bordered(&format!("history ({})", state.history.len()))),
        columns[0],
    );

    let mut detail = String::new();
    if let Some(row) = state.selected_history() {
        detail.push_str(&format!("{} {}\n", row.verb, row.subject));
        detail.push_str(&format!(
            "{}  ·  {}ms  ·  {}\n\nsent\n{}\n\n{}\n{}",
            clock(row.at_ms),
            row.took_ms,
            if row.ok { "ok" } else { "failed" },
            row.args,
            if row.ok { "result" } else { "error" },
            row.body,
        ));
        detail.push_str("\n\nenter loads this back into the screen that sent it");
    }
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .block(bordered("entry")),
        columns[1],
    );
}

/// A live subscription: what is being watched, and what has arrived.
fn draw_subs(frame: &mut Frame, area: Rect, state: &AppState) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(3)])
        .split(area);

    let watched = state.watch_uris();
    let mut header = Line::from(vec![
        Span::styled(
            if state.subscribed {
                "● listening"
            } else {
                "○ idle"
            },
            Style::default().fg(if state.subscribed {
                Color::Green
            } else {
                Color::DarkGray
            }),
        ),
        Span::raw("  enter to "),
        Span::raw(if state.subscribed { "stop" } else { "start" }),
        Span::raw(", a to edit the watch list"),
    ]);
    if state.editing_args {
        header = Line::from("editing the watch list — comma-separated URIs, enter to accept");
    }
    let watching = if watched.is_empty() {
        // Not a degenerate case: catalog notifications need no URI, so an
        // empty watch list is a real subscription, not a broken one.
        "catalog changes only (tools, prompts, resources list-changed)".to_owned()
    } else {
        format!(
            "catalog changes + {} resource{}: {}",
            watched.len(),
            if watched.len() == 1 { "" } else { "s" },
            watched.join(", ")
        )
    };
    let mut body = vec![header, Line::from(watching)];
    if state.editing_args {
        body.push(Line::from(format!("{}▎", state.watch)));
    }
    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .block(bordered("subscription")),
        rows[0],
    );

    if state.pushes.is_empty() {
        return frame.render_widget(
            Paragraph::new(if state.subscribed {
                "Listening. Nothing pushed yet."
            } else {
                "No pushes. Connect a target (c), then press enter to listen."
            })
            .block(bordered("pushes")),
            rows[1],
        );
    }
    let capacity = rows[1].height.saturating_sub(2) as usize;
    let start = state.pushes.len().saturating_sub(capacity);
    let shown = &state.pushes[start..];
    // Method names are long enough to overrun any fixed column, and one that
    // overruns runs straight into the body with no separator.
    let width = column_width(shown.iter().map(|p| p.method.as_str()));
    let items: Vec<ListItem> = shown
        .iter()
        .map(|push| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} ", clock(push.ts_ms)),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{:<width$}", push.method),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(one_line(&push.body)),
            ]))
        })
        .collect();
    frame.render_widget(
        List::new(items).block(bordered(&format!("pushes ({})", state.pushes.len()))),
        rows[1],
    );
}

/// The protocol checks, and what they found.
fn draw_diagnose(frame: &mut Frame, area: Rect, state: &AppState) {
    let mut lines = gateway_lines(state);

    let Some(report) = &state.checks else {
        lines.push(Line::from(
            "No report yet. Connect an HTTP target (c), then press enter to run the checks, \
             or g to read the gateway behind it.",
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "The checks assert transport rules a client call would paper over, so they build \
             their own requests. Checks that do not apply to the negotiated revision are \
             skipped, not failed.",
            Style::default().fg(Color::DarkGray),
        )));
        return frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(bordered("diagnose")),
            area,
        );
    };

    lines.extend(vec![
        Line::from(vec![
            Span::raw("wire "),
            Span::styled(
                report.protocol_version.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(
                format!("{} passed", report.passed),
                Style::default().fg(Color::Green),
            ),
            Span::raw(" · "),
            Span::styled(
                format!("{} failed", report.failed),
                Style::default().fg(if report.failed == 0 {
                    Color::DarkGray
                } else {
                    Color::Red
                }),
            ),
            Span::raw(" · "),
            Span::styled(
                format!("{} skipped", report.skipped),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(""),
    ]);
    let width = column_width(report.checks.iter().map(|c| c.id.as_str()));
    for check in &report.checks {
        let (mark, colour) = match check.outcome {
            Outcome::Pass => ("✓", Color::Green),
            Outcome::Fail => ("✗", Color::Red),
            Outcome::Skip => ("–", Color::DarkGray),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{mark} "), Style::default().fg(colour)),
            Span::styled(format!("{:<width$}", check.id), Style::default().fg(colour)),
            Span::raw(check.description.clone()),
        ]));
        if let Some(detail) = &check.detail {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(detail.clone(), Style::default().fg(Color::DarkGray)),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "The portable subset, not the full conformance suite. enter re-runs; g reads the gateway.",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(bordered("diagnose")),
        area,
    );
}

/// The gateway block that sits above the checks.
///
/// The checks say whether the server speaks MCP correctly. This says whether
/// the thing behind it is actually working — the other half of "my tool is
/// missing", and the half the MCP surface cannot show you.
fn gateway_lines(state: &AppState) -> Vec<Line<'static>> {
    let Some(gateway) = &state.gateway else {
        return Vec::new();
    };
    let degraded =
        !gateway.failing_checks.is_empty() || gateway.plugins.iter().any(|p| p.state != "active");
    let mut lines = vec![Line::from(vec![
        Span::styled(
            gateway.service.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            gateway.version.clone(),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("   "),
        Span::styled(
            gateway.readiness.clone(),
            Style::default().fg(if degraded { Color::Red } else { Color::Green }),
        ),
        Span::styled(
            format!(
                "   up {}   log {}",
                uptime(gateway.uptime_secs),
                gateway.log_level
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ])];

    for check in &gateway.failing_checks {
        lines.push(Line::from(vec![
            Span::styled("✗ ", Style::default().fg(Color::Red)),
            Span::styled(check.name.clone(), Style::default().fg(Color::Red)),
            Span::raw(" "),
            Span::styled(check.status.clone(), Style::default().fg(Color::DarkGray)),
        ]));
        if let Some(detail) = &check.detail {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(detail.clone(), Style::default().fg(Color::DarkGray)),
            ]));
        }
    }

    // Only the plugins that are not active. Twenty green rows push the one red
    // row off a terminal screen, and the red row is why you looked.
    let unhappy: Vec<_> = gateway
        .plugins
        .iter()
        .filter(|p| p.state != "active")
        .collect();
    lines.push(Line::from(Span::styled(
        if unhappy.is_empty() {
            format!("{} plugin(s) loaded, all active", gateway.plugin_count)
        } else {
            format!(
                "{} plugin(s) loaded, {} not active",
                gateway.plugin_count,
                unhappy.len()
            )
        },
        Style::default().fg(if unhappy.is_empty() {
            Color::DarkGray
        } else {
            Color::Yellow
        }),
    )));
    let width = column_width(unhappy.iter().map(|p| p.id.as_str()));
    for plugin in unhappy {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<width$}", plugin.id),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(plugin.state.clone(), Style::default().fg(Color::Red)),
            Span::styled(
                format!("  {}", plugin.class),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines
}

/// Seconds as something a person reads at a glance.
fn uptime(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h{}m", s / 3600, (s % 3600) / 60),
        s => format!("{}d{}h", s / 86_400, (s % 86_400) / 3600),
    }
}

/// Width for a left-hand column, from what is actually in it plus a
/// separating space. A fixed width silently runs the longest entry into the
/// next column, with nothing between them.
fn column_width<'a>(entries: impl Iterator<Item = &'a str>) -> usize {
    entries.map(|e| e.chars().count()).max().unwrap_or(0) + 1
}

/// `HH:MM:SS` from an epoch-millisecond stamp. Enough to correlate a push
/// with something that just happened; a date would only cost width.
fn clock(ts_ms: u64) -> String {
    let secs = ts_ms / 1_000 % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3_600,
        secs % 3_600 / 60,
        secs % 60
    )
}

fn draw_wire(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.wire.is_empty() {
        return frame.render_widget(
            Paragraph::new("No frames yet. Connect a target (c) to see its traffic.")
                .block(bordered("wire")),
            area,
        );
    }
    // The tail: the newest frames that fit, oldest first, so reading
    // top-to-bottom follows the conversation.
    let capacity = area.height.saturating_sub(2) as usize;
    let start = state.wire.len().saturating_sub(capacity);
    let items: Vec<ListItem> = state.wire[start..]
        .iter()
        .map(|event| {
            let sent = event.is_sent();
            ListItem::new(Line::from(vec![
                Span::styled(
                    if sent { "→ " } else { "← " },
                    Style::default().fg(if sent { Color::Blue } else { Color::Green }),
                ),
                Span::styled(
                    format!("{:<14}", event.channel),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(one_line(&event.body)),
            ]))
        })
        .collect();
    frame.render_widget(List::new(items).block(bordered("wire")), area);
}

/// Collapse a frame to a single line. Server-supplied text is data, so
/// control characters are stripped rather than written to the terminal.
fn one_line(body: &str) -> String {
    let flattened: String = body
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flattened.trim();
    if trimmed.chars().count() <= 200 {
        return trimmed.to_owned();
    }
    trimmed.chars().take(200).collect::<String>() + "…"
}

fn draw_footer(frame: &mut Frame, area: Rect, state: &AppState) {
    let keys = if state.editing_args {
        "editing arguments · enter accept · esc cancel"
    } else {
        if state.screen() == Screen::Pending && !state.pending.is_empty() {
            "enter answer · x decline · a edit the answer · tab screen · q quit"
        } else {
            "q quit · tab · j/k · c connect · r refresh · a edit · f form · J raw · enter run · ? help"
        }
    };
    let status = if state.busy {
        format!("… {}", state.status)
    } else {
        state.status.clone()
    };
    let text = if status.is_empty() {
        keys.to_owned()
    } else {
        format!("{status}   │   {keys}")
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let width = area.width.min(64);
    let height = area.height.min(18);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    let body = "\
q         quit
tab / l   next screen
h         previous screen
j / k     move the selection
c         connect the selected target
d         disconnect it
r         refresh (re-list every surface, re-read frames)
a         edit arguments, or the watch list
f         form / JSON, when a schema describes the arguments
s         in a form: ask the server what completes this field
w         write this exchange out as a recording
J         the result as it arrived, or as read
g         on diagnose: what the mcpg gateway behind
          this target says about itself
enter     run this screen — call a tool, read a
          resource, render a prompt, listen for
          pushes, run the protocol checks
?         close this help";
    frame.render_widget(
        Paragraph::new(body)
            .block(bordered("keys"))
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn bordered(title: &str) -> Block<'_> {
    Block::default().borders(Borders::ALL).title(title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{TargetRow, ToolRow};
    use crate::view::{GatewayCheckRow, GatewayPluginRow, GatewayView, WireRow};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::state::{PendingRow, PushRow};
    use crate::view::{CheckRow, CheckSummary};

    /// Screens by name. An index is a moving target — inserting a screen
    /// renumbers every test that hardcoded one.
    fn at(screen: Screen) -> AppState {
        AppState {
            screen_index: Screen::ALL.iter().position(|s| *s == screen).unwrap(),
            ..Default::default()
        }
    }

    fn render(state: &AppState) -> String {
        let mut terminal = Terminal::new(TestBackend::new(90, 12)).unwrap();
        terminal.draw(|frame| draw(frame, state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn degraded_gateway() -> GatewayView {
        GatewayView {
            service: "mcpg".into(),
            version: "1.0.0-rc.1".into(),
            uptime_secs: 4_000,
            readiness: "degraded".into(),
            failing_checks: vec![GatewayCheckRow {
                name: "backend:orders".into(),
                status: "fail".into(),
                detail: Some("connection refused".into()),
            }],
            log_level: "info".into(),
            plugin_count: 12,
            plugins: vec![
                GatewayPluginRow {
                    id: "dev.mcpg.backend.sql".into(),
                    version: "1.0.0".into(),
                    class: "backend".into(),
                    state: "degraded".into(),
                },
                GatewayPluginRow {
                    id: "dev.mcpg.identity.oidc".into(),
                    version: "1.0.0".into(),
                    class: "identity".into(),
                    state: "active".into(),
                },
            ],
        }
    }

    #[test]
    fn the_gateway_block_leads_with_what_is_wrong() {
        let state = AppState {
            gateway: Some(degraded_gateway()),
            ..at(Screen::Diagnose)
        };
        let screen = render(&state);
        assert!(screen.contains("degraded"), "{screen}");
        assert!(screen.contains("backend:orders"), "{screen}");
        assert!(screen.contains("connection refused"), "{screen}");
        // The count is the gateway's; the list is only what is not active.
        assert!(
            screen.contains("12 plugin(s) loaded, 1 not active"),
            "{screen}"
        );
        assert!(screen.contains("dev.mcpg.backend.sql"), "{screen}");
        assert!(
            !screen.contains("dev.mcpg.identity.oidc"),
            "an active plugin is not the finding, and crowds out the one that is: {screen}"
        );
    }

    #[test]
    fn a_healthy_gateway_says_so_in_one_line() {
        let mut gateway = degraded_gateway();
        gateway.readiness = "ready".into();
        gateway.failing_checks.clear();
        gateway
            .plugins
            .iter_mut()
            .for_each(|p| p.state = "active".into());
        let state = AppState {
            gateway: Some(gateway),
            ..at(Screen::Diagnose)
        };
        let screen = render(&state);
        assert!(
            screen.contains("12 plugin(s) loaded, all active"),
            "{screen}"
        );
        assert!(
            screen.contains("up 1h6m"),
            "uptime should read as time: {screen}"
        );
    }

    #[test]
    fn targets_screen_shows_each_target_and_its_state() {
        let state = AppState {
            targets: vec![
                TargetRow {
                    id: "gateway".into(),
                    endpoint: "http://127.0.0.1:8787/mcp".into(),
                    session: SessionView::Ready {
                        negotiated_version: "2026-07-28".into(),
                    },
                },
                TargetRow {
                    id: "other".into(),
                    endpoint: "http://elsewhere/mcp".into(),
                    session: SessionView::Idle,
                },
            ],
            ..Default::default()
        };
        let screen = render(&state);
        assert!(screen.contains("gateway"), "{screen}");
        assert!(screen.contains("2026-07-28"), "{screen}");
        assert!(screen.contains("other"), "{screen}");
        assert!(screen.contains("▸ gateway"), "selection marker: {screen}");
        assert!(screen.contains("q quit"), "footer keys: {screen}");
    }

    #[test]
    fn empty_screens_say_what_to_do_instead_of_going_blank() {
        let screen = render(&AppState::default());
        assert!(screen.contains("No targets"), "{screen}");

        assert!(render(&at(Screen::Tools)).contains("Connect a target"));
        assert!(render(&at(Screen::Wire)).contains("No frames yet"));
        assert!(render(&at(Screen::Subs)).contains("No pushes"));
        assert!(render(&at(Screen::Diagnose)).contains("No report yet"));
    }

    #[test]
    fn tools_screen_lists_names_with_the_selected_detail() {
        let state = AppState {
            tools: vec![
                ToolRow {
                    name: "dev.mock.echo".into(),
                    description: Some("Echo a canned value".into()),
                    input_schema: None,
                    output_schema: None,
                    app_uri: None,
                },
                ToolRow {
                    name: "other.tool".into(),
                    description: None,
                    input_schema: None,
                    output_schema: None,
                    app_uri: None,
                },
            ],
            ..at(Screen::Tools)
        };
        let screen = render(&state);
        assert!(screen.contains("dev.mock.echo"), "{screen}");
        assert!(screen.contains("Echo a canned value"), "detail: {screen}");
    }

    #[test]
    fn wire_screen_tails_frames_with_direction() {
        let state = AppState {
            wire: vec![
                WireRow {
                    seq: 1,
                    ts_ms: 0,
                    direction: "sent".into(),
                    channel: "http-request".into(),
                    body: "{\"method\":\"tools/list\"}".into(),
                },
                WireRow {
                    seq: 2,
                    ts_ms: 0,
                    direction: "received".into(),
                    channel: "http-response".into(),
                    body: "{\"result\":{}}".into(),
                },
            ],
            ..at(Screen::Wire)
        };
        let screen = render(&state);
        assert!(screen.contains("→"), "{screen}");
        assert!(screen.contains("←"), "{screen}");
        assert!(screen.contains("tools/list"), "{screen}");
    }

    #[test]
    fn server_text_cannot_write_control_characters_to_the_terminal() {
        // A frame body is server-supplied. Escape sequences in it must
        // never reach the terminal.
        let line = one_line("before\u{1b}[31m\u{7}after\nnext");
        assert!(!line.contains('\u{1b}'), "{line}");
        assert!(!line.contains('\u{7}'), "{line}");
        assert!(line.contains("before"), "{line}");
        assert!(line.contains("after"), "{line}");
    }

    #[test]
    fn help_overlays_the_screen_when_asked() {
        let state = AppState {
            show_help: true,
            ..Default::default()
        };
        let screen = render(&state);
        assert!(screen.contains("connect the selected target"), "{screen}");
    }

    /// The subscription screen must say what it is watching, not merely that
    /// something is on: an empty watch list is a real subscription (catalog
    /// changes need no URI), and it would otherwise look broken.
    #[test]
    fn subs_screen_names_what_it_watches_and_tails_the_pushes() {
        let idle = at(Screen::Subs);
        let screen = render(&idle);
        assert!(screen.contains("○ idle"), "{screen}");
        assert!(screen.contains("catalog changes only"), "{screen}");

        let live = AppState {
            subscribed: true,
            watch: "docs://runbook, docs://oncall".into(),
            pushes: vec![
                PushRow {
                    ts_ms: 3_661_000,
                    method: "notifications/resources/updated".into(),
                    body: "{\"params\":{\"uri\":\"docs://runbook\"}}".into(),
                },
                PushRow {
                    ts_ms: 3_662_000,
                    method: "notifications/subscriptions/acknowledged".into(),
                    body: "{\"params\":{\"ok\":true}}".into(),
                },
            ],
            ..at(Screen::Subs)
        };
        let screen = render(&live);
        assert!(screen.contains("● listening"), "{screen}");
        assert!(screen.contains("2 resources"), "{screen}");
        assert!(screen.contains("docs://runbook"), "{screen}");
        assert!(
            screen.contains("notifications/resources/updated"),
            "{screen}"
        );
        assert!(screen.contains("01:01:01"), "push clock: {screen}");
        assert!(
            screen.contains("notifications/subscriptions/acknowledged {"),
            "the longest method must not run into the body: {screen}"
        );
    }

    /// A skipped check must not read as a failure — asserting a sessionful
    /// rule against a stateless server is a false failure, and the screen is
    /// where that distinction has to survive.
    #[test]
    fn diagnose_screen_separates_failed_from_skipped() {
        let state = AppState {
            checks: Some(CheckSummary {
                protocol_version: "2026-07-28".into(),
                passed: 1,
                failed: 1,
                skipped: 1,
                checks: vec![
                    CheckRow {
                        id: "post-accept".into(),
                        description: "POST advertises both media types".into(),
                        outcome: Outcome::Pass,
                        detail: None,
                    },
                    CheckRow {
                        id: "origin-rejected".into(),
                        description: "a foreign Origin is refused".into(),
                        outcome: Outcome::Fail,
                        detail: Some("answered 200".into()),
                    },
                    CheckRow {
                        id: "session-header".into(),
                        description: "Mcp-Session-Id is returned".into(),
                        outcome: Outcome::Skip,
                        detail: Some("sessionful only".into()),
                    },
                    // Longer than any fixed column would allow; a real check
                    // id runs to this length.
                    CheckRow {
                        id: "protocol-version-header-required".into(),
                        description: "a bad version header is refused".into(),
                        outcome: Outcome::Pass,
                        detail: None,
                    },
                ],
            }),
            ..at(Screen::Diagnose)
        };
        let screen = render(&state);
        assert!(screen.contains("1 passed"), "{screen}");
        assert!(screen.contains("1 failed"), "{screen}");
        assert!(screen.contains("1 skipped"), "{screen}");
        assert!(screen.contains("✓ post-accept"), "{screen}");
        assert!(screen.contains("✗ origin-rejected"), "{screen}");
        assert!(screen.contains("– session-header"), "{screen}");
        assert!(screen.contains("answered 200"), "detail: {screen}");
        // The id column is sized from its contents: the longest id must not
        // run into the description with no space between them.
        assert!(
            screen.contains("protocol-version-header-required a bad version header"),
            "long id collides with the description: {screen}"
        );
        assert!(
            screen.contains("post-accept                      POST advertises"),
            "short ids still line up with the long one: {screen}"
        );
    }

    /// The pending screen has to show what was asked and what is going back,
    /// and the tab strip has to say something is waiting from anywhere else —
    /// the request is blocking a call no matter which screen is on.
    #[test]
    fn pending_shows_the_request_and_flags_itself_from_other_screens() {
        assert!(render(&at(Screen::Pending)).contains("Nothing is waiting"));

        let waiting = vec![PendingRow {
            id: 1,
            method: "sampling/createMessage".into(),
            params: "{\n  \"maxTokens\": 64\n}".into(),
            regime: "sessionful".into(),
        }];
        let state = AppState {
            pending: waiting.clone(),
            answer: "{\"role\":\"assistant\"}".into(),
            ..at(Screen::Pending)
        };
        let screen = render(&state);
        assert!(screen.contains("sampling/createMessage"), "{screen}");
        assert!(screen.contains("sessionful wire"), "{screen}");
        assert!(screen.contains("maxTokens"), "what was asked: {screen}");
        assert!(screen.contains("x to decline"), "{screen}");
        assert!(screen.contains("enter answer"), "footer: {screen}");

        // From another screen the badge is the only signal.
        let elsewhere = AppState {
            pending: waiting,
            ..at(Screen::Wire)
        };
        let screen = render(&elsewhere);
        assert!(screen.contains("pending(1)"), "tab badge: {screen}");
    }
}
