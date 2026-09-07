//! Input/output format handling, matching `support/scripts/mup1cc`'s
//! `input_data_read`/`output_data_process` (mup1cc:155-277): YAML by
//! default, with a flag > file-extension > default priority chain.

use std::io::{Read, Write};

use serde_json::Value as Json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Yaml,
}

impl Format {
    fn from_flag(s: Option<&str>) -> Option<Format> {
        match s {
            Some("json") => Some(Format::Json),
            Some("yaml") => Some(Format::Yaml),
            _ => None,
        }
    }

    fn from_extension(path: &str) -> Option<Format> {
        if path.ends_with("json") {
            Some(Format::Json)
        } else if path.ends_with("yaml") {
            Some(Format::Yaml)
        } else {
            None
        }
    }
}

pub fn parse(text: &str, format: Format) -> Result<Json, String> {
    match format {
        Format::Json => serde_json::from_str(text).map_err(|e| format!("JSON parse error: {e}")),
        Format::Yaml => serde_yaml_ng::from_str(text).map_err(|e| format!("YAML parse error: {e}")),
    }
}

pub fn render(value: &Json, format: Format) -> Result<String, String> {
    match format {
        Format::Json => serde_json::to_string_pretty(value).map_err(|e| format!("JSON encode error: {e}")),
        Format::Yaml => serde_yaml_ng::to_string(value).map_err(|e| format!("YAML encode error: {e}")),
    }
}

/// Read the request body from `-i FILE` or STDIN, applying the same
/// format-priority chain as the Ruby reference.
pub fn read_input(input_file: Option<&str>, input_format_flag: Option<&str>) -> Result<(Json, Format), String> {
    let (text, format) = match input_file {
        Some(path) => {
            let format = Format::from_flag(input_format_flag).or_else(|| Format::from_extension(path)).unwrap_or(Format::Yaml);
            let text = std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?;
            (text, format)
        }
        None => {
            let format = Format::from_flag(input_format_flag).unwrap_or(Format::Yaml);
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text).map_err(|e| format!("reading stdin: {e}"))?;
            (text, format)
        }
    };
    let value = parse(&text, format)?;
    Ok((value, format))
}

/// True if this JSON value is "present and non-empty", matching Ruby's
/// `output_data and output_data.any?` (mup1cc:252): an empty array/object
/// (or null) means nothing to write.
pub fn is_nonempty(value: &Json) -> bool {
    match value {
        Json::Null => false,
        Json::Array(a) => !a.is_empty(),
        Json::Object(o) => !o.is_empty(),
        _ => true,
    }
}

/// Write the response body to `-o FILE` or STDOUT, applying the same
/// format-priority chain as the Ruby reference (`output_data_process`).
pub fn write_output(value: &Json, output_file: Option<&str>, output_format_flag: Option<&str>) -> Result<(), String> {
    if !is_nonempty(value) {
        return Ok(());
    }
    match output_file {
        None => {
            let format = Format::from_flag(output_format_flag).unwrap_or(Format::Yaml);
            // `render` (YAML in particular) already ends its string with
            // a trailing newline; avoid println! doubling it up into a
            // blank final line.
            let rendered = render(value, format)?;
            print!("{}", rendered.trim_end_matches('\n'));
            println!();
        }
        Some(path) => {
            let format = Format::from_flag(output_format_flag).or_else(|| Format::from_extension(path)).unwrap_or(Format::Yaml);
            let rendered = render(value, format)?;
            std::fs::write(path, rendered).map_err(|e| format!("writing {path}: {e}"))?;
        }
    }
    Ok(())
}

pub fn flush_stdout() {
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("mup1cc-io-test-{}-{name}", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn parses_json_text() {
        let v = parse(r#"{"a": 1}"#, Format::Json).unwrap();
        assert_eq!(v, serde_json::json!({"a": 1}));
    }

    #[test]
    fn parses_yaml_text() {
        let v = parse("a: 1\n", Format::Yaml).unwrap();
        assert_eq!(v, serde_json::json!({"a": 1}));
    }

    #[test]
    fn renders_json_and_yaml() {
        let v = serde_json::json!({"a": 1});
        assert_eq!(render(&v, Format::Json).unwrap(), "{\n  \"a\": 1\n}");
        assert_eq!(render(&v, Format::Yaml).unwrap(), "a: 1\n");
    }

    #[test]
    fn read_input_uses_explicit_format_flag_over_extension() {
        // A .yaml-named file whose content is actually JSON, forced to
        // parse as JSON via the flag -- flag must win over extension.
        let path = temp_file("explicit-flag.yaml", r#"{"a": 1}"#);
        let (v, fmt) = read_input(Some(path.to_str().unwrap()), Some("json")).unwrap();
        assert_eq!(fmt, Format::Json);
        assert_eq!(v, serde_json::json!({"a": 1}));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn read_input_infers_json_from_extension_when_no_flag_given() {
        let path = temp_file("inferred.json", r#"{"b": 2}"#);
        let (v, fmt) = read_input(Some(path.to_str().unwrap()), None).unwrap();
        assert_eq!(fmt, Format::Json);
        assert_eq!(v, serde_json::json!({"b": 2}));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn read_input_defaults_to_yaml_with_no_flag_or_recognized_extension() {
        let path = temp_file("no-hint.txt", "c: 3\n");
        let (v, fmt) = read_input(Some(path.to_str().unwrap()), None).unwrap();
        assert_eq!(fmt, Format::Yaml);
        assert_eq!(v, serde_json::json!({"c": 3}));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn write_output_infers_json_from_extension_when_no_flag_given() {
        let path = temp_file("out.json", "");
        write_output(&serde_json::json!({"d": 4}), Some(path.to_str().unwrap()), None).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(parse(&written, Format::Json).unwrap(), serde_json::json!({"d": 4}));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn write_output_explicit_format_flag_overrides_extension() {
        let path = temp_file("out.yaml", "");
        write_output(&serde_json::json!({"e": 5}), Some(path.to_str().unwrap()), Some("json")).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(parse(&written, Format::Json).unwrap(), serde_json::json!({"e": 5}));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn write_output_skips_empty_values() {
        let path = temp_file("should-stay-empty.json", "");
        write_output(&Json::Null, Some(path.to_str().unwrap()), None).unwrap();
        write_output(&serde_json::json!([]), Some(path.to_str().unwrap()), None).unwrap();
        write_output(&serde_json::json!({}), Some(path.to_str().unwrap()), None).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        std::fs::remove_file(path).unwrap();
    }
}
