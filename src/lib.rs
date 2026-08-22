//! The terminal face of mcpg-inspector.
//!
//! Knows nothing about the engine. Everything it displays arrives through
//! [`api::InspectorApi`], which the server implements over its own engine and
//! [`api::RemoteApi`] implements over an inspector's HTTP API — so the same
//! screens serve a target dialed from this process and one held open by an
//! instance across the network.
//!
//! [`state`] and [`draw`] hold everything worth testing and touch no
//! terminal; this file is the IO shell around them — raw mode, the event
//! loop, and the async calls a keypress asks for.

pub mod api;
pub mod draw;
pub mod schema;
pub mod state;
pub mod view;

use std::io::stdout;
use std::sync::Arc;

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use api::{InspectorApi, RemoteApi};
use state::{Action, AppState};

#[derive(clap::Args, Debug)]
pub struct TuiArgs {
    /// Target to inspect (repeatable): an http(s) URL,
    /// `stdio:<command> [args…]`, or a JSON target object
    #[arg(long = "target", value_name = "SPEC")]
    pub targets: Vec<String>,

    /// Frames retained per target
    #[arg(long, env = "MCPG_INSPECTOR_FRAME_BUFFER", default_value_t = 2_000)]
    pub frame_buffer: usize,

    /// Drive a RUNNING inspector instead of dialing targets from here — the
    /// URL it printed, `?token=` and all. Its targets, its sessions and its
    /// wire log, which is the point: a gateway's supervised sidecar holds the
    /// frames of that gateway's own traffic, and a TUI dialing the gateway
    /// directly opens a fresh session and sees none of them.
    #[arg(long, value_name = "URL")]
    pub attach: Option<String>,
}

/// Run the terminal. `local` builds whatever serves a target dialed from
/// this process; with `--attach` it is never called, because there is no
/// local engine to build.
pub fn run<F>(args: TuiArgs, local: F) -> !
where
    F: FnOnce(&TuiArgs) -> Result<Arc<dyn InspectorApi>, String>,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = match runtime.block_on(main_loop(args, local)) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!(
                "{}",
                serde_json::json!({"error": {"code": "tui", "message": err}})
            );
            1
        }
    };
    std::process::exit(code);
}

