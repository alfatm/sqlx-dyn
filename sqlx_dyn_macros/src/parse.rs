//! Template scanner for `${...}` / `#{...}` markers.
//!
//! Everything outside a marker is copied verbatim. This is not a SQL parser: it
//! tracks bracket nesting and string literals only as far as needed to find the
//! end of an interpolation.

use proc_macro2::Span;

#[derive(Debug)]
pub enum Part {
    Text(String),
    /// Rust source between `${` and `}`, plus the span to blame for a parse error.
    Bind(String, Span),
    /// Rust source between `#{` and `}`.
    Fragment(String, Span),
    /// `${?expr}`: an optional predicate. `expr` is an `Option<T>`; on `None`
    /// the whole predicate — including the `AND`/`OR` joining it — is dropped
    /// from the SQL.
    ///
    /// `predicate` is the predicate's literal SQL without the marker, split
    /// around the bind site: `before` is the part left of the marker (e.g.
    /// `"name ILIKE "`), `after` is the part right of it. `joiner` is the
    /// `AND`/`OR`/`WHERE` keyword that introduced the predicate; it is kept so
    /// it can be re-emitted only if the predicate survives.
    OptBind(OptPredicate),
    /// Mandatory SQL whose leading `AND`/`OR` must be decided at runtime.
    ///
    /// Created when literal text follows an optional predicate: if that
    /// predicate is dropped, the text becomes first in the clause and needs a
    /// `WHERE` rather than the written `AND`. `joiner` is the keyword lifted out
    /// of the text; `text` is the remainder. `clause` is the predicate list it
    /// belongs to.
    Joined {
        joiner: String,
        text: String,
        clause: u32,
    },
}

/// A predicate emitted only if its bind is `Some`.
#[derive(Debug)]
pub struct OptPredicate {
    /// Rust expression; must be an `Option<T>`.
    pub expr: String,
    /// `AND`, `OR`, `WHERE` or `HAVING` — the keyword that introduced this
    /// predicate; kept so it can be re-emitted only if the predicate survives.
    ///
    /// The cut in [`split_predicate`] always consumes the keyword itself (that
    /// is exactly what `text_len` points at), so it never stays behind in the
    /// mandatory text and is never absent: the first predicate of a clause
    /// carries `WHERE` or `HAVING` as its own joiner.
    pub joiner: String,
    /// SQL between the joiner and the marker, e.g. `" name ILIKE "`.
    pub before: String,
    /// SQL between the marker and the end of the predicate; usually empty.
    pub after: String,
    /// Which top-level predicate list it belongs to. Predicates of different
    /// clauses must not share joiner bookkeeping.
    pub clause: u32,
    pub span: Span,
}

/// Spans are ignored: two parts are equal if their kind and contents match.
/// `proc_macro2::Span` is not comparable, and the tests only care about content.
impl PartialEq for Part {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Part::Text(a), Part::Text(b)) => a == b,
            (Part::Bind(a, _), Part::Bind(b, _)) => a == b,
            (Part::Fragment(a, _), Part::Fragment(b, _)) => a == b,
            (
                Part::Joined {
                    joiner: ja,
                    text: ta,
                    clause: ca,
                },
                Part::Joined {
                    joiner: jb,
                    text: tb,
                    clause: cb,
                },
            ) => ja == jb && ta == tb && ca == cb,
            (Part::OptBind(a), Part::OptBind(b)) => {
                a.expr == b.expr
                    && a.joiner == b.joiner
                    && a.before == b.before
                    && a.after == b.after
                    && a.clause == b.clause
            }
            _ => false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
}

impl ParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Splits the template into literal text and interpolations.
///
/// Escapes: `$${` yields a literal `${`, `##{` a literal `#{`. A `$` or `#` not
/// followed by `{` is never special and needs no escaping.
pub fn parse_template(input: &str, span: Span) -> Result<Vec<Part>, ParseError> {
    let bytes = input.as_bytes();
    // Clause bookkeeping and the whole-template comment check only matter when a
    // `${?..}` marker is present, so both are computed on the first one. The
    // stripped view is computed once and shared between them.
    let mut stripped: Option<String> = None;
    let mut clauses: Option<Vec<(usize, u32)>> = None;
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            // `$${` / `##{` escape to a literal marker.
            b'$' if bytes[i + 1..].starts_with(b"${") => {
                text.push_str("${");
                i += 3;
            }
            b'#' if bytes[i + 1..].starts_with(b"#{") => {
                text.push_str("#{");
                i += 3;
            }
            b'$' | b'#' if bytes[i + 1..].starts_with(b"{") => {
                let marker = bytes[i];
                let expr_start = i + 2;
                let expr_end = find_close(input, expr_start).ok_or_else(|| {
                    ParseError::new(format!(
                        "unterminated interpolation `{}{{...}}`: missing closing `}}`",
                        marker as char
                    ))
                })?;
                let expr = input[expr_start..expr_end].trim().to_string();
                if expr.is_empty() {
                    return Err(ParseError::new(format!(
                        "empty interpolation `{}{{}}`",
                        marker as char
                    )));
                }
                // `${?expr}` is an optional predicate, not a plain bind.
                if marker == b'$' {
                    if let Some(inner) = expr.strip_prefix('?') {
                        let inner = inner.trim();
                        if inner.is_empty() {
                            return Err(ParseError::new(
                                "empty optional interpolation `${?}`".to_string(),
                            ));
                        }
                        let mut predicate = split_predicate(&text, inner.to_string(), span)?;
                        // `split_predicate` cut the predicate's own SQL out of
                        // the accumulated text; whatever remains is mandatory.
                        text.truncate(predicate.text_len);
                        if !text.is_empty() {
                            parts.push(Part::Text(std::mem::take(&mut text)));
                        }
                        // Text right of the marker that still belongs to this
                        // predicate (a cast, a concatenation) must disappear
                        // along with it.
                        let rest = &input[expr_end + 1..];
                        let tail = predicate_tail(rest);
                        // The tail stops at the next marker or escape, because
                        // that text is not ours to copy. If no boundary was seen
                        // before it, the interpolation is inside this predicate
                        // and the predicate cannot be emitted or removed as one
                        // unit.
                        if tail.split_by_interpolation {
                            return Err(ParseError::new(
                                "another interpolation appears inside the same \
                                 predicate as `${?...}`.\n\
                                 The predicate must be removable as one piece, \
                                 while each interpolation is expanded \
                                 separately, so the two cannot overlap: \
                                 `a = ${?x} || ${y}` would drop to \
                                 `... FROM t $1`.\n\
                                 Give the optional condition its own `AND` \
                                 clause, or use a plain bind `${...}`."
                                    .to_string(),
                            ));
                        }
                        let tail = tail.len;
                        predicate.predicate.after = rest[..tail].to_string();
                        // The marker sits inside the predicate its joiner
                        // opened, and `clause_map` binds a boundary keyword to
                        // the clause it opens, so the marker's offset yields
                        // that same clause.
                        let stripped = stripped.get_or_insert_with(|| strip_literals(input));
                        let clauses = clauses.get_or_insert_with(|| clause_map(stripped));
                        predicate.predicate.clause = clause_at(clauses, i);
                        parts.push(Part::OptBind(predicate.predicate));
                        i = expr_end + 1 + tail;
                        continue;
                    }
                }

                if !text.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut text)));
                }
                parts.push(if marker == b'$' {
                    Part::Bind(expr, span)
                } else {
                    Part::Fragment(expr, span)
                });
                i = expr_end + 1;
            }
            _ => {
                // Push the whole UTF-8 character, not a byte, so multi-byte
                // characters in the SQL survive.
                let ch = input[i..].chars().next().expect("index on a char boundary");
                text.push(ch);
                i += ch.len_utf8();
            }
        }
    }

    if !text.is_empty() {
        parts.push(Part::Text(text));
    }

    // A comment *after* a `${?..}` marker is just as dangerous: the predicate
    // tail stops at the next keyword even inside a comment, and `trim_end` drops
    // the newline that closes a line comment, so the surviving word lands on the
    // comment line and gets commented out — `WHERE a = ${?x} -- n\nAND b = 1`
    // would emit `WHERE a = $1 -- n AND b = 1`. The check in `split_predicate`
    // only covers the text before the marker; reject the whole template if it
    // mixes `${?..}` with a comment. Literals are stripped first so that `'--'`
    // stays data, and marker regions are skipped so a Rust comment inside
    // `${..}` is not mistaken for SQL.
    // `stripped` is `Some` exactly when a `${?..}` marker was found, so this is
    // the whole-template comment check.
    if stripped
        .as_deref()
        .is_some_and(|s| find_comment(s).is_some())
    {
        return Err(ParseError::new(
            "SQL comments are not supported in a template using `${?..}`.\n\
             A comment after the marker hides the keyword that follows it: the \
             predicate tail stops at that keyword, and the newline closing a line \
             comment is dropped, so the surviving keyword ends up commented out \
             and the query silently matches the wrong rows.\n\
             Remove the comment or build this query without `${?..}`."
                .to_string(),
        ));
    }

    Ok(lift_joiners_after_optionals(parts))
}

/// Keywords that start a new top-level predicate list.
///
/// Each resets joiner bookkeeping: a `WHERE` after `UNION` belongs to a new
/// select, and `HAVING` is a separate list from the `WHERE` above it. Without
/// per-clause state, one clause's `WHERE` could introduce a predicate in
/// another, or a clause's own `WHERE` would be replaced by a dangling `AND`.
const CLAUSE_BOUNDARIES: [&str; 6] = ["WHERE", "HAVING", "UNION", "INTERSECT", "EXCEPT", "QUALIFY"];

