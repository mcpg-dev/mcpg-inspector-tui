//! What the TUI shows, and how a keypress changes it.
//!
//! Deliberately free of terminal IO and of the engine: state in, state
//! out. That is what makes the screens testable against a
//! `TestBackend` without a terminal or a live server.

use crate::view::{CheckSummary, GatewayView, SessionView, WireRow};

/// Pushes kept on screen. A chatty server would otherwise grow this without
/// bound for as long as the TUI is left running.
const PUSH_CAP: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Targets,
    Tools,
    Resources,
    Prompts,
    Subs,
    History,
    Pending,
    Diagnose,
    Wire,
}

impl Screen {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Targets => "targets",
            Self::Tools => "tools",
            Self::Resources => "resources",
            Self::Prompts => "prompts",
            Self::Subs => "subs",
            Self::History => "history",
            Self::Pending => "pending",
            Self::Diagnose => "diagnose",
            Self::Wire => "wire",
        }
    }

    /// Tab order, and the single place it is written down. Screen movement
    /// used to be spelled four times — a `next`, a `previous`, a modulo and
    /// two index maps — which is four chances for a new screen to be
    /// reachable one way and not the other.
    pub const ALL: [Screen; 9] = [
        Screen::Targets,
        Screen::Tools,
        Screen::Resources,
        Screen::Prompts,
        Screen::Subs,
        Screen::History,
        Screen::Pending,
        Screen::Diagnose,
        Screen::Wire,
    ];
}

/// One row of the targets list.
#[derive(Clone, Debug)]
pub struct TargetRow {
    pub id: String,
    pub endpoint: String,
    pub session: SessionView,
}

/// One row of the tools list.
#[derive(Clone, Debug)]
pub struct ToolRow {
    pub name: String,
    pub description: Option<String>,
    /// What the server says it accepts — the form is built from this.
    pub input_schema: Option<serde_json::Value>,
    /// What it says it returns, which is what labels a structured result.
    pub output_schema: Option<serde_json::Value>,
    /// SEP-1865: the `ui://` resource holding this tool's app, when it ships
    /// one. A terminal cannot render the page, but which page it is — and
    /// what it declares it may reach — is exactly what an inspector is for.
    pub app_uri: Option<String>,
}

/// One row of the resources list. Templates ride the same list, flagged,
/// because they are the same entity at a different degree of resolution
/// and a terminal has no room for a second pane to separate them.
#[derive(Clone, Debug)]
pub struct ResourceRow {
    /// URI for a resource, URI *template* for a template.
    pub uri: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub is_template: bool,
}

/// One row of the prompts list.
#[derive(Clone, Debug)]
pub struct PromptRow {
    pub name: String,
    pub description: Option<String>,
    /// Every argument the prompt declares, required or not. Carrying only
    /// the required ones made the optional ones unreachable from here — the
    /// form could not offer a field for something it did not know about.
    pub arguments: Vec<crate::schema::PromptArgument>,
}

impl PromptRow {
    /// The names a call cannot omit, for the one-line summary.
    pub fn required_args(&self) -> Vec<&str> {
        self.arguments
            .iter()
            .filter(|argument| argument.required)
            .map(|argument| argument.name.as_str())
            .collect()
    }
}

/// One notification a subscribed server pushed.
#[derive(Clone, Debug)]
pub struct PushRow {
    /// Milliseconds since the epoch, from the same clock the wire log uses.
    pub ts_ms: u64,
    /// JSON-RPC method, or `?` for a frame that carries none.
    pub method: String,
    /// The frame, one line.
    pub body: String,
}

/// One thing already asked of this server.
///
/// The screens show one call at a time — fill, send, read, and the answer
/// before it is gone. That is fine for the first call and wrong for the
/// fifth, which is usually the one that matters, because by then you are
/// comparing rather than calling.
#[derive(Clone, Debug)]
pub struct HistoryRow {
    /// `called` / `read` / `rendered`.
    pub verb: &'static str,
    /// Tool name, resource URI, or prompt name.
    pub subject: String,
    /// What was sent, as it was sent.
    pub args: String,
    pub at_ms: u64,
    pub took_ms: u64,
    pub ok: bool,
    /// The result, or the failure.
    pub body: String,
}

/// One server→client request waiting on an answer.
#[derive(Clone, Debug)]
pub struct PendingRow {
    pub id: u64,
    /// `sampling/createMessage`, `elicitation/create`, `roots/list`.
    pub method: String,
    /// What the server sent, pretty-printed.
    pub params: String,
    /// Which regime produced it; the two differ on timeout and cancellation.
    pub regime: String,
}

/// What the event loop should do after a keypress. Keeping the intent
/// separate from the state change is what lets the pure handler ask
/// for async work it cannot perform itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    Connect,
    Disconnect,
    Refresh,
    /// Call the selected tool with the current arguments.
    CallTool,
    /// Read the selected resource, or expand the selected template with the
    /// current arguments substituted.
    ReadResource,
    /// Render the selected prompt with the current arguments.
    GetPrompt,
    /// Open the push stream, or close the one that is open.
    ToggleSubscription,
    /// Run the protocol checks against the selected target.
    Diagnose,
    /// Read what the mcpg gateway behind the selected target says about
    /// itself.
    Gateway,
    /// Ask the server what would complete the selected field.
    Complete,
    /// Write this target's exchange out as a recording.
    Record,
    /// Load the selected history entry back into the screen that sent it.
    ReplayHistory,
    /// Answer the selected server→client request with the answer buffer.
    Answer,
    /// Decline it — a legitimate answer, not an error.
    Decline,
}

#[derive(Debug, Default)]
pub struct AppState {
    pub screen_index: usize,
    pub targets: Vec<TargetRow>,
    pub selected_target: usize,
    pub tools: Vec<ToolRow>,
    pub selected_tool: usize,
    pub resources: Vec<ResourceRow>,
    pub selected_resource: usize,
    pub prompts: Vec<PromptRow>,
    pub selected_prompt: usize,
    /// Body of the last resource read or prompt render, pretty-printed.
    pub entity_result: Option<String>,
    /// Notifications the subscribed target has pushed, oldest first.
    pub pushes: Vec<PushRow>,
    /// A push stream is open on the selected target.
    pub subscribed: bool,
    /// Report from the last protocol run, if one has been run.
    pub checks: Option<CheckSummary>,
    pub gateway: Option<GatewayView>,
    /// Server→client requests waiting on a human, oldest first.
    pub pending: Vec<PendingRow>,
    pub selected_pending: usize,
    /// The answer being composed for the selected request. Its own buffer
    /// for the same reason the watch list has one: it belongs to a blocked
    /// call, not to wherever the cursor happens to be.
    pub answer: String,
    pub wire: Vec<WireRow>,
    /// Last thing that happened, shown in the status bar.
    pub status: String,
    pub show_help: bool,
    pub busy: bool,
    /// JSON arguments for the next tool call, edited in place.
    pub args: String,
    /// Comma-separated URIs the subscription screen watches. Its own buffer,
    /// not [`Self::args`]: a watch list belongs to the stream it opened, so
    /// it has to survive the screen changes that reseed everything else.
    pub watch: String,
    /// Typing arguments rather than driving the app. A terminal has one
    /// keyboard, so an edit mode is what keeps `j`/`q` from being swallowed
    /// by a JSON body — and what keeps them working when it is not open.
    pub editing_args: bool,
    /// Result of the last call, pretty-printed.
    pub call_result: Option<String>,
    /// Editing field by field rather than composing JSON by hand. A schema
    /// is the only thing that makes this possible, so it is off wherever
    /// there is none.
    pub form: bool,
    pub selected_field: usize,
    /// Field values by name, as text. Parsed to their declared types on the
    /// way out — which is the whole reason to read the schema.
    pub field_values: std::collections::BTreeMap<String, String>,
    /// Show the result as the envelope that arrived rather than as the
    /// reading of it.
    pub raw_result: bool,
    /// What the server last suggested for the selected field.
    pub suggestions: Vec<String>,
    /// Everything asked of this target, newest first.
    pub history: Vec<HistoryRow>,
    pub selected_history: usize,
    /// Pushes that arrived while the subs screen was not on show, so the tab
    /// can say something happened.
    pub unseen_pushes: usize,
}

