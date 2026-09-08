// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! A from-scratch RFC 7950 YANG tokenizer + statement-tree parser.
//!
//! The Ruby reference (`support/yang-enc/yang-schema.rb`) has no YANG-text
//! parser at all -- it shells out to `pyang --format yin` and walks the
//! resulting XML tree in `yang-utils.rb`. There is nothing Ruby to port
//! here; this is a fresh implementation against RFC 7950 directly,
//! structurally modeled on `sw-velocitydrive-devclient`'s
//! `src/yang/yang-parser.ts` (itself a from-scratch RFC 7950 parser, since
//! it faces the same absence of a Ruby original).
//!
//! Produces a generic statement tree (keyword/argument/substatements);
//! semantic interpretation (groupings, augments, types, ...) happens in
//! [`crate::schema`].

use std::fmt;

/// One parsed YANG statement: `keyword [argument] (";" | "{" *stmt "}")`.
/// `prefix` is `Some` only for an extension statement (`prefix:keyword`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stmt {
    pub prefix: Option<String>,
    pub keyword: String,
    pub arg: Option<String>,
    pub subs: Vec<Stmt>,
}

impl Stmt {
    /// The first immediate substatement with this keyword, if any.
    pub fn sub(&self, keyword: &str) -> Option<&Stmt> {
        self.subs.iter().find(|s| s.prefix.is_none() && s.keyword == keyword)
    }

    /// All immediate substatements with this keyword.
    pub fn subs_of<'a>(&'a self, keyword: &'a str) -> impl Iterator<Item = &'a Stmt> {
        self.subs.iter().filter(move |s| s.prefix.is_none() && s.keyword == keyword)
    }

    /// The argument, or an empty string if the statement takes none.
    pub fn arg_str(&self) -> &str {
        self.arg.as_deref().unwrap_or("")
    }
}

#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub col: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)
    }
}

impl std::error::Error for ParseError {}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl Lexer {
    /// Normalizes CRLF/lone-CR to LF up front, so every downstream
    /// consumer (bare tokens, quoted-string content, comments) sees
    /// LF-only input -- a source file's own line-ending convention
    /// otherwise leaks into quoted-string *content* verbatim (e.g. a
    /// `\r` embedded in the middle of a `description`'s text), which a
    /// real `pyang`-based toolchain doesn't produce.
    fn new(src: &str) -> Self {
        let normalized = src.replace("\r\n", "\n").replace('\r', "\n");
        Self { chars: normalized.chars().collect(), pos: 0, line: 1, col: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 0;
        } else if c == '\t' {
            self.col = (self.col / 8 + 1) * 8;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn err(&self, message: impl Into<String>) -> ParseError {
        ParseError { message: message.into(), line: self.line, col: self.col }
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('/') if self.peek_at(1) == Some('/') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    self.bump();
                    self.bump();
                    loop {
                        match self.peek() {
                            None => break,
                            Some('*') if self.peek_at(1) == Some('/') => {
                                self.bump();
                                self.bump();
                                break;
                            }
                            _ => {
                                self.bump();
                            }
                        }
                    }
                }
                _ => break,
            }
        }
    }

    /// A bare run of keyword/identifier characters: anything but
    /// whitespace, braces, semicolon, or quote marks.
    fn read_bare(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, '{' | '}' | ';' | '"' | '\'') {
                break;
            }
            s.push(c);
            self.bump();
        }
        s
    }

    fn read_double_quoted(&mut self) -> Result<String, ParseError> {
        let quote_col = self.col;
        self.bump(); // opening quote
        let mut raw = String::new();
        loop {
            match self.bump() {
                None => return Err(self.err("unterminated double-quoted string")),
                Some('"') => break,
                Some('\\') => match self.bump() {
                    Some('n') => raw.push('\n'),
                    Some('t') => raw.push('\t'),
                    Some('"') => raw.push('"'),
                    Some('\\') => raw.push('\\'),
                    Some(other) => {
                        raw.push('\\');
                        raw.push(other);
                    }
                    None => return Err(self.err("unterminated escape in double-quoted string")),
                },
                Some(c) => raw.push(c),
            }
        }
        Ok(strip_indentation(&raw, quote_col))
    }

    fn read_single_quoted(&mut self) -> Result<String, ParseError> {
        self.bump(); // opening quote
        let mut raw = String::new();
        loop {
            match self.bump() {
                None => return Err(self.err("unterminated single-quoted string")),
                Some('\'') => break,
                Some(c) => raw.push(c),
            }
        }
        Ok(raw)
    }

    fn read_string_component(&mut self) -> Result<String, ParseError> {
        match self.peek() {
            Some('"') => self.read_double_quoted(),
            Some('\'') => self.read_single_quoted(),
            Some(_) => Ok(self.read_bare()),
            None => Err(self.err("expected a string, found end of input")),
        }
    }

    /// A full statement argument: one or more string components joined by
    /// `+` (RFC 7950 string concatenation, always across whitespace).
    fn read_argument(&mut self) -> Result<String, ParseError> {
        let mut result = self.read_string_component()?;
        loop {
            self.skip_trivia();
            if self.peek() == Some('+') {
                self.bump();
                self.skip_trivia();
                result.push_str(&self.read_string_component()?);
            } else {
                break;
            }
        }
        Ok(result)
    }

    fn read_keyword(&mut self) -> Result<(Option<String>, String), ParseError> {
        let word = self.read_bare();
        if word.is_empty() {
            return Err(self.err(format!("expected a keyword, found {:?}", self.peek())));
        }
        match word.split_once(':') {
            Some((prefix, local)) if !local.is_empty() => Ok((Some(prefix.to_string()), local.to_string())),
            _ => Ok((None, word)),
        }
    }

    fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
        self.skip_trivia();
        let (prefix, keyword) = self.read_keyword()?;
        self.skip_trivia();

        let arg = match self.peek() {
            Some('{') | Some(';') => None,
            Some(_) => {
                let a = self.read_argument()?;
                self.skip_trivia();
                Some(a)
            }
            None => return Err(self.err("unexpected end of input after keyword")),
        };

        let mut subs = Vec::new();
        match self.bump() {
            Some(';') => {}
            Some('{') => loop {
                self.skip_trivia();
                if self.peek() == Some('}') {
                    self.bump();
                    break;
                }
                if self.at_end() {
                    return Err(self.err("unterminated block, missing '}'"));
                }
                subs.push(self.parse_statement()?);
            },
            other => return Err(self.err(format!("expected ';' or '{{', found {other:?}"))),
        }

        Ok(Stmt { prefix, keyword, arg, subs })
    }
}

