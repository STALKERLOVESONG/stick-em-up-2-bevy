//! A reader for the YAML subset that Unity serializes (`%YAML 1.1` / `--- !u!<class> &<id>`).
//!
//! Unity's writer only produces block mappings, block sequences (sometimes at the same indent as
//! their key), flow mappings/sequences (`{x: 1, y: 2}`), and plain scalars, so a purpose-built
//! parser is both smaller and far faster than a general YAML library on the 20 MB scenes.

use std::collections::HashMap;

/// A parsed YAML node.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Null,
    Scalar(String),
    Map(Vec<(String, Value)>),
    Seq(Vec<Value>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Scalar(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        parse_unity_float(self.as_str()?)
    }

    pub fn as_i64(&self) -> Option<i64> {
        let s = self.as_str()?;
        s.parse::<i64>().ok().or_else(|| s.parse::<f64>().ok().map(|f| f as i64))
    }

    pub fn as_bool(&self) -> Option<bool> {
        self.as_i64().map(|v| v != 0)
    }

    pub fn as_seq(&self) -> &[Value] {
        match self {
            Value::Seq(items) => items,
            _ => &[],
        }
    }

    pub fn as_map(&self) -> &[(String, Value)] {
        match self {
            Value::Map(entries) => entries,
            _ => &[],
        }
    }

    pub fn str_of(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    pub fn f32_of(&self, key: &str) -> Option<f32> {
        self.get(key)?.as_f32()
    }

    pub fn i64_of(&self, key: &str) -> Option<i64> {
        self.get(key)?.as_i64()
    }

    pub fn bool_of(&self, key: &str) -> Option<bool> {
        self.get(key)?.as_bool()
    }

    /// `{x: 1, y: 2}` → `(1, 2)`.
    pub fn as_vec2(&self) -> Option<(f32, f32)> {
        Some((self.f32_of("x")?, self.f32_of("y")?))
    }

    /// `{x: 1, y: 2, z: 3}` → `(1, 2, 3)`.
    pub fn as_vec3(&self) -> Option<(f32, f32, f32)> {
        Some((self.f32_of("x")?, self.f32_of("y")?, self.f32_of("z")?))
    }

    /// `{x: .., y: .., z: .., w: ..}`.
    pub fn as_quat(&self) -> Option<(f32, f32, f32, f32)> {
        Some((self.f32_of("x")?, self.f32_of("y")?, self.f32_of("z")?, self.f32_of("w")?))
    }

    /// File id of a Unity object reference (`{fileID: 12}`); 0 means null.
    pub fn file_id(&self) -> Option<i64> {
        self.get("fileID")?.as_i64()
    }

    /// GUID of an external object reference.
    pub fn guid(&self) -> Option<&str> {
        self.str_of("guid")
    }
}

/// Unity writes floats as `1E-05`, `Infinity`, `-Infinity`, `NaN` as well as plain decimals.
pub fn parse_unity_float(s: &str) -> Option<f32> {
    match s {
        "Infinity" => Some(f32::INFINITY),
        "-Infinity" => Some(f32::NEG_INFINITY),
        "NaN" => Some(f32::NAN),
        _ => s.parse::<f32>().ok(),
    }
}

#[derive(Clone, Copy)]
struct Line<'a> {
    indent: usize,
    text: &'a str,
}

struct Parser<'a> {
    lines: Vec<Line<'a>>,
    i: usize,
}