/// History kept. Long enough to compare a session's worth, bounded so a
/// terminal left open overnight does not grow without limit.
const HISTORY_CAP: usize = 200;

impl AppState {
    pub fn screen(&self) -> Screen {
        Screen::ALL[self.screen_index % Screen::ALL.len()]
    }

    pub fn selected_target(&self) -> Option<&TargetRow> {
        self.targets.get(self.selected_target)
    }

    /// The text `a` edits on this screen.
    pub fn buffer(&self) -> &str {
        if self.form && self.screen() != Screen::Subs && self.screen() != Screen::Pending {
            return self
                .fields()
                .get(self.selected_field)
                .and_then(|field| self.field_values.get(&field.name))
                .map(String::as_str)
                .unwrap_or("");
        }
        match self.screen() {
            Screen::Subs => &self.watch,
            Screen::Pending => &self.answer,
            _ => &self.args,
        }
    }

    fn buffer_mut(&mut self) -> &mut String {
        if self.form && self.screen() != Screen::Subs && self.screen() != Screen::Pending {
            let name = self
                .fields()
                .get(self.selected_field)
                .map(|field| field.name.clone())
                .unwrap_or_default();
            return self.field_values.entry(name).or_default();
        }
        match self.screen() {
            Screen::Subs => &mut self.watch,
            Screen::Pending => &mut self.answer,
            _ => &mut self.args,
        }
    }

    /// Apply a keypress. Returns what the event loop should do next.
    pub fn on_key(&mut self, key: char) -> Action {
        if self.show_help && key != '?' {
            // Any key dismisses help — a modal that only one key closes
            // is a trap in a terminal.
            self.show_help = false;
            return Action::None;
        }
        // Edit mode owns every key except the two that leave it, so a `q`
        // inside a JSON body types a `q` instead of quitting.
        if self.editing_args {
            match key {
                '\u{1b}' => {
                    self.editing_args = false;
                    self.status = "arguments unchanged".to_owned();
                }
                '\r' | '\n' => {
                    self.editing_args = false;
                    // What counts as well-formed depends on the screen: the
                    // subscription buffer is a list of URIs, and validating
                    // it as JSON would reject every correct watch list.
                    self.status = match self.screen() {
                        Screen::Subs => {
                            let watched = self.watch_uris();
                            if watched.is_empty() {
                                "watching catalog changes only".to_owned()
                            } else {
                                format!("watching {} resource(s)", watched.len())
                            }
                        }
                        _ if self.form => {
                            self.sync_args_from_form();
                            let complaints = self.field_problems();
                            if complaints.is_empty() {
                                "field set".to_owned()
                            } else {
                                format!("{} — the call is still allowed", complaints.join(" · "))
                            }
                        }
                        Screen::Resources => "uri set".to_owned(),
                        Screen::Pending => {
                            match serde_json::from_str::<serde_json::Value>(&self.answer) {
                                Ok(_) => "answer set — enter to send, x to decline".to_owned(),
                                Err(e) => format!("answer is not valid JSON: {e}"),
                            }
                        }
                        _ => match serde_json::from_str::<serde_json::Value>(&self.args) {
                            Ok(_) => "arguments set".to_owned(),
                            Err(e) => format!("arguments are not valid JSON: {e}"),
                        },
                    };
                }
                '\u{7f}' | '\u{8}' => {
                    self.buffer_mut().pop();
                }
                c => self.buffer_mut().push(c),
            }
            return Action::None;
        }
        match key {
            'q' => Action::Quit,
            '?' => {
                self.show_help = !self.show_help;
                Action::None
            }
            '\t' => {
                self.screen_index = self.screen_index.wrapping_add(1);
                self.reseed_args();
                self.mark_pushes_seen();
                Action::None
            }
            'h' => {
                let n = Screen::ALL.len();
                self.screen_index = (self.screen_index % n + n - 1) % n;
                self.reseed_args();
                self.mark_pushes_seen();
                Action::None
            }
            'l' => {
                self.screen_index = (self.screen_index + 1) % Screen::ALL.len();
                self.reseed_args();
                self.mark_pushes_seen();
                Action::None
            }
            'j' => {
                self.move_selection(1);
                Action::None
            }
            'k' => {
                self.move_selection(-1);
                Action::None
            }
            'f' if !self.fields().is_empty() => {
                self.form = !self.form;
                if self.form {
                    self.seed_form();
                    self.status = "form — j/k move, a edits, f back to JSON".to_owned();
                } else {
                    self.sync_args_from_form();
                    self.status = "arguments as JSON".to_owned();
                }
                Action::None
            }
            // Not `c`: that already connects, and a key that means two
            // unrelated things depending on a mode is a trap.
            's' if self.form => Action::Complete,
            'w' => Action::Record,
            'J' => {
                self.raw_result = !self.raw_result;
                Action::None
            }
            'x' if self.screen() == Screen::Pending => Action::Decline,
            'c' => Action::Connect,
            'd' => Action::Disconnect,
            'r' => Action::Refresh,
            'a' if matches!(
                self.screen(),
                Screen::Tools
                    | Screen::Resources
                    | Screen::Prompts
                    | Screen::Subs
                    | Screen::Pending
            ) =>
            {
                // An empty buffer gets the shape of a valid value put in it —
                // except in the form, where a field is empty because it has
                // no value yet, and reseeding would tear the form down.
                if self.buffer().is_empty() && !self.form {
                    self.reseed_args();
                }
                self.editing_args = true;
                self.status = "editing arguments — enter to accept, esc to cancel".to_owned();
                Action::None
            }
            'g' if self.screen() == Screen::Diagnose => self.gate_op(Action::Gateway),
            '\r' | '\n' => match self.screen() {
                Screen::Tools => self.gate_op(Action::CallTool),
                Screen::Resources => self.gate_op(Action::ReadResource),
                Screen::Prompts => self.gate_op(Action::GetPrompt),
                Screen::Subs => Action::ToggleSubscription,
                Screen::History => Action::ReplayHistory,
                Screen::Pending => Action::Answer,
                Screen::Diagnose => Action::Diagnose,
                _ => Action::None,
            },
            _ => Action::None,
        }
    }