/// Strip the leading whitespace introduced by RFC 7950 6.1.3 from every
/// continuation line of a double-quoted string: up to `quote_col` columns
/// of indentation (tabs advance to the next multiple of 8) are removed
/// from each line after the first. Free-text content (description,
/// contact, ...) is never used by the SID-CBOR wire encoding, so exact
/// RFC compliance here isn't wire-relevant -- this is a reasonable,
/// not pedantically exact, implementation.
fn strip_indentation(raw: &str, quote_col: usize) -> String {
    if !raw.contains('\n') {
        return raw.to_string();
    }
    let mut lines = raw.split('\n');
    let mut out = String::from(lines.next().unwrap());
    for line in lines {
        out.push('\n');
        // RFC 7950 6.1.3: strip up to and including the opening quote's
        // *own* column -- `quote_col` (0-indexed, the quote character's
        // position) is therefore one column short of the count to
        // strip.
        out.push_str(strip_leading_ws(line, quote_col + 1));
    }
    out
}

fn strip_leading_ws(line: &str, max_col: usize) -> &str {
    let mut col = 0usize;
    let mut idx = 0usize;
    for ch in line.chars() {
        if col >= max_col {
            break;
        }
        match ch {
            ' ' => {
                col += 1;
                idx += ch.len_utf8();
            }
            '\t' => {
                col = (col / 8 + 1) * 8;
                idx += ch.len_utf8();
            }
            _ => break,
        }
    }
    &line[idx..]
}

/// Parse one `.yang` file's source text into its top-level `module` (or
/// `submodule`) statement.
pub fn parse_module(src: &str) -> Result<Stmt, ParseError> {
    let mut lexer = Lexer::new(src);
    let stmt = lexer.parse_statement()?;
    lexer.skip_trivia();
    if !lexer.at_end() {
        return Err(lexer.err("unexpected content after the top-level statement"));
    }
    if stmt.keyword != "module" && stmt.keyword != "submodule" {
        return Err(ParseError {
            message: format!("expected a 'module' or 'submodule' statement, found '{}'", stmt.keyword),
            line: 1,
            col: 0,
        });
    }
    Ok(stmt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_module() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix "f";

              container bar {
                leaf baz {
                  type string;
                }
              }
            }
        "#;
        let m = parse_module(src).unwrap();
        assert_eq!(m.keyword, "module");
        assert_eq!(m.arg_str(), "foo");
        assert_eq!(m.sub("namespace").unwrap().arg_str(), "urn:foo");
        let bar = m.sub("container").unwrap();
        assert_eq!(bar.arg_str(), "bar");
        let baz = bar.sub("leaf").unwrap();
        assert_eq!(baz.arg_str(), "baz");
        assert_eq!(baz.sub("type").unwrap().arg_str(), "string");
    }

    #[test]
    fn handles_string_concatenation_and_comments() {
        let src = r#"
            // a line comment
            module foo {
              namespace "urn:"
                + "foo"; /* a block comment */
              prefix f;
            }
        "#;
        let m = parse_module(src).unwrap();
        assert_eq!(m.sub("namespace").unwrap().arg_str(), "urn:foo");
    }

    #[test]
    fn handles_single_quoted_strings_without_escapes() {
        let src = r#"module foo { pattern '[0-9\.]*'; }"#;
        let m = parse_module(src).unwrap();
        assert_eq!(m.sub("pattern").unwrap().arg_str(), "[0-9\\.]*");
    }

    #[test]
    fn parses_extension_statements_with_a_prefix() {
        let src = r#"
            module foo {
              rc:yang-data coreconf-error {
                container error;
              }
            }
        "#;
        let m = parse_module(src).unwrap();
        let ext = &m.subs[0];
        assert_eq!(ext.prefix.as_deref(), Some("rc"));
        assert_eq!(ext.keyword, "yang-data");
        assert_eq!(ext.arg_str(), "coreconf-error");
    }

    #[test]
    fn rejects_non_module_top_level_statement() {
        let src = "leaf foo { type string; }";
        assert!(parse_module(src).is_err());
    }
}