/// One lexical pass over a fragment, producing every view the fragment checks
/// need.
///
/// The checks cannot be layered on top of each other, because literals and
/// comments are *mutually exclusive* contexts and each hides the other's
/// delimiters. Running [`strip_literals`] first and looking for comments in its
/// output gets this wrong in both directions: a `'` written inside a comment
/// (`/* it's */`) opens a literal that swallows the comment's `*/`, so the
/// comment reads as unterminated; and a `--` inside a literal is only *not* a
/// comment because the literal was blanked first. Only a single scan that tracks
/// which context it is in can tell the two apart.
///
/// Offsets are preserved byte for byte in both views, so an index found in one
/// indexes the source and the other.
pub struct FragmentLex {
    /// The source with comment bytes blanked to spaces and everything else —
    /// literals included — left exactly as written. This is the text the
    /// fragment contributes to the query.
    ///
    /// Blanked, never deleted: `1/*x*/AND` must not become `1AND`, so each
    /// comment byte becomes a space and the tokens it separated stay separated.
    comments_blanked: String,
    /// Uppercased, with both literals and comments blanked. Structure only, for
    /// keyword and bracket matching.
    structure: String,
    /// Whether a `/*` is left unclosed at the end of the fragment.
    unterminated_block: bool,
    /// Whether a `'`, `"` or `$tag$` is left unclosed at the end of the
    /// fragment.
    unterminated_literal: bool,
    /// Whether any comment was found at all.
    has_comment: bool,
}

impl FragmentLex {
    pub fn scan(sql: &str) -> Self {
        let bytes = sql.as_bytes();
        let mut blanked = bytes.to_vec();
        // Structure starts fully blank and is filled in only for bytes the scan
        // classifies as SQL, so anything it skips is already blank.
        let mut structure = vec![b' '; bytes.len()];
        let mut has_comment = false;
        let mut unterminated_block = false;
        let mut unterminated_literal = false;
        let mut i = 0;

        while i < bytes.len() {
            // A line comment runs to the newline, which stays: it is whitespace
            // either way and keeps the tokens on both sides apart.
            if bytes[i] == b'-' && bytes[i + 1..].starts_with(b"-") {
                has_comment = true;
                while i < bytes.len() && bytes[i] != b'\n' {
                    blanked[i] = b' ';
                    i += 1;
                }
                continue;
            }
            // Block comments nest in PostgreSQL. Quotes, `$tag$` and non-ASCII
            // bytes inside one are comment text, not delimiters — which is the
            // whole reason this scan exists.
            if bytes[i] == b'/' && bytes[i + 1..].starts_with(b"*") {
                has_comment = true;
                let mut nesting = 0usize;
                while i < bytes.len() {
                    if bytes[i] == b'/' && bytes[i + 1..].starts_with(b"*") {
                        nesting += 1;
                        blanked[i] = b' ';
                        blanked[i + 1] = b' ';
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'*' && bytes[i + 1..].starts_with(b"/") {
                        nesting -= 1;
                        blanked[i] = b' ';
                        blanked[i + 1] = b' ';
                        i += 2;
                        if nesting == 0 {
                            break;
                        }
                        continue;
                    }
                    blanked[i] = b' ';
                    i += 1;
                }
                if nesting != 0 {
                    unterminated_block = true;
                }
                continue;
            }
            // Dollar-quoted literal: `$tag$ ... $tag$`. Its body is arbitrary
            // text, so brackets, quotes and comment markers inside it are data.
            if bytes[i] == b'$' {
                if let Some(tag) = dollar_tag(bytes, i) {
                    match find_subslice(bytes, i + tag, &bytes[i..i + tag]) {
                        Some(at) => i = at + tag,
                        None => {
                            unterminated_literal = true;
                            i = bytes.len();
                        }
                    }
                    continue;
                }
                structure[i] = b'$';
                i += 1;
                continue;
            }
            // `'...'` / `"..."`. Its body is data, so a comment marker or
            // bracket inside it is not structure. Where the literal *ends* is
            // decided by [`quoted_end`], which is also what keeps `'a\'` from
            // swallowing the SQL after it.
            if bytes[i] == b'\'' || bytes[i] == b'"' {
                // An `E`/`e` prefix is part of the literal, not a keyword, so it
                // is blanked with it rather than left in `structure`.
                if opens_extended_string(bytes, i) {
                    structure[i - 1] = b' ';
                }
                // An unclosed literal consumes the rest of the fragment: text
                // that never closes cannot be read as structure.
                i = quoted_end(bytes, i).unwrap_or_else(|| {
                    unterminated_literal = true;
                    bytes.len()
                });
                continue;
            }
            if bytes[i].is_ascii() {
                structure[i] = bytes[i].to_ascii_uppercase();
            }
            // A non-ASCII byte cannot be part of a keyword; leaving it blank in
            // `structure` keeps that view ASCII and the offsets aligned.
            i += 1;
        }

        Self {
            comments_blanked: String::from_utf8(blanked)
                .expect("only ASCII spaces were written over comment bytes"),
            structure: String::from_utf8(structure).expect("structure is ASCII by construction"),
            unterminated_block,
            unterminated_literal,
            has_comment,
        }
    }

    /// Blanks a fragment's SQL comments, or `None` when it has none.
    ///
    /// A template using `${?...}` may not contain a comment, because one can hide
    /// the joiner between two predicates and silently change which rows match. A
    /// fragment is opaque to that whole-template check, so a comment smuggled inside
    /// one reopens exactly the hole:
    ///
    /// ```text
    /// const F: SqlFragment = sql_fragment!("c = 1 --");
    /// query!("SELECT * FROM t WHERE a = ${?x} AND #{F} AND b = 1")
    /// -> SELECT * FROM t WHERE a = $1 AND c = 1 -- AND b = 1
    /// ```
    ///
    /// `AND b = 1` is commented out and the query matches more rows than written.
    /// PostgreSQL accepts it, so nothing fails loudly.
    ///
    /// A comment is a *comment about the fragment*, not SQL the fragment
    /// contributes, so it is blanked rather than rejected: the fragment keeps
    /// working and the hazard is gone. Only the comment is touched — a `--` or `/*`
    /// inside a string literal or a dollar-quoted body is data and survives, and a
    /// quote *inside* a comment is comment text, not a literal. See [`FragmentLex`].
    ///
    /// Whitespace is not trimmed: a fragment is spliced verbatim, so its leading and
    /// trailing spaces are the separators between it and the template around it.
    /// Blanking a comment must not also delete them — that is what would turn
    /// `WHERE#{F}` into `WHEREa = 1`. An unclosed `/*` is a separate matter — see
    /// [`fragment_comment_unterminated`].
    pub fn comments_blanked(&self) -> Option<&str> {
        self.has_comment.then_some(self.comments_blanked.as_str())
    }

    /// Whether a fragment is empty, or contributes nothing but whitespace.
    ///
    /// A fragment holding only a comment blanks away to nothing. Emitting it would
    /// splice an empty string where SQL was expected, so `WHERE #{NOTE}` becomes a
    /// bare `WHERE` — a syntax error at runtime, pointing at the template rather
    /// than at the fragment that caused it. Rejecting it names the real problem.
    ///
    /// Runs on the comment-blanked text, so this is what the fragment would actually
    /// contribute, not what was written.
    pub fn is_empty(&self) -> bool {
        self.comments_blanked.trim().is_empty()
    }

    /// Whether a fragment leaves a block comment unterminated.
    ///
    /// An unclosed `/*` cannot be blanked away: it would comment out the template
    /// text that follows the marker, and there is no end for the blanking to stop
    /// at. `--` needs no such rule — a line comment is closed by the end of the
    /// fragment, and blanking it removes it entirely.
    ///
    /// A `/*` inside a literal is data, and a `/*` inside another block comment
    /// nests, so both are resolved by the single scan in [`FragmentLex`].
    pub fn comment_unterminated(&self) -> bool {
        self.unterminated_block
    }

    /// Whether a fragment leaves a `'`, `"` or `$tag$` unclosed.
    ///
    /// The literal runs off the end of the fragment and into the template, where
    /// it swallows SQL the fragment never wrote — the same over-extension the
    /// backslash rule exists to prevent, only reaching past the marker instead
    /// of past the quote. Unlike an unclosed `/*` this cannot silently change
    /// which rows match; PostgreSQL rejects the statement. It is still rejected
    /// here, because a fragment is validated so that it is safe to splice
    /// anywhere, and this one is not: whether it parses depends on the template
    /// that happens to follow it.
    ///
    /// A quote inside a comment or a `$tag$` body is not a delimiter, so this
    /// reads the same single scan as every other check.
    pub fn literal_unterminated(&self) -> bool {
        self.unterminated_literal
    }

    /// Whether a fragment starts with `AND`/`OR`, which the template must own.
    ///
    /// A fragment marker is opaque, so codegen cannot lift a joiner out of it the
    /// way [`lift_joiners_after_optionals`] does for literal text. When the optional
    /// predicate before such a fragment drops, its `WHERE` goes with it and the
    /// fragment's own `AND` is left dangling:
    ///
    /// ```text
    /// query!("SELECT * FROM t WHERE a = ${?x} #{AND_B}")   // x = None
    /// -> SELECT * FROM t AND b = 1
    /// ```
    ///
    /// Writing the joiner in the template instead — `WHERE a = ${?x} AND #{B}` —
    /// puts it where the scanner can see it, and the `WHERE` is handed over
    /// correctly. Nothing is lost: the joiner belongs to how the fragment is
    /// *combined*, not to the fragment.
    ///
    /// Matched on the [`FragmentLex`] structure view, so a leading comment does not
    /// hide the keyword and a leading literal cannot fake one.
    pub fn starts_with_joiner(&self) -> Option<&'static str> {
        let head = self.structure.trim_start();
        ["AND", "OR"].into_iter().find(|kw| {
            head.strip_prefix(*kw).is_some_and(|rest| {
                // A word boundary is required, or a column named `android` would
                // look like a leading `AND`.
                rest.is_empty()
                    || !(rest.as_bytes()[0].is_ascii_alphanumeric() || rest.as_bytes()[0] == b'_')
            })
        })
    }