    /// Refuse to start a second operation while one is running.
    ///
    /// Answering a pending request is deliberately NOT gated: it is the one
    /// thing that has to work while an operation is in flight, because that
    /// operation is what is waiting for it.
    fn gate_op(&mut self, action: Action) -> Action {
        if self.busy {
            self.status = "still working — one operation at a time".to_owned();
            return Action::None;
        }
        action
    }

    /// Refill the argument buffer for whatever is selected now.
    ///
    /// One buffer serves every screen, so without this a tool's arguments
    /// follow you to the prompts screen and render as invalid JSON there.
    /// Reseeding on every move also puts the *shape* of a valid call on
    /// screen before anything is typed.
    fn reseed_args(&mut self) {
        if self.editing_args {
            return; // never yank the buffer out from under a keystroke
        }
        self.entity_result = None;
        self.raw_result = false;
        self.suggestions.clear();
        // A different entity is a different form; leaving the last one's
        // values in place is how one tool's arguments get sent to another.
        self.field_values.clear();
        self.selected_field = 0;
        self.form = false;
        if self.screen() == Screen::Subs {
            // The watch list describes a stream, not a cursor, so it is
            // seeded once — from whatever resource brought the user here —
            // and then left alone. Reseeding it under a running subscription
            // would misdescribe what is actually being watched.
            if !self.subscribed && self.watch.is_empty() {
                self.watch = self
                    .resources
                    .get(self.selected_resource)
                    .map(|r| r.uri.clone())
                    .unwrap_or_default();
            }
            return;
        }
        if self.screen() == Screen::Pending {
            self.answer = self
                .pending
                .get(self.selected_pending)
                .map(|row| answer_template(&row.method))
                .unwrap_or_else(|| "{}".to_owned());
            return;
        }
        self.args = match self.screen() {
            Screen::Resources => self
                .resources
                .get(self.selected_resource)
                .map(|r| r.uri.clone())
                .unwrap_or_default(),
            Screen::Prompts => self
                .prompts
                .get(self.selected_prompt)
                .map(|p| {
                    let body = p
                        .required_args()
                        .iter()
                        .map(|name| format!("\"{name}\": \"\""))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{{{body}}}")
                })
                .unwrap_or_else(|| "{}".to_owned()),
            _ => "{}".to_owned(),
        };
    }

    /// The URIs the subscription screen is asking to watch.
    pub fn watch_uris(&self) -> Vec<String> {
        self.watch
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "{}")
            .map(str::to_owned)
            .collect()
    }

    /// Record one operation, newest first.
    pub fn remember(&mut self, row: HistoryRow) {
        self.history.insert(0, row);
        self.history.truncate(HISTORY_CAP);
        self.selected_history = 0;
    }

    /// The selected entry, if there is one.
    pub fn selected_history(&self) -> Option<&HistoryRow> {
        self.history.get(self.selected_history)
    }

    /// Record one pushed notification, newest last.
    pub fn record_push(&mut self, ts_ms: u64, frame: &serde_json::Value) {
        let method = frame
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("?")
            .to_owned();
        if self.screen() != Screen::Subs {
            self.unseen_pushes += 1;
        }
        self.pushes.push(PushRow {
            ts_ms,
            method,
            body: frame.to_string(),
        });
        if self.pushes.len() > PUSH_CAP {
            let overflow = self.pushes.len() - PUSH_CAP;
            self.pushes.drain(..overflow);
        }
    }

    /// Take the queue as the responder reports it.
    ///
    /// A request that arrives while the user is elsewhere pulls them here:
    /// it is blocking the call they just made, and a queue nobody looks at
    /// is the same as no queue at all. Answering is still their choice.
    pub fn sync_pending(&mut self, queue: Vec<PendingRow>) {
        let arrived = !queue.is_empty() && self.pending.is_empty();
        self.pending = queue;
        if self.selected_pending >= self.pending.len() {
            self.selected_pending = 0;
        }
        if arrived {
            self.screen_index = Screen::ALL
                .iter()
                .position(|s| *s == Screen::Pending)
                .unwrap_or(self.screen_index);
            let asked = self
                .pending
                .first()
                .map(|row| row.method.clone())
                .unwrap_or_default();
            self.status = format!("the server is asking: {asked} — enter to answer, x to decline");
            self.editing_args = false;
            self.reseed_args();
        } else if self.pending.is_empty() && !self.answer.is_empty() {
            self.answer.clear();
        }
    }

    /// The schema of whatever the current screen would send.
    pub fn current_schema(&self) -> Option<serde_json::Value> {
        match self.screen() {
            Screen::Tools => self
                .tools
                .get(self.selected_tool)
                .and_then(|tool| tool.input_schema.clone()),
            Screen::Prompts => self
                .prompts
                .get(self.selected_prompt)
                .map(|prompt| crate::schema::schema_from_prompt_arguments(&prompt.arguments)),
            // A template's variables are what a read needs filled. A concrete
            // resource has none, so it keeps the plain URI editor.
            Screen::Resources => self
                .resources
                .get(self.selected_resource)
                .filter(|row| row.is_template)
                .and_then(|row| crate::schema::schema_from_uri_template(&row.uri)),
            _ => None,
        }
    }

    /// The fields the form is showing.
    pub fn fields(&self) -> Vec<crate::schema::SchemaField> {
        // A template's variables keep the order the URI names them in; a
        // schema's properties have no recoverable order and are sorted
        // required-first instead.
        if self.screen() == Screen::Resources {
            return self
                .resources
                .get(self.selected_resource)
                .filter(|row| row.is_template)
                .map(|row| crate::schema::fields_from_uri_template(&row.uri))
                .unwrap_or_default();
        }
        crate::schema::fields_of(self.current_schema().as_ref())
    }

    /// The value of one field, as text.
    pub fn field_text(&self, name: &str) -> String {
        self.field_values.get(name).cloned().unwrap_or_default()
    }

    /// Rebuild the argument buffer from the form. The buffer stays the single
    /// source of what will be sent, so the form is a way of editing it rather
    /// than a second thing to keep in step.
    pub fn sync_args_from_form(&mut self) {
        let fields = self.fields();
        let values = self.field_values.clone();
        let args = crate::schema::to_arguments(&fields, &|name: &str| {
            values.get(name).cloned().unwrap_or_default()
        });
        self.args = args.to_string();
    }

    /// Seed the form from the current argument buffer, so switching into it
    /// shows what is actually about to be sent.
    pub fn seed_form(&mut self) {
        let fields = self.fields();
        let parsed = serde_json::from_str::<serde_json::Value>(&self.args)
            .unwrap_or_else(|_| serde_json::json!({}));
        self.field_values = crate::schema::from_arguments(&fields, &parsed)
            .into_iter()
            .collect();
        if self.selected_field >= fields.len() {
            self.selected_field = 0;
        }
    }

    /// What names the thing being completed, in the shape MCP asks for.
    ///
    /// Completions exist for prompts and resource templates and nothing else
    /// — a tool's arguments have no equivalent — so this is `None` on every
    /// other screen and the key does nothing there.
    pub fn completion_ref(&self) -> Option<serde_json::Value> {
        match self.screen() {
            Screen::Prompts => self
                .prompts
                .get(self.selected_prompt)
                .map(|prompt| serde_json::json!({ "type": "ref/prompt", "name": prompt.name })),
            Screen::Resources => self
                .resources
                .get(self.selected_resource)
                .filter(|row| row.is_template)
                .map(|row| serde_json::json!({ "type": "ref/resource", "uri": row.uri })),
            _ => None,
        }
    }

    /// The field a completion would be for.
    pub fn selected_field_name(&self) -> Option<String> {
        self.fields()
            .get(self.selected_field)
            .map(|field| field.name.clone())
    }

    /// Move to a screen by name. The index is a moving target; the screen is
    /// what a caller means.
    pub fn go_to(&mut self, screen: Screen) {
        if let Some(index) = Screen::ALL.iter().position(|s| *s == screen) {
            self.screen_index = index;
        }
        self.mark_pushes_seen();
    }

    fn mark_pushes_seen(&mut self) {
        if self.screen() == Screen::Subs {
            self.unseen_pushes = 0;
        }
    }

    /// The URI a read would send: a template expanded from the form when one
    /// is open, and whatever is in the buffer otherwise.
    pub fn resource_uri(&self) -> String {
        let Some(row) = self.resources.get(self.selected_resource) else {
            return String::new();
        };
        if self.form && row.is_template {
            let values = self.field_values.clone();
            return crate::schema::expand_uri_template(&row.uri, &|name: &str| {
                values.get(name).cloned().unwrap_or_default()
            });
        }
        let typed = self.args.trim();
        if typed.is_empty() || typed == "{}" {
            row.uri.clone()
        } else {
            typed.to_owned()
        }
    }

    /// What the schema disagrees with, reported and never enforced.
    pub fn field_problems(&self) -> Vec<String> {
        let values = self.field_values.clone();
        crate::schema::problems(&self.fields(), &|name: &str| {
            values.get(name).cloned().unwrap_or_default()
        })
    }

    /// The selected request, if there is one.
    pub fn selected_pending(&self) -> Option<&PendingRow> {
        self.pending.get(self.selected_pending)
    }

    /// Fold in what an operation that ran off the loop came back with.
    pub fn apply_outcome(&mut self, outcome: crate::OpOutcome) {
        let (ok, fail, slot, body) = outcome.into_parts();
        let rendered = match body {
            Ok(rendered) => {
                self.status = ok;
                Some(rendered)
            }
            Err(message) => {
                self.status = format!("{fail}: {message}");
                None
            }
        };
        match slot {
            crate::ResultSlot::Call => self.call_result = rendered,
            crate::ResultSlot::Entity => self.entity_result = rendered,
        }
    }

    #[cfg(test)]
    fn reseed_for_test(&mut self) {
        self.reseed_args();
    }

    fn move_selection(&mut self, delta: isize) {
        if self.form {
            let len = self.fields().len();
            if len == 0 {
                return;
            }
            self.selected_field =
                (self.selected_field as isize + delta).rem_euclid(len as isize) as usize;
            // A suggestion list belongs to the field it was fetched for.
            self.suggestions.clear();
            return;
        }
        let (len, index) = match self.screen() {
            Screen::Targets => (self.targets.len(), &mut self.selected_target),
            Screen::Tools => (self.tools.len(), &mut self.selected_tool),
            Screen::Resources => (self.resources.len(), &mut self.selected_resource),
            Screen::Prompts => (self.prompts.len(), &mut self.selected_prompt),
            Screen::Pending => (self.pending.len(), &mut self.selected_pending),
            Screen::History => (self.history.len(), &mut self.selected_history),
            // These tails follow a stream and the report is short enough to
            // fit; there is nothing to select, so navigation keys are inert
            // rather than wrong.
            Screen::Subs | Screen::Diagnose | Screen::Wire => return,
        };
        if len == 0 {
            return;
        }
        let next = (*index as isize + delta).rem_euclid(len as isize);
        *index = next as usize;
        self.reseed_args();
    }
}