async fn main_loop<F>(args: TuiArgs, local: F) -> Result<(), String>
where
    F: FnOnce(&TuiArgs) -> Result<Arc<dyn InspectorApi>, String>,
{
    let api: Arc<dyn InspectorApi> = match &args.attach {
        Some(url) => {
            if !args.targets.is_empty() {
                // Adding a target to someone else's inspector is a different
                // thing from listing your own, and silently ignoring the flag
                // would look like the target failing to connect.
                return Err(
                    "--attach drives a running inspector, whose targets are its \
                            own; --target has nothing to add to it"
                        .to_owned(),
                );
            }
            // The env form is preferred: a token on the command line is
            // readable by every process on the box.
            Arc::new(RemoteApi::new(
                url,
                std::env::var("MCPG_INSPECTOR_TOKEN")
                    .ok()
                    .filter(|t| !t.is_empty()),
            )?)
        }
        None => local(&args)?,
    };

    let mut app = AppState {
        status: match (&args.attach, args.targets.is_empty()) {
            (Some(url), _) => format!("attached to {url}"),
            (None, true) => "no targets — start with --target <url>".to_owned(),
            (None, false) => "press c to connect".to_owned(),
        },
        ..Default::default()
    };
    sync_targets(&*api, &mut app).await;

    let mut terminal = enter_terminal()?;
    let result = event_loop(&mut terminal, &api, &mut app).await;
    // Restore before surfacing any error, or the message lands in a
    // terminal still in raw mode and the shell is left unusable.
    leave_terminal(&mut terminal);
    result
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    api: &Arc<dyn InspectorApi>,
    app: &mut AppState,
) -> Result<(), String> {
    let mut events = EventStream::new();
    // Pushes arrive on their own schedule, so the loop cannot simply await a
    // keypress: it waits on whichever comes first. The sender is held here
    // for the whole loop so the channel never closes between subscriptions.
    let (push_tx, mut push_rx) = tokio::sync::mpsc::channel::<serde_json::Value>(256);
    let mut subscription: Option<Subscription> = None;
    // Operations run OFF this loop and report back here. They have to: a
    // server may answer a call by asking the client something, and under the
    // interactive responder that parks until a human answers. Awaiting the
    // call inline made the human's keyboard the thing that was blocked, so
    // the request could never be answered and the TUI froze for good.
    let (op_tx, mut op_rx) = tokio::sync::mpsc::channel::<OpOutcome>(16);
    // While an operation is in flight the queue of what the server is asking
    // has to reach the screen without a keypress to prompt it.
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(150));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        terminal
            .draw(|frame| draw::draw(frame, app))
            .map_err(|e| format!("draw failed: {e}"))?;

        let event = tokio::select! {
            frame = push_rx.recv() => {
                // `None` is only reachable if the sender were dropped, which
                // it is not; nothing to record rather than an EOF.
                if let Some(frame) = frame {
                    app.record_push(now_ms(), &frame);
                }
                continue;
            }
            outcome = op_rx.recv() => {
                if let Some(outcome) = outcome {
                    app.busy = false;
                    app.remember(outcome.history_row(now_ms()));
                    app.apply_outcome(outcome);
                    if let Some(id) = selected_id(app) {
                        app.wire = api.wire(&id).await;
                    }
                }
                continue;
            }
            _ = tick.tick(), if app.busy || !app.pending.is_empty() => {
                if let Some(id) = selected_id(app) {
                    app.sync_pending(api.pending(&id).await);
                }
                continue;
            }
            terminal_event = events.next() => match terminal_event {
                Some(Ok(event)) => event,
                // A closed stdin ends the session; a decode error does not.
                Some(Err(_)) => continue,
                None => break,
            },
        };
        let Event::Key(key) = event else { continue };
        // Windows reports press AND release; acting on both double-runs
        // every action.
        if key.kind != KeyEventKind::Press {
            continue;
        }
        // Ctrl-C must work even though 'q' does: a terminal program
        // that ignores it feels broken.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            break;
        }
        // Arrow keys double as motion, which is wrong while typing: `l`
        // would insert an `l` into a JSON body. In edit mode only the keys
        // that produce or remove text are forwarded.
        let pressed = match key.code {
            KeyCode::Char(c) => c,
            KeyCode::Enter => '\r',
            KeyCode::Backspace => '\u{7f}',
            KeyCode::Esc if app.editing_args => '\u{1b}',
            KeyCode::Tab if !app.editing_args => '\t',
            KeyCode::Down if !app.editing_args => 'j',
            KeyCode::Up if !app.editing_args => 'k',
            KeyCode::Left if !app.editing_args => 'h',
            KeyCode::Right if !app.editing_args => 'l',
            KeyCode::Esc => {
                app.show_help = false;
                continue;
            }
            _ => continue,
        };

        let action = app.on_key(pressed);
        match action {
            Action::Quit => break,
            Action::None => {}
            Action::Connect => {
                let Some(id) = selected_id(app) else {
                    app.status = "no target selected".to_owned();
                    continue;
                };
                app.busy = true;
                app.status = format!("connecting {id}…");
                // Redraw before awaiting, so the busy state is visible
                // rather than appearing only after the work finishes.
                let _ = terminal.draw(|frame| draw::draw(frame, app));
                match api.connect(&id).await {
                    Ok(()) => app.status = format!("connected {id}"),
                    Err(e) => app.status = format!("connect failed: {e}"),
                }
                app.busy = false;
                sync_targets(&**api, app).await;
                refresh_target(&**api, &id, app).await;
            }
            Action::Disconnect => {
                let Some(id) = selected_id(app) else { continue };
                match api.disconnect(&id).await {
                    Ok(()) => app.status = format!("disconnected {id}"),
                    Err(e) => app.status = format!("disconnect failed: {e}"),
                }
                app.tools.clear();
                // The stream rode the session that just went away, so
                // leaving the indicator lit would claim a subscription
                // that no longer exists.
                subscription = None;
                app.subscribed = false;
                sync_targets(&**api, app).await;
            }
            Action::ToggleSubscription => {
                if subscription.is_some() {
                    subscription = None;
                    app.subscribed = false;
                    app.status = "stopped listening".to_owned();
                    continue;
                }
                let Some(id) = selected_id(app) else {
                    app.status = "no target selected".to_owned();
                    continue;
                };
                let uris = app.watch_uris();
                app.busy = true;
                app.status = "subscribing…".to_owned();
                let _ = terminal.draw(|frame| draw::draw(frame, app));
                match api.subscribe(&id, &uris).await {
                    Ok(mut pushes) => {
                        let tx = push_tx.clone();
                        subscription = Some(Subscription(tokio::spawn(async move {
                            // A full channel means the UI is behind; blocking
                            // here applies backpressure rather than dropping
                            // frames an inspector exists to show.
                            while let Some(frame) = pushes.next().await {
                                if tx.send(frame).await.is_err() {
                                    break;
                                }
                            }
                        })));
                        app.subscribed = true;
                        app.status = if uris.is_empty() {
                            "listening for catalog changes".to_owned()
                        } else {
                            format!("listening — {} watched", uris.len())
                        };
                    }
                    Err(e) => {
                        app.subscribed = false;
                        app.status = format!("subscribe failed: {e}");
                    }
                }
                app.busy = false;
            }
            Action::Diagnose => {
                let Some(id) = selected_id(app) else {
                    app.status = "no target selected".to_owned();
                    continue;
                };
                app.busy = true;
                app.status = "running the protocol checks…".to_owned();
                let _ = terminal.draw(|frame| draw::draw(frame, app));
                match api.checks(&id).await {
                    Ok(report) => {
                        app.status = format!(
                            "{} passed · {} failed · {} skipped",
                            report.passed, report.failed, report.skipped
                        );
                        app.checks = Some(report);
                    }
                    Err(e) => {
                        app.status = format!("checks failed: {e}");
                        app.checks = None;
                    }
                }
                app.busy = false;
            }
            Action::Gateway => {
                let Some(id) = selected_id(app) else {
                    app.status = "no target selected".to_owned();
                    continue;
                };
                app.busy = true;
                app.status = "reading the gateway…".to_owned();
                let _ = terminal.draw(|frame| draw::draw(frame, app));
                match api.gateway(&id).await {
                    Ok(report) => {
                        app.status = format!(
                            "{} · {} · {} plugin(s)",
                            report.service, report.readiness, report.plugin_count
                        );
                        app.gateway = Some(report);
                    }
                    Err(e) => {
                        app.status = format!("gateway: {e}");
                        app.gateway = None;
                    }
                }
                app.busy = false;
            }
            Action::Record => {
                let Some(id) = selected_id(app) else {
                    app.status = "no target selected".to_owned();
                    continue;
                };
                match api.recording(&id).await {
                    Ok(text) => {
                        // A deterministic name in the working directory: a
                        // terminal has nowhere good to ask for a path, and a
                        // name that says which target and when is enough to
                        // find again.
                        let name = format!(
                            "mcpg-inspector-{}-{}.jsonl",
                            id.replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
                            now_ms()
                        );
                        match std::fs::write(&name, text) {
                            Ok(()) => app.status = format!("wrote {name}"),
                            Err(e) => app.status = format!("could not write {name}: {e}"),
                        }
                    }
                    Err(e) => app.status = format!("recording failed: {e}"),
                }
            }
            Action::ReplayHistory => {
                let Some(row) = app.selected_history().cloned() else {
                    app.status = "nothing in the history yet".to_owned();
                    continue;
                };
                // Put the old call back where it was sent from rather than
                // re-sending it: re-running silently is how the difference
                // being compared gets lost.
                let (screen, found) = match row.verb {
                    "called" => (
                        state::Screen::Tools,
                        app.tools.iter().position(|t| t.name == row.subject),
                    ),
                    "rendered" => (
                        state::Screen::Prompts,
                        app.prompts.iter().position(|p| p.name == row.subject),
                    ),
                    _ => (
                        state::Screen::Resources,
                        app.resources.iter().position(|r| r.uri == row.subject),
                    ),
                };
                app.go_to(screen);
                match (screen, found) {
                    (state::Screen::Tools, Some(i)) => app.selected_tool = i,
                    (state::Screen::Prompts, Some(i)) => app.selected_prompt = i,
                    (state::Screen::Resources, Some(i)) => app.selected_resource = i,
                    _ => {}
                }
                app.args = row.args.clone();
                app.status = if found.is_some() {
                    format!("loaded {} — enter to run it again", row.subject)
                } else {
                    // The catalog can have changed since; the arguments are
                    // still worth having, and saying so beats a silent miss.
                    format!(
                        "{} is no longer listed; its arguments are loaded",
                        row.subject
                    )
                };
            }
            Action::Complete => {
                let (Some(reference), Some(field)) =
                    (app.completion_ref(), app.selected_field_name())
                else {
                    app.status =
                        "completions are for prompt arguments and template variables".to_owned();
                    continue;
                };
                let Some(id) = selected_id(app) else { continue };
                let typed = app.field_text(&field);
                app.status = format!("completing {field}…");
                let _ = terminal.draw(|frame| draw::draw(frame, app));
                match api.complete(&id, &reference, &field, &typed).await {
                    Ok(values) if values.is_empty() => {
                        app.suggestions.clear();
                        app.status = format!("no suggestions for {field}");
                    }
                    Ok(values) => {
                        app.status = format!("{} suggestion(s) for {field}", values.len());
                        app.suggestions = values;
                    }
                    Err(e) => {
                        app.suggestions.clear();
                        app.status = format!("completion failed: {e}");
                    }
                }
            }
            Action::Answer | Action::Decline => {
                let Some(row) = app.selected_pending().cloned() else {
                    app.status = "nothing is waiting on an answer".to_owned();
                    continue;
                };
                let Some(id) = selected_id(app) else { continue };
                let answer = if action == Action::Decline {
                    Err("declined by the operator in mcpg-inspector".to_owned())
                } else {
                    match serde_json::from_str::<serde_json::Value>(&app.answer) {
                        Ok(value) => Ok(value),
                        Err(e) => {
                            app.status = format!("answer is not valid JSON: {e} — press a to edit");
                            continue;
                        }
                    }
                };
                let declined = answer.is_err();
                match api.respond(&id, row.id, answer).await {
                    Ok(()) => {
                        app.status = if declined {
                            format!("declined {}", row.method)
                        } else {
                            format!("answered {}", row.method)
                        }
                    }
                    // The call it was blocking may have been abandoned.
                    Err(e) => app.status = format!("{}: {e}", row.method),
                }
                app.sync_pending(api.pending(&id).await);
            }
            Action::Refresh => {
                sync_targets(&**api, app).await;
                if let Some(id) = selected_id(app) {
                    refresh_target(&**api, &id, app).await;
                    app.status = "refreshed".to_owned();
                }
            }
            Action::ReadResource => {
                let Some(_) = app.resources.get(app.selected_resource) else {
                    app.status = "no resource selected".to_owned();
                    continue;
                };
                let Some(id) = selected_id(app) else {
                    app.status = "no target selected".to_owned();
                    continue;
                };
                // A template names nothing until its variables are filled.
                // The form fills them; without one the buffer is the URI.
                let uri = app.resource_uri();
                app.busy = true;
                app.status = format!("reading {uri}…");
                let tx = op_tx.clone();
                let api = Arc::clone(api);
                let started_ms = now_ms();
                tokio::spawn(async move {
                    let result = api.read_resource(&id, &uri).await;
                    let about = OpSubject {
                        verb: "read",
                        subject: uri.clone(),
                        args: serde_json::json!({ "uri": uri }).to_string(),
                        started_ms,
                    };
                    let _ = tx
                        .send(OpOutcome::read(format!("read {uri}"), about, result))
                        .await;
                });
            }
            Action::GetPrompt => {
                let Some(row) = app.prompts.get(app.selected_prompt).cloned() else {
                    app.status = "no prompt selected".to_owned();
                    continue;
                };
                let Some(id) = selected_id(app) else {
                    app.status = "no target selected".to_owned();
                    continue;
                };
                let arguments =
                    match serde_json::from_str::<serde_json::Value>(if app.args.trim().is_empty() {
                        "{}"
                    } else {
                        &app.args
                    }) {
                        Ok(value) => value,
                        Err(e) => {
                            app.status =
                                format!("arguments are not valid JSON: {e} — press a to edit");
                            continue;
                        }
                    };
                app.busy = true;
                app.status = format!("rendering {}…", row.name);
                let name = row.name.clone();
                let tx = op_tx.clone();
                let api = Arc::clone(api);
                let started_ms = now_ms();
                tokio::spawn(async move {
                    let result = api.get_prompt(&id, &name, &arguments).await;
                    let about = OpSubject {
                        verb: "rendered",
                        subject: name.clone(),
                        args: arguments.to_string(),
                        started_ms,
                    };
                    let _ = tx
                        .send(OpOutcome::rendered(
                            format!("rendered {name}"),
                            about,
                            result,
                        ))
                        .await;
                });
            }
            Action::CallTool => {
                let Some(tool) = app.tools.get(app.selected_tool).map(|t| t.name.clone()) else {
                    app.status = "no tool selected".to_owned();
                    continue;
                };
                let Some(id) = selected_id(app) else {
                    app.status = "no target selected".to_owned();
                    continue;
                };
                let arguments =
                    match serde_json::from_str::<serde_json::Value>(if app.args.trim().is_empty() {
                        "{}"
                    } else {
                        &app.args
                    }) {
                        Ok(value) => value,
                        Err(e) => {
                            app.status =
                                format!("arguments are not valid JSON: {e} — press a to edit");
                            continue;
                        }
                    };
                app.busy = true;
                app.status = format!("calling {tool}…");
                let tx = op_tx.clone();
                let api = Arc::clone(api);
                let started_ms = now_ms();
                tokio::spawn(async move {
                    let result = api.call_tool(&id, &tool, &arguments).await;
                    let about = OpSubject {
                        verb: "called",
                        subject: tool.clone(),
                        args: arguments.to_string(),
                        started_ms,
                    };
                    let _ = tx
                        .send(OpOutcome::call(format!("called {tool}"), about, result))
                        .await;
                });
            }
        }
    }
    Ok(())
}