    /// Whether a fragment's brackets fail to balance within it.
    ///
    /// A fragment is spliced verbatim into the template, so an unmatched `(` or `)`
    /// does not just break the fragment's own SQL — it reaches into the template's
    /// nesting, where a `)` can close a construct the fragment never opened and the
    /// clause map is built on brackets the final SQL does not have. `a = 1) AND (b =
    /// 2` is the shape: bracket counts balance, yet the first `)` closes the
    /// template's.
    ///
    /// Clause keywords are deliberately *not* rejected: a fragment *may* contain
    /// `UNION`, `HAVING` and friends, because how deep the fragment lands is a
    /// property of the template, not of the fragment, so the two cannot be judged
    /// apart. A body like `SELECT .. UNION ALL SELECT ..` is exactly what belongs in
    /// `WITH t AS (#{body})`. The cost is documented instead — see the "fragments
    /// and optional predicates" section of the crate docs — and it produces SQL
    /// Postgres rejects, not SQL that silently means something else.
    ///
    /// Brackets are matched on the [`FragmentLex`] structure view, so a bracket
    /// inside a string literal, a dollar-quoted body or a SQL comment is data and
    /// does not count.
    ///
    /// Returns `true` when the fragment must be rejected.
    pub fn brackets_unbalanced(&self) -> bool {
        let bytes = self.structure.as_bytes();
        let mut depth = 0i32;

        for &b in bytes {
            match b {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    // Negative means this `)` closes a bracket belonging to the
                    // template. Checking only the final balance would pass
                    // `a = 1) AND (b = 2`.
                    if depth < 0 {
                        return true;
                    }
                }
                _ => {}
            }
        }
        depth != 0
    }
}

/// Convenience wrappers over [`FragmentLex`], one scan each.
///
/// `sql_fragment!` scans once and calls the methods; these exist for tests and
/// for callers checking a single property, where the extra scan costs nothing
/// and reads better than threading the lexer through.
#[cfg(test)]
mod one_shot {
    use super::FragmentLex;

    pub fn fragment_comments_blanked(sql: &str) -> Option<String> {
        FragmentLex::scan(sql).comments_blanked().map(str::to_owned)
    }

    pub fn fragment_is_empty(sql: &str) -> bool {
        FragmentLex::scan(sql).is_empty()
    }

    pub fn fragment_comment_unterminated(sql: &str) -> bool {
        FragmentLex::scan(sql).comment_unterminated()
    }

    pub fn fragment_literal_unterminated(sql: &str) -> bool {
        FragmentLex::scan(sql).literal_unterminated()
    }

    pub fn fragment_starts_with_joiner(sql: &str) -> Option<&'static str> {
        FragmentLex::scan(sql).starts_with_joiner()
    }

    pub fn fragment_brackets_unbalanced(sql: &str) -> bool {
        FragmentLex::scan(sql).brackets_unbalanced()
    }
}

/// Assigns a clause index to every byte offset of the template.
///
/// Index 0 covers everything before the first predicate list; each boundary
/// keyword at bracket depth zero starts the next index. Nested keywords (a
/// subquery's `WHERE`) keep the surrounding clause's index, because their
/// predicates join inside the subquery, not across it.
///
/// Returns a sorted list of `(offset, clause)` starts; use [`clause_at`].
///
/// Takes the [`strip_literals`] view of the template, not the template itself.
fn clause_map(upper: &str) -> Vec<(usize, u32)> {
    let bytes = upper.as_bytes();
    let mut starts = vec![(0usize, 0u32)];
    let mut clause = 0u32;
    let mut depth = 0i32;
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
            }
            // A statement separator starts a new list wherever it appears.
            b';' => {
                clause += 1;
                starts.push((i + 1, clause));
                depth = 0;
                i += 1;
            }
            b if b.is_ascii_alphabetic() => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                // The `depth == 0` guard currently has no *observable* effect,
                // and that is a coincidence of how introducers are built, not a
                // reason to drop it. Codegen records an introducer only for a
                // predicate whose own joiner is `WHERE`/`HAVING`; a predicate
                // separated from an earlier one by a nested boundary always
                // carries the written `AND` instead, so its clause has no
                // introducer either way and `open` emits `AND` in both. Deleting
                // this guard therefore fails no test in the suite. It becomes
                // load-bearing again the moment an introducer can be recorded
                // for a clause a nested boundary opened — so keep it, and do not
                // read the passing suite as evidence it is dead.
                if depth == 0 && CLAUSE_BOUNDARIES.contains(&&upper[start..i]) {
                    clause += 1;
                    // The keyword itself belongs to the clause it opens, so a
                    // predicate it introduces joins inside that clause.
                    starts.push((start, clause));
                }
            }
            _ => i += 1,
        }
    }
    starts
}

/// The clause index covering `offset`, per [`clause_map`].
fn clause_at(starts: &[(usize, u32)], offset: usize) -> u32 {
    starts
        .partition_point(|(at, _)| *at <= offset)
        .checked_sub(1)
        .map_or(0, |idx| starts[idx].1)
}

/// Upper-cases `sql` and blanks out string literals and quoted identifiers so
/// keyword scanning does not trip over data.
///
/// **Byte offsets are preserved**: each input byte maps to exactly one output
/// byte, so an index found in the result also indexes the source. Callers rely
/// on this to cut the original text at a position found here. Multi-byte
/// characters therefore become runs of spaces, one per byte.
fn strip_literals(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut out = vec![b' '; bytes.len()];
    let mut i = 0;

    while i < bytes.len() {
        // Interpolation markers hold *Rust*, not SQL, and escapes are literal
        // text. Skipping both keeps the offsets of real SQL keywords correct:
        // otherwise an escape's `$${` would be scanned as SQL and shift the
        // clause it opens, and an apostrophe in a marker's text (a lifetime, a
        // char literal) would open a string literal that never closes.
        if let Some(end) = marker_span(sql, i) {
            i = end;
            continue;
        }
        match bytes[i] {
            // Dollar-quoted literal: `$tag$ ... $tag$`. Its body is arbitrary
            // text, so brackets and quotes inside it must not be seen.
            b'$' => match dollar_tag(bytes, i) {
                Some(tag) => {
                    i = find_subslice(bytes, i + tag, &bytes[i..i + tag])
                        .map_or(bytes.len(), |at| at + tag);
                }
                None => {
                    out[i] = b'$';
                    i += 1;
                }
            },
            // `'...'` / `"..."`. The body is data: a keyword, bracket or
            // comment marker inside it is not structure. [`quoted_end`] decides
            // where it ends, and is shared with [`FragmentLex`] so the two views
            // cannot disagree about that.
            b'\'' | b'"' => {
                // An `E`/`e` prefix belongs to the literal, not to the SQL
                // around it, so it is blanked with it.
                if opens_extended_string(bytes, i) {
                    out[i - 1] = b' ';
                }
                // An unclosed literal blanks to the end: text that never closes
                // cannot be read as structure.
                i = quoted_end(bytes, i).unwrap_or(bytes.len());
            }
            b if b.is_ascii() => {
                out[i] = b.to_ascii_uppercase();
                i += 1;
            }
            // A non-ASCII byte cannot be part of a keyword; blanking it keeps
            // the result ASCII and the offsets aligned.
            _ => i += 1,
        }
    }
    String::from_utf8(out).expect("all bytes are ASCII by construction")
}

/// Whether the quote at `at` opens an *extended* string, where `\` escapes the
/// next character.
///
/// Verified against PostgreSQL 16 with the default
/// `standard_conforming_strings = on`:
///
/// - `select 'a\'` yields `a\` — in an ordinary literal a backslash is an
///   ordinary character, so that literal is *complete*;
/// - `select E'a\' -- c'` yields `a' -- c` — here `\'` really is an escaped
///   quote and the `--` is data;
/// - `select text'a\'` yields `a\` — a *type-prefixed* literal does not escape,
///   so the `E` must be a standalone token, not the tail of an identifier;
/// - `U&'a\'` raises "invalid Unicode escape" — there `\` introduces a codepoint
///   (`\0041`), never a quote escape, so it needs no handling here.
///
/// Quoted identifiers (`"..."`) never escape with a backslash under any setting.
fn opens_extended_string(bytes: &[u8], at: usize) -> bool {
    if bytes[at] != b'\'' || at == 0 {
        return false;
    }
    if !bytes[at - 1].eq_ignore_ascii_case(&b'E') {
        return false;
    }
    // `text'a\'` is a type-prefixed literal and does not escape, so the `E` only
    // counts as a prefix when it stands alone. A non-ASCII byte counts as
    // identifier material too: it cannot be established that the `E` stands
    // alone, and guessing wrong here would over-extend the literal and hide
    // whatever follows it.
    if at < 2 {
        return true;
    }
    let prev = bytes[at - 2];
    !(prev.is_ascii_alphanumeric() || prev == b'_' || !prev.is_ascii())
}