/// A starting point for an answer, per request kind.
///
/// The inspector will not call a model, so a sampling answer is a shell for
/// a human to type into rather than something generated. An unrecognised
/// method gets an empty object: better a blank than an invented shape.
pub fn answer_template(method: &str) -> String {
    match method {
        "sampling/createMessage" => serde_json::json!({
            "role": "assistant",
            "content": { "type": "text", "text": "" },
            "model": "mcpg-inspector-human",
            "stopReason": "endTurn",
        }),
        "elicitation/create" => serde_json::json!({ "action": "accept", "content": {} }),
        "roots/list" => serde_json::json!({ "roots": [] }),
        _ => serde_json::json!({}),
    }
    .to_string()
}

#[cfg(test)]
mod tests {

    /// Put the app on `screen` by name. Tests that hardcoded an index all
    /// broke the moment a screen was inserted between two others, which is
    /// exactly the churn a name avoids.
    fn on_screen(screen: Screen) -> AppState {
        AppState {
            screen_index: Screen::ALL.iter().position(|s| *s == screen).unwrap(),
            ..Default::default()
        }
    }

    /// Enter on the tools screen calls; on any other screen it does not,
    /// because there is nothing there to call.
    #[test]
    fn enter_runs_what_the_current_screen_is_about() {
        assert_eq!(on_screen(Screen::Tools).on_key('\r'), Action::CallTool);
        assert_eq!(
            on_screen(Screen::Resources).on_key('\r'),
            Action::ReadResource
        );
        assert_eq!(on_screen(Screen::Prompts).on_key('\r'), Action::GetPrompt);
        assert_eq!(
            on_screen(Screen::Subs).on_key('\r'),
            Action::ToggleSubscription
        );
        assert_eq!(on_screen(Screen::Diagnose).on_key('\r'), Action::Diagnose);
        // Nothing to run on a list of targets or a tail of frames.
        assert_eq!(on_screen(Screen::Targets).on_key('\r'), Action::None);
        assert_eq!(on_screen(Screen::Wire).on_key('\r'), Action::None);
    }

    /// `a` opens the editor seeded with an empty object, so the shape of a
    /// call is on screen before anything is typed.
    #[test]
    fn a_opens_the_argument_editor_seeded() {
        let mut app = on_screen(Screen::Tools);
        assert_eq!(app.on_key('a'), Action::None);
        assert!(app.editing_args);
        assert_eq!(app.args, "{}");
    }

    /// Edit mode must own the keyboard. `q` inside a JSON body types a `q`
    /// — a terminal editor that quits on its own text is unusable.
    #[test]
    fn edit_mode_swallows_the_command_keys() {
        let mut app = on_screen(Screen::Tools);
        app.on_key('a');
        app.args.clear();
        for c in ['q', 'j', 'c', 'd', 'r', '?'] {
            assert_eq!(
                app.on_key(c),
                Action::None,
                "{c} must not act while editing"
            );
        }
        assert_eq!(app.args, "qjcdr?");
        assert!(app.editing_args, "still editing");
    }

    /// Enter accepts and reports whether what was typed is usable; esc
    /// leaves without changing the buffer.
    #[test]
    fn enter_accepts_and_esc_cancels() {
        let mut app = on_screen(Screen::Tools);
        app.on_key('a');
        app.args.clear();
        for c in "{\"a\":1}".chars() {
            app.on_key(c);
        }
        app.on_key('\r');
        assert!(!app.editing_args);
        assert_eq!(app.args, "{\"a\":1}");
        assert!(app.status.contains("set"), "{}", app.status);

        app.on_key('a');
        app.on_key('x');
        app.on_key('\u{1b}');
        assert!(!app.editing_args);
        assert!(app.status.contains("unchanged"), "{}", app.status);
    }

