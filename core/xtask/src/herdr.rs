//! `cargo xtask gen-herdr-types`: regenerates the herdr wire types.
//!
//!   core/or2-core/src/herdr/schema.json     normalized schema (see [`normalize`])
//!   core/or2-core/src/herdr/generated.rs    cargo-typify output, one module per family
//!
//! `herdr api schema --json` prints a bundle: `schemas.<family>` are JSON Schemas whose `$ref`s
//! point into the bundle (`#/schemas/<family>/$defs/X`), which typify cannot resolve. The
//! normalized file keeps the families that or2 generates code for with their refs rewritten to
//! local definitions (`#/$defs/X`), so each family stands alone, plus the herdr version and
//! protocol it came from.
//!
//! Normalization also makes the client tolerant, because herdr may grow:
//!   * Validation keywords a client has no use for (`pattern`, `propertyNames`, `maxProperties`,
//!     `minProperties`) are dropped, so one odd map key cannot fail a whole snapshot.
//!   * In the families herdr *sends* (everything but `request`), a string enum becomes
//!     `oneOf [enum, string]`. typify turns that into an untagged enum whose second variant
//!     holds any value this build does not know, so a new status never fails a message.
//!   * `request` is split from its `id` envelope: the bundle describes `{id} + oneOf
//!     [{method, params}]`, which typify renders as an untyped struct. Only the `oneOf` is kept
//!     (titled `RequestBody`); typify generates an adjacently tagged enum (`method`, `params`)
//!     and or2's wire layer adds the `id`.
//!
//! The `event` and `subscription_event` families are deliberately left out: typify renders their
//! internally tagged `EventData` as an enum that rejects an unknown event type, and nothing
//! consumes event payloads (events are invalidations). A consumer that needs them must add the
//! families here together with an open event-type tag.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Map, Value, json};

use crate::json::dumps;
use crate::{Result, fail, repo_root};

const FAMILIES: [&str; 3] = ["request", "success_response", "error_response"];
const DROPPED_KEYWORDS: [&str; 4] = ["pattern", "propertyNames", "maxProperties", "minProperties"];
const SENT_BY_CLIENT: [&str; 1] = ["request"];

pub const USAGE: &str = "\
Usage: cargo xtask gen-herdr-types [--herdr PATH] [--offline] [--check]
  --herdr PATH  the herdr binary whose `api schema --json` is the source (default: herdr)
  --offline     regenerate generated.rs from the checked-in schema.json without running herdr
  --check       write nothing; fail if the checked-in files differ from a regeneration

Needs rustfmt and cargo-typify (`cargo install cargo-typify --version 0.10.0-alpha.1 --locked`).
Never edit generated.rs: change core/xtask/src/herdr.rs and regenerate.";

struct Options {
    herdr: String,
    offline: bool,
    check: bool,
}

fn parse_args(args: &[String]) -> Option<Options> {
    let mut options = Options {
        herdr: "herdr".to_string(),
        offline: false,
        check: false,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--herdr" => options.herdr = args.next()?.clone(),
            "--offline" => options.offline = true,
            "--check" => options.check = true,
            _ => return None,
        }
    }
    Some(options)
}