/// What an operation that ran off the event loop came back with.
///
/// The loop cannot await the call itself — see the channel it arrives on —
/// so the work of turning a result into screen state happens here instead.
pub struct OpOutcome {
    /// Status line when it worked; names what was done.
    ok: String,
    /// What was asked, for the history: the verb, the subject, the arguments
    /// as they were sent, and when it started.
    verb: &'static str,
    subject: String,
    args: String,
    started_ms: u64,
    /// Status prefix when it did not. Kept separate because "called echo
    /// failed" is not a sentence.
    fail: &'static str,
    /// Where the body belongs: a tool call and an entity read occupy
    /// different panes, and one must not overwrite the other.
    slot: ResultSlot,
    body: Result<String, String>,
}

pub enum ResultSlot {
    Call,
    Entity,
}

type OpResult = Result<serde_json::Value, String>;

/// What an operation was, for the history it writes.
pub struct OpSubject {
    pub verb: &'static str,
    pub subject: String,
    pub args: String,
    pub started_ms: u64,
}

impl OpOutcome {
    fn new(
        ok: String,
        fail: &'static str,
        slot: ResultSlot,
        about: OpSubject,
        result: OpResult,
    ) -> Self {
        Self {
            ok,
            verb: about.verb,
            subject: about.subject,
            args: about.args,
            started_ms: about.started_ms,
            fail,
            slot,
            body: result.map(|value| serde_json::to_string_pretty(&value).unwrap_or_default()),
        }
    }

