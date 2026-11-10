//! Small helpers that reproduce JavaScript semantics the ported code relies on.

use serde_json::Value;

/// The character class JS uses for `\s` and `String.prototype.trim`, for use inside a regex.
pub const WS_CLASS: &str =
    r"[\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}]";

pub fn is_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_whitespace)
}

/// `/^\d+$/.test(s)`, where `\d` is ASCII only in JS.
pub fn is_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `html.slice(start, end)` where `end` is the result of an `indexOf` that may have failed.
/// A failed `indexOf` gives -1, which `slice` reads as "one before the end".
pub fn slice_to_index_of(s: &str, start: usize, end: Option<usize>) -> &str {
    let end = match end {
        Some(i) => i,
        None => s.char_indices().next_back().map_or(0, |(i, _)| i),
    };
    if end <= start {
        ""
    } else {
        &s[start..end]
    }
}

/// `Number.prototype.toString()` for the values JSON can carry.
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if n == 0.0 {
        return "0".to_string();
    }
    let abs = n.abs();
    if (1e-6..1e21).contains(&abs) {
        return format!("{n}");
    }
    let exp = format!("{n:e}");
    match exp.split_once('e') {
        Some((mantissa, power)) if !power.starts_with('-') => format!("{mantissa}e+{power}"),
        _ => exp,
    }
}

/// `String(value)`.
pub fn to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => s.clone(),
        Value::Array(items) => join(items, ","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

/// `Array.prototype.join(separator)`, which renders null and undefined as empty.
pub fn join(items: &[Value], separator: &str) -> String {
    items
        .iter()
        .map(|item| match item {
            Value::Null => String::new(),
            other => to_string(other),
        })
        .collect::<Vec<_>>()
        .join(separator)
}

/// `Array.prototype.includes` equality. Two parsed arrays or objects are never the same object.
pub fn same_value_zero(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::String(x), Value::String(y)) => x == y,
        _ => false,
    }
}

/// `Number(value)`, with `None` standing for `undefined`.
pub fn to_number(value: Option<&Value>) -> f64 {
    match value {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => string_to_number(s),
        Some(Value::Array(items)) => match items.as_slice() {
            [] => 0.0,
            [only] => string_to_number(&join(std::slice::from_ref(only), ",")),
            _ => f64::NAN,
        },
        Some(Value::Object(_)) => f64::NAN,
    }
}

fn string_to_number(s: &str) -> f64 {
    let text = trim(s);
    if text.is_empty() {
        return 0.0;
    }
    match text {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let radix = match text.get(..2) {
        Some("0x" | "0X") => Some(16),
        Some("0o" | "0O") => Some(8),
        Some("0b" | "0B") => Some(2),
        _ => None,
    };
    if let Some(radix) = radix {
        let digits = &text[2..];
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return f64::NAN;
        }
        return digits.chars().fold(0.0, |acc, c| acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0)));
    }
    // Rust also accepts "inf" and "nan" spellings that JS rejects.
    if text.bytes().any(|b| b.is_ascii_alphabetic() && b != b'e' && b != b'E') {
        return f64::NAN;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

/// `Number.prototype.toFixed(2)`. JS rounds exact ties up, Rust's formatter rounds them to even.
pub fn to_fixed_2(x: f64) -> String {
    if !x.is_finite() || x.abs() >= 1e21 {
        return number_to_string(x);
    }
    if x == 0.0 {
        return "0.00".to_string();
    }
    let scaled = x.abs() * 1000.0;
    let is_tie = scaled.fract() == 0.0 && scaled % 10.0 == 5.0 && (x.abs() * 8.0).fract() == 0.0;
    if is_tie {
        let nudged = x.abs() + 0.001;
        let sign = if x < 0.0 { "-" } else { "" };
        return format!("{sign}{nudged:.2}");
    }
    format!("{x:.2}")
}

/// JS truthiness for a JSON value.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn to_fixed_2_rounds_ties_up_like_js() {
        assert_eq!(to_fixed_2(0.125), "0.13");
        assert_eq!(to_fixed_2(0.375), "0.38");
        assert_eq!(to_fixed_2(-0.0), "0.00");
        assert_eq!(to_fixed_2(0.95), "0.95");
        assert_eq!(to_fixed_2(0.8), "0.80");
        assert_eq!(to_fixed_2(0.4), "0.40");
    }

    #[test]
    fn join_matches_array_join() {
        assert_eq!(join(&[json!("A"), json!(null), json!(3), json!(["x", null])], ", "), "A, , 3, x,");
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(1e-7), "1e-7");
        assert_eq!(number_to_string(0.5), "0.5");
    }
}