pub fn run(args: &[String]) -> Result<()> {
    let Some(options) = parse_args(args) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let dest = repo_root().join("core/or2-core/src/herdr");
    let work = WorkDir::new()?;
    fs::create_dir(work.path().join("families")).map_err(|e| format!("mkdir: {e}"))?;

    let schema_text = if options.offline {
        read(&dest.join("schema.json"))?
    } else {
        let raw = capture(Command::new(&options.herdr).args(["api", "schema", "--json"]))?;
        let version_output = capture(Command::new(&options.herdr).arg("--version"))?;
        let version = herdr_version(&version_output)
            .ok_or_else(|| format!("`{} --version` printed nothing", options.herdr))?;
        let bundle: Value =
            serde_json::from_str(&raw).map_err(|e| format!("herdr schema is not JSON: {e}"))?;
        normalize_text(&bundle, version)?
    };
    let schema_path = work.path().join("schema.json");
    write(&schema_path, &schema_text)?;

    let normalized: Value =
        serde_json::from_str(&schema_text).map_err(|e| format!("schema.json: {e}"))?;
    let (version, protocol) = split(&normalized, &work.path().join("families"))?;

    let mut generated = header(&version, &protocol);
    for family in FAMILIES {
        let out = work.path().join(format!("{family}.rs"));
        let input = work.path().join("families").join(format!("{family}.json"));
        let output = Command::new("cargo")
            .args(["typify", "-B", "-o"])
            .arg(&out)
            .arg(&input)
            .output()
            .map_err(|e| format!("cannot run cargo typify: {e}"))?;
        eprint!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !output.status.success() {
            return fail(format!("cargo typify failed for {family}"));
        }
        generated.push_str(&format!(
            "\n/// Types generated from the `{family}` schema.\npub mod {family} {{\n"
        ));
        for line in read(&out)?.lines() {
            if !line.starts_with("#![") {
                generated.push_str(line);
                generated.push('\n');
            }
        }
        generated.push_str("}\n");
    }
    let generated_path = work.path().join("generated.rs");
    write(&generated_path, &generated)?;
    let status = Command::new("rustfmt")
        .args(["--edition", "2024"])
        .arg(&generated_path)
        .status()
        .map_err(|e| format!("cannot run rustfmt: {e}"))?;
    if !status.success() {
        return fail("rustfmt failed on the generated file");
    }
    let generated = read(&generated_path)?;

    if options.check {
        let mut stale = Vec::new();
        if !options.offline && read(&dest.join("schema.json"))? != schema_text {
            stale.push("schema.json");
        }
        if read(&dest.join("generated.rs"))? != generated {
            stale.push("generated.rs");
        }
        if stale.is_empty() {
            eprintln!("gen-herdr-types: schema.json and generated.rs up to date");
            return Ok(());
        }
        return fail(format!(
            "stale: {} (run cargo xtask gen-herdr-types)",
            stale.join(", ")
        ));
    }
    if !options.offline {
        write(&dest.join("schema.json"), &schema_text)?;
    }
    write(&dest.join("generated.rs"), &generated)?;
    eprintln!(
        "wrote {0}/schema.json and generated.rs (herdr {version}, protocol {protocol})",
        dest.display()
    );
    Ok(())
}

/// The version is the last word of the first line of `herdr --version`.
fn herdr_version(output: &str) -> Option<&str> {
    output.lines().next()?.split_whitespace().last()
}

fn header(version: &str, protocol: &str) -> String {
    format!(
        "\
//! **Generated. Do not edit.** herdr's socket API types for herdr {version} (protocol {protocol}),
//! produced by `scripts/gen-herdr-types.sh` from `schema.json` (the normalized output of
//! `herdr api schema --json`) with cargo-typify. To regenerate after a herdr update, run
//! `scripts/gen-herdr-types.sh`; fix problems in `scripts/herdr_schema.py`, never here.
//!
//! One module per schema family. Enums herdr sends carry a catch-all variant for values this
//! build does not know; unknown fields are ignored.

#![allow(clippy::all, dead_code, unused_imports)]

/// The herdr release this code was generated from.
pub const HERDR_VERSION: &str = \"{version}\";
/// The herdr API protocol number this code was generated from.
pub const PROTOCOL: u32 = {protocol};
"
    )
}