    fn call(ok: String, about: OpSubject, result: OpResult) -> Self {
        Self::new(ok, "call failed", ResultSlot::Call, about, result)
    }

    fn read(ok: String, about: OpSubject, result: OpResult) -> Self {
        Self::new(ok, "read failed", ResultSlot::Entity, about, result)
    }

    fn rendered(ok: String, about: OpSubject, result: OpResult) -> Self {
        Self::new(ok, "render failed", ResultSlot::Entity, about, result)
    }

    /// The history row this outcome is, whichever way it went. A failure is
    /// the more interesting half of a history, so it is recorded the same way
    /// a success is.
    pub fn history_row(&self, now_ms: u64) -> state::HistoryRow {
        state::HistoryRow {
            verb: self.verb,
            subject: self.subject.clone(),
            args: self.args.clone(),
            at_ms: self.started_ms,
            took_ms: now_ms.saturating_sub(self.started_ms),
            ok: self.body.is_ok(),
            body: match &self.body {
                Ok(rendered) => rendered.clone(),
                Err(message) => message.clone(),
            },
        }
    }

    pub fn into_parts(self) -> (String, &'static str, ResultSlot, Result<String, String>) {
        (self.ok, self.fail, self.slot, self.body)
    }
}

/// A running push stream, aborted when it is dropped — including on the
/// error paths out of the event loop, where a leaked task would go on
/// holding an upstream connection open.
struct Subscription(tokio::task::JoinHandle<()>);

