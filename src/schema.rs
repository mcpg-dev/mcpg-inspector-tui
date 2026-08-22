//! Reading a JSON Schema well enough to build a form from it.
//!
//! Deliberately not a validator. A server's schema describes what it will
//! accept, and an inspector exists partly to find out what happens when you
//! send something else — so this shapes the input and reports what looks
//! wrong, and never refuses a call the user means to make.
//!
//! The typing is the part that matters. A terminal hands back a string for
//! everything; sending `{"count": "3"}` where the schema says integer earns
//! a server-side type error that reads like a server bug, so the declared
//! type decides how a value is serialized.

use serde_json::{Map, Value, json};

/// The single type a field is edited as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    String,
    Number,
    Integer,
    Boolean,
    Enum,
    Array,
    Object,
    /// No usable type information — edited as raw JSON.
    Json,
}

impl Kind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Integer => "integer",
            Self::Boolean => "boolean",
            Self::Enum => "enum",
            Self::Array => "array",
            Self::Object => "object",
            Self::Json => "any",
        }
    }
}

/// One editable field of an object schema.
#[derive(Clone, Debug)]
pub struct SchemaField {
    pub name: String,
    pub kind: Kind,
    pub required: bool,
    pub description: Option<String>,
    /// The values the schema restricts this to, as text.
    pub options: Vec<String>,
    /// The schema's own default, as text.
    pub default: Option<String>,
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn kind_of(schema: &Value) -> Kind {
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|options| !options.is_empty())
    {
        return Kind::Enum;
    }
    // A union including "null" is the common "optional" spelling; edit it as
    // the type it actually carries.
    let declared = match schema.get("type") {
        Some(Value::String(t)) => Some(t.as_str()),
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null"),
        _ => None,
    };
    match declared {
        Some("string") => Kind::String,
        Some("number") => Kind::Number,
        Some("integer") => Kind::Integer,
        Some("boolean") => Kind::Boolean,
        Some("array") => Kind::Array,
        Some("object") => Kind::Object,
        _ => Kind::Json,
    }
}

/// The fields of an object schema, required ones first.
///
/// Not declaration order — that is already gone. A schema reaches here having
/// been parsed and re-serialized on the way, and JSON objects come out of
/// that with their keys sorted, so the author's ordering is not recoverable.
/// Given a choice between alphabetical and useful, the fields a call cannot
/// omit go at the top.
pub fn fields_of(schema: Option<&Value>) -> Vec<SchemaField> {
    let Some(properties) = schema
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let required: Vec<&str> = schema
        .and_then(|s| s.get("required"))
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut fields: Vec<SchemaField> = properties
        .iter()
        .map(|(name, property)| SchemaField {
            name: name.clone(),
            kind: kind_of(property),
            required: required.contains(&name.as_str()),
            description: property
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            options: property
                .get("enum")
                .and_then(Value::as_array)
                .map(|options| options.iter().map(text_of).collect())
                .unwrap_or_default(),
            default: property.get("default").map(text_of),
        })
        .collect();
    fields.sort_by_key(|field| !field.required);
    fields
}

/// What a field's text means, given its declared type.
///
/// A value that will not parse as its declared type is sent as typed: the
/// server's complaint about it is more informative than this refusing to ask.
pub fn parse_field(field: &SchemaField, text: &str) -> Option<Value> {
    let trimmed = text.trim();
    // Empty means absent, so an optional field left alone is not sent as "".
    if trimmed.is_empty() {
        return None;
    }
    Some(match field.kind {
        Kind::Boolean => json!(trimmed == "true"),
        Kind::Integer => trimmed
            .parse::<i64>()
            .map(|n| json!(n))
            .unwrap_or_else(|_| json!(text)),
        Kind::Number => trimmed
            .parse::<f64>()
            .map(|n| json!(n))
            .unwrap_or_else(|_| json!(text)),
        Kind::String | Kind::Enum => json!(text),
        _ => serde_json::from_str::<Value>(text).unwrap_or_else(|_| json!(text)),
    })
}

/// The arguments object a form's values describe.
pub fn to_arguments(fields: &[SchemaField], values: &dyn Fn(&str) -> String) -> Value {
    let mut out = Map::new();
    for field in fields {
        if let Some(value) = parse_field(field, &values(&field.name)) {
            out.insert(field.name.clone(), value);
        }
    }
    Value::Object(out)
}