    /// Bad JSON is reported on accept rather than at call time, so the
    /// mistake is visible where it was made.
    #[test]
    fn accepting_bad_json_says_so() {
        let mut app = on_screen(Screen::Tools);
        app.on_key('a');
        app.args.clear();
        app.on_key('{');
        app.on_key('\r');
        assert!(!app.editing_args);
        assert!(app.status.contains("not valid JSON"), "{}", app.status);
    }

    #[test]
    fn backspace_removes_a_character() {
        let mut app = on_screen(Screen::Tools);
        app.on_key('a');
        app.on_key('z');
        app.on_key('\u{7f}');
        assert_eq!(app.args, "{}");
    }

    use super::*;

    fn state_with_targets(n: usize) -> AppState {
        AppState {
            targets: (0..n)
                .map(|i| TargetRow {
                    id: format!("t{i}"),
                    endpoint: "http://x/mcp".to_owned(),
                    session: SessionView::Idle,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn tab_cycles_every_screen_and_wraps() {
        let mut state = AppState::default();
        for expected in Screen::ALL {
            assert_eq!(state.screen(), expected);
            state.on_key('\t');
        }
        assert_eq!(state.screen(), Screen::ALL[0], "wraps around");
    }

    /// `h`/`l` must reach the same screens as tab, in both directions. They
    /// were separate index maps once, so one could go somewhere the other
    /// could not.
    #[test]
    fn h_and_l_walk_the_same_ring_as_tab() {
        let mut state = AppState::default();
        for expected in Screen::ALL {
            assert_eq!(state.screen(), expected);
            state.on_key('l');
        }
        assert_eq!(state.screen(), Screen::ALL[0]);
        for expected in Screen::ALL.iter().rev() {
            state.on_key('h');
            assert_eq!(state.screen(), *expected);
        }
    }

    #[test]
    fn selection_wraps_and_never_indexes_an_empty_list() {
        let mut state = state_with_targets(2);
        state.on_key('j');
        assert_eq!(state.selected_target, 1);
        state.on_key('j');
        assert_eq!(state.selected_target, 0, "wraps forward");
        state.on_key('k');
        assert_eq!(state.selected_target, 1, "wraps backward");

        // An empty list must not panic or leave a stale index.
        let mut empty = AppState::default();
        empty.on_key('j');
        empty.on_key('k');
        assert_eq!(empty.selected_target, 0);
        assert!(empty.selected_target().is_none());
    }

    #[test]
    fn keys_map_to_the_actions_the_loop_performs() {
        let mut state = state_with_targets(1);
        assert_eq!(state.on_key('c'), Action::Connect);
        assert_eq!(state.on_key('d'), Action::Disconnect);
        assert_eq!(state.on_key('r'), Action::Refresh);
        assert_eq!(state.on_key('q'), Action::Quit);
        assert_eq!(state.on_key('z'), Action::None);
    }

    #[test]
    fn any_key_dismisses_help() {
        let mut state = AppState::default();
        state.on_key('?');
        assert!(state.show_help);
        // Not just '?': a modal only one key closes is a trap.
        state.on_key('j');
        assert!(!state.show_help);
    }

    #[test]
    fn wire_screen_ignores_selection_keys() {
        let mut state = state_with_targets(3);
        state.screen_index = Screen::ALL.iter().position(|s| *s == Screen::Wire).unwrap();
        assert_eq!(state.screen(), Screen::Wire);
        state.on_key('j');
        assert_eq!(
            state.selected_target, 0,
            "the tail follows, nothing selects"
        );
    }

    /// One buffer serves every screen, so switching screens must refill it.
    /// A tool's arguments following you to the prompts screen render as
    /// invalid JSON there — which is exactly what happened before this.
    #[test]
    fn switching_screens_reseeds_the_argument_buffer() {
        let mut app = on_screen(Screen::Tools);
        app.prompts = vec![PromptRow {
            name: "greet".into(),
            description: None,
            arguments: vec![crate::schema::PromptArgument {
                name: "who".into(),
                description: None,
                required: true,
            }],
        }];
        app.resources = vec![ResourceRow {
            uri: "docs://runbook".into(),
            name: None,
            description: None,
            is_template: false,
        }];
        app.args = "{\"text\": \"from the tool\"}".into();

        app.on_key('\t'); // → resources: the URI is what a read needs
        assert_eq!(app.screen(), Screen::Resources);
        assert_eq!(app.args, "docs://runbook");

        app.on_key('\t'); // → prompts: the required arguments, shaped
        assert_eq!(app.screen(), Screen::Prompts);
        assert_eq!(app.args, "{\"who\": \"\"}");

        // All the way round the ring, back to where it started.
        while app.screen() != Screen::Tools {
            app.on_key('\t');
        }
        assert_eq!(app.args, "{}");
    }

    /// Moving the selection reseeds too — a second resource's URI is not the
    /// first one's.
    #[test]
    fn moving_the_selection_reseeds() {
        let mut app = on_screen(Screen::Resources);
        app.resources = vec![
            ResourceRow {
                uri: "docs://a".into(),
                name: None,
                description: None,
                is_template: false,
            },
            ResourceRow {
                uri: "docs://b".into(),
                name: None,
                description: None,
                is_template: false,
            },
        ];
        app.on_key('j');
        assert_eq!(app.args, "docs://b");
        app.on_key('j');
        assert_eq!(app.args, "docs://a", "wraps");
    }

    /// Reseeding mid-keystroke would delete what the user is typing.
    #[test]
    fn reseeding_never_interrupts_an_edit() {
        let mut app = on_screen(Screen::Prompts);
        app.prompts = vec![PromptRow {
            name: "greet".into(),
            description: None,
            arguments: vec![crate::schema::PromptArgument {
                name: "who".into(),
                description: None,
                required: true,
            }],
        }];
        app.on_key('a');
        app.args = "half-typed".into();
        app.reseed_args();
        assert_eq!(app.args, "half-typed");
    }

    /// The subscription screen's buffer is a comma-separated watch list, not
    /// JSON: `{}` is what an empty tool-argument buffer looks like and must
    /// never be sent as a resource URI.
    #[test]
    fn the_watch_list_is_uris_not_json() {
        let mut app = on_screen(Screen::Subs);
        app.watch = " docs://a , ,docs://b ".into();
        assert_eq!(app.watch_uris(), vec!["docs://a", "docs://b"]);

        app.watch = "{}".into();
        assert!(app.watch_uris().is_empty());
        app.watch = String::new();
        assert!(app.watch_uris().is_empty(), "empty means catalog-only");
    }

    /// Reseeding the watch list while a stream is running would misdescribe
    /// what is actually being watched — the stream was opened for the old
    /// list and does not follow the cursor.
    #[test]
    fn a_live_watch_list_is_not_reseeded() {
        let mut app = on_screen(Screen::Subs);
        app.resources = vec![ResourceRow {
            uri: "docs://elsewhere".into(),
            name: None,
            description: None,
            is_template: false,
        }];
        app.watch = "docs://watched".into();
        app.subscribed = true;

        // Away and back — the leg that used to clobber it, because leaving
        // reseeded the shared buffer for the screen being entered.
        app.on_key('h');
        app.on_key('l');
        assert_eq!(app.screen(), Screen::Subs);
        assert_eq!(app.watch, "docs://watched");

        // Stopping the stream does not silently rewrite it either; only an
        // empty list gets seeded from the selection.
        app.subscribed = false;
        app.on_key('h');
        app.on_key('l');
        assert_eq!(app.watch, "docs://watched");

        app.watch.clear();
        app.on_key('h');
        app.on_key('l');
        assert_eq!(app.watch, "docs://elsewhere", "an empty list is seeded");
    }

    /// A server pushing all day must not grow the buffer without bound, and
    /// what falls off has to be the oldest — the newest push is the one
    /// someone is watching for.
    #[test]
    fn pushes_are_capped_keeping_the_newest() {
        let mut app = AppState::default();
        for i in 0..(PUSH_CAP + 25) {
            app.record_push(
                i as u64,
                &serde_json::json!({"method": "notifications/tools/list_changed", "n": i}),
            );
        }
        assert_eq!(app.pushes.len(), PUSH_CAP);
        assert_eq!(app.pushes.last().unwrap().ts_ms, (PUSH_CAP + 24) as u64);
        assert_eq!(app.pushes.first().unwrap().ts_ms, 25);
        assert_eq!(
            app.pushes[0].method, "notifications/tools/list_changed",
            "method is lifted out for the list column"
        );
    }

    /// A frame with no method still has to be recorded; dropping it would
    /// hide exactly the malformed push an inspector is for.
    #[test]
    fn a_push_without_a_method_is_still_recorded() {
        let mut app = AppState::default();
        app.record_push(7, &serde_json::json!({"jsonrpc": "2.0", "id": 1}));
        assert_eq!(app.pushes.len(), 1);
        assert_eq!(app.pushes[0].method, "?");
        assert!(app.pushes[0].body.contains("jsonrpc"));
    }

    /// The two buffers must not leak into each other: typing a watch list is
    /// not typing tool arguments, and a screen change reseeds one of them.
    #[test]
    fn the_watch_list_and_the_argument_buffer_are_separate() {
        let mut app = on_screen(Screen::Tools);
        app.args = "{\"text\": \"hi\"}".into();

        app.on_key('\t'); // resources
        app.on_key('\t'); // prompts
        app.on_key('\t'); // subs
        assert_eq!(app.screen(), Screen::Subs);
        app.on_key('a');
        for c in "docs://a".chars() {
            app.on_key(c);
        }
        app.on_key('\r');
        assert_eq!(app.watch, "docs://a");
        assert_eq!(app.status, "watching 1 resource(s)");

        // Back to tools: its own arguments are reseeded, not the watch list.
        while app.screen() != Screen::Tools {
            app.on_key('\t');
        }
        assert_eq!(app.args, "{}");
        assert_eq!(app.watch, "docs://a");
    }

    fn asked(id: u64, method: &str) -> PendingRow {
        PendingRow {
            id,
            method: method.into(),
            params: "{}".into(),
            regime: "sessionful".into(),
        }
    }

    /// A request arriving is not a notification to be found later: it is
    /// blocking the call the user just made, so it takes the screen.
    #[test]
    fn an_arriving_request_pulls_the_user_to_it() {
        let mut app = on_screen(Screen::Tools);
        app.busy = true;
        app.sync_pending(vec![asked(1, "sampling/createMessage")]);

        assert_eq!(app.screen(), Screen::Pending);
        assert!(
            app.status.contains("sampling/createMessage"),
            "{}",
            app.status
        );
        // Seeded with a shape to fill in, not with a generated answer. The
        // seed is compact because `enter` accepts the buffer, so a
        // pretty-printed one could not be typed back in.
        assert!(
            app.answer.contains("\"role\":\"assistant\""),
            "{}",
            app.answer
        );
        assert!(
            app.answer.contains("\"text\":\"\""),
            "no model is called: {}",
            app.answer
        );
    }

    /// A second request arriving while the first is on screen must not yank
    /// the view or the half-typed answer out from under the user.
    #[test]
    fn a_second_request_does_not_re_steal_the_screen() {
        let mut app = on_screen(Screen::Tools);
        app.sync_pending(vec![asked(1, "roots/list")]);
        assert_eq!(app.screen(), Screen::Pending);

        app.screen_index = Screen::ALL.iter().position(|s| *s == Screen::Wire).unwrap();
        app.sync_pending(vec![asked(1, "roots/list"), asked(2, "elicitation/create")]);
        assert_eq!(app.screen(), Screen::Wire, "the user's place is theirs");
        assert_eq!(app.pending.len(), 2);
    }

    /// Enter answers, `x` declines — and a decline is a real answer, so it
    /// has to be reachable without composing JSON.
    #[test]
    fn the_pending_screen_answers_and_declines() {
        let mut app = on_screen(Screen::Pending);
        app.pending = vec![asked(1, "elicitation/create")];
        assert_eq!(app.on_key('\r'), Action::Answer);
        assert_eq!(app.on_key('x'), Action::Decline);
        // `x` means nothing anywhere else, so it must not be swallowed there.
        assert_eq!(on_screen(Screen::Tools).on_key('x'), Action::None);
    }

    /// The answer buffer is per-request: an elicitation reply must not be
    /// left over from the sampling request before it.
    #[test]
    fn each_request_reseeds_its_own_answer() {
        let mut app = on_screen(Screen::Pending);
        app.pending = vec![asked(1, "sampling/createMessage"), asked(2, "roots/list")];
        app.reseed_for_test();
        assert!(app.answer.contains("stopReason"));

        app.on_key('j');
        assert_eq!(app.answer, "{\"roots\":[]}");
    }

    /// The answer is JSON like a tool's arguments, and its own buffer — a
    /// tool's arguments must not become the answer to a server's question.
    #[test]
    fn the_answer_is_its_own_buffer() {
        let mut app = on_screen(Screen::Tools);
        app.args = "{\"text\": \"a tool argument\"}".into();
        app.pending = vec![asked(1, "roots/list")];
        app.screen_index = Screen::ALL
            .iter()
            .position(|s| *s == Screen::Pending)
            .unwrap();
        app.reseed_for_test();

        assert_eq!(app.answer, "{\"roots\":[]}");
        assert_eq!(app.args, "{\"text\": \"a tool argument\"}", "untouched");

        app.on_key('a');
        app.on_key('!');
        assert!(app.answer.ends_with('!'));
        assert_eq!(app.args, "{\"text\": \"a tool argument\"}");
    }

    /// A malformed answer is caught on accept, where it can still be fixed —
    /// not on send, where the call is already waiting.
    #[test]
    fn a_malformed_answer_is_reported_on_accept() {
        let mut app = on_screen(Screen::Pending);
        app.pending = vec![asked(1, "roots/list")];
        app.on_key('a');
        app.answer = "{not json".into();
        app.on_key('\r');
        assert!(app.status.contains("not valid JSON"), "{}", app.status);
    }

    /// One operation at a time: a second call while the first is running
    /// would race two results into the same slot. Answering is exempt — it
    /// is what the running operation is waiting for.
    #[test]
    fn a_second_operation_is_refused_but_answering_is_not() {
        let mut app = on_screen(Screen::Tools);
        app.busy = true;
        assert_eq!(app.on_key('\r'), Action::None);
        assert!(
            app.status.contains("one operation at a time"),
            "{}",
            app.status
        );

        let mut app = on_screen(Screen::Pending);
        app.busy = true;
        app.pending = vec![asked(1, "sampling/createMessage")];
        assert_eq!(
            app.on_key('\r'),
            Action::Answer,
            "answering must work while busy"
        );
        assert_eq!(app.on_key('x'), Action::Decline);
    }

    /// An answered queue clears the buffer, so the next request does not
    /// inherit the last one's reply.
    #[test]
    fn emptying_the_queue_clears_the_answer() {
        let mut app = on_screen(Screen::Pending);
        app.sync_pending(vec![asked(1, "roots/list")]);
        app.answer = "{\"roots\":[{\"name\":\"x\",\"uri\":\"file:///x\"}]}".into();
        app.sync_pending(Vec::new());
        assert!(app.answer.is_empty());
        assert!(app.pending.is_empty());
    }

    fn tool_with_schema() -> AppState {
        let mut app = on_screen(Screen::Tools);
        app.tools = vec![ToolRow {
            name: "incident.file".into(),
            description: None,
            input_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "count": { "type": "integer", "default": 3 },
                    "urgent": { "type": "boolean" },
                },
                "required": ["title"],
            })),
            output_schema: None,
            app_uri: None,
        }];
        app
    }

