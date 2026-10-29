// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The file tools a coding-style assistant gets over the ledger's own
//! directory: `read`, `edit`, `grep`, `glob`. One registry defines each tool
//! once — name, description, JSON Schema — and [`invoke`] is the only way in,
//! so the model-facing description and the implementation cannot drift apart.
//!
//! Nothing here knows about Tauri, the loaded ledger or the network: a caller
//! passes the directory to confine the tools to, and `edit` writes the file
//! the caller named. There is deliberately no shell.
//!
//! # The edit protocol
//!
//! 1. `read` returns lines as `{"n": 12, "hash": "9f2a", "text": "..."}`. The
//!    hash is four hex characters of that line's *content*, so it survives
//!    edits to other lines.
//! 2. `edit` takes a batch of operations that name lines by those hashes:
//!
//!    - `{"op": "replace", "at": "9f2a", "lines": ["new text"]}` — replace the
//!      line with `lines` (an empty `lines` deletes it);
//!    - `{"op": "insert_after", "after": "9f2a", "lines": ["new text"]}` —
//!      insert after that line;
//!    - `{"op": "delete", "from": "9f2a", "to": "1c7d"}` — remove the
//!      inclusive range;
//!    - `{"op": "write", "content": "..."}` — replace the whole file, creating
//!      it if needed; it must be the only operation.
//!
//!    An anchor may also be written `hash@line` (e.g. `9f2a@12`), which is how
//!    two identical lines — two blank lines, say — are told apart.
//! 3. The batch is atomic: every anchor is checked against the file as it is
//!    on disk first, and one that no longer matches refuses the whole edit
//!    with `line changed at N`, leaving the file byte-identical. Operations
//!    are read in the coordinates of that same text and must be given in
//!    ascending, non-overlapping order.
//! 4. The result echoes the touched region with fresh hashes, so the next edit
//!    can chain without another `read`.

use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

pub mod hashline;
pub(crate) mod paths;
pub mod search;

use hashline::EditOp;
use paths::Root;

/// One tool, as both the model and the dispatcher see it.
#[derive(Debug, Clone)]
pub struct ToolDef {
    /// The name `invoke` takes, and the Python function's name.
    pub name: &'static str,
    /// What the model is told the tool does.
    pub description: &'static str,
    /// A draft-07 subset: `type`, `properties`, `required`, `items`, `oneOf`,
    /// `enum`, `const`, `default`, `minimum`, `maximum`, `minItems`, `description`.
    pub schema: Value,
    /// Whether a call can only read: `edit` is the one that writes.
    pub read_only: bool,
}

/// The registry, in the order tools are offered to the model.
pub fn registry() -> &'static [ToolDef] {
    static REGISTRY: std::sync::LazyLock<Vec<ToolDef>> = std::sync::LazyLock::new(build);
    REGISTRY.as_slice()
}

/// A failed tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// The name is not in the registry.
    UnknownTool { name: String },
    /// The arguments do not match the tool's schema.
    BadArgs { tool: String, message: String },
    /// The call ran and refused: a stale anchor, a path outside the root, an
    /// unreadable file.
    Failed { tool: String, message: String },
}

impl ToolError {
    fn bad_args(tool: &str, message: impl Into<String>) -> Self {
        Self::BadArgs {
            tool: tool.to_string(),
            message: message.into(),
        }
    }