fn is_dash(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

/// Split `key: rest` / `key:`. Keys never contain `: ` and never start with a flow bracket.
fn split_key(text: &str) -> Option<(&str, &str)> {
    if text.starts_with('{') || text.starts_with('[') || text.starts_with('"') {
        return None;
    }
    if let Some(pos) = text.find(": ") {
        Some((&text[..pos], text[pos + 2..].trim_start()))
    } else {
        text.strip_suffix(':').map(|key| (key, ""))
    }
}

impl<'a> Parser<'a> {
    fn node(&mut self) -> Value {
        let Some(&line) = self.lines.get(self.i) else {
            return Value::Null;
        };
        if is_dash(line.text) {
            self.seq(line.indent)
        } else if split_key(line.text).is_some() {
            self.map(line.indent)
        } else {
            self.i += 1;
            parse_inline(line.text)
        }
    }

    fn seq(&mut self, indent: usize) -> Value {
        let mut items = Vec::new();
        while let Some(&line) = self.lines.get(self.i) {
            if line.indent != indent || !is_dash(line.text) {
                break;
            }
            let rest = line.text[1..].trim_start();
            if rest.is_empty() {
                self.i += 1;
                match self.lines.get(self.i) {
                    Some(next) if next.indent > indent => items.push(self.node()),
                    _ => items.push(Value::Null),
                }
            } else {
                // Re-seat the line as if the item content started on its own line.
                let consumed = line.text.len() - rest.len();
                self.lines[self.i] = Line { indent: indent + consumed, text: rest };
                items.push(self.node());
            }
        }
        Value::Seq(items)
    }

    fn map(&mut self, indent: usize) -> Value {
        let mut entries = Vec::new();
        while let Some(&line) = self.lines.get(self.i) {
            if line.indent > indent {
                // Stray continuation line (multi-line scalar); ignore it.
                self.i += 1;
                continue;
            }
            if line.indent != indent || is_dash(line.text) {
                break;
            }
            let Some((key, rest)) = split_key(line.text) else {
                self.i += 1;
                continue;
            };
            self.i += 1;
            let value = if rest.is_empty() {
                match self.lines.get(self.i) {
                    Some(next) if next.indent > indent => self.node(),
                    Some(next) if next.indent == indent && is_dash(next.text) => self.seq(indent),
                    _ => Value::Null,
                }
            } else {
                parse_inline(rest)
            };
            entries.push((key.to_owned(), value));
        }
        Value::Map(entries)
    }
}

/// Parse a block-structured YAML fragment (no document headers).
pub fn parse_block(text: &str) -> Value {
    let lines = text
        .lines()
        .filter_map(|raw| {
            let trimmed = raw.trim_start_matches(' ');
            let content = trimmed.trim_end();
            if content.is_empty() || content.starts_with('#') {
                None
            } else {
                Some(Line { indent: raw.len() - trimmed.len(), text: content })
            }
        })
        .collect();
    Parser { lines, i: 0 }.node()
}

fn parse_inline(text: &str) -> Value {
    let text = text.trim();
    if text.starts_with('{') || text.starts_with('[') {
        let mut chars = FlowCursor { bytes: text.as_bytes(), pos: 0, src: text };
        chars.value()
    } else {
        Value::Scalar(unquote(text))
    }
}

/// A plain, `'single'` or `"double"` quoted scalar's value (YAML escapes in double quotes, e.g.
/// tk2d texts' `"Unused\nControllers"`).
fn unquote(text: &str) -> String {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'' {
        return text[1..text.len() - 1].replace("''", "'");
    }
    if !(bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"') {
        return text.to_owned();
    }
    let inner = &text[1..text.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    let hex = |chars: &mut std::str::Chars, n: usize| {
        let digits: String = chars.by_ref().take(n).collect();
        u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32)
    };
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let decoded = match chars.next() {
            Some('0') => Some('\0'),
            Some('a') => Some('\u{7}'),
            Some('b') => Some('\u{8}'),
            Some('t') | Some('\t') => Some('\t'),
            Some('n') => Some('\n'),
            Some('v') => Some('\u{b}'),
            Some('f') => Some('\u{c}'),
            Some('r') => Some('\r'),
            Some('e') => Some('\u{1b}'),
            Some(' ') => Some(' '),
            Some('"') => Some('"'),
            Some('/') => Some('/'),
            Some('\\') => Some('\\'),
            Some('N') => Some('\u{85}'),
            Some('_') => Some('\u{a0}'),
            Some('L') => Some('\u{2028}'),
            Some('P') => Some('\u{2029}'),
            Some('x') => hex(&mut chars, 2),
            Some('u') => hex(&mut chars, 4),
            Some('U') => hex(&mut chars, 8),
            Some(other) => {
                out.push('\\');
                Some(other)
            }
            None => Some('\\'),
        };
        out.extend(decoded);
    }
    out
}

struct FlowCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    src: &'a str,
}

impl<'a> FlowCursor<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos] == b' ' {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Value {
        self.skip_ws();
        match self.bytes.get(self.pos) {
            Some(b'{') => {
                self.pos += 1;
                let mut entries = Vec::new();
                loop {
                    self.skip_ws();
                    match self.bytes.get(self.pos) {
                        None => break,
                        Some(b'}') => {
                            self.pos += 1;
                            break;
                        }
                        Some(b',') => self.pos += 1,
                        _ => {
                            let key = self.scalar_until(&[b':']).to_owned();
                            if self.bytes.get(self.pos) == Some(&b':') {
                                self.pos += 1;
                            }
                            let value = self.value();
                            entries.push((key, value));
                        }
                    }
                }
                Value::Map(entries)
            }
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_ws();
                    match self.bytes.get(self.pos) {
                        None => break,
                        Some(b']') => {
                            self.pos += 1;
                            break;
                        }
                        Some(b',') => self.pos += 1,
                        _ => items.push(self.value()),
                    }
                }
                Value::Seq(items)
            }
            _ => Value::Scalar(unquote(self.scalar_until(&[b',', b'}', b']']))),
        }
    }

    fn scalar_until(&mut self, stops: &[u8]) -> &'a str {
        let start = self.pos;
        while self.pos < self.bytes.len() && !stops.contains(&self.bytes[self.pos]) {
            self.pos += 1;
        }
        self.src[start..self.pos].trim()
    }
}

/// One `--- !u!<class> &<fileID>` document.
#[derive(Clone, Debug)]
pub struct Document {
    pub class_id: u32,
    pub file_id: i64,
    /// The serialized type name, e.g. `GameObject`.
    pub type_name: String,
    /// The mapping under the type name.
    pub body: Value,
}