/// Fill a form from an arguments object — the other direction of the toggle.
pub fn from_arguments(fields: &[SchemaField], args: &Value) -> Vec<(String, String)> {
    fields
        .iter()
        .map(|field| {
            let text = args
                .get(&field.name)
                .map(text_of)
                .or_else(|| field.default.clone())
                .unwrap_or_default();
            (field.name.clone(), text)
        })
        .collect()
}

/// What looks wrong, reported rather than enforced.
pub fn problems(fields: &[SchemaField], values: &dyn Fn(&str) -> String) -> Vec<String> {
    let mut found = Vec::new();
    for field in fields {
        let value = values(&field.name);
        let text = value.trim();
        if text.is_empty() {
            if field.required {
                found.push(format!("{}: required", field.name));
            }
            continue;
        }
        match field.kind {
            Kind::Integer if text.parse::<i64>().is_err() => {
                found.push(format!("{}: not an integer", field.name))
            }
            Kind::Number if text.parse::<f64>().is_err() => {
                found.push(format!("{}: not a number", field.name))
            }
            Kind::Array | Kind::Object | Kind::Json => {
                if let Err(e) = serde_json::from_str::<Value>(text) {
                    found.push(format!("{}: {e}", field.name));
                }
            }
            _ => {}
        }
    }
    found
}

/// A prompt's arguments as a schema.
///
/// MCP describes prompt arguments as a name/description/required list rather
/// than as JSON Schema, and their values are strings. Shaping them into a
/// schema is what lets one form serve prompts and tools.
pub fn schema_from_prompt_arguments(arguments: &[PromptArgument]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for argument in arguments {
        let mut property = Map::new();
        property.insert("type".into(), json!("string"));
        if let Some(description) = &argument.description {
            property.insert("description".into(), json!(description));
        }
        properties.insert(argument.name.clone(), Value::Object(property));
        if argument.required {
            required.push(argument.name.clone());
        }
    }
    json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": required,
    })
}

/// One argument a prompt declares. Not JSON Schema — MCP describes these as
/// a name/description/required list — but everything a form needs.
#[derive(Clone, Debug)]
pub struct PromptArgument {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
}

/// The variables an RFC 6570 URI template names, as a schema.
///
/// Only the level-1 `{name}` form and the common operator prefixes are
/// recognised — enough for what servers actually publish. A template this
/// does not understand still falls through to being edited as a URI, which
/// is why it may be conservative without being a problem.
pub fn schema_from_uri_template(template: &str) -> Option<Value> {
    let names = template_variables(template);
    if names.is_empty() {
        return None;
    }
    let mut properties = Map::new();
    for name in &names {
        properties.insert(name.clone(), json!({ "type": "string" }));
    }
    Some(json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": names,
    }))
}

/// A template's variables as fields, in the order the template names them.
///
/// Not via [`schema_from_uri_template`] and [`fields_of`], because a JSON
/// object sorts its keys and that would show `id` before `year` for
/// `{year}/{id}`. Unlike a schema's properties — whose declaration order is
/// gone by the time it reaches us — a template carries its order in the
/// string, so there is no reason to lose it.
pub fn fields_from_uri_template(template: &str) -> Vec<SchemaField> {
    template_variables(template)
        .into_iter()
        .map(|name| SchemaField {
            name,
            kind: Kind::String,
            required: true,
            description: None,
            options: Vec::new(),
            default: None,
        })
        .collect()
}