    fn failed(tool: &str, message: impl Into<String>) -> Self {
        Self::Failed {
            tool: tool.to_string(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTool { name } => write!(
                formatter,
                "unknown tool `{name}`; the available tools are {}",
                registry_names()
            ),
            Self::BadArgs { tool, message } => {
                write!(formatter, "`{tool}`: {message}")
            }
            Self::Failed { tool, message } => {
                write!(formatter, "`{tool}`: {message}")
            }
        }
    }
}

impl std::error::Error for ToolError {}

pub type ToolResult = Result<Value, ToolError>;

/// The tool names, comma-separated, for error messages.
fn registry_names() -> String {
    registry()
        .iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Run the tool `name` with `args` over `root`.
///
/// Every path the tools take is relative to `root` (an absolute path is taken
/// only if it already sits inside it), and anything that would leave it is
/// refused. The result is the tool's JSON answer; the error message is written
/// to be handed straight back to a model.
pub fn invoke(root: &Path, name: &str, args: &Value) -> ToolResult {
    let definition = registry()
        .iter()
        .find(|definition| definition.name == name)
        .ok_or_else(|| ToolError::UnknownTool {
            name: name.to_string(),
        })?;
    if !args.is_object() {
        return Err(ToolError::bad_args(
            name,
            format!(
                "the arguments must be a JSON object; expected {}",
                expected_args(&definition.schema)
            ),
        ));
    }
    let root = Root::new(root).map_err(|message| ToolError::failed(name, message))?;

    match definition.name {
        "read" => {
            let args: ReadArgs = decode(definition, args)?;
            let path = root
                .resolve(&args.path)
                .map_err(|message| ToolError::failed(name, message))?;
            let outcome = hashline::read_file(&path, args.offset, args.limit)
                .map_err(|message| ToolError::failed(name, message))?;
            tagged(name, &args.path, &outcome)
        }
        "edit" => {
            let args: EditArgs = decode(definition, args)?;
            let path = root
                .resolve(&args.path)
                .map_err(|message| ToolError::failed(name, message))?;
            let outcome = hashline::apply_edits(&path, &args.ops)
                .map_err(|message| ToolError::failed(name, message))?;
            tagged(name, &args.path, &outcome)
        }
        "grep" => {
            let args: GrepArgs = decode(definition, args)?;
            let target = root
                .resolve(args.path.as_deref().unwrap_or("."))
                .map_err(|message| ToolError::failed(name, message))?;
            let outcome = search::grep(
                root.path(),
                &target,
                &args.pattern,
                args.glob.as_deref(),
                args.case_sensitive,
                args.max_results.unwrap_or(search::DEFAULT_GREP_RESULTS),
            )
            .map_err(|message| ToolError::failed(name, message))?;
            to_json(&outcome, name)
        }
        "glob" => {
            let args: GlobArgs = decode(definition, args)?;
            let outcome = search::glob_paths(
                root.path(),
                root.path(),
                &args.pattern,
                args.max_results.unwrap_or(search::DEFAULT_GLOB_RESULTS),
            )
            .map_err(|message| ToolError::failed(name, message))?;
            to_json(&outcome, name)
        }
        other => Err(ToolError::UnknownTool {
            name: other.to_string(),
        }),
    }
}

/// An outcome as JSON with the path the caller used in front of it.
fn tagged(tool: &str, path: &str, outcome: &impl Serialize) -> ToolResult {
    let answer = to_json(outcome, tool)?;
    let mut object = Map::new();
    object.insert("path".to_string(), Value::String(path.to_string()));
    if let Value::Object(fields) = answer {
        object.extend(fields);
    }
    Ok(Value::Object(object))
}

fn to_json(value: &impl Serialize, tool: &str) -> ToolResult {
    serde_json::to_value(value)
        .map_err(|error| ToolError::failed(tool, format!("cannot render the result: {error}")))
}

fn decode<T: DeserializeOwned>(definition: &ToolDef, args: &Value) -> Result<T, ToolError> {
    T::deserialize(args).map_err(|error| {
        ToolError::bad_args(
            definition.name,
            format!("{error}; expected {}", expected_args(&definition.schema)),
        )
    })
}

/// The arguments a schema takes, for a message a model can act on.
fn expected_args(schema: &Value) -> String {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return "no arguments".to_string();
    };
    if properties.is_empty() {
        return "no arguments".to_string();
    }
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    properties
        .iter()
        .map(|(name, property)| {
            let kind = property
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("any");
            let presence = if required.contains(&name.as_str()) {
                "required"
            } else {
                "optional"
            };
            format!("`{name}` ({kind}, {presence})")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    path: String,
    ops: Vec<EditOp>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GrepArgs {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    #[serde(default)]
    case_sensitive: bool,
    max_results: Option<usize>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GlobArgs {
    pattern: String,
    max_results: Option<usize>,
}

fn build() -> Vec<ToolDef> {
    let read = json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "File path relative to the ledger directory."
            },
            "offset": {
                "type": "integer",
                "minimum": 1,
                "default": hashline::DEFAULT_OFFSET,
                "description": "First line to return (1-based)."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": hashline::MAX_READ_LIMIT,
                "default": hashline::DEFAULT_READ_LIMIT,
                "description": "How many lines to return."
            }
        },
        "required": ["path"],
        "additionalProperties": false
    });

    let edit = json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "File path relative to the ledger directory."
            },
            "ops": {
                "type": "array",
                "minItems": 1,
                "description": "Operations, resolved against the file as it is on disk and applied all or nothing. Give them in ascending line order.",
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "description": "Replace the whole file, creating it if needed. The only operation allowed.",
                            "properties": {
                                "op": {"const": "write"},
                                "content": {"type": "string"}
                            },
                            "required": ["op", "content"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Replace the line whose hash is `at` with `lines` (empty deletes it).",
                            "properties": {
                                "op": {"const": "replace"},
                                "at": {"type": "string"},
                                "lines": {"type": "array", "items": {"type": "string"}}
                            },
                            "required": ["op", "at", "lines"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Insert `lines` after the line whose hash is `after`.",
                            "properties": {
                                "op": {"const": "insert_after"},
                                "after": {"type": "string"},
                                "lines": {"type": "array", "items": {"type": "string"}}
                            },
                            "required": ["op", "after", "lines"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Remove the lines from `from` through `to`, inclusive.",
                            "properties": {
                                "op": {"const": "delete"},
                                "from": {"type": "string"},
                                "to": {"type": "string"}
                            },
                            "required": ["op", "from", "to"],
                            "additionalProperties": false
                        }
                    ]
                }
            }
        },
        "required": ["path", "ops"],
        "additionalProperties": false
    });

    let grep = json!({
        "type": "object",
        "properties": {
            "pattern": {
                "type": "string",
                "description": "Regular expression, matched line by line."
            },
            "path": {
                "type": "string",
                "default": ".",
                "description": "File or directory to search, relative to the ledger directory."
            },
            "glob": {
                "type": "string",
                "description": "Only search files whose relative path matches this glob, e.g. `*.bean`."
            },
            "case_sensitive": {
                "type": "boolean",
                "default": false,
                "description": "Match case exactly. Defaults to false."
            },
            "max_results": {
                "type": "integer",
                "minimum": 1,
                "maximum": search::MAX_GREP_RESULTS,
                "default": search::DEFAULT_GREP_RESULTS,
                "description": "Stop after this many matching lines."
            }
        },
        "required": ["pattern"],
        "additionalProperties": false
    });

    let glob = json!({
        "type": "object",
        "properties": {
            "pattern": {
                "type": "string",
                "description": "Glob on paths relative to the ledger directory; `*` stays in one component and `**` crosses directories. A pattern without `/` also matches bare file names."
            },
            "max_results": {
                "type": "integer",
                "minimum": 1,
                "maximum": search::MAX_GLOB_RESULTS,
                "default": search::DEFAULT_GLOB_RESULTS,
                "description": "Stop after this many paths."
            }
        },
        "required": ["pattern"],
        "additionalProperties": false
    });

    vec![
        ToolDef {
            name: "read",
            description: "Read a text file under the ledger directory. Returns numbered lines, each with a four-hex-character hash of its content; an edit anchors to those hashes, optionally as hash@line. offset is 1-based.",
            schema: read,
            read_only: true,
        },
        ToolDef {
            name: "edit",
            description: "Edit a file by line hash, all or nothing. Operations: replace {at, lines}; insert_after {after, lines}; delete {from, to} (inclusive); write {content} for the whole file. Anchors are the hashes read returned, as hash or hash@line; if one no longer matches the file, nothing is written and the error names the line. To put lines above the first line, replace it with those lines and its own text. The result echoes the touched region with fresh hashes for chaining.",
            schema: edit,
            read_only: false,
        },
        ToolDef {
            name: "grep",
            description: "Search the ledger directory line by line for a regular expression. Respects .gitignore and .ignore, skips hidden and binary files, and returns the file, the 1-based line and the text. Stops at max_results and says so.",
            schema: grep,
            read_only: true,
        },
        ToolDef {
            name: "glob",
            description: "List files under the ledger directory whose relative path matches a glob. Respects .gitignore and .ignore, skips hidden files, and returns the paths sorted. Stops at max_results and says so.",
            schema: glob,
            read_only: true,
        },
    ]
}