/// Advances past the quoted literal or identifier opening at `at`, returning the
/// offset just past its closing quote, or `None` if it never closes.
///
/// Shared by [`FragmentLex`] and [`strip_literals`] so the two cannot drift: the
/// whole point of both is that structure outside a literal is found reliably,
/// and that fails if they disagree on where a literal ends.
///
/// A doubled quote (`''`, `""`) is an escaped quote and stays inside. A
/// backslash escapes the next character **only** in an extended string — see
/// [`opens_extended_string`] for why, and for the PostgreSQL behaviour this
/// mirrors. Treating `\` as an escape everywhere would let `'a\'` swallow the
/// SQL after it, hiding a comment, a bracket or an unterminated `/*` from every
/// check that runs on these views.
///
/// `None` is returned separately from `Some(bytes.len())` — a literal closing on
/// the very last byte is well-formed, one that never closes is not, and only the
/// caller knows whether that distinction matters to it.
fn quoted_end(bytes: &[u8], at: usize) -> Option<usize> {
    let q = bytes[at];
    let escapes = opens_extended_string(bytes, at);
    let mut i = at + 1;
    while i < bytes.len() {
        if escapes && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == q {
            if bytes.get(i + 1) == Some(&q) {
                i += 2;
                continue;
            }
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// If a marker or escape starts at `at`, the offset just past it.
///
/// Escapes (`$${`, `##{`) are three bytes of literal text; a real marker runs to
/// its closing brace, which [`find_close`] locates by Rust nesting rules.
///
/// Both must be skipped for [`clause_map`] to assign the right clause: without
/// the escape case, `$${?x} WHERE a = ${?y}` would put the predicate in clause 0
/// instead of the clause its `WHERE` opens.
fn marker_span(sql: &str, at: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    if bytes[at + 1..].starts_with(b"${") && bytes[at] == b'$'
        || bytes[at + 1..].starts_with(b"#{") && bytes[at] == b'#'
    {
        return Some(at + 3);
    }
    if matches!(bytes[at], b'$' | b'#') && bytes[at + 1..].starts_with(b"{") {
        // An unterminated marker is reported later by `parse_template`; here it
        // just means the rest of the template is marker text.
        return Some(find_close(sql, at + 2).map_or(bytes.len(), |end| end + 1));
    }
    None
}

/// Length of the opening `$tag$` sequence at `at`, if one starts there.
///
/// The tag is empty (`$$`) or a single identifier name; `$1` and `$foo bar` are
/// not tags.
fn dollar_tag(bytes: &[u8], at: usize) -> Option<usize> {
    let mut i = at + 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        // A tag cannot start with a digit — that is what stops `$1` matching.
        if bytes[i].is_ascii_digit() && i == at + 1 {
            return None;
        }
        i += 1;
    }
    (bytes.get(i) == Some(&b'$')).then_some(i + 1 - at)
}

/// First occurrence of `needle` in `haystack` starting at `from`.
fn find_subslice(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || from + needle.len() > haystack.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

/// Makes the leading `AND`/`OR` of any text following an optional predicate
/// conditional too.
///
/// Without this, `WHERE a = ${?x} AND b IS NULL` emits a dangling
/// `AND b IS NULL` when `x` is `None`: the `WHERE` belonged to the dropped
/// predicate, but the literal `AND` was written anyway. By lifting the keyword
/// out, `Predicates` picks between `WHERE` and `AND` based on what actually
/// survived.
fn lift_joiners_after_optionals(parts: Vec<Part>) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::with_capacity(parts.len());
    // `Some(clause)` while the previous part was an optional predicate; the
    // lifted text joins inside that same clause.
    let mut after_optional: Option<u32> = None;

    for part in parts {
        match part {
            Part::Text(text) if after_optional.is_some() => {
                let clause = after_optional.take().expect("checked by the guard");
                match strip_leading_joiner(&text) {
                    Some((joiner, rest)) => out.push(Part::Joined {
                        joiner,
                        text: rest,
                        clause,
                    }),
                    None => out.push(Part::Text(text)),
                }
            }
            Part::OptBind(pred) => {
                after_optional = Some(pred.clause);
                out.push(Part::OptBind(pred));
            }
            other => {
                // A bind or fragment right after an optional predicate carries
                // no joiner of its own, so there is nothing to lift.
                out.push(other);
                after_optional = None;
            }
        }
    }
    out
}

/// Strips a leading `AND`/`OR` (ignoring whitespace) from `text`, returning the
/// keyword as written and the remainder.
fn strip_leading_joiner(text: &str) -> Option<(String, String)> {
    let trimmed = text.trim_start();
    for kw in ["AND", "OR"] {
        if trimmed.len() >= kw.len() && trimmed[..kw.len()].eq_ignore_ascii_case(kw) {
            let rest = &trimmed[kw.len()..];
            // Must be a whole word: `ORDER BY` must not match `OR`.
            if rest
                .as_bytes()
                .first()
                .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_')
            {
                return Some((trimmed[..kw.len()].to_string(), rest.to_string()));
            }
        }
    }
    None
}

/// Result of cutting an optional predicate out of the accumulated literal
/// text.
struct SplitPredicate {
    predicate: OptPredicate,
    /// How much of the accumulated text belongs to the *mandatory* SQL before
    /// the predicate; the caller truncates to this length.
    text_len: usize,
}

/// Keywords that can introduce a top-level predicate.
///
/// `HAVING` introduces a predicate list with the same removal semantics as
/// `WHERE`; [`clause_map`] counts it as its own clause, so a `HAVING` predicate
/// and a `WHERE` predicate never share joiner bookkeeping — a template may carry
/// both.
const JOINERS: [&str; 4] = ["AND", "OR", "WHERE", "HAVING"];

/// Finds an opening `--` or `/*` comment, returning its byte offset.
///
/// Takes a [`strip_literals`] view, never raw SQL: both callers pass one, and
/// that is what makes `WHERE note = 'a--b'` safe here — the literal is already
/// blanked, so this function needs no notion of quoting at all. Deciding where a
/// literal ends is [`quoted_end`]'s job, in one place, for every view.
fn find_comment(sql: &str) -> Option<usize> {
    let bytes = sql.as_bytes();
    (0..bytes.len()).find(|&i| {
        (bytes[i] == b'-' && bytes[i + 1..].starts_with(b"-"))
            || (bytes[i] == b'/' && bytes[i + 1..].starts_with(b"*"))
    })
}

/// Keywords that end the predicate the marker sits in. Everything from one of
/// them onward is mandatory SQL, never part of the optional predicate.
/// `FOR` covers the locking clauses (`FOR UPDATE`, `FOR SHARE`); `ON` covers
/// `ON CONFLICT`. Both are safe to list only because the scan runs over the
/// [`strip_literals`] view: on raw text, every keyword here is also a false
/// positive waiting inside some string literal.
const CLAUSE_ENDS: [&str; 13] = [
    "GROUP",
    "ORDER",
    "LIMIT",
    "OFFSET",
    "HAVING",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "RETURNING",
    "FETCH",
    "FOR",
    "WINDOW",
    "ON",
];

/// Captures the part of the predicate sitting *right* of the marker, e.g. the
/// `::uuid` in `a = ${?x}::uuid` or the `|| '%'` in `a LIKE ${?x} || '%'`.
///
/// Without this, the trailing text is emitted unconditionally and grafts onto
/// whatever preceded the dropped predicate (`SELECT * FROM t::uuid`).
///
/// The tail runs to the end of the predicate: the next top-level `AND`/`OR`, a
/// clause keyword from [`CLAUSE_ENDS`], or the `)`/`;` closing the surrounding
/// construct. Returns the tail's byte length within `rest`, excluding any
/// trailing whitespace — that belongs to the mandatory text, so the separator
/// before the next keyword survives a dropped predicate.
///
/// "Top-level" is relative to the marker: a group opened inside the tail is part
/// of the operand, so neither its `)` nor a keyword inside it ends the predicate.
/// `coalesce(tax, 0)` and `IN (SELECT 1 UNION SELECT 2)` are taken whole. A `)`
/// at depth zero closes a group opened *before* the marker and does end the
/// tail, which is what keeps `EXISTS (SELECT 1 WHERE k = ${?x})` working.
///
/// Every token above is recognised on the [`strip_literals`] view, never on raw
/// text: a `)` or an `AND` inside a string literal, a quoted identifier or a
/// dollar-quoted body is data, and stopping there would cut the predicate
/// mid-literal — emitting a dangling joiner and an unterminated quote.
fn predicate_tail(rest: &str) -> Tail {
    // Never scan past another interpolation: its text is not ours, and
    // swallowing it would silently turn a bind into literal SQL — or worse, cut
    // an escape so that `$${z}` loses its leading `$` and the rest is re-read as
    // a live marker, yielding SQL that Postgres accepts with the wrong string
    // inside.
    let limit = next_marker(rest).unwrap_or(rest.len());
    // Offsets in the stripped view index `rest` unchanged, so the end found
    // there cuts the original bytes.
    let (end, stop) = predicate_tail_end(&strip_literals(&rest[..limit]));
    let end = end.min(limit);
    // Whether an interpolation capped the scan at all. `next_marker` returns
    // `None` when the tail holds none, and `unwrap_or` then sets `limit` to
    // `rest.len()` — so this is what separates "no interpolation" from "capped by
    // one", and dropping it rejects every template whose tail simply ends.
    let capped = limit < rest.len();
    // A bare `#{...}` at the top level of the tail is exempt: a fragment's SQL
    // is opaque here, and it may *be* the boundary — `WHERE a = ${?x}
    // #{ORDER_BY_ID}` is a documented, working pattern. Whether it is instead a
    // continuation (`#{"|| 'x'"}`) cannot be decided from the template, and may
    // be chosen at runtime, so it stays under the fragment constraint the crate
    // documents rather than enforces. Nested in a group the tail opened it can
    // be neither: `f(#{F})` is positionally inside the predicate whatever the
    // fragment says. Escapes are never exempt — `##{` is literal text, not SQL.
    let exempt_fragment = matches!(stop, TailStop::EndOfText) && rest[limit..].starts_with("#{");
    Tail {
        len: rest[..end].trim_end().len(),
        split_by_interpolation: capped && !matches!(stop, TailStop::Boundary) && !exempt_fragment,
    }
}

/// Why [`predicate_tail_end`] stopped scanning.
///
/// The caller caps the scan at the next interpolation, so where it stopped
/// decides whether that interpolation belongs to this predicate.
enum TailStop {
    /// A depth-zero `)`/`]`/`;` or a top-level clause keyword. The predicate
    /// ends here and anything after it stands on its own.
    Boundary,
    /// The scan ran out of text without a boundary, which means it reached the
    /// interpolation that capped it — or the genuine end of the template.
    EndOfText,
    /// A group opened inside the tail never closed, so the scan ended within it.
    /// Whatever capped the scan is nested in that group, and therefore inside
    /// this predicate.
    InsideGroup,
}

/// The trailing text of a predicate, plus whether capturing it was possible.
struct Tail {
    /// Byte length within `rest`, excluding trailing whitespace.
    len: usize,
    /// Whether another `${...}`/`#{...}` marker — or a `$${`/`##{` escape — sits
    /// inside this predicate rather than after it.
    ///
    /// The tail stops at the first interpolation either way, because its text is
    /// not ours to copy. Where it stopped is what distinguishes the two cases: a
    /// boundary seen first means the interpolation belongs to the SQL *after*
    /// the predicate and both parts stand on their own. No boundary means the
    /// predicate straddles it, and the predicate cannot be emitted or removed as
    /// one unit — `a = ${?x} || ${y}` would drop to `SELECT * FROM t $1`.
    split_by_interpolation: bool,
}

/// Offset of the next `${`/`#{` marker **or `$${`/`##{` escape**.
///
/// The tail is copied verbatim into the generated SQL, so it must not contain an
/// escape: it is the main scanner that turns `$${` into a literal `${`, and text
/// taken as a tail never reaches it. Stopping at an escape leaves it to the main
/// loop, which keeps `'$${z}'` rendering as `'${z}'` whether or not an optional
/// precedes it. Cutting through it instead would lose the leading sigil and
/// re-read the remainder as a live marker — SQL that Postgres accepts with the
/// wrong string inside.
fn next_marker(rest: &str) -> Option<usize> {
    // `$` and `#` are ASCII, so a candidate is always on a char boundary and
    // [`marker_span`] can be asked directly — it is the single source of truth
    // for which prefixes are escapes and which are markers.
    let bytes = rest.as_bytes();
    (0..bytes.len()).find(|&i| matches!(bytes[i], b'$' | b'#') && marker_span(rest, i).is_some())
}

/// Scans to the first byte past the predicate; see [`predicate_tail`].
///
/// Takes the [`strip_literals`] view of the tail, not the tail itself: keyword
/// and bracket matching must not see literal data. Offsets are preserved by that
/// view, so the returned index also indexes the source.
///
/// Returns the offset and whether the scan stopped at a *boundary* — a
/// depth-zero `)`/`]`/`;`, a depth-zero clause keyword, or a group opened before
/// the marker. `false` means it ran to the end of its input, which the caller
/// capped at the next interpolation: the predicate straddles that interpolation
/// and cannot be removed as one unit.
fn predicate_tail_end(stripped: &str) -> (usize, TailStop) {
    let bytes = stripped.as_bytes();
    let mut i = 0;
    // Groups opened *within* the tail. A `)` or a keyword only ends the
    // predicate at depth zero: inside `coalesce(tax, 0)` or
    // `IN (SELECT 1 UNION SELECT 2)` both are part of the operand, and stopping
    // there cut the group in half — leaving the `)` as mandatory text, so a
    // dropped predicate emitted `SELECT * FROM t)`.
    let mut depth = 0u32;
    // Where a group opened *inside* the tail that never closes began. Only a
    // depth-zero `(` records one, and the scan starts at depth zero relative to
    // the marker, so this is always a group the tail itself opened — a group
    // opened before the marker closes through the depth-zero `)` branch below.
    // The tail ends where such a group began: text past an unbalanced `(` cannot
    // be taken as the operand, since dropping it would leave the `(` behind.
    let mut unclosed_at = None;

    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' => {
                if depth == 0 {
                    unclosed_at = Some(i);
                }
                depth += 1;
                i += 1;
                continue;
            }
            // A `)` below our own depth closes a group opened before the
            // marker, so it is where the surrounding construct resumes.
            b')' | b']' if depth == 0 => return (i, TailStop::Boundary),
            b')' | b']' => {
                depth -= 1;
                if depth == 0 {
                    unclosed_at = None;
                }
                i += 1;
                continue;
            }
            // A statement separator ends the predicate at any depth: it cannot
            // legally appear inside a group, so treating it as an operand would
            // only ever extend the tail over SQL that is not ours.
            b';' => return (i, TailStop::Boundary),
            b if b.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            _ => {}
        }
        // At a word start, check whether that word ends the predicate.
        let start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && !matches!(bytes[i], b'(' | b')' | b'[' | b']' | b';')
        {
            i += 1;
        }
        let word = &stripped[start..i];
        // Only a top-level keyword ends the predicate. A `UNION` inside a
        // subquery in the tail belongs to that subquery, not to the clause the
        // marker sits in.
        if depth == 0 && (JOINERS.contains(&word) || CLAUSE_ENDS.contains(&word)) {
            return (start, TailStop::Boundary);
        }
    }
    // An unbalanced `(` is *not* a boundary. The group was opened inside the
    // tail, so the interpolation that capped the scan sits within it and within
    // this predicate: `a = ${?x} + f(${y})` dropped to `SELECT * FROM t($1)`.
    // Reporting a boundary here declared the predicate separable when it is not.
    // `len` still cuts at the `(`, which is what keeps the no-interpolation case
    // (`a = ${?x} + f(b`) from swallowing text it does not own.
    match unclosed_at {
        Some(at) => (at, TailStop::InsideGroup),
        None => (stripped.len(), TailStop::EndOfText),
    }
}