    /// The form is only possible where a schema describes the arguments, so
    /// `f` does nothing on a tool that declares none rather than opening an
    /// empty one.
    #[test]
    fn the_form_needs_a_schema_to_open() {
        let mut bare = on_screen(Screen::Tools);
        bare.tools = vec![ToolRow {
            name: "opaque".into(),
            description: None,
            input_schema: None,
            output_schema: None,
            app_uri: None,
        }];
        bare.on_key('f');
        assert!(!bare.form, "no schema, no form");

        let mut app = tool_with_schema();
        app.on_key('f');
        assert!(app.form);
        assert!(app.status.contains("form"), "{}", app.status);
    }

    /// Opening the form seeds it from what is about to be sent, so the two
    /// views never disagree about the call.
    #[test]
    fn the_form_and_the_json_edit_one_value() {
        let mut app = tool_with_schema();
        app.args = r#"{"title":"disk full"}"#.into();
        app.on_key('f');

        assert_eq!(app.field_text("title"), "disk full");
        assert_eq!(
            app.field_text("count"),
            "3",
            "a schema default seeds a field"
        );

        // Type into the selected field; the JSON follows.
        app.selected_field = app.fields().iter().position(|f| f.name == "count").unwrap();
        app.on_key('a');
        app.on_key('\u{7f}');
        app.on_key('7');
        app.on_key('\r');

        let sent: serde_json::Value = serde_json::from_str(&app.args).expect("args are json");
        assert_eq!(sent["count"], serde_json::json!(7), "an integer, not \"7\"");
        assert_eq!(sent["title"], serde_json::json!("disk full"));
    }