/// The registry rendered for a model: the OpenAI `tools` array and a Python
/// stub module, both generated from the same definitions.
///
/// A later layer can put either in front of the model; a test fails if the two

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use serde_json::{Value, json};

    use crate::testing::TempDir;

    use super::{ToolError, expected_args, invoke, registry};

    fn scratch() -> TempDir {
        TempDir::new("tools")
    }

    fn seed(dir: &TempDir, name: &str, text: &str) {
        dir.seed(name, text);
    }

    #[test]
    fn invoke_reads_and_edits() {
        let dir = scratch();
        seed(
            &dir,
            "main.bean",
            "2026-01-01 open Assets:Bank\n2026-01-02 open Assets:Cash\n",
        );

        let read = invoke(dir.dir(), "read", &json!({"path": "main.bean"})).unwrap();
        assert_eq!(read["path"], "main.bean");
        assert_eq!(read["total_lines"], 2);
        assert_eq!(read["truncated"], false);
        let first = read["lines"][0]["hash"].as_str().unwrap().to_string();

        let edited = invoke(
            dir.dir(),
            "edit",
            &json!({
                "path": "main.bean",
                "ops": [{
                    "op": "insert_after",
                    "after": format!("{first}@1"),
                    "lines": ["; a note"]
                }]
            }),
        )
        .unwrap();
        assert_eq!(edited["path"], "main.bean");
        assert_eq!(edited["written"], true);
        assert_eq!(edited["first"], 2);
        assert_eq!(edited["lines"][0]["text"], "; a note");
        assert_eq!(
            fs::read_to_string(dir.dir().join("main.bean")).unwrap(),
            "2026-01-01 open Assets:Bank\n; a note\n2026-01-02 open Assets:Cash\n"
        );

        // Through the dispatcher a stale anchor is a failed call, not bad
        // arguments: the caller sent a well-formed request the file outran.
        let error = invoke(
            dir.dir(),
            "edit",
            &json!({
                "path": "main.bean",
                "ops": [{"op": "replace", "at": format!("{first}@2"), "lines": ["x"]}]
            }),
        )
        .unwrap_err();
        assert!(matches!(error, ToolError::Failed { .. }), "{error:?}");
        assert!(error.to_string().contains("line changed at 2"), "{error}");

        let missing = invoke(dir.dir(), "read", &json!({"path": "gone.bean"})).unwrap_err();
        assert!(matches!(missing, ToolError::Failed { .. }), "{missing:?}");
        assert!(missing.to_string().contains("no such file"), "{missing}");
    }

    #[test]
    fn invoke_refuses_paths_outside_the_root() {
        let dir = scratch();
        seed(&dir, "main.bean", "x\n");
        let outside = dir
            .dir()
            .parent()
            .unwrap()
            .join(format!("elsewhere-{}.bean", std::process::id()));

        for path in [
            "../elsewhere.bean",
            outside.to_str().unwrap(),
            "sub/../../elsewhere.bean",
        ] {
            for (tool, args) in [
                ("read", json!({"path": path})),
                (
                    "edit",
                    json!({"path": path, "ops": [{"op": "write", "content": "x"}]}),
                ),
                ("grep", json!({"pattern": "x", "path": path})),
            ] {
                let error = invoke(dir.dir(), tool, &args).unwrap_err();
                assert!(
                    matches!(error, ToolError::Failed { .. }),
                    "{tool} {path}: {error:?}"
                );
                assert!(
                    error.to_string().contains("outside the ledger directory"),
                    "{tool} {path}: {error}"
                );
            }
        }
        assert!(!outside.exists(), "nothing was created outside the root");
    }

    #[test]
    fn registry_is_well_formed() {
        let tools = registry();
        assert_eq!(
            tools
                .iter()
                .map(|definition| definition.name)
                .collect::<Vec<_>>(),
            ["read", "edit", "grep", "glob"]
        );
        let names: BTreeSet<&str> = tools.iter().map(|definition| definition.name).collect();
        assert_eq!(names.len(), tools.len(), "names are unique");

        for definition in tools {
            assert!(!definition.name.is_empty());
            assert!(!definition.description.is_empty());
            check_schema(&definition.schema, definition.name)
                .unwrap_or_else(|error| panic!("{}: {error}", definition.name));
            assert_eq!(
                definition.schema.get("type").and_then(Value::as_str),
                Some("object"),
                "{}: arguments are an object",
                definition.name
            );
            assert_eq!(
                definition
                    .schema
                    .get("additionalProperties")
                    .and_then(Value::as_bool),
                Some(false),
                "{}: unknown arguments are refused",
                definition.name
            );
        }
        let read_only: Vec<(&str, bool)> = tools
            .iter()
            .map(|definition| (definition.name, definition.read_only))
            .collect();
        assert_eq!(
            read_only,
            [
                ("read", true),
                ("edit", false),
                ("grep", true),
                ("glob", true)
            ]
        );
    }

    #[test]
    fn invoke_rejects_unknown_tools_and_misshapen_args() {
        let dir = scratch();
        seed(&dir, "main.bean", "2026-01-01 open Assets:Bank\n");

        let error = invoke(dir.dir(), "bash", &json!({})).unwrap_err();
        assert_eq!(
            error,
            ToolError::UnknownTool {
                name: "bash".to_string()
            }
        );
        let message = error.to_string();
        assert!(message.contains("unknown tool `bash`"), "{message}");
        assert!(message.contains("read, edit, grep, glob"), "{message}");

        let error = invoke(dir.dir(), "read", &json!({})).unwrap_err();
        assert!(matches!(error, ToolError::BadArgs { .. }), "{error:?}");
        assert!(error.to_string().contains("`path`"), "{error}");
        assert!(error.to_string().contains("required"), "{error}");

        let error = invoke(dir.dir(), "read", &json!({"path": 7})).unwrap_err();
        assert!(matches!(error, ToolError::BadArgs { .. }), "{error:?}");
        assert!(error.to_string().contains("invalid type"), "{error}");

        let error = invoke(
            dir.dir(),
            "read",
            &json!({"path": "main.bean", "offest": 2}),
        )
        .unwrap_err();
        assert!(matches!(error, ToolError::BadArgs { .. }), "{error:?}");
        assert!(error.to_string().contains("offest"), "{error}");

        let error = invoke(dir.dir(), "read", &json!(["main.bean"])).unwrap_err();
        assert!(error.to_string().contains("JSON object"), "{error}");

        let error = invoke(
            dir.dir(),
            "edit",
            &json!({
                "path": "main.bean",
                "ops": [{"op": "modify", "at": "0000"}]
            }),
        )
        .unwrap_err();
        assert!(matches!(error, ToolError::BadArgs { .. }), "{error:?}");
        assert!(error.to_string().contains("unknown variant"), "{error}");

        let error =
            invoke(dir.dir(), "edit", &json!({"path": "main.bean", "ops": []})).unwrap_err();
        assert!(
            error.to_string().contains("at least one operation"),
            "{error}"
        );

        let error = invoke(dir.dir(), "grep", &json!({"pattern": "(", "path": "."})).unwrap_err();
        assert!(
            error.to_string().contains("invalid regular expression"),
            "{error}"
        );
    }

    fn dummy(schema: &Value) -> Value {
        let mut object = serde_json::Map::new();
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (name, property) in properties {
                if let Some(value) = dummy_value(property) {
                    object.insert(name.clone(), value);
                }
            }
        }
        Value::Object(object)
    }

    fn dummy_value(schema: &Value) -> Option<Value> {
        if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
            return branches.first().map(dummy);
        }
        if let Some(value) = schema.get("const") {
            return Some(value.clone());
        }
        if let Some(default) = schema.get("default") {
            return Some(default.clone());
        }
        match schema.get("type").and_then(Value::as_str)? {
            "string" => Some(Value::String("x".to_string())),
            "integer" => Some(Value::from(
                schema.get("minimum").and_then(Value::as_u64).unwrap_or(1),
            )),
            "boolean" => Some(Value::Bool(false)),
            "array" => {
                let items = schema.get("items")?;
                let count = schema
                    .get("minItems")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .max(1);
                Some(Value::Array(
                    (0..count)
                        .map(|_| dummy_value(items).unwrap_or(Value::Null))
                        .collect(),
                ))
            }
            "object" => Some(dummy(schema)),
            _ => None,
        }
    }

    #[test]
    fn every_schema_property_is_accepted() {
        for definition in registry() {
            assert!(!expected_args(&definition.schema).is_empty());
            let dir = scratch();
            let args = dummy(&definition.schema);
            let result = invoke(dir.dir(), definition.name, &args);
            if let Err(ToolError::BadArgs { message, .. }) = &result {
                panic!(
                    "{}: the schema and the argument type disagree: {message}",
                    definition.name
                );
            }
        }
    }

    #[test]
    fn every_required_argument_is_named_when_missing() {
        for definition in registry() {
            let dir = scratch();
            let required: Vec<String> = definition
                .schema
                .get("required")
                .and_then(Value::as_array)
                .map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                !required.is_empty(),
                "{}: nothing is required",
                definition.name
            );
            for name in required {
                let mut args = dummy(&definition.schema);
                args.as_object_mut().unwrap().remove(&name);
                let error = invoke(dir.dir(), definition.name, &args).unwrap_err();
                assert!(
                    matches!(error, ToolError::BadArgs { .. }),
                    "{}: {error:?}",
                    definition.name
                );
                assert!(
                    error.to_string().contains(&name),
                    "{}: the message does not name `{name}`: {error}",
                    definition.name
                );
            }
        }
    }

    /// A draft-07 subset strict enough to catch a typo or a misplaced keyword.
    fn check_schema(node: &Value, at: &str) -> Result<(), String> {
        let Some(object) = node.as_object() else {
            return Err(format!("{at}: a schema must be an object"));
        };
        for (keyword, value) in object {
            let nested = |name: &str| format!("{at}.{name}");
            match keyword.as_str() {
                "type" => {
                    let kind = value
                        .as_str()
                        .ok_or_else(|| format!("{at}: `type` must be a string"))?;
                    if !matches!(
                        kind,
                        "object" | "array" | "string" | "integer" | "number" | "boolean" | "null"
                    ) {
                        return Err(format!("{at}: `type` is not a JSON type: {kind}"));
                    }
                }
                "description" | "title" => {
                    if !value.is_string() {
                        return Err(format!("{at}: `{keyword}` must be a string"));
                    }
                }
                "properties" => {
                    let properties = value
                        .as_object()
                        .ok_or_else(|| format!("{at}: `properties` must be an object"))?;
                    for (name, property) in properties {
                        check_schema(property, &nested(name))?;
                    }
                    if let Some(required) = object.get("required").and_then(Value::as_array) {
                        for name in required {
                            let name = name
                                .as_str()
                                .ok_or_else(|| format!("{at}: `required` holds strings"))?;
                            if !properties.contains_key(name) {
                                return Err(format!(
                                    "{at}: `required` names `{name}`, which is not a property"
                                ));
                            }
                        }
                    }
                }
                "required" => {
                    let names = value
                        .as_array()
                        .ok_or_else(|| format!("{at}: `required` must be an array"))?;
                    if names.is_empty() {
                        return Err(format!("{at}: `required` is empty"));
                    }
                    if !names.iter().all(Value::is_string) {
                        return Err(format!("{at}: `required` holds strings"));
                    }
                }
                "additionalProperties" => {
                    if !value.is_boolean() {
                        check_schema(value, &nested(keyword))?;
                    }
                }
                "items" => check_schema(value, &nested(keyword))?,
                "oneOf" | "anyOf" | "allOf" => {
                    let branches = value
                        .as_array()
                        .ok_or_else(|| format!("{at}: `{keyword}` must be an array"))?;
                    if branches.is_empty() {
                        return Err(format!("{at}: `{keyword}` is empty"));
                    }
                    for (index, branch) in branches.iter().enumerate() {
                        check_schema(branch, &nested(&format!("{keyword}[{index}]")))?;
                    }
                }
                "enum" => {
                    let values = value
                        .as_array()
                        .ok_or_else(|| format!("{at}: `enum` must be an array"))?;
                    if values.is_empty() {
                        return Err(format!("{at}: `enum` is empty"));
                    }
                }
                "const" | "default" => {}
                "minimum" | "maximum" => {
                    if !value.is_number() {
                        return Err(format!("{at}: `{keyword}` must be a number"));
                    }
                }
                "minItems" | "maxItems" | "minLength" | "maxLength" => {
                    if value.as_u64().is_none() {
                        return Err(format!("{at}: `{keyword}` must be a non-negative integer"));
                    }
                }
                "pattern" => {
                    let pattern = value
                        .as_str()
                        .ok_or_else(|| format!("{at}: `pattern` must be a string"))?;
                    regex::Regex::new(pattern)
                        .map_err(|error| format!("{at}: `pattern` is not a regex: {error}"))?;
                }
                other => return Err(format!("{at}: unknown schema keyword `{other}`")),
            }
        }
        Ok(())
    }
}