fn rewrite_refs(node: &mut Value, prefix: &str) -> Result<()> {
    match node {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get_mut("$ref") {
                let Some(rest) = reference.strip_prefix(prefix) else {
                    return fail(format!(
                        "unexpected $ref {reference:?}, expected prefix {prefix:?}"
                    ));
                };
                *reference = format!("#/{rest}");
            }
            for value in map.values_mut() {
                rewrite_refs(value, prefix)?;
            }
        }
        Value::Array(items) => {
            for value in items {
                rewrite_refs(value, prefix)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn drop_validation(node: &mut Value) {
    match node {
        Value::Object(map) => {
            for key in DROPPED_KEYWORDS {
                map.remove(key);
            }
            for value in map.values_mut() {
                drop_validation(value);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(drop_validation),
        _ => {}
    }
}

/// Rewrites every string enum to `oneOf [enum, string]`.
fn open_enums(node: &mut Value) {
    match node {
        Value::Object(map) => {
            for value in map.values_mut() {
                open_enums(value);
            }
            let is_string_enum = map.get("type") == Some(&json!("string"))
                && matches!(map.get("enum"), Some(Value::Array(_)));
            if is_string_enum {
                let known = json!({"type": "string", "enum": map.remove("enum")});
                map.remove("type");
                map.insert("oneOf".to_string(), json!([known, {"type": "string"}]));
            }
        }
        Value::Array(items) => items.iter_mut().for_each(open_enums),
        _ => {}
    }
}

/// The normalized schema for a `herdr api schema --json` bundle.
pub fn normalize(bundle: &Value, herdr_version: &str) -> Result<Value> {
    let schemas = bundle
        .get("schemas")
        .and_then(Value::as_object)
        .ok_or("bundle has no `schemas` object")?;
    let missing: Vec<&str> = FAMILIES
        .into_iter()
        .filter(|name| !schemas.contains_key(*name))
        .collect();
    if !missing.is_empty() {
        return fail(format!("bundle lacks schema families: {missing:?}"));
    }
    let mut families = Map::new();
    for name in FAMILIES {
        let mut schema = schemas[name].clone();
        rewrite_refs(&mut schema, &format!("#/schemas/{name}/"))?;
        drop_validation(&mut schema);
        if name == "request"
            && let Some(map) = schema.as_object_mut()
        {
            for key in ["properties", "required", "type"] {
                map.remove(key);
            }
            map.insert("title".to_string(), json!("RequestBody"));
        }
        if !SENT_BY_CLIENT.contains(&name) {
            open_enums(&mut schema);
        }
        families.insert(name.to_string(), schema);
    }
    Ok(json!({
        "source": {
            "herdr_version": herdr_version,
            "protocol": bundle.get("protocol").ok_or("bundle has no `protocol`")?,
            "schema_version": bundle.get("schema_version").ok_or("bundle has no `schema_version`")?,
            "command": "herdr api schema --json",
        },
        "families": families,
    }))
}

/// `schema.json` text: sorted keys, one-space indent, a trailing newline.
fn normalize_text(bundle: &Value, herdr_version: &str) -> Result<String> {
    Ok(dumps(&normalize(bundle, herdr_version)?, Some(1), true) + "\n")
}

/// Writes one typify input per family; returns the herdr version and protocol.
fn split(normalized: &Value, out_dir: &Path) -> Result<(String, String)> {
    let families = normalized
        .get("families")
        .and_then(Value::as_object)
        .ok_or("schema.json has no `families`")?;
    for (name, schema) in families {
        write(
            &out_dir.join(format!("{name}.json")),
            &dumps(schema, Some(1), true),
        )?;
    }
    let source = normalized
        .get("source")
        .ok_or("schema.json has no `source`")?;
    let scalar = |key: &str| match source.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        _ => fail(format!("schema.json source has no `{key}`")),
    };
    Ok((scalar("herdr_version")?, scalar("protocol")?))
}

fn capture(command: &mut Command) -> Result<String> {
    let output = command
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cannot run {:?}: {e}", command.get_program()))?;
    if !output.status.success() {
        return fail(format!("{:?} failed", command.get_program()));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// A scratch directory removed on drop.
struct WorkDir(PathBuf);

impl WorkDir {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("or2-xtask-herdr-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> Value {
        let schema = |extra: Value| {
            let mut base = json!({
                "$defs": {
                    "Status": {"type": "string", "enum": ["idle", "busy"]},
                    "Name": {"type": "string", "pattern": "^a", "maxProperties": 3},
                },
                "properties": {"s": {"$ref": "#/schemas/NAME/$defs/Status"}},
            });
            base.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            base
        };
        let mut request = schema(json!({
            "required": ["id"],
            "type": "object",
            "oneOf": [{"properties": {"method": {"enum": ["ping"]}}}],
        }));
        request["properties"]["s"]["$ref"] = json!("#/schemas/request/$defs/Status");
        let mut success = schema(json!({}));
        success["properties"]["s"]["$ref"] = json!("#/schemas/success_response/$defs/Status");
        let mut error = schema(json!({}));
        error["properties"]["s"]["$ref"] = json!("#/schemas/error_response/$defs/Status");
        json!({
            "protocol": 3,
            "schema_version": "x",
            "schemas": {
                "request": request,
                "success_response": success,
                "error_response": error,
                "event": {"type": "object"},
            },
        })
    }

    #[test]
    fn refs_become_local_and_foreign_refs_fail() {
        let normalized = normalize(&bundle(), "9.9.9").unwrap();
        assert_eq!(
            normalized["families"]["success_response"]["properties"]["s"]["$ref"],
            json!("#/$defs/Status")
        );
        let mut broken = bundle();
        broken["schemas"]["error_response"]["properties"]["s"]["$ref"] = json!("#/other/X");
        assert!(
            normalize(&broken, "9.9.9")
                .unwrap_err()
                .contains("unexpected $ref")
        );
    }

    #[test]
    fn validation_keywords_are_dropped_everywhere() {
        let normalized = normalize(&bundle(), "1").unwrap();
        let name = &normalized["families"]["request"]["$defs"]["Name"];
        assert_eq!(name, &json!({"type": "string"}));
    }

    #[test]
    fn only_received_families_get_open_enums() {
        let normalized = normalize(&bundle(), "1").unwrap();
        // Sent by the client: stays closed.
        assert_eq!(
            normalized["families"]["request"]["$defs"]["Status"],
            json!({"type": "string", "enum": ["idle", "busy"]})
        );
        // Sent by herdr: the known values plus any other string.
        assert_eq!(
            normalized["families"]["success_response"]["$defs"]["Status"],
            json!({"oneOf": [{"type": "string", "enum": ["idle", "busy"]}, {"type": "string"}]})
        );
    }

    #[test]
    fn request_keeps_only_its_one_of_under_the_body_title() {
        let normalized = normalize(&bundle(), "1").unwrap();
        let request = normalized["families"]["request"].as_object().unwrap();
        assert_eq!(request["title"], json!("RequestBody"));
        assert!(request.contains_key("oneOf"));
        for key in ["properties", "required", "type"] {
            assert!(!request.contains_key(key), "{key} survived");
        }
    }

    #[test]
    fn source_block_and_family_set_are_fixed() {
        let normalized = normalize(&bundle(), "9.9.9").unwrap();
        assert_eq!(
            normalized["source"],
            json!({
                "herdr_version": "9.9.9",
                "protocol": 3,
                "schema_version": "x",
                "command": "herdr api schema --json",
            })
        );
        let families: Vec<_> = normalized["families"].as_object().unwrap().keys().collect();
        assert_eq!(families, ["error_response", "request", "success_response"]);
        let mut missing = bundle();
        missing["schemas"]
            .as_object_mut()
            .unwrap()
            .remove("request");
        assert!(normalize(&missing, "1").unwrap_err().contains("request"));
    }

    #[test]
    fn schema_text_is_sorted_indented_and_newline_terminated() {
        let text = normalize_text(&bundle(), "1").unwrap();
        assert!(text.starts_with("{\n \"families\": {\n  \"error_response\": {"));
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn version_is_the_last_word_of_the_first_line() {
        assert_eq!(herdr_version("herdr 0.9.3\nmore\n"), Some("0.9.3"));
        assert_eq!(herdr_version(""), None);
    }
}