/// Splits the SQL accumulated so far so the trailing predicate — the one the
/// `${?...}` marker belongs to — can be emitted conditionally.
///
/// The predicate must be introduced by `AND`, `OR` or `WHERE`. That restriction
/// is what makes removal well defined: anywhere else (inside a `BETWEEN`, a
/// function call, or a `SELECT` list) there is no way to know how much text to
/// drop without parsing SQL, so such positions are rejected outright.
fn split_predicate(pending: &str, expr: String, span: Span) -> Result<SplitPredicate, ParseError> {
    // A comment would make the joiner search wrong in both directions:
    // `-- and` yields a phantom joiner, and the newline closing the comment is
    // consumed as the space before a joiner, which silently comments out the
    // surviving predicate. Recognising comments properly needs a SQL lexer, so
    // reject them.
    // Strip literals and quoted identifiers before scanning: an `or` inside
    // `'p or q'` or `"c or d"` would otherwise be picked as the joiner, and the
    // cut would land mid-token, leaving an unclosed quote. Offsets are preserved
    // because each character is replaced by exactly one space. The same view
    // answers the comment question: a `--` inside a literal or a dollar-quoted
    // body is data, and scanning raw text rejected `s = $tag$--$tag$ AND ..`.
    let upper = strip_literals(pending);

    if let Some(at) = find_comment(&upper) {
        return Err(ParseError::new(format!(
            "SQL comments are not supported in a template using `${{?...}}`, but \
             `{}` appears before the marker.\n\
             A comment can hide an `AND`/`OR` or swallow the predicate itself, \
             silently changing which rows match.\n\
             Remove the comment or build this query without `${{?...}}`.",
            if upper[at..].starts_with("--") {
                "--"
            } else {
                "/*"
            }
        )));
    }

    // Find the last joiner keyword, matching on word boundaries.
    let mut best: Option<(usize, &str)> = None;
    for joiner in JOINERS {
        let mut from = 0;
        while let Some(rel) = upper[from..].find(joiner) {
            let at = from + rel;
            let end = at + joiner.len();
            let before_ok = at == 0
                || !upper.as_bytes()[at - 1].is_ascii_alphanumeric()
                    && upper.as_bytes()[at - 1] != b'_';
            let after_ok = end >= upper.len()
                || !upper.as_bytes()[end].is_ascii_alphanumeric() && upper.as_bytes()[end] != b'_';
            if before_ok && after_ok && best.is_none_or(|(b, _)| at > b) {
                best = Some((at, joiner));
            }
            from = end;
        }
    }

    let (at, joiner) = best.ok_or_else(|| {
        ParseError::new(
            "`${?...}` must sit in a predicate introduced by `WHERE`, `AND` or `OR`.\n\
             No such keyword precedes this marker, so there is no way to tell \
             which SQL to remove when the value is `None`.\n\
             Use a plain bind `${...}` or restructure the query so the optional \
             condition is its own `AND` clause.",
        )
    })?;

    let after_joiner = &pending[at + joiner.len()..];
    // Structure is matched on the stripped view, never on `after_joiner`: a
    // bracket or comma inside a string literal, a quoted identifier or a
    // dollar-quoted body is data, and stopping there rejects valid SQL such as
    // `coalesce(a, ')') = ${?v}`. Offsets are preserved, so both slices index
    // the same positions.
    let after_joiner_upper = &upper[at + joiner.len()..];

    // The marker must sit at bracket depth zero relative to the joiner. An open
    // group means the marker is an argument or inside a group (`ANY(${?x})`,
    // `make_interval(days => ${?d})`, `(a = ${?x} OR b)`); a comma at depth zero
    // means the predicate is one element of a list. A *balanced* group is fine:
    // it is just the left operand, as in `HAVING count(*) >= ${?n}`.
    let mut depth = 0i32;
    let mut offence = None;
    for (idx, ch) in after_joiner_upper.char_indices() {
        match ch {
            // Subscripts count toward depth like parentheses do: the comma in
            // `b[1,2] = ${?x}` delimits array bounds, not a predicate list, and
            // reading it as top-level rejected valid SQL.
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                // Closing below the joiner means the group was opened before
                // it, so the predicate is not the whole clause.
                if depth < 0 {
                    offence = Some((idx, ch));
                    break;
                }
            }
            ',' if depth == 0 => {
                offence = Some((idx, ','));
                break;
            }
            _ => {}
        }
    }
    if offence.is_none() && depth > 0 {
        offence = after_joiner_upper
            .find(['(', '['])
            .map(|idx| (idx, after_joiner_upper[idx..].chars().next().unwrap()));
    }
    if let Some((_, ch)) = offence {
        return Err(ParseError::new(format!(
            "`${{?...}}` must be a whole top-level predicate, but `{ch}` appears \
             between `{joiner}` and the marker.\n\
             Removing the predicate would leave unbalanced SQL. Move the optional \
             condition into its own `AND` clause or use a plain bind `${{...}}`.",
        )));
    }

    // The marker must be the predicate's right operand, i.e. the text between
    // the joiner and the marker must end in a comparison operator. That is what
    // rejects `BETWEEN ${?lo} AND ${?hi}`: `BETWEEN` needs a second operand, and
    // removing either half leaves `WHERE age BETWEEN $1` or `WHERE $1`. Symbol
    // operators need no word boundary; `LIKE`/`ILIKE` do, otherwise a column
    // named `dislike` would pass the check and emit `WHERE dislike $1`.
    const SYMBOL_OPS: [&str; 7] = ["=", "<>", "!=", "<", ">", "<=", ">="];
    const WORD_OPS: [&str; 2] = ["LIKE", "ILIKE"];
    let body = after_joiner.trim_end();
    // `upper` already upper-cases and blanks literals, so an operator inside a
    // literal cannot be mistaken for the predicate's own.
    let body_upper = after_joiner_upper[..body.len()].to_string();
    let ends_with_symbol = SYMBOL_OPS.iter().any(|op| body_upper.ends_with(op));
    let ends_with_word = WORD_OPS.iter().any(|op| {
        body_upper.strip_suffix(op).is_some_and(|head| {
            head.as_bytes()
                .last()
                .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_')
        })
    });
    if !ends_with_symbol && !ends_with_word {
        let operand = body.split_whitespace().last().unwrap_or(body);
        return Err(ParseError::new(format!(
            "`${{?...}}` must follow a comparison operator, but follows \
             `{operand}`.\n\
             Supported: = <> != < > <= >= LIKE ILIKE\n\
             Only predicates of the form `col = ${{?x}}` are unambiguously \
             removable: `BETWEEN ${{?lo}} AND ${{?hi}}` would leave \
             `WHERE age BETWEEN $1`, and `LIMIT`/`OFFSET`/`ORDER BY` are not \
             predicates at all.\n\
             Make the value unconditional via `COALESCE(${{x}}, <default>)`, use a \
             plain bind `${{...}}`, or give the condition its own `AND` clause.",
        )));
    }

    // `WHERE` is emitted unconditionally when other predicates follow it; only a
    // trailing `AND`/`OR` is safe to fold into the optional part. Treating
    // `WHERE` as the joiner is correct only when it is the sole predicate, which
    // is not known yet — so keep it and let codegen handle the empty-WHERE case.
    // Also take the whitespace indenting the joiner, so a removed predicate
    // leaves no blank line or double space. The surviving separator is re-emitted
    // as a single space by `Predicates::open`.
    let mut cut = at;
    while cut > 0 && pending.as_bytes()[cut - 1].is_ascii_whitespace() {
        cut -= 1;
    }

    Ok(SplitPredicate {
        predicate: OptPredicate {
            expr,
            // Filled in by the caller, which knows the marker's offset.
            clause: 0,
            joiner: pending[at..at + joiner.len()].to_string(),
            // Normalise the gap after the keyword to a single space, but keep
            // the trailing space before the bind — `name ILIKE ${?x}` needs it.
            before: format!(" {}", after_joiner.trim_start()),
            after: String::new(),
            span,
        },
        text_len: cut,
    })
}

