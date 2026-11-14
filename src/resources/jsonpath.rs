//! A small JSONPath and helpers for reading cell values out of objects.

use serde_json::Value;

// ---------------------------------------------------------------------------
// A small JSONPath: `.a.b`, `[0]`, `[*]`, `['k']`, `[?(@.f=="v")]`.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Seg {
    Field(String),
    Index(usize),
    All,
    /// `[?(@.path == lit)]` / `!=`
    Filter {
        path: Vec<String>,
        negate: bool,
        lit: Value,
    },
    /// Something we cannot evaluate: matches nothing.
    Unsupported,
}

pub(super) fn parse_path(p: &str) -> Vec<Seg> {
    let p = p.trim();
    let p = p.strip_prefix('$').unwrap_or(p);
    let chars: Vec<char> = p.chars().collect();
    let mut segs = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '[' => {
                // Find the matching `]`, skipping quoted text.
                let start = i + 1;
                let mut j = start;
                let mut quote: Option<char> = None;
                while j < chars.len() {
                    match (quote, chars[j]) {
                        (None, ']') => break,
                        (None, c @ ('"' | '\'')) => quote = Some(c),
                        (Some(q), c) if c == q => quote = None,
                        _ => {}
                    }
                    j += 1;
                }
                let inner: String = chars[start..j.min(chars.len())].iter().collect();
                segs.push(parse_bracket(inner.trim()));
                i = j + 1;
            }
            c => {
                // `.name` or a bare leading name; `\.` escapes a dot.
                if c == '.' {
                    i += 1;
                }
                let mut name = String::new();
                while i < chars.len() && chars[i] != '.' && chars[i] != '[' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    name.push(chars[i]);
                    i += 1;
                }
                if !name.is_empty() {
                    segs.push(Seg::Field(name));
                }
            }
        }
    }
    segs
}

fn unquote(s: &str) -> Option<&str> {
    let q = s.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    (s.len() >= 2 && s.ends_with(q)).then(|| &s[1..s.len() - 1])
}

fn parse_bracket(inner: &str) -> Seg {
    if inner == "*" {
        return Seg::All;
    }
    if let Ok(n) = inner.parse::<usize>() {
        return Seg::Index(n);
    }
    if let Some(key) = unquote(inner) {
        return Seg::Field(key.to_string());
    }
    let Some(expr) = inner
        .strip_prefix("?(")
        .and_then(|e| e.strip_suffix(')'))
        .map(str::trim)
    else {
        return Seg::Unsupported;
    };
    let (negate, at) = match (expr.find("=="), expr.find("!=")) {
        (Some(i), _) => (false, i),
        (None, Some(i)) => (true, i),
        _ => return Seg::Unsupported,
    };
    let Some(left) = expr[..at].trim().strip_prefix('@') else {
        return Seg::Unsupported;
    };
    let right = expr[at + 2..].trim();
    let lit = match unquote(right) {
        Some(s) => Value::String(s.to_string()),
        None => match serde_json::from_str(right) {
            Ok(v) => v,
            Err(_) => return Seg::Unsupported,
        },
    };
    Seg::Filter {
        path: left
            .split('.')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        negate,
        lit,
    }
}

fn eval<'a>(root: &'a Value, segs: &[Seg]) -> Vec<&'a Value> {
    let mut cur = vec![root];
    for seg in segs {
        let mut next = Vec::new();
        for v in cur {
            match seg {
                Seg::Field(f) => next.extend(v.get(f)),
                Seg::Index(i) => next.extend(v.get(*i)),
                Seg::All => match v {
                    Value::Array(a) => next.extend(a),
                    Value::Object(o) => next.extend(o.values()),
                    _ => {}
                },
                Seg::Filter { path, negate, lit } => {
                    if let Value::Array(a) = v {
                        for item in a {
                            let mut at = Some(item);
                            for p in path {
                                at = at.and_then(|x| x.get(p));
                            }
                            if (at == Some(lit)) != *negate {
                                next.push(item);
                            }
                        }
                    }
                }
                Seg::Unsupported => {}
            }
        }
        cur = next;
    }
    cur
}

/// Text of a JSON value as a table cell.
pub(super) fn cell_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(cell_text).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

pub(super) fn eval_str(v: &Value, segs: &[Seg]) -> String {
    eval(v, segs)
        .into_iter()
        .map(cell_text)
        .collect::<Vec<_>>()
        .join(",")
}

/// Evaluate `path` (e.g. `.status.phase`) and join the matches.
pub(super) fn path_str(v: &Value, path: &str) -> String {
    eval_str(v, &parse_path(path))
}

pub(super) fn path_i64(v: &Value, path: &str) -> i64 {
    eval(v, &parse_path(path))
        .first()
        .and_then(|x| x.as_i64())
        .unwrap_or(0)
}

/// `a/b` where both are numbers at the given paths (missing = 0).
pub(super) fn ratio(v: &Value, num: &str, den: &str) -> String {
    format!("{}/{}", path_i64(v, num), path_i64(v, den))
}

pub(super) fn count_at(v: &Value, path: &str) -> String {
    eval(v, &parse_path(path))
        .first()
        .and_then(|x| x.as_array().map(Vec::len))
        .unwrap_or(0)
        .to_string()
}

pub(super) fn count_keys(v: &Value, pointer: &str) -> usize {
    v.pointer(pointer)
        .and_then(Value::as_object)
        .map_or(0, |o| o.len())
}

pub(super) fn first_of(options: &[String]) -> String {
    options
        .iter()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_default()
}

pub(super) fn array_at<'a>(v: &'a Value, pointer: &str) -> impl Iterator<Item = &'a Value> {
    v.pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jsonpath_fields_indexes_and_filters() {
        let v = json!({
            "status": { "phase": "Running", "conditions": [
                {"type": "Ready", "status": "True"},
                {"type": "Other", "status": "False"}
            ], "ports": [80, 443] },
            "metadata": { "annotations": { "a.b/c": "x" } }
        });
        assert_eq!(path_str(&v, ".status.phase"), "Running");
        assert_eq!(path_str(&v, "$.status.phase"), "Running");
        assert_eq!(path_str(&v, "status.phase"), "Running");
        assert_eq!(path_str(&v, ".status.ports[1]"), "443");
        assert_eq!(path_str(&v, ".status.ports"), "80,443");
        assert_eq!(path_str(&v, ".status.ports[*]"), "80,443");
        assert_eq!(
            path_str(&v, ".status.conditions[?(@.type==\"Ready\")].status"),
            "True"
        );
        assert_eq!(
            path_str(&v, ".status.conditions[?(@.type!='Ready')].status"),
            "False"
        );
        assert_eq!(path_str(&v, ".metadata.annotations['a.b/c']"), "x");
        assert_eq!(path_str(&v, ".metadata.annotations.a\\.b/c"), "x");
        assert_eq!(path_str(&v, ".missing.deeper"), "");
        assert_eq!(path_str(&v, ".status.conditions[?(@.a==1 && @.b==2)]"), "");
    }
}
