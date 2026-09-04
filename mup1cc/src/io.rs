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
            let rendered = render(value, format)?;
            println!("{rendered}");
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