/// Fill a URI template from values, leaving unfilled expressions alone — a
/// half-expanded URI shows what is still missing, which a silently dropped
/// variable does not.
pub fn expand_uri_template(template: &str, values: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        out.push_str(&rest[..open]);
        let expression = &rest[open + 1..open + close];
        let name = expression
            .trim_start_matches(['+', '#', '.', '/', ';', '?', '&'])
            .split([',', ':', '*'])
            .next()
            .unwrap_or("")
            .trim();
        let value = values(name);
        if value.is_empty() {
            out.push_str(&rest[open..open + close + 1]);
        } else {
            out.push_str(&percent_encode(&value));
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

/// The variable names a template mentions, in the order it mentions them.
fn template_variables(template: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        let expression = &rest[open + 1..open + close];
        for part in expression
            .trim_start_matches(['+', '#', '.', '/', ';', '?', '&'])
            .split(',')
        {
            let name = part.split([':', '*']).next().unwrap_or("").trim();
            if !name.is_empty() && !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
        rest = &rest[open + close + 1..];
    }
    names
}

/// Percent-encode a template variable. A URI path segment is not a place to
/// paste a raw value: a `/` in an id silently names a different resource.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The rows a structured result is displayed as. An output schema names what
/// came back; without one the keys are all there is, and showing them is
/// still better than showing nothing.
pub fn result_rows(schema: Option<&Value>, value: &Value) -> Vec<(String, String)> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    let properties = schema
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object);
    object
        .iter()
        .map(|(key, entry)| {
            let label = properties
                .and_then(|p| p.get(key))
                .and_then(|sub| sub.get("title"))
                .and_then(Value::as_str)
                .unwrap_or(key)
                .to_owned();
            let rendered = match entry {
                Value::String(s) => s.clone(),
                other => serde_json::to_string_pretty(other).unwrap_or_default(),
            };
            (label, rendered)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "One line." },
                "count": { "type": "integer", "default": 3 },
                "ratio": { "type": "number" },
                "urgent": { "type": "boolean" },
                "severity": { "type": "string", "enum": ["low", "high"] },
                "tags": { "type": "array" },
                "meta": { "type": "object" },
                "loose": {},
                "maybe": { "type": ["string", "null"] },
            },
            "required": ["title", "severity"],
        })
    }

    #[test]
    fn every_declared_type_maps_to_the_control_that_edits_it() {
        let fields = fields_of(Some(&demo()));
        let kind = |name: &str| fields.iter().find(|f| f.name == name).unwrap().kind;
        assert_eq!(kind("title"), Kind::String);
        assert_eq!(kind("count"), Kind::Integer);
        assert_eq!(kind("ratio"), Kind::Number);
        assert_eq!(kind("urgent"), Kind::Boolean);
        assert_eq!(kind("severity"), Kind::Enum, "an enum wins over its type");
        assert_eq!(kind("tags"), Kind::Array);
        assert_eq!(kind("meta"), Kind::Object);
        assert_eq!(kind("loose"), Kind::Json, "no type is edited as JSON");
        assert_eq!(
            kind("maybe"),
            Kind::String,
            "`[T, null]` is optional-T, not a union to hand-write"
        );
    }

    /// The order a schema was written in does not survive being parsed and
    /// re-serialized, so the useful ordering is the one this picks.
    #[test]
    fn required_fields_come_first() {
        let fields = fields_of(Some(&demo()));
        let required: Vec<&str> = fields
            .iter()
            .take_while(|f| f.required)
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(required.len(), 2, "{fields:#?}");
        assert!(required.contains(&"title") && required.contains(&"severity"));
    }

    /// The whole point of reading the schema: a form hands back text, and
    /// `{"count": "3"}` is a different call from `{"count": 3}`.
    #[test]
    fn values_are_serialized_as_their_declared_type() {
        let fields = fields_of(Some(&demo()));
        let values = |name: &str| {
            match name {
                "title" => "disk full",
                "count" => "7",
                "ratio" => "0.5",
                "urgent" => "true",
                "severity" => "high",
                "tags" => "[\"ops\"]",
                _ => "",
            }
            .to_owned()
        };
        let args = to_arguments(&fields, &values);
        assert_eq!(args["count"], json!(7));
        assert_eq!(args["ratio"], json!(0.5));
        assert_eq!(args["urgent"], json!(true));
        assert_eq!(args["title"], json!("disk full"));
        assert_eq!(args["tags"], json!(["ops"]));
        // Untouched optional fields are absent, not empty strings.
        assert!(!args.as_object().unwrap().contains_key("meta"));
        assert!(!args.as_object().unwrap().contains_key("ratio2"));
    }

    /// A value the schema disagrees with still goes, because finding out what
    /// the server does with it is the job.
    #[test]
    fn an_unparseable_value_is_sent_as_typed() {
        let fields = fields_of(Some(&demo()));
        let values = |name: &str| if name == "count" { "many" } else { "" }.to_owned();
        let args = to_arguments(&fields, &values);
        assert_eq!(args["count"], json!("many"));

        let complaints = problems(&fields, &values);
        assert!(complaints.iter().any(|p| p == "count: not an integer"));
        assert!(complaints.iter().any(|p| p == "title: required"));
    }

    #[test]
    fn a_form_round_trips_through_its_arguments() {
        let fields = fields_of(Some(&demo()));
        let filled = from_arguments(&fields, &json!({ "title": "x", "count": 9 }));
        let by_name = |name: &str| {
            filled
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(by_name("title"), "x");
        assert_eq!(by_name("count"), "9");
        // A field the arguments do not mention falls back to its default.
        let fresh = from_arguments(&fields, &json!({}));
        let count = fresh.iter().find(|(n, _)| n == "count").unwrap();
        assert_eq!(count.1, "3", "the schema default seeds the field");
    }

    #[test]
    fn a_schema_with_no_properties_has_no_form() {
        assert!(fields_of(None).is_empty());
        assert!(fields_of(Some(&json!({ "type": "object" }))).is_empty());
        assert!(fields_of(Some(&json!("nonsense"))).is_empty());
    }

    #[test]
    fn prompt_arguments_become_a_schema_the_same_form_can_read() {
        let schema = schema_from_prompt_arguments(&[
            PromptArgument {
                name: "incident_id".into(),
                description: Some("Which one".into()),
                required: true,
            },
            PromptArgument {
                name: "tone".into(),
                description: None,
                required: false,
            },
        ]);
        let fields = fields_of(Some(&schema));
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "incident_id", "required first");
        assert!(fields[0].required);
        assert!(!fields[1].required);
        assert_eq!(fields[0].kind, Kind::String);
    }

    #[test]
    fn structured_results_take_their_labels_from_the_output_schema() {
        let schema = json!({
            "type": "object",
            "properties": { "id": { "title": "Incident ID" } },
        });
        let rows = result_rows(Some(&schema), &json!({ "id": "INC-1", "queued": true }));
        let by = |label: &str| {
            rows.iter()
                .find(|(l, _)| l == label)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(by("Incident ID"), Some("INC-1".to_owned()));
        // A key the schema does not describe still shows, under its own name.
        assert_eq!(by("queued"), Some("true".to_owned()));
    }

    /// A template's variables are what a read needs filled; the URI it
    /// expands to is what actually gets sent.
    #[test]
    fn a_uri_template_becomes_a_form_and_expands_back() {
        let schema = schema_from_uri_template("incidents://{year}/{id}").expect("variables");
        let fields = fields_of(Some(&schema));
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"year") && names.contains(&"id"));
        assert!(
            fields.iter().all(|f| f.required),
            "a variable is not optional"
        );

        let filled = |name: &str| match name {
            "year" => "2026".to_owned(),
            "id" => "INC-1".to_owned(),
            _ => String::new(),
        };
        assert_eq!(
            expand_uri_template("incidents://{year}/{id}", &filled),
            "incidents://2026/INC-1"
        );
    }

    /// A value that would change which resource is named must not pass
    /// through as a path separator.
    #[test]
    fn a_variable_is_escaped_into_the_uri() {
        let sneaky = |_: &str| "../secrets/1".to_owned();
        assert_eq!(
            expand_uri_template("docs://{name}", &sneaky),
            "docs://..%2Fsecrets%2F1"
        );
    }

    /// An unfilled variable stays visible rather than collapsing to an empty
    /// segment, so a half-filled URI reads as half-filled.
    #[test]
    fn an_unfilled_variable_is_left_in_place() {
        let partial = |name: &str| if name == "year" { "2026" } else { "" }.to_owned();
        assert_eq!(
            expand_uri_template("incidents://{year}/{id}", &partial),
            "incidents://2026/{id}"
        );
    }

    /// The operator forms servers actually publish are read; a template with
    /// no variables has no form and is edited as a URI.
    #[test]
    fn template_syntax_beyond_the_simple_form_is_still_read() {
        let schema = schema_from_uri_template("s://{+path}/{?a,b}").expect("variables");
        let names: Vec<String> = fields_of(Some(&schema))
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert!(names.contains(&"path".to_owned()), "{names:?}");
        assert!(names.contains(&"a".to_owned()) && names.contains(&"b".to_owned()));
        assert!(schema_from_uri_template("docs://runbook").is_none());
    }

    /// `{year}/{id}` must offer year first. Routing this through a schema
    /// sorted the keys and put `id` first, which reads as a different
    /// template and fills the URI backwards.
    #[test]
    fn template_fields_keep_the_order_the_template_names_them_in() {
        let fields = fields_from_uri_template("incidents://{year}/{id}");
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["year", "id"]);
        assert!(fields.iter().all(|f| f.required && f.kind == Kind::String));
        assert!(fields_from_uri_template("docs://runbook").is_empty());
    }
}