/// Parse a multi-document Unity YAML file.
pub fn parse_documents(text: &str) -> Vec<Document> {
    let mut docs = Vec::new();
    let mut header: Option<(u32, i64)> = None;
    let mut start = 0usize;
    let mut pos = 0usize;
    let flush = |header: Option<(u32, i64)>, body: &str, docs: &mut Vec<Document>| {
        if let Some((class_id, file_id)) = header {
            if let Value::Map(mut entries) = parse_block(body) {
                if let Some((type_name, body)) = entries.pop() {
                    docs.push(Document { class_id, file_id, type_name, body });
                }
            }
        }
    };
    for line in text.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("--- !u!") {
            flush(header, &text[start..pos], &mut docs);
            let mut parts = rest.trim().split_whitespace();
            let class_id = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let file_id = parts.next().and_then(|s| s.strip_prefix('&')).and_then(|s| s.parse().ok()).unwrap_or(0);
            header = Some((class_id, file_id));
            start = pos + line.len();
        }
        pos += line.len();
    }
    flush(header, &text[start..], &mut docs);
    docs
}

/// Index documents by file id.
pub fn index_by_file_id(docs: Vec<Document>) -> HashMap<i64, Document> {
    docs.into_iter().map(|d| (d.file_id, d)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_scalars_are_unescaped() {
        let v = parse_block("text: \"Unused\\nControllers\"\nname: 'it''s'\nplain: a\\nb\n");
        assert_eq!(v.str_of("text"), Some("Unused\nControllers"));
        assert_eq!(v.str_of("name"), Some("it's"));
        assert_eq!(v.str_of("plain"), Some("a\\nb"));
        assert_eq!(unquote("\"\\u00e9\\x41\\\"\""), "\u{e9}A\"");
    }

    #[test]
    fn parses_nested_sequences_at_key_indent_and_flow_maps() {
        let text = "\
PolygonCollider2D:
  m_Enabled: 1
  m_Offset: {x: 0, y: 0}
  m_Points:
    m_Paths:
    - - {x: 1, y: -2.5}
      - {x: 3, y: 4}
    - - {x: 5, y: 6}
  near clip plane: 0.3
";
        let v = parse_block(text);
        let body = v.get("PolygonCollider2D").unwrap();
        assert_eq!(body.i64_of("m_Enabled"), Some(1));
        assert_eq!(body.f32_of("near clip plane"), Some(0.3));
        let paths = body.get("m_Points").unwrap().get("m_Paths").unwrap().as_seq();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].as_seq().len(), 2);
        assert_eq!(paths[0].as_seq()[0].as_vec2(), Some((1.0, -2.5)));
        assert_eq!(paths[1].as_seq()[0].as_vec2(), Some((5.0, 6.0)));
    }

    #[test]
    fn parses_component_lists_with_numeric_keys() {
        let text = "\
GameObject:
  m_Component:
  - 4: {fileID: 479954}
  - 114: {fileID: 11409266}
  m_Layer: 8
  m_Name: Player
";
        let v = parse_block(text);
        let go = v.get("GameObject").unwrap();
        let comps = go.get("m_Component").unwrap().as_seq();
        assert_eq!(comps.len(), 2);
        assert_eq!(comps[1].get("114").unwrap().file_id(), Some(11409266));
        assert_eq!(go.str_of("m_Name"), Some("Player"));
    }

    #[test]
    fn parses_map_items_in_sequences() {
        let text = "\
AnimationCurve:
  m_Curve:
  - time: 0
    value: 1E-05
    inSlope: -Infinity
    outSlope: 0
  - time: 0.5
    value: 2
    inSlope: 0
    outSlope: 0
  m_PreInfinity: 2
";
        let v = parse_block(text);
        let keys = v.get("AnimationCurve").unwrap().get("m_Curve").unwrap().as_seq();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].f32_of("value"), Some(1e-5));
        assert_eq!(keys[0].f32_of("inSlope"), Some(f32::NEG_INFINITY));
        assert_eq!(keys[1].f32_of("time"), Some(0.5));
    }

    #[test]
    fn splits_documents() {
        let text = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!1 &10
GameObject:
  m_Name: A
--- !u!4 &11
Transform:
  m_LocalPosition: {x: 1, y: 2, z: 3}
";
        let docs = parse_documents(text);
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].file_id, 10);
        assert_eq!(docs[0].type_name, "GameObject");
        assert_eq!(docs[1].body.get("m_LocalPosition").unwrap().as_vec3(), Some((1.0, 2.0, 3.0)));
    }

    #[test]
    fn empty_flow_sequences_and_null_values() {
        let v = parse_block("A:\n  m_Children: []\n  m_Script:\n  m_X: {fileID: 0}\n");
        let a = v.get("A").unwrap();
        assert!(a.get("m_Children").unwrap().as_seq().is_empty());
        assert_eq!(a.get("m_Script"), Some(&Value::Null));
        assert_eq!(a.get("m_X").unwrap().file_id(), Some(0));
    }
}