impl Drop for Subscription {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// The id of whatever is selected, or nothing when the list is empty.
fn selected_id(app: &AppState) -> Option<String> {
    app.selected_target().map(|target| target.id.clone())
}

async fn sync_targets(api: &dyn InspectorApi, app: &mut AppState) {
    app.targets = api.targets().await;
    if app.selected_target >= app.targets.len() {
        app.selected_target = 0;
    }
}

/// Re-read what one target advertises, plus its frame log.
async fn refresh_target(api: &dyn InspectorApi, id: &str, app: &mut AppState) {
    let catalog = api.catalog(id).await;
    app.tools = catalog.tools;
    app.resources = catalog.resources;
    app.prompts = catalog.prompts;
    if app.selected_tool >= app.tools.len() {
        app.selected_tool = 0;
    }
    if app.selected_resource >= app.resources.len() {
        app.selected_resource = 0;
    }
    if app.selected_prompt >= app.prompts.len() {
        app.selected_prompt = 0;
    }
    // A server may implement one surface and not another, which is legal, so
    // one failure must not blank the whole refresh. It must still be VISIBLE
    // though: an empty list that silently means "this call errored" is
    // indistinguishable from one that means "there are none".
    if !catalog.errors.is_empty() {
        app.status = catalog.errors.join(" · ");
    }
    app.wire = api.wire(id).await;
}

fn enter_terminal() -> Result<Terminal<CrosstermBackend<std::io::Stdout>>, String> {
    enable_raw_mode().map_err(|e| format!("cannot enter raw mode: {e}"))?;
    let mut out = stdout();
    crossterm::execute!(out, EnterAlternateScreen)
        .map_err(|e| format!("cannot enter the alternate screen: {e}"))?;
    // A panic between here and the restore would leave the terminal in
    // raw mode with no echo — unusable until `reset`. Put it back first,
    // then let the panic proceed.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(stdout(), LeaveAlternateScreen);
        previous(info);
    }));
    Terminal::new(CrosstermBackend::new(out)).map_err(|e| format!("terminal setup failed: {e}"))
}

fn leave_terminal(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) {
    let _ = disable_raw_mode();
    let _ = crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
}
