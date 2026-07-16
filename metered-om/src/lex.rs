//! The shared label-block lexer behind both text parsers.
//!
//! [`text`](crate::text) (strict OpenMetrics) and [`prom_text`](crate::prom_text)
//! (lenient classic Prometheus) read the same `key="value"` label syntax and
//! previously each carried its own quote-aware scanner. The one lexer lives
//! here, with the dialect differences expressed as a [`Strictness`] option:
//! whitespace tolerance around `=` / `,`, and whether an unknown escape is an
//! error or passes through.

/// Which dialect the lexer accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Strictness {
    /// OpenMetrics text: no padding around `=` or `,`; escapes limited to
    /// `\\`, `\"`, and `\n`.
    OpenMetrics,
    /// Classic Prometheus text from lenient producers: whitespace tolerated
    /// around `=` and after `,`; an unknown escape passes the character
    /// through.
    Lenient,
}

/// A lexing failure; the callers attach their own line numbers.
#[derive(Debug)]
pub(crate) struct LexError {
    pub(crate) message: String,
}

fn err(message: impl Into<String>) -> LexError {
    LexError {
        message: message.into(),
    }
}

/// Splits a label-block body (the text between `{` and `}`) into
/// `(name, value)` pairs, unescaping the quoted values.
pub(crate) fn parse_label_pairs(
    body: &str,
    strictness: Strictness,
) -> Result<Vec<(String, String)>, LexError> {
    let lenient = strictness == Strictness::Lenient;
    let mut labels = Vec::new();
    let mut rest = if lenient { body.trim() } else { body };
    while !rest.is_empty() {
        let eq = rest.find('=').ok_or_else(|| err("label without `=`"))?;
        let raw_key = &rest[..eq];
        let key = if lenient { raw_key.trim() } else { raw_key };
        if key.is_empty() {
            return Err(err("empty label name"));
        }
        let mut after_eq = &rest[eq + 1..];
        if lenient {
            after_eq = after_eq.trim_start();
        }
        if !after_eq.starts_with('"') {
            return Err(err("label value must be quoted"));
        }
        let (value, consumed) = parse_quoted(after_eq, strictness)?;
        labels.push((key.to_owned(), value));
        rest = &after_eq[consumed..];
        if lenient {
            rest = rest.trim_start();
        }
        if rest.is_empty() {
            break;
        }
        rest = rest
            .strip_prefix(',')
            .ok_or_else(|| err("expected `,` between labels"))?;
        if lenient {
            // Classic Prometheus text permits a trailing comma before `}`.
            rest = rest.trim_start();
        } else if rest.is_empty() {
            return Err(err("trailing comma in label set"));
        }
    }
    Ok(labels)
}

/// Parses a leading quoted string, returning the unescaped value and the byte
/// length consumed (including both quotes).
fn parse_quoted(input: &str, strictness: Strictness) -> Result<(String, usize), LexError> {
    debug_assert!(input.starts_with('"'));
    let mut value = String::new();
    let mut chars = input.char_indices().skip(1);
    while let Some((index, ch)) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some((_, 'n')) => value.push('\n'),
                Some((_, '\\')) => value.push('\\'),
                Some((_, '"')) => value.push('"'),
                Some((_, other)) => match strictness {
                    Strictness::OpenMetrics => {
                        return Err(err(format!("invalid label escape `\\{other}`")));
                    }
                    Strictness::Lenient => value.push(other),
                },
                None => return Err(err("dangling escape in label value")),
            },
            '"' => return Ok((value, index + 1)),
            other => value.push(other),
        }
    }
    Err(err("unterminated label value"))
}

/// Finds the `}` matching the `{` at `open`, skipping quoted strings (which may
/// contain escaped quotes and braces).
pub(crate) fn find_closing_brace(line: &str, open: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate().skip(open + 1) {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if in_quotes => escaped = true,
            b'"' => in_quotes = !in_quotes,
            b'}' if !in_quotes => return Some(index),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_rejects_padding_and_unknown_escapes_lenient_accepts() {
        let padded = r#"a = "1""#;
        assert!(parse_label_pairs(padded, Strictness::OpenMetrics).is_err());
        assert_eq!(
            parse_label_pairs(padded, Strictness::Lenient).unwrap(),
            vec![("a".to_owned(), "1".to_owned())]
        );

        let unknown_escape = r#"a="\q""#;
        assert!(parse_label_pairs(unknown_escape, Strictness::OpenMetrics).is_err());
        assert_eq!(
            parse_label_pairs(unknown_escape, Strictness::Lenient).unwrap(),
            vec![("a".to_owned(), "q".to_owned())]
        );
    }

    #[test]
    fn strict_rejects_trailing_comma() {
        let trailing = r#"a="1","#;
        assert!(parse_label_pairs(trailing, Strictness::OpenMetrics).is_err());
        // Classic Prometheus text permits it.
        assert_eq!(
            parse_label_pairs(trailing, Strictness::Lenient).unwrap(),
            vec![("a".to_owned(), "1".to_owned())]
        );
    }

    #[test]
    fn both_dialects_handle_quoted_commas_equals_and_escapes() {
        let body = r#"path="a,b=c",kind="x\"y\\z\n""#;
        for strictness in [Strictness::OpenMetrics, Strictness::Lenient] {
            assert_eq!(
                parse_label_pairs(body, strictness).unwrap(),
                vec![
                    ("path".to_owned(), "a,b=c".to_owned()),
                    ("kind".to_owned(), "x\"y\\z\n".to_owned()),
                ]
            );
        }
    }
}