    /// `j`/`k` move between fields while the form is open and between
    /// entities when it is not — one pair of keys, read in context.
    #[test]
    fn movement_follows_whichever_list_is_open() {
        let mut app = tool_with_schema();
        app.tools.push(ToolRow {
            name: "second".into(),
            description: None,
            input_schema: None,
            output_schema: None,
            app_uri: None,
        });
        app.on_key('j');
        assert_eq!(app.selected_tool, 1, "no form: the tool list moves");

        app.selected_tool = 0;
        app.on_key('f');
        app.on_key('j');
        assert_eq!(app.selected_tool, 0, "the form has the keys now");
        assert_eq!(app.selected_field, 1);
    }

    /// A different tool is a different form. Values left behind are how one
    /// tool's arguments get sent to another.
    #[test]
    fn changing_the_selection_clears_the_form() {
        let mut app = tool_with_schema();
        app.on_key('f');
        app.field_values.insert("title".into(), "left over".into());
        app.tools.push(ToolRow {
            name: "second".into(),
            description: None,
            input_schema: None,
            output_schema: None,
            app_uri: None,
        });

        app.form = false; // leave the form so j moves the tool list
        app.on_key('j');
        assert!(app.field_values.is_empty(), "{:?}", app.field_values);
        assert!(!app.form);
    }

    /// Required fields are reported when empty, and the call still goes:
    /// finding out how a server handles a malformed call is the job.
    #[test]
    fn missing_and_mistyped_fields_are_reported_not_enforced() {
        let mut app = tool_with_schema();
        app.on_key('f');
        assert!(
            app.field_problems().iter().any(|p| p == "title: required"),
            "{:?}",
            app.field_problems()
        );
        assert_eq!(app.on_key('\r'), Action::CallTool, "still callable");

        app.field_values.insert("count".into(), "many".into());
        assert!(
            app.field_problems()
                .iter()
                .any(|p| p == "count: not an integer"),
            "{:?}",
            app.field_problems()
        );
    }

    /// The reading of a result and the envelope it arrived in are both worth
    /// having, so one key moves between them.
    #[test]
    fn the_raw_result_toggles() {
        let mut app = tool_with_schema();
        assert!(!app.raw_result);
        app.on_key('J');
        assert!(app.raw_result);
        app.on_key('J');
        assert!(!app.raw_result);
    }

    /// A prompt has arguments but no JSON Schema; shaping them into one is
    /// what lets prompts and tools share a form.
    #[test]
    fn prompts_get_a_form_from_their_arguments() {
        let mut app = on_screen(Screen::Prompts);
        app.prompts = vec![PromptRow {
            name: "summarize".into(),
            description: None,
            arguments: vec![crate::schema::PromptArgument {
                name: "incident_id".into(),
                description: None,
                required: true,
            }],
        }];
        let fields = app.fields();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "incident_id");
        assert!(fields[0].required);

