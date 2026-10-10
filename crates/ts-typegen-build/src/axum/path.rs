//! Route path template grammar: parsing, rendering, and TS-literal escaping.

use crate::diag::{Diagnostic, Diagnostics};
use crate::ir::SourceLoc;

/// One piece of a route template between `/` separators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Text matched literally, with any `{{`/`}}` escapes already resolved.
    Literal(String),
    /// A captured parameter.
    Capture {
        /// The capture's name, without its syntax markers.
        name: String,
        /// True for `{*rest}` / `*rest`, which captures the remaining path.
        wildcard: bool,
    },
}

/// Parses a route template, or raises a diagnostic and returns `None`.
pub fn parse_template(
    t: &str,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Option<Vec<Segment>> {
    let mut out = Vec::new();
    let bad = |diags: &mut Diagnostics, why: &str| {
        diags.push(
            Diagnostic::new(loc.clone(), format!("route path `{t}` is malformed: {why}"))
                .with_help("a capture is `{name}`, `{*rest}`, `:name` or `*rest`; \
                            write `{{` and `}}` for a literal brace"),
        );
    };

    for raw in t.split('/') {
        if raw.starts_with('{') && raw.ends_with('}') && raw.len() >= 2 && !raw.starts_with("{{") {
            let inner = &raw[1..raw.len() - 1];
            if inner.is_empty() || inner.contains('{') || inner.contains('}') {
                bad(diags, "empty or nested braces in a capture");
                return None;
            }
            let wildcard = inner.starts_with('*');
            out.push(Segment::Capture {
                name: inner.trim_start_matches('*').to_string(),
                wildcard,
            });
            continue;
        }
        if raw.contains('{') || raw.contains('}') {
            let unescaped = raw.replace("{{", "\u{0}").replace("}}", "\u{1}");
            if unescaped.contains('{') || unescaped.contains('}') {
                bad(diags, "unbalanced brace outside a capture");
                return None;
            }
            out.push(Segment::Literal(
                unescaped.replace('\u{0}', "{").replace('\u{1}', "}"),
            ));
            continue;
        }
        if let Some(name) = raw.strip_prefix(':') {
            if name.is_empty() {
                bad(diags, "a `:` capture needs a name");
                return None;
            }
            out.push(Segment::Capture { name: name.to_string(), wildcard: false });
            continue;
        }
        if let Some(name) = raw.strip_prefix('*') {
            out.push(Segment::Capture {
                name: if name.is_empty() { "rest".into() } else { name.to_string() },
                wildcard: true,
            });
            continue;
        }
        out.push(Segment::Literal(raw.to_string()));
    }
    Some(out)
}

/// Names of every [`Segment::Capture`] in the template, in order.
pub fn capture_names(segs: &[Segment]) -> Vec<String> {
    segs.iter()
        .filter_map(|seg| match seg {
            Segment::Capture { name, .. } => Some(name.clone()),
            Segment::Literal(_) => None,
        })
        .collect()
}

/// Renders the canonical template: segments joined by `/`, captures emitted
/// in brace syntax, and literal braces re-escaped as `{{`/`}}` so the result
/// reparses to the same segments regardless of which syntax the source used.
pub fn render(segs: &[Segment]) -> String {
    segs.iter()
        .map(|seg| match seg {
            Segment::Literal(text) => text.replace('{', "{{").replace('}', "}}"),
            Segment::Capture { name, wildcard } => {
                if *wildcard {
                    format!("{{*{name}}}")
                } else {
                    format!("{{{name}}}")
                }
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Escapes a string for embedding in a TypeScript template literal:
/// backslash first, then backtick, then `${`.
pub fn escape_ts_literal(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

/// Whether `path` is exempt under `pattern`: either an exact match, or
/// `pattern` ends in a single trailing `*` and `path` starts with the prefix
/// before it. A pattern with a `*` anywhere else matches nothing.
pub fn matches_exempt(path: &str, pattern: &str) -> bool {
    if path == pattern {
        return true;
    }
    match pattern.strip_suffix('*') {
        Some(prefix) if !prefix.contains('*') => path.starts_with(prefix),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Diagnostics;
    use crate::ir::SourceLoc;

    fn loc() -> SourceLoc {
        SourceLoc { file: "h.rs".into(), line: 1 }
    }

    fn parse(t: &str) -> Vec<Segment> {
        let mut d = Diagnostics::default();
        let segs = parse_template(t, &loc(), &mut d).expect("must parse");
        assert!(!d.has_errors(), "unexpected diagnostics for {t}");
        segs
    }

    #[test]
    fn brace_and_colon_captures_both_parse() {
        assert_eq!(capture_names(&parse("/a/{id}/b/{slug}")), vec!["id", "slug"]);
        assert_eq!(capture_names(&parse("/a/:id/b/:slug")), vec!["id", "slug"]);
    }

    #[test]
    fn wildcards_parse_in_both_syntaxes() {
        let segs = parse("/a/{*rest}");
        assert_eq!(capture_names(&segs), vec!["rest"]);
        assert!(matches!(segs.last(), Some(Segment::Capture { wildcard: true, .. })));
        assert!(matches!(parse("/a/*rest").last(), Some(Segment::Capture { wildcard: true, .. })));
    }

    #[test]
    fn doubled_braces_are_literal_text_not_captures() {
        let segs = parse("/a/{{id}}");
        assert!(capture_names(&segs).is_empty());
        assert_eq!(render(&segs), "/a/{{id}}");
    }

    #[test]
    fn a_trailing_slash_is_preserved_because_axum_distinguishes_it() {
        assert_eq!(render(&parse("/a/")), "/a/");
        assert_ne!(render(&parse("/a/")), render(&parse("/a")));
    }

    #[test]
    fn malformed_templates_diagnose_instead_of_panicking() {
        for bad in ["/a/{", "/a/{}", "/a/{id", "/a/}", "/a/{i{d}"] {
            let mut d = Diagnostics::default();
            assert!(parse_template(bad, &loc(), &mut d).is_none(), "{bad} must not parse");
            assert!(d.has_errors(), "{bad} must raise an error");
        }
    }

    #[test]
    fn ts_literal_hostile_characters_are_escaped() {
        assert_eq!(escape_ts_literal("/a/`x`"), "/a/\\`x\\`");
        assert_eq!(escape_ts_literal("/a/${x}"), "/a/\\${x}");
        assert_eq!(escape_ts_literal("/a/b\\c"), "/a/b\\\\c");
    }

    #[test]
    fn exempt_patterns_are_exact_or_trailing_star_only() {
        assert!(matches_exempt("/auth/refresh", "/auth/*"));
        assert!(matches_exempt("/monitor", "/monitor"));
        assert!(!matches_exempt("/monitoring", "/monitor"));
        assert!(!matches_exempt("/auth/refresh", "/*/refresh"));
    }
}