/// Finds the `}` closing the interpolation that starts at `from`.
///
/// Tracks `{}`, `()`, `[]` nesting and skips string/char literals so expressions
/// like `${map["a}b"]` or `${S { x: 1 }.x}` end at the right brace.
fn find_close(input: &str, from: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut depth = 0usize;
    let mut i = from;

    while i < bytes.len() {
        // Rust comments first: a `}` or a quote inside one is text, and without
        // this `${/* } */ 1}` cut at the wrong brace and reported the valid Rust
        // `/*` as an invalid expression.
        if bytes[i] == b'/' && bytes[i + 1..].starts_with(b"/") {
            // A line comment cannot be closed inside a single-line string
            // literal, so the interpolation would be unterminated anyway; stop
            // at the newline and let the normal scan continue.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && bytes[i + 1..].starts_with(b"*") {
            // Rust block comments nest.
            let mut nesting = 0usize;
            while i < bytes.len() {
                if bytes[i] == b'/' && bytes[i + 1..].starts_with(b"*") {
                    nesting += 1;
                    i += 2;
                    continue;
                }
                if bytes[i] == b'*' && bytes[i + 1..].starts_with(b"/") {
                    nesting -= 1;
                    i += 2;
                    if nesting == 0 {
                        break;
                    }
                    continue;
                }
                i += 1;
            }
            // An unterminated block comment consumed the rest: no closing brace.
            if nesting != 0 {
                return None;
            }
            continue;
        }
        match bytes[i] {
            b'"' => i = skip_string(input, i)?,
            b'\'' => {
                // May be a char literal or a lifetime/label. Treat it as a
                // literal only if it actually closes at the same nesting
                // level.
                match skip_char_literal(input, i) {
                    Some(next) => i = next,
                    None => i += 1,
                }
            }
            b'{' | b'(' | b'[' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' => {
                depth = depth.checked_sub(1)?;
                i += 1;
            }
            b'}' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Whether the byte at `at` starts a token, i.e. is not preceded by an
/// identifier character.
///
/// Distinguishes the raw-string prefix in `r"x"` from an identifier's trailing
/// `r` in `var"x"`. `b` is allowed, since `br"x"` is a raw byte string.
fn starts_a_token(bytes: &[u8], at: usize) -> bool {
    if at == 0 {
        return true;
    }
    let prev = bytes[at - 1];
    if prev == b'b' {
        return starts_a_token(bytes, at - 1);
    }
    !prev.is_ascii_alphanumeric() && prev != b'_'
}

/// Skips a `"..."` or `r#"..."#` literal starting at `start`; returns the index
/// just past it.
fn skip_string(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    // Raw strings: count the hashes before the quote.
    let mut hashes = 0;
    let mut k = start;
    while k > 0 && bytes[k - 1] == b'#' {
        hashes += 1;
        k -= 1;
    }
    // `r"..."` is raw with zero hashes, so the hash count alone cannot be the
    // gate: without this check the `\\` in `r"a\"` would be treated as an escape
    // and eat the closing quote, and `${ pat.replace(r"\", r"\\") }` would not
    // parse.
    let is_raw = k > 0 && bytes[k - 1] == b'r' && starts_a_token(bytes, k - 1);

    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if !is_raw => i += 2,
            b'"' => {
                if !is_raw {
                    return Some(i + 1);
                }
                let close = &bytes[i + 1..];
                if close.len() >= hashes && close[..hashes].iter().all(|&b| b == b'#') {
                    return Some(i + 1 + hashes);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Skips a char literal `'x'` / `'\n'`. Returns `None` if the quote is not a
/// char literal (a lifetime such as `'static`).
fn skip_char_literal(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut i = start + 1;
    if i >= bytes.len() {
        return None;
    }
    if bytes[i] == b'\\' {
        i += 2;
    } else {
        i += input[i..].chars().next()?.len_utf8();
    }
    if bytes.get(i) == Some(&b'\'') {
        Some(i + 1)
    } else {
        None
    }
}

#[cfg(test)]
mod fragment_checks {
    use super::one_shot::*;
    use super::FragmentLex;

    #[test]
    fn balanced_fragments_are_accepted() {
        // Every fragment shipped in this repo's tests, examples and README,
        // plus the CTE-body shape meant for `WITH t AS (#{body})`.
        for sql in [
            "deleted_at IS NULL",
            "deleted_at IS NULL AND published",
            "t.kind = 'template' AND t.deleted_at IS NULL",
            "true",
            "t.published",
            "r.data->>'locale'",
            "created_at DESC, id DESC",
            "ORDER BY name ASC",
            "LEFT JOIN profiles p ON p.user_id = u.id",
            "CREATE DATABASE \"cms_test_fixed\"",
            "'document'",
            "a IN (SELECT x FROM u WHERE y = 1)",
            "WITH active AS (SELECT id FROM u WHERE deleted_at IS NULL)",
        ] {
            assert!(!fragment_brackets_unbalanced(sql), "should accept {sql:?}");
        }
    }

    #[test]
    fn a_clause_boundary_is_not_rejected() {
        // How deep a fragment lands is a property of the template, so a
        // boundary cannot be judged from the fragment alone. A CTE body is the
        // motivating case: it is top-level *within the fragment*, but the
        // template wraps it in `WITH t AS (...)`.
        for sql in [
            "SELECT id FROM t WHERE parent IS NULL \
             UNION ALL \
             SELECT c.id FROM t c JOIN tree ON c.parent = tree.id",
            "SELECT k, count(*) n FROM t GROUP BY k HAVING count(*) > 1",
            "SELECT 1 EXCEPT SELECT 2",
            "1 UNION SELECT * FROM u",
            "x = 1; DELETE FROM u",
        ] {
            assert!(!fragment_brackets_unbalanced(sql), "should accept {sql:?}");
        }
    }

    #[test]
    fn unbalanced_brackets_are_rejected() {
        // The middle case has equal counts, but its `)` closes the template's
        // bracket before opening one of its own.
        for sql in ["(a = 1", "a = 1)", "a = 1) AND (b = 2"] {
            assert!(fragment_brackets_unbalanced(sql), "should reject {sql:?}");
        }
    }

    #[test]
    fn a_closed_comment_is_blanked_not_deleted() {
        // Deleting would glue the tokens the comment separated: `1/*x*/AND`
        // must not become `1AND`.
        assert_eq!(
            fragment_comments_blanked("c = 1/*x*/AND d = 2").as_deref(),
            Some("c = 1     AND d = 2")
        );
        assert_eq!(
            fragment_comments_blanked("c = 1 /* x */ AND d = 2").as_deref(),
            Some("c = 1         AND d = 2")
        );
        // Whitespace is not trimmed: the fragment's edges are the separators
        // between it and the template, so blanking a comment must not also
        // delete them.
        assert_eq!(
            fragment_comments_blanked("c = 1 -- trailing").as_deref(),
            Some("c = 1            ")
        );
        assert_eq!(
            fragment_comments_blanked("/* lead */ c = 1").as_deref(),
            Some("           c = 1")
        );
    }

    #[test]
    fn a_quote_inside_a_comment_is_comment_text() {
        // The delimiters are mutually exclusive: a `'` written inside a comment
        // must not open a literal, or it would swallow the comment's `*/` and
        // leave part of the comment in the emitted SQL.
        for (sql, want) in [
            ("c = 1 /* it's */ AND d = 2", "c = 1            AND d = 2"),
            ("c = 1 -- it's", "c = 1        "),
            ("c = 1 -- 'x' = 'y'", "c = 1             "),
            ("c = 1 /* \"x\" */ AND d = 2", "c = 1           AND d = 2"),
            ("/* $tag$ */ a = 1", "            a = 1"),
            ("/* ' */ a = 1", "        a = 1"),
        ] {
            assert_eq!(
                fragment_comments_blanked(sql).as_deref(),
                Some(want),
                "{sql:?}"
            );
            assert!(!fragment_comment_unterminated(sql), "{sql:?}");
        }
    }

    #[test]
    fn a_non_ascii_comment_byte_is_blanked() {
        // A multi-byte char in a comment is comment text like any other. Each of
        // its bytes is blanked, so nothing of it reaches the SQL.
        assert_eq!(
            fragment_comments_blanked("c = 1 -- \u{e9}").as_deref(),
            Some("c = 1      ")
        );
        // A non-ASCII byte outside a comment is data and survives.
        assert_eq!(fragment_comments_blanked("s = '\u{e9}'"), None);
    }

    #[test]
    fn a_backslash_does_not_escape_in_an_ordinary_literal() {
        // Verified against PostgreSQL 16 (`standard_conforming_strings = on`):
        // `select 'a\'` yields `a\`, so the literal is complete and what follows
        // is real SQL. Treating `\'` as an escape hid it — a false *acceptance*,
        // letting a comment, a bracket or an unclosed `/*` through.
        assert_eq!(
            fragment_comments_blanked(r"s = 'a\' -- c").as_deref(),
            Some(r"s = 'a\'     ")
        );
        assert!(fragment_comment_unterminated(r"s = 'a\' /*"));
        assert!(fragment_brackets_unbalanced(r"s = 'a\' )"));
        // A quoted identifier never escapes with a backslash under any setting.
        assert_eq!(
            fragment_comments_blanked(r#"s = "a\" -- c"#).as_deref(),
            Some(r#"s = "a\"     "#)
        );
        assert!(fragment_brackets_unbalanced(r#"s = "a\" )"#));
    }

    #[test]
    fn a_backslash_escapes_in_an_extended_string() {
        // `select E'a\' -- c'` yields `a' -- c`: here `\'` really is an escaped
        // quote, so the `--` is data and the literal runs on.
        for sql in [
            r"s = E'a\' -- c' AND d = 2",
            r"s = e'a\' -- c' AND d = 2",
            r"s = E'a\' /*' AND d = 2",
        ] {
            assert_eq!(fragment_comments_blanked(sql), None, "{sql:?}");
            assert!(!fragment_comment_unterminated(sql), "{sql:?}");
        }
        assert!(!fragment_brackets_unbalanced(r"s = E'a\' )'"));
        // The `E` is part of the literal, so it must not survive in the
        // structure view as a stray keyword byte.
        assert!(!FragmentLex::scan(r"s = E'x'").structure.contains('E'));
    }

    #[test]
    fn only_a_standalone_e_prefixes_an_extended_string() {
        // `select text'a\'` yields `a\`: a *type-prefixed* literal does not
        // escape, so an `e` ending an identifier is not a prefix.
        assert!(fragment_brackets_unbalanced(r"s = code'a\' )"));
        assert!(fragment_brackets_unbalanced(r"s = t_e'a\' )"));
        // Standalone, it is — including at the very start of the fragment.
        assert!(!fragment_brackets_unbalanced(r"s = E'a\' )'"));
        assert!(!fragment_brackets_unbalanced(r"E'a\' )' = s"));
    }

    #[test]
    fn a_non_ascii_byte_before_e_does_not_make_it_a_prefix() {
        // Whether `éE` names a type is unknowable here, and reading the `E` as a
        // prefix is the direction that hides SQL: the literal would run past its
        // closing quote and swallow the `)`. Stay conservative.
        assert!(fragment_brackets_unbalanced(r"s = éE'a\' )"));
        assert!(fragment_comments_blanked(r"s = éE'a\' -- c").is_some());
    }

    #[test]
    fn an_unclosed_literal_is_rejected() {
        for sql in ["'", "\"", "$q$abc", "s = 'a", "c = \"x", "s = 'it''s"] {
            assert!(
                fragment_literal_unterminated(sql),
                "{sql:?} leaves a literal open"
            );
        }
    }

    #[test]
    fn a_closed_literal_is_not_reported_unclosed() {
        // A literal ending on the very last byte closes; the check must not read
        // "ran to the end" as "never closed".
        for sql in [
            "s = 'a'",
            "s = 'it''s'",
            "\"col\" = 1",
            "s = $q$a$q$",
            "a = 1 /* it's */",
            "a = 1",
            r"s = 'a\'",
        ] {
            assert!(
                !fragment_literal_unterminated(sql),
                "{sql:?} closes its literals"
            );
        }
    }

    #[test]
    fn a_fragment_contributing_nothing_is_rejected() {
        for sql in ["", "   ", "-- only a note", "/* note */", "\n\t"] {
            assert!(fragment_is_empty(sql), "{sql:?}");
        }
        for sql in ["c = 1", "-- x\nc = 1", " a = 1 "] {
            assert!(!fragment_is_empty(sql), "{sql:?}");
        }
    }

    #[test]
    fn a_fragment_without_comments_is_left_alone() {
        for sql in ["c = 1", "deleted_at IS NULL", "a = 1 AND b = 2"] {
            assert_eq!(fragment_comments_blanked(sql), None, "{sql:?}");
        }
    }

    #[test]
    fn a_comment_marker_inside_a_literal_is_not_a_comment() {
        // Inside a literal there is no comment to find, so these survive
        // verbatim.
        for sql in ["s = '--'", "s = $tag$--$tag$", "s = '/*'", "\"--\" = 1"] {
            assert_eq!(fragment_comments_blanked(sql), None, "{sql:?}");
            assert!(!fragment_comment_unterminated(sql), "{sql:?}");
        }
    }

    #[test]
    fn an_unterminated_block_comment_is_rejected() {
        // No end to blank up to: it would swallow the template text after the
        // marker.
        assert!(fragment_comment_unterminated("c = 1 /* x"));
        assert!(fragment_comment_unterminated("c = 1 /* a /* b */"));
        // A line comment needs no such rule — the fragment's end closes it.
        assert!(!fragment_comment_unterminated("c = 1 -- x"));
        assert!(!fragment_comment_unterminated("c = 1 /* x */"));
        // A `--` inside a block comment does not close it.
        assert!(fragment_comment_unterminated("c = 1 /* -- */ /* x"));
    }

    #[test]
    fn a_leading_joiner_is_rejected() {
        assert_eq!(fragment_starts_with_joiner("AND b = 1"), Some("AND"));
        assert_eq!(fragment_starts_with_joiner("  or b = 1"), Some("OR"));
        assert_eq!(
            fragment_starts_with_joiner("/* c */ AND b = 1"),
            Some("AND")
        );
    }

    #[test]
    fn a_word_merely_starting_with_a_joiner_is_accepted() {
        for sql in [
            "android = true",
            "order_id = 1",
            "origin = 'x'",
            "ORDER BY name",
            "a = 1 AND b = 2",
            "deleted_at IS NULL",
        ] {
            assert_eq!(fragment_starts_with_joiner(sql), None, "{sql:?}");
        }
    }

    #[test]
    fn a_bracket_inside_a_comment_is_data() {
        // The bracket check runs on a comment-blanked view, so a `)` written
        // inside a comment does not look like an unbalanced bracket.
        assert!(!fragment_brackets_unbalanced("a = 1 /* ) */"));
        assert!(!fragment_brackets_unbalanced("a = 1 -- )"));
        assert!(!fragment_brackets_unbalanced("a = 1 /* nested /* ) */ */"));
        // ...but a real imbalance outside a comment still is one.
        assert!(fragment_brackets_unbalanced("a = 1 /* x */ )"));
    }

    #[test]
    fn a_bracket_inside_a_literal_is_data() {
        assert!(!fragment_brackets_unbalanced("note = '('"));
        assert!(!fragment_brackets_unbalanced("note = $q$)$q$"));
        assert!(!fragment_brackets_unbalanced("\"(\" = 1"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Vec<Part> {
        parse_template(s, Span::call_site()).expect("template should parse")
    }

    fn text(s: &str) -> Part {
        Part::Text(s.to_string())
    }
    fn bind(s: &str) -> Part {
        Part::Bind(s.to_string(), Span::call_site())
    }
    fn frag(s: &str) -> Part {
        Part::Fragment(s.to_string(), Span::call_site())
    }
    /// `Part::OptBind` in clause 1 with an empty `after` — the shape produced by
    /// a single-clause template whose marker ends its predicate.
    fn opt(expr: &str, joiner: &str, before: &str) -> Part {
        opt_in(expr, joiner, before, "", 1)
    }

    /// `Part::OptBind` with every field spelled out, for multi-clause and tail
    /// cases.
    fn opt_in(expr: &str, joiner: &str, before: &str, after: &str, clause: u32) -> Part {
        Part::OptBind(OptPredicate {
            expr: expr.to_string(),
            joiner: joiner.to_string(),
            before: before.to_string(),
            after: after.to_string(),
            clause,
            span: Span::call_site(),
        })
    }

    #[test]
    fn plain_sql_is_one_text_part() {
        assert_eq!(parse("SELECT 1"), vec![text("SELECT 1")]);
    }

    #[test]
    fn empty_template_yields_no_parts() {
        assert_eq!(parse(""), vec![]);
    }

    #[test]
    fn single_bind() {
        assert_eq!(
            parse("WHERE id = ${id}"),
            vec![text("WHERE id = "), bind("id")]
        );
    }

    #[test]
    fn multiple_binds_keep_order() {
        assert_eq!(
            parse("a = ${x} AND b = ${y}"),
            vec![text("a = "), bind("x"), text(" AND b = "), bind("y")]
        );
    }

    #[test]
    fn adjacent_interpolations_have_no_text_between() {
        assert_eq!(parse("${a}${b}"), vec![bind("a"), bind("b")]);
    }

    #[test]
    fn fragment_marker() {
        assert_eq!(
            parse("WHERE #{FILTER}"),
            vec![text("WHERE "), frag("FILTER")]
        );
    }

    #[test]
    fn bind_and_fragment_mixed() {
        assert_eq!(
            parse("WHERE a = ${v} AND #{F} AND c = ${w}"),
            vec![
                text("WHERE a = "),
                bind("v"),
                text(" AND "),
                frag("F"),
                text(" AND c = "),
                bind("w"),
            ]
        );
    }

    #[test]
    fn expr_is_trimmed() {
        assert_eq!(parse("${  foo.bar()  }"), vec![bind("foo.bar()")]);
    }

    #[test]
    fn nested_braces_in_struct_literal() {
        assert_eq!(parse("${ S { id: 1 }.id }"), vec![bind("S { id: 1 }.id")]);
    }

    #[test]
    fn nested_parens_and_brackets() {
        assert_eq!(parse("${ f(g(1), h[2]) }"), vec![bind("f(g(1), h[2])")]);
    }

    #[test]
    fn brace_inside_string_literal_does_not_close() {
        assert_eq!(parse(r#"${ f("}") }"#), vec![bind(r#"f("}")"#)]);
    }

    #[test]
    fn escaped_brace_in_string_literal() {
        assert_eq!(parse(r#"${ f("\"}") }"#), vec![bind(r#"f("\"}")"#)]);
    }

    #[test]
    fn char_literal_brace() {
        assert_eq!(parse("${ f('}') }"), vec![bind("f('}')")]);
    }

    #[test]
    fn hashless_raw_string_with_trailing_backslash() {
        // `r"a\"` is a complete two-character raw string: `\` is not an escape
        // in a raw string, so it must not swallow the closing quote.
        assert_eq!(parse(r#"${ r"a\".len() }"#), vec![bind(r#"r"a\".len()"#)]);
    }

    #[test]
    fn hashless_raw_string_is_recognised_as_raw() {
        assert_eq!(parse(r#"${ r"\" }"#), vec![bind(r#"r"\""#)]);
    }

    #[test]
    fn raw_byte_string_prefix_is_recognised() {
        assert_eq!(parse(r#"${ br"a\".len() }"#), vec![bind(r#"br"a\".len()"#)]);
    }

    #[test]
    fn identifier_ending_in_r_is_not_a_raw_prefix() {
        // `var"x"` is not a raw string; the `r` belongs to the identifier.
        assert_eq!(parse(r#"${ f(var, "x") }"#), vec![bind(r#"f(var, "x")"#)]);
    }

    #[test]
    fn lifetime_is_not_a_char_literal() {
        assert_eq!(
            parse("${ x as &'static str }"),
            vec![bind("x as &'static str")]
        );
    }

    #[test]
    fn reference_expr() {
        assert_eq!(
            parse("ANY(${&all_ids})"),
            vec![text("ANY("), bind("&all_ids"), text(")")]
        );
    }

    #[test]
    fn escape_bind_marker() {
        assert_eq!(
            parse("SELECT '$${literal}'"),
            vec![text("SELECT '${literal}'")]
        );
    }

    #[test]
    fn escape_fragment_marker() {
        assert_eq!(
            parse("SELECT '##{literal}'"),
            vec![text("SELECT '#{literal}'")]
        );
    }

    #[test]
    fn escape_then_real_interpolation() {
        assert_eq!(parse("$${a} AND ${b}"), vec![text("${a} AND "), bind("b")]);
    }

    #[test]
    fn bare_dollar_is_literal() {
        // Positional parameters and casts must survive untouched.
        assert_eq!(
            parse("SELECT $1, a::text"),
            vec![text("SELECT $1, a::text")]
        );
    }

    #[test]
    fn bare_hash_is_literal() {
        assert_eq!(parse("SELECT a # b"), vec![text("SELECT a # b")]);
    }

    #[test]
    fn dollar_at_end_of_input_is_literal() {
        assert_eq!(parse("SELECT $"), vec![text("SELECT $")]);
    }

    #[test]
    fn multibyte_text_is_preserved() {
        assert_eq!(
            parse("WHERE name = 'é' AND id = ${id}"),
            vec![text("WHERE name = 'é' AND id = "), bind("id")]
        );
    }

    #[test]
    fn unterminated_interpolation_errors() {
        let err = parse_template("WHERE id = ${id", Span::call_site()).unwrap_err();
        assert!(err.message.contains("unterminated"), "{}", err.message);
    }

    #[test]
    fn empty_interpolation_errors() {
        let err = parse_template("WHERE ${}", Span::call_site()).unwrap_err();
        assert!(err.message.contains("empty"), "{}", err.message);
    }

    #[test]
    fn empty_fragment_interpolation_errors() {
        assert!(parse_template("WHERE #{}", Span::call_site()).is_err());
    }

    // --- `${?expr}` optional predicates ---

    #[test]
    fn optional_predicate_takes_the_where_as_its_joiner() {
        // The predicate's own SQL is cut out of the accumulated text, so no
        // `Text` part remains before it.
        assert_eq!(
            parse("WHERE col = ${?x}"),
            vec![opt("x", "WHERE", " col = ")]
        );
    }

    #[test]
    fn optional_after_required_bind_joins_with_and() {
        // `WHERE` is emitted unconditionally by the leading text, so only the
        // `AND` is folded into the optional part.
        assert_eq!(
            parse("WHERE org = ${org} AND name = ${?name}"),
            vec![
                text("WHERE org = "),
                bind("org"),
                opt("name", "AND", " name = "),
            ]
        );
    }

    #[test]
    fn two_optionals_keep_where_then_and() {
        assert_eq!(
            parse("WHERE a = ${?x} AND b = ${?y}"),
            vec![opt("x", "WHERE", " a = "), opt("y", "AND", " b = ")]
        );
    }

    #[test]
    fn or_is_kept_as_the_joiner() {
        assert_eq!(
            parse("WHERE a = ${?x} OR b = ${?y}"),
            vec![opt("x", "WHERE", " a = "), opt("y", "OR", " b = ")]
        );
    }

    #[test]
    fn joiner_matching_is_case_insensitive() {
        assert_eq!(
            parse("where a = ${?x} and b = ${?y}"),
            vec![opt("x", "where", " a = "), opt("y", "and", " b = ")]
        );
    }

    #[test]
    fn comparison_operators_are_accepted() {
        for (sql, before) in [
            ("WHERE n ILIKE ${?v}", " n ILIKE "),
            ("WHERE n LIKE ${?v}", " n LIKE "),
            ("WHERE n <> ${?v}", " n <> "),
            ("WHERE n != ${?v}", " n != "),
            ("WHERE n >= ${?v}", " n >= "),
            ("WHERE n <= ${?v}", " n <= "),
            ("WHERE n > ${?v}", " n > "),
            ("WHERE n < ${?v}", " n < "),
        ] {
            assert_eq!(parse(sql), vec![opt("v", "WHERE", before)], "{sql}");
        }
    }

    #[test]
    fn text_after_the_optional_predicate_is_its_own_part() {
        assert_eq!(
            parse("WHERE a = ${?x} ORDER BY id"),
            vec![opt("x", "WHERE", " a = "), text(" ORDER BY id")]
        );
    }

    #[test]
    fn expr_inside_optional_marker_is_trimmed() {
        assert_eq!(
            parse("WHERE a = ${? foo.bar() }"),
            vec![opt("foo.bar()", "WHERE", " a = ")]
        );
    }

    #[test]
    fn whitespace_before_the_joiner_is_consumed() {
        // The indent before `AND` is dropped so a removed predicate leaves no
        // blank line or double space; `Predicates::open` emits a single space.
        assert_eq!(
            parse("SELECT 1\n    WHERE a = ${?x}\n      AND b = ${?y}"),
            vec![
                text("SELECT 1"),
                opt("x", "WHERE", " a = "),
                opt("y", "AND", " b = "),
            ]
        );
    }

    #[test]
    fn escaped_optional_marker_is_literal_text() {
        assert_eq!(parse("SELECT '$${?x}'"), vec![text("SELECT '${?x}'")]);
    }

    #[test]
    fn optional_marker_can_follow_an_escaped_one() {
        assert_eq!(
            parse("$${?x} WHERE a = ${?y}"),
            vec![text("${?x}"), opt("y", "WHERE", " a = ")]
        );
    }

    #[test]
    fn empty_optional_interpolation_errors() {
        let err = parse_template("WHERE a = ${?}", Span::call_site()).unwrap_err();
        assert_eq!(err.message, "empty optional interpolation `${?}`");
    }

    #[test]
    fn optional_without_a_joiner_errors() {
        let err = parse_template("SELECT id, ${?extra} FROM t", Span::call_site()).unwrap_err();
        assert!(
            err.message.starts_with(
                "`${?...}` must sit in a predicate introduced by `WHERE`, `AND` or `OR`."
            ),
            "{}",
            err.message
        );
    }

    #[test]
    fn optional_after_between_errors() {
        let err = parse_template("WHERE age BETWEEN ${?lo} AND 9", Span::call_site()).unwrap_err();
        assert!(
            err.message
                .starts_with("`${?...}` must follow a comparison operator, but follows `BETWEEN`."),
            "{}",
            err.message
        );
    }

    #[test]
    fn optional_inside_parens_errors() {
        let err = parse_template("WHERE (a = ${?x} OR b = 1)", Span::call_site()).unwrap_err();
        assert!(
            err.message.starts_with(
                "`${?...}` must be a whole top-level predicate, but `(` appears \
                 between `WHERE` and the marker."
            ),
            "{}",
            err.message
        );
    }

    #[test]
    fn optional_inside_function_call_errors() {
        let err = parse_template("WHERE d >= make_interval(days => ${?d})", Span::call_site())
            .unwrap_err();
        assert!(
            err.message
                .starts_with("`${?...}` must be a whole top-level predicate, but `(` appears"),
            "{}",
            err.message
        );
    }

    #[test]
    fn line_comment_after_the_optional_marker_errors() {
        let err =
            parse_template("WHERE a = ${?x} -- note\nAND b = 1", Span::call_site()).unwrap_err();
        assert!(
            err.message.contains("SQL comments are not supported"),
            "{}",
            err.message
        );
    }

    #[test]
    fn block_comment_after_the_optional_marker_errors() {
        let err =
            parse_template("WHERE a = ${?x} /* AND */ AND b = 1", Span::call_site()).unwrap_err();
        assert!(
            err.message.contains("SQL comments are not supported"),
            "{}",
            err.message
        );
    }

    #[test]
    fn comment_like_text_inside_a_literal_after_the_marker_is_not_a_comment() {
        // `'--'` is data, and marker regions of a template without optionals are
        // never flagged: only templates using `${?..}` are checked at all.
        let parts = parse_template("WHERE a = ${?x} AND note = '--'", Span::call_site()).unwrap();
        assert_eq!(
            parts,
            vec![
                opt("x", "WHERE", " a = "),
                Part::Joined {
                    joiner: "AND".to_string(),
                    text: " note = '--'".to_string(),
                    clause: 1
                }
            ]
        );
    }

    #[test]
    fn a_rust_comment_inside_a_marker_is_not_a_sql_comment() {
        parse_template("WHERE a = ${?opt /* keep */}", Span::call_site()).unwrap();
    }

    #[test]
    fn optional_not_after_a_comparison_errors() {
        // `IS NULL` is not a comparison the marker could be the right side of.
        let err = parse_template("WHERE a IS NULL ${?x}", Span::call_site()).unwrap_err();
        assert!(
            err.message
                .starts_with("`${?...}` must follow a comparison operator"),
            "{}",
            err.message
        );
    }
}