        app.on_key('f');
        assert!(app.form);
        app.on_key('a');
        for c in "INC-1".chars() {
            app.on_key(c);
        }
        app.on_key('\r');
        let sent: serde_json::Value = serde_json::from_str(&app.args).expect("args are json");
        assert_eq!(sent["incident_id"], serde_json::json!("INC-1"));
    }

    fn template_resource() -> AppState {
        let mut app = on_screen(Screen::Resources);
        app.resources = vec![
            ResourceRow {
                uri: "docs://runbook".into(),
                name: None,
                description: None,
                is_template: false,
            },
            ResourceRow {
                uri: "incidents://{year}/{id}".into(),
                name: None,
                description: None,
                is_template: true,
            },
        ];
        app
    }

    /// A template's variables are fields; a concrete resource has none and
    /// keeps the plain URI editor.
    #[test]
    fn only_a_template_gets_a_form() {
        let mut app = template_resource();
        assert!(
            app.fields().is_empty(),
            "a concrete resource has no variables"
        );
        app.on_key('f');
        assert!(!app.form, "nothing to open");

        app.selected_resource = 1;
        let names: Vec<String> = app.fields().into_iter().map(|f| f.name).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.contains(&"year".to_owned()) && names.contains(&"id".to_owned()));
    }

    /// The read sends the expanded URI, not the template and not the JSON the
    /// form composed.
    #[test]
    fn the_form_expands_the_uri_a_read_sends() {
        let mut app = template_resource();
        app.selected_resource = 1;
        app.on_key('f');
        app.field_values.insert("year".into(), "2026".into());
        app.field_values.insert("id".into(), "INC-1".into());
        assert_eq!(app.resource_uri(), "incidents://2026/INC-1");

        // Half-filled reads as half-filled rather than as an empty segment.
        app.field_values.remove("id");
        assert_eq!(app.resource_uri(), "incidents://2026/{id}");

        // With no form the buffer is the URI, as before.
        app.form = false;
        app.args = "incidents://2020/OLD".into();
        assert_eq!(app.resource_uri(), "incidents://2020/OLD");
    }

    /// Optional arguments were unreachable when only the required ones were
    /// carried: the form could not offer a field for something it did not
    /// know existed.
    #[test]
    fn a_prompt_form_offers_its_optional_arguments_too() {
        let mut app = on_screen(Screen::Prompts);
        app.prompts = vec![PromptRow {
            name: "summarize".into(),
            description: None,
            arguments: vec![
                crate::schema::PromptArgument {
                    name: "incident_id".into(),
                    description: Some("Which one".into()),
                    required: true,
                },
                crate::schema::PromptArgument {
                    name: "tone".into(),
                    description: Some("How to write it".into()),
                    required: false,
                },
            ],
        }];
        let fields = app.fields();
        assert_eq!(fields.len(), 2, "{fields:#?}");
        assert_eq!(fields[0].name, "incident_id", "required first");
        assert!(!fields[1].required);
        assert_eq!(
            fields[1].description.as_deref(),
            Some("How to write it"),
            "a description reaches the form"
        );
        assert_eq!(app.prompts[0].required_args(), vec!["incident_id"]);
    }

    /// Completions exist for prompts and resource templates and nothing else,
    /// so the key does nothing where the protocol defines nothing.
    #[test]
    fn completions_are_asked_for_only_where_they_exist() {
        let mut app = on_screen(Screen::Prompts);
        app.prompts = vec![PromptRow {
            name: "summarize".into(),
            description: None,
            arguments: vec![crate::schema::PromptArgument {
                name: "incident_id".into(),
                description: None,
                required: true,
            }],
        }];
        app.on_key('f');
        assert_eq!(app.on_key('s'), Action::Complete);
        assert_eq!(
            app.completion_ref(),
            Some(serde_json::json!({ "type": "ref/prompt", "name": "summarize" }))
        );
        assert_eq!(app.selected_field_name().as_deref(), Some("incident_id"));

        let mut templates = template_resource();
        templates.selected_resource = 1;
        assert_eq!(
            templates.completion_ref(),
            Some(serde_json::json!({
                "type": "ref/resource",
                "uri": "incidents://{year}/{id}"
            }))
        );
        // A tool's arguments have no completion surface in MCP.
        let mut tools = on_screen(Screen::Tools);
        tools.form = true;
        assert!(tools.completion_ref().is_none());
        // And outside a form the key still connects.
        let mut idle = on_screen(Screen::Prompts);
        assert_eq!(idle.on_key('s'), Action::None);
    }

    /// A suggestion list is for one field. Carrying it to the next would
    /// offer one field's values for another.
    #[test]
    fn suggestions_belong_to_the_field_they_were_fetched_for() {
        let mut app = on_screen(Screen::Prompts);
        app.prompts = vec![PromptRow {
            name: "summarize".into(),
            description: None,
            arguments: vec![
                crate::schema::PromptArgument {
                    name: "a".into(),
                    description: None,
                    required: true,
                },
                crate::schema::PromptArgument {
                    name: "b".into(),
                    description: None,
                    required: true,
                },
            ],
        }];
        app.on_key('f');
        app.suggestions = vec!["INC-1".into()];
        app.on_key('j');
        assert!(app.suggestions.is_empty());
    }

    fn done(verb: &'static str, subject: &str, ok: bool) -> HistoryRow {
        HistoryRow {
            verb,
            subject: subject.into(),
            args: r#"{"text":"hi"}"#.into(),
            at_ms: 1,
            took_ms: 12,
            ok,
            body: "{}".into(),
        }
    }

    /// Newest first: the thing you just did is the thing you are looking for.
    /// A failure is kept too — it is the more interesting half of a history.
    #[test]
    fn history_keeps_the_newest_first_and_keeps_failures() {
        let mut app = on_screen(Screen::History);
        app.remember(done("called", "first", true));
        app.remember(done("called", "second", false));

        assert_eq!(app.history.len(), 2);
        assert_eq!(app.history[0].subject, "second");
        assert!(!app.history[0].ok, "a failure is recorded, not dropped");
        assert_eq!(app.selected_history, 0, "the newest is selected");
    }

    /// A terminal left open overnight must not grow without limit.
    #[test]
    fn history_is_bounded() {
        let mut app = AppState::default();
        for i in 0..(HISTORY_CAP + 20) {
            app.remember(done("called", &format!("tool-{i}"), true));
        }
        assert_eq!(app.history.len(), HISTORY_CAP);
        assert_eq!(
            app.history[0].subject,
            format!("tool-{}", HISTORY_CAP + 19),
            "the newest survives"
        );
    }

    /// `enter` on a history entry loads it back rather than re-sending it:
    /// re-running silently is how the difference being compared gets lost.
    #[test]
    fn enter_on_history_asks_to_load_not_to_resend() {
        let mut app = on_screen(Screen::History);
        app.history = vec![done("called", "dev.mock.echo", true)];
        assert_eq!(app.on_key('\r'), Action::ReplayHistory);
    }

    /// A push that lands while you are elsewhere is only noticeable if the
    /// tab says so — and arriving at the screen is what clears it.
    #[test]
    fn pushes_arriving_elsewhere_are_counted_until_seen() {
        let mut app = on_screen(Screen::Tools);
        app.record_push(1, &serde_json::json!({ "method": "notifications/x" }));
        app.record_push(2, &serde_json::json!({ "method": "notifications/x" }));
        assert_eq!(app.unseen_pushes, 2);

        // Walking past other screens does not clear it.
        app.on_key('\t');
        assert_eq!(app.unseen_pushes, 2, "{}", app.screen().title());

        while app.screen() != Screen::Subs {
            app.on_key('\t');
        }
        assert_eq!(app.unseen_pushes, 0, "arriving is what 'seen' means");

        // A push that lands while the screen IS showing was already seen.
        app.record_push(3, &serde_json::json!({ "method": "notifications/x" }));
        assert_eq!(app.unseen_pushes, 0);
    }

    /// The screen ring is named, not numbered: `go_to` is what a caller means
    /// when it wants a particular screen.
    #[test]
    fn go_to_moves_by_name() {
        let mut app = AppState::default();
        app.go_to(Screen::Prompts);
        assert_eq!(app.screen(), Screen::Prompts);
        app.go_to(Screen::Wire);
        assert_eq!(app.screen(), Screen::Wire);
    }
}
