//! Source-preprocessing pass that rewrites LaTeX-flavored macro definitions into
//! the plain TeX `\def` form that [`pulldown_latex`] understands natively.
//!
//! `pulldown-latex` already handles `\def` (including parameters, e.g.
//! `\def\v#1{\vec{#1}}`), but it does *not* recognize the LaTeX spellings
//! `\newcommand`, `\renewcommand`, `\providecommand`, or `\DeclareMathOperator`.
//! [`expand_definitions`] scans the source and translates those spellings into
//! equivalent `\def`s before the string reaches the parser; all non-definition
//! text is copied verbatim, and existing `\def`s are left untouched.
//!
//! This is deliberately a *narrow* textual pass over the common `\newcommand`
//! shapes — not a full TeX `\def` prefix / delimited-parameter emulator. The
//! optional-argument default form (`\newcommand{\name}[n][default]{body}`) maps
//! poorly to `\def` and is skipped (left verbatim) rather than mistranslated.

/// Rewrite LaTeX macro-definition spellings into TeX `\def` form, in place across
/// the whole string. Recognized (anywhere in `src`):
///
/// - `\newcommand{\name}{body}` / `\newcommand\name{body}` → `\def\name{body}`
/// - `\newcommand{\name}[n]{body}` → `\def\name#1#2…#n{body}` (`n` in `1..=9`)
/// - `\renewcommand` / `\providecommand` → identical to `\newcommand`
///   (pulldown's `\def` overwrites, so provide-vs-renew need not be distinguished)
/// - `\DeclareMathOperator{\op}{text}` → `\def\op{\operatorname{text}}`
/// - `\DeclareMathOperator*{\op}{text}` → `\def\op{\operatorname*{text}}`
///
/// The body is matched by balanced-brace counting (nested `{}` are handled), and
/// whitespace between tokens is tolerated. Anything that does not parse as one of
/// the above shapes — including the optional-argument-default form — is left
/// verbatim so the parser sees it unchanged.
pub fn expand_definitions(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some((replacement, next)) = try_definition(src, i) {
            out.push_str(&replacement);
            i = next;
        } else {
            // Copy this UTF-8 char verbatim.
            let ch = src[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Extract the `\arraystretch` factor from already-expanded `src` and strip its
/// definition out of the string.
///
/// pulldown-latex does not surface `\arraystretch` as a value — it is an ordinary
/// LaTeX length macro. By the time this runs, [`expand_definitions`] has already
/// turned `\renewcommand{\arraystretch}{F}` into `\def\arraystretch{F}` (and a
/// hand-written `\def\arraystretch{F}` is left as-is). We scan for those `\def`s,
/// parse the last `F` (LaTeX semantics: the latest redefinition wins), remove the
/// definitions from the source (so pulldown never sees a useless macro), and hand
/// the factor to layout via [`box_layout`]'s `Ctx`. A missing/invalid factor
/// leaves the source untouched and yields `None` (layout then uses the default 1.0).
///
/// Only the simple `\def\arraystretch{NUMBER}` shape is recognized; anything more
/// elaborate is left verbatim.
pub fn extract_arraystretch(src: &str) -> (String, Option<f32>) {
    const KEY: &str = r"\def\arraystretch";
    let mut out = String::with_capacity(src.len());
    let mut factor: Option<f32> = None;
    let mut i = 0;
    while i < src.len() {
        if src[i..].starts_with(KEY) {
            let after_key = i + KEY.len();
            // Require a non-alphabetic boundary so we don't match `\arraystretchX`.
            let boundary_ok = src[after_key..]
                .chars()
                .next()
                .map(|c| !c.is_ascii_alphabetic())
                .unwrap_or(true);
            let pos = skip_ws(src, after_key);
            if boundary_ok && src[pos..].starts_with('{') {
                if let Some((body, after)) = balanced_group(src, pos) {
                    if let Ok(f) = body.trim().parse::<f32>() {
                        if f.is_finite() && f > 0.0 {
                            factor = Some(f);
                        }
                        // Strip the whole definition regardless of parse success
                        // only when it parsed; otherwise fall through verbatim.
                        if f.is_finite() && f > 0.0 {
                            i = after;
                            continue;
                        }
                    }
                }
            }
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    (out, factor)
}

/// If a recognized definition spelling begins at byte offset `at`, return its
/// `\def` translation plus the byte offset just past the consumed input.
/// Returns `None` if `at` is not the start of a translatable definition (the
/// caller then copies one char and advances).
fn try_definition(src: &str, at: usize) -> Option<(String, usize)> {
    let rest = &src[at..];
    if let Some(after) = rest
        .strip_prefix(r"\newcommand")
        .or_else(|| rest.strip_prefix(r"\renewcommand"))
        .or_else(|| rest.strip_prefix(r"\providecommand"))
    {
        let cmd_len = rest.len() - after.len();
        return parse_newcommand(src, at + cmd_len);
    }
    if let Some(after) = rest.strip_prefix(r"\DeclareMathOperator") {
        let cmd_len = rest.len() - after.len();
        return parse_declare_operator(src, at + cmd_len);
    }
    // `\substack{ROWS}` — pulldown does *not* parse `\substack`, so rewrite it to a
    // centered `matrix` environment (which it does parse). Must be tried before the
    // `\cancel`/`\not` shim so it isn't mis-split by it.
    if let Some(after) = rest.strip_prefix(r"\substack") {
        let cmd_len = rest.len() - after.len();
        return parse_substack(src, at + cmd_len);
    }
    // `\cancel{ARG}` — pulldown lowers both `\cancel` and `\not` to the same
    // `Visual::Negation`, so we mark `\cancel` with a Private-Use-Area sentinel the
    // layout pass detects; `\not` is left verbatim and keeps its negation rendering.
    if let Some(after) = rest.strip_prefix(r"\cancel") {
        // Only a true `\cancel` control word — not a longer name like `\cancelto`.
        let next = after.chars().next();
        if !matches!(next, Some(c) if c.is_ascii_alphabetic()) {
            let cmd_len = rest.len() - after.len();
            return parse_cancel(src, at + cmd_len);
        }
    }
    None
}

/// The Private-Use-Area sentinel emitted in place of `\cancel`, kept in sync with
/// `box_layout`'s `CANCEL_SENTINEL`. pulldown passes it through as an ordinary
/// content character, which the layout pass picks up to strike the next element.
const CANCEL_SENTINEL: char = '\u{E000}';

/// The Private-Use-Area sentinel emitted just before a `\substack`-derived
/// `matrix`, kept in sync with `box_layout`'s `SUBSTACK_SENTINEL`. pulldown passes
/// it through as an ordinary content character; the layout pass picks it up to
/// retag the following matrix as a script-sized [`box_layout::MatrixKind::Substack`].
const SUBSTACK_SENTINEL: char = '\u{E001}';

/// Parse the tail of a `\cancel` (just past the keyword) at `pos`. Rewrites
/// `\cancel{ARG}` → `<sentinel>{ARG}` (the sentinel marks the braced argument as
/// struck). Leaves the form verbatim if no braced argument follows.
fn parse_cancel(src: &str, pos: usize) -> Option<(String, usize)> {
    let pos = skip_ws(src, pos);
    if !src[pos..].starts_with('{') {
        return None;
    }
    let (arg, after) = balanced_group(src, pos)?;
    // Recursively expand the argument so nested shims/definitions inside it are
    // also rewritten (the result string is not re-scanned).
    let arg = expand_definitions(&arg);
    Some((format!("{CANCEL_SENTINEL}{{{arg}}}"), after))
}

/// Parse the tail of a `\substack` (just past the keyword) at `pos`. Rewrites
/// `\substack{ROWS}` → `{<sentinel>\begin{matrix}ROWS\end{matrix}}` — the `ROWS`
/// already use `\\` row separators that `matrix` understands, the wrapping `{…}`
/// keeps it a single element under operator limits (`\sum_{\substack{…}}`), and the
/// leading [`SUBSTACK_SENTINEL`] tells the layout pass to render the matrix at
/// script size (one step smaller than its surroundings), as real TeX does. Leaves
/// the form verbatim if no braced argument follows.
fn parse_substack(src: &str, pos: usize) -> Option<(String, usize)> {
    let pos = skip_ws(src, pos);
    if !src[pos..].starts_with('{') {
        return None;
    }
    let (rows, after) = balanced_group(src, pos)?;
    // Recursively expand the rows so nested shims/definitions inside them are also
    // rewritten (the result string is not re-scanned).
    let rows = expand_definitions(&rows);
    Some((format!(r"{{{SUBSTACK_SENTINEL}\begin{{matrix}}{rows}\end{{matrix}}}}"), after))
}

/// Parse the tail of a `\newcommand`/`\renewcommand`/`\providecommand` starting at
/// `pos` (just past the keyword). Builds the `\def` translation. Returns `None`
/// (leave verbatim) on any shape we don't handle, including the optional-argument
/// default form.
fn parse_newcommand(src: &str, pos: usize) -> Option<(String, usize)> {
    let mut pos = skip_ws(src, pos);
    // Command name: either `{\name}` or a bare `\name`.
    let name;
    if src[pos..].starts_with('{') {
        let (inner, after) = balanced_group(src, pos)?;
        name = inner.trim().to_string();
        pos = after;
    } else if src[pos..].starts_with('\\') {
        let (tok, after) = control_sequence(src, pos)?;
        name = tok.to_string();
        pos = after;
    } else {
        return None;
    }
    if !is_control_sequence(&name) {
        return None;
    }

    pos = skip_ws(src, pos);

    // Optional `[n]` argument count.
    let mut nargs = 0usize;
    if src[pos..].starts_with('[') {
        let (inner, after) = bracket_group(src, pos)?;
        nargs = inner.trim().parse::<usize>().ok().filter(|n| (1..=9).contains(n))?;
        pos = after;
        pos = skip_ws(src, pos);
        // TODO: optional-argument defaults (`\newcommand{\n}[k][default]{body}`)
        // map poorly to `\def`; skip the whole definition rather than corrupt it.
        if src[pos..].starts_with('[') {
            return None;
        }
    }

    // Body: a balanced `{…}` group.
    if !src[pos..].starts_with('{') {
        return None;
    }
    let (body, after) = balanced_group(src, pos)?;

    let mut def = String::from(r"\def");
    def.push_str(&name);
    for k in 1..=nargs {
        def.push('#');
        def.push(char::from(b'0' + k as u8));
    }
    def.push('{');
    def.push_str(&body);
    def.push('}');
    Some((def, after))
}

/// Parse the tail of a `\DeclareMathOperator` (or `\DeclareMathOperator*`) starting
/// at `pos` (just past the keyword). Translates to
/// `\def\op{\operatorname{text}}` (or `\operatorname*` for the starred form).
fn parse_declare_operator(src: &str, pos: usize) -> Option<(String, usize)> {
    let mut pos = pos;
    let starred = src[pos..].starts_with('*');
    if starred {
        pos += 1;
    }
    pos = skip_ws(src, pos);

    // Operator name: `{\op}` or bare `\op`.
    let name;
    if src[pos..].starts_with('{') {
        let (inner, after) = balanced_group(src, pos)?;
        name = inner.trim().to_string();
        pos = after;
    } else if src[pos..].starts_with('\\') {
        let (tok, after) = control_sequence(src, pos)?;
        name = tok.to_string();
        pos = after;
    } else {
        return None;
    }
    if !is_control_sequence(&name) {
        return None;
    }

    pos = skip_ws(src, pos);

    // Operator text: a balanced `{…}` group.
    if !src[pos..].starts_with('{') {
        return None;
    }
    let (text, after) = balanced_group(src, pos)?;

    let opname = if starred { r"\operatorname*" } else { r"\operatorname" };
    let def = format!(r"\def{name}{{{opname}{{{text}}}}}");
    Some((def, after))
}

/// Advance past ASCII whitespace starting at `pos`; returns the new offset.
fn skip_ws(src: &str, pos: usize) -> usize {
    let bytes = src.as_bytes();
    let mut p = pos;
    while p < bytes.len() && bytes[p].is_ascii_whitespace() {
        p += 1;
    }
    p
}

/// Read a balanced `{…}` group whose opening brace is at `pos`. Returns the inner
/// contents (without the outer braces) and the offset just past the closing brace.
/// Nested `{}` are tracked by depth. Returns `None` if unbalanced.
fn balanced_group(src: &str, pos: usize) -> Option<(String, usize)> {
    let bytes = src.as_bytes();
    debug_assert_eq!(bytes[pos], b'{');
    let mut depth = 0i32;
    let mut p = pos;
    let start_inner = pos + 1;
    while p < bytes.len() {
        match bytes[p] {
            b'\\' => {
                // Skip an escaped char (e.g. `\{` / `\}`) so it doesn't affect depth.
                p += 2;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((src[start_inner..p].to_string(), p + 1));
                }
            }
            _ => {}
        }
        p += 1;
    }
    None
}

/// Read a balanced `[…]` group whose opening bracket is at `pos`. Returns the inner
/// contents and the offset just past the closing bracket. Returns `None` if there
/// is no matching `]`.
fn bracket_group(src: &str, pos: usize) -> Option<(String, usize)> {
    let bytes = src.as_bytes();
    debug_assert_eq!(bytes[pos], b'[');
    let mut p = pos + 1;
    let start_inner = p;
    while p < bytes.len() {
        if bytes[p] == b']' {
            return Some((src[start_inner..p].to_string(), p + 1));
        }
        p += 1;
    }
    None
}

/// Read a control sequence beginning with `\` at `pos`: the backslash plus a run of
/// ASCII letters (or, for a single non-letter, that one char). Returns the token
/// (including the leading `\`) and the offset just past it.
fn control_sequence(src: &str, pos: usize) -> Option<(&str, usize)> {
    let bytes = src.as_bytes();
    if bytes.get(pos) != Some(&b'\\') {
        return None;
    }
    let mut p = pos + 1;
    if p < bytes.len() && bytes[p].is_ascii_alphabetic() {
        while p < bytes.len() && bytes[p].is_ascii_alphabetic() {
            p += 1;
        }
    } else if p < bytes.len() {
        // Single-char control symbol.
        p += 1;
    } else {
        return None;
    }
    Some((&src[pos..p], p))
}

/// Whether `s` looks like a control sequence (`\` followed by at least one char).
fn is_control_sequence(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next() == Some('\\') && cs.next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newcommand_braced_name() {
        assert_eq!(
            expand_definitions(r"\newcommand{\R}{\mathbb{R}}\R"),
            r"\def\R{\mathbb{R}}\R"
        );
    }

    #[test]
    fn newcommand_bare_name() {
        assert_eq!(
            expand_definitions(r"\newcommand\R{\mathbb R}\R"),
            r"\def\R{\mathbb R}\R"
        );
    }

    #[test]
    fn newcommand_with_args() {
        assert_eq!(
            expand_definitions(r"\newcommand{\v}[1]{\vec{#1}}\v{x}"),
            r"\def\v#1{\vec{#1}}\v{x}"
        );
        assert_eq!(
            expand_definitions(r"\newcommand{\pair}[2]{(#1,#2)}"),
            r"\def\pair#1#2{(#1,#2)}"
        );
    }

    #[test]
    fn nested_braces_in_body() {
        assert_eq!(
            expand_definitions(r"\newcommand{\f}{\frac{a}{b}}"),
            r"\def\f{\frac{a}{b}}"
        );
    }

    #[test]
    fn renew_and_provide_treated_like_newcommand() {
        assert_eq!(
            expand_definitions(r"\renewcommand{\R}{X}"),
            r"\def\R{X}"
        );
        assert_eq!(
            expand_definitions(r"\providecommand{\R}{X}"),
            r"\def\R{X}"
        );
    }

    #[test]
    fn declare_math_operator() {
        assert_eq!(
            expand_definitions(r"\DeclareMathOperator{\lcm}{lcm}"),
            r"\def\lcm{\operatorname{lcm}}"
        );
    }

    #[test]
    fn declare_math_operator_starred() {
        assert_eq!(
            expand_definitions(r"\DeclareMathOperator*{\argmax}{arg\,max}"),
            r"\def\argmax{\operatorname*{arg\,max}}"
        );
    }

    #[test]
    fn existing_def_untouched() {
        let s = r"\def\R{\mathbb R} x\in\R";
        assert_eq!(expand_definitions(s), s);
    }

    #[test]
    fn non_definition_text_verbatim() {
        let s = r"a + b = c \frac{1}{2}";
        assert_eq!(expand_definitions(s), s);
    }

    #[test]
    fn whitespace_tolerated() {
        assert_eq!(
            expand_definitions("\\newcommand {\\R} {X}"),
            r"\def\R{X}"
        );
    }

    #[test]
    fn optional_arg_default_skipped() {
        // The `[1][0]` default form is left verbatim (untranslated) — no panic.
        let s = r"\newcommand{\inc}[1][0]{#1+1}";
        assert_eq!(expand_definitions(s), s);
    }

    #[test]
    fn multiple_definitions_and_trailing_math() {
        assert_eq!(
            expand_definitions(r"\newcommand{\R}{\mathbb{R}}\newcommand{\Z}{\mathbb{Z}}\R \Z"),
            r"\def\R{\mathbb{R}}\def\Z{\mathbb{Z}}\R \Z"
        );
    }

    #[test]
    fn cancel_rewrites_to_sentinel() {
        // `\cancel{x}` → the PUA sentinel followed by the braced argument.
        assert_eq!(expand_definitions(r"\cancel{x}"), format!("{CANCEL_SENTINEL}{{x}}"));
        assert_eq!(
            expand_definitions(r"a + \cancel{5y}"),
            format!("a + {CANCEL_SENTINEL}{{5y}}")
        );
    }

    #[test]
    fn cancel_does_not_match_longer_names() {
        // A longer control word like `\cancelto` is left verbatim (not shimmed).
        let s = r"\cancelto{0}{x}";
        assert_eq!(expand_definitions(s), s);
    }

    #[test]
    fn not_is_left_verbatim() {
        // `\not` keeps pulldown's negation behaviour — never struck.
        let s = r"\not= \not\subset";
        assert_eq!(expand_definitions(s), s);
    }

    #[test]
    fn substack_rewrites_to_matrix() {
        assert_eq!(
            expand_definitions(r"\substack{a \\ b}"),
            format!(r"{{{SUBSTACK_SENTINEL}\begin{{matrix}}a \\ b\end{{matrix}}}}")
        );
    }
}
