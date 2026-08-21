//! Ergonomic template macros over [`sqlx::QueryBuilder`] for PostgreSQL.
//!
//! Write SQL almost as-is and interpolate Rust values as bind parameters or
//! pre-approved SQL fragments — no `format!`, no manual `push`/`push_bind`.
//!
//! ```no_run
//! use sqlx_dyn::{query, sql_fragment, SqlFragment};
//!
//! const REPRESENTABLE_KINDS: SqlFragment =
//!     sql_fragment!("kind::text IN ('document', 'template', 'i18n')");
//!
//! # async fn run(pool: &sqlx::PgPool, all_ids: Vec<i64>, period_days: i32) -> sqlx::Result<()> {
//! let rows = query!(r#"
//!     SELECT action_type::text AS action_type, COUNT(*)::int AS count
//!     FROM system.record_audit_log
//!     WHERE author = ANY(${&all_ids})
//!       AND create_time >= now() - make_interval(days => ${period_days})
//!       AND #{REPRESENTABLE_KINDS}
//!     GROUP BY action_type
//! "#)
//! .fetch_all(pool)
//! .await?;
//! # Ok(()) }
//! ```
//!
//! # Interpolation
//!
//! | Syntax | Meaning | Accepts |
//! |---|---|---|
//! | `${expr}` | bind parameter (`push_bind`) | anything `Encode + Type` |
//! | `${?expr}` | optional predicate — vanishes on `None` | `Option<T>` |
//! | `#{expr}` | raw SQL syntax (`push`) | only [`SqlFragmentLike`] |
//!
//! PostgreSQL parameter numbering (`$1`, `$2`, …) is done by `QueryBuilder`;
//! this crate never generates a `$N` itself.
//!
//! # SQL injection protection model
//!
//! `${expr}` is **safe for untrusted data**: the value never becomes SQL text,
//! it is sent as a parameter.
//!
//! `#{expr}` **is** SQL text, which is why it is restricted to
//! [`SqlFragmentLike`]. That trait is **sealed** — implemented for
//! [`SqlFragment`] and references to it, and impossible to implement
//! downstream — and a [`SqlFragment`] can only be built from a `&'static str`.
//! Hence `#{user_input}` does not compile:
//!
//! ```compile_fail
//! # use sqlx_dyn::query;
//! let filter = String::from("1=1 OR true");
//! query!("SELECT * FROM users WHERE #{filter}");
//! ```
//!
//! ## What this guarantees and what it does not
//!
//! The *accidental* path is ruled out: there is no coercion, no `From` impl, no
//! blanket impl via `Display` and no downstream implementation through which a
//! runtime string could reach SQL text without the author saying so explicitly.
//!
//! Two deliberate paths remain, by design:
//!
//! - `str::leak` (and `Box::leak`) create a `&'static str` from a `String` in
//!   safe code, and [`SqlFragment::new`] accepts that.
//! - [`builder_mut`](DynQuery::builder_mut) hands out the raw
//!   `sqlx::QueryBuilder`, whose `push` takes an `impl Display`.
//!
//! Both are visible at the call site, and neither can happen by accident, but
//! neither is prevented. Treat them like `unsafe`: rarely, deliberately, and
//! worth a second look in review. If untrusted text is needed in a query, its
//! place is `${expr}`, which is safe unconditionally.
//!
//! Dynamic yet safe choices are expressed by selecting among constants:
//!
//! ```
//! use sqlx_dyn::{query, sql_fragment, SqlFragment};
//!
//! const ORDER_CREATED: SqlFragment = sql_fragment!("created_at DESC");
//! const ORDER_NAME: SqlFragment = sql_fragment!("name ASC");
//!
//! # enum Sort { Created, Name }
//! # let sort = Sort::Name;
//! let order = match sort {
//!     Sort::Created => ORDER_CREATED,
//!     Sort::Name => ORDER_NAME,
//! };
//! let q = query!("SELECT * FROM users ORDER BY #{order}");
//! assert_eq!(q.sql(), "SELECT * FROM users ORDER BY name ASC");
//! ```
//!
//! # Optional predicates
//!
//! `${?expr}` takes an `Option`. On `None` the whole predicate is dropped along
//! with the `AND`/`OR` joining it, and the `WHERE` disappears if nothing is
//! left. The template therefore reads like finished SQL, with no `if let`
//! scaffolding and no `WHERE true` placeholder:
//!
//! ```
//! # use sqlx_dyn::query;
//! fn find(name: Option<&str>, min_age: Option<i32>) -> String {
//!     query!(r#"
//!         SELECT id FROM users
//!         WHERE name = ${?name}
//!           AND age >= ${?min_age}
//!         ORDER BY id
//!     "#).sql()
//! }
//!
//! fn norm(s: String) -> String { s.split_whitespace().collect::<Vec<_>>().join(" ") }
//!
//! // Both present.
//! assert_eq!(norm(find(Some("ada"), Some(18))),
//!            "SELECT id FROM users WHERE name = $1 AND age >= $2 ORDER BY id");
//! // First dropped: the second predicate is introduced by `WHERE` and becomes $1.
//! assert_eq!(norm(find(None, Some(18))),
//!            "SELECT id FROM users WHERE age >= $1 ORDER BY id");
//! // All dropped: the `WHERE` goes too.
//! assert_eq!(norm(find(None, None)), "SELECT id FROM users ORDER BY id");
//! ```
//!
//! Each predicate list keeps its own bookkeeping, so a template may hold
//! several — a `WHERE` and a `HAVING`, `UNION` branches, or two statements — and
//! a predicate dropped in one never disturbs another:
//!
//! ```
//! # use sqlx_dyn::query;
//! let gone: Option<i32> = None;
//! let kept: Option<i64> = Some(2);
//! // The `WHERE` left unused by `gone` did not leak in after `HAVING`.
//! assert_eq!(
//!     query!("SELECT k FROM t WHERE a = ${?gone} GROUP BY k HAVING count(*) > ${?kept}").sql(),
//!     "SELECT k FROM t GROUP BY k HAVING count(*) > $1"
//! );
//! ```
//!
//! Mandatory predicates may sit on either side of an optional one; the joining
//! keyword comes out right regardless of which parts survived:
//!
//! ```
//! # use sqlx_dyn::query;
//! let missing: Option<i32> = None;
//! // The optional predicate owned the `WHERE`; dropping it hands the `WHERE`
//! // to the literal predicate that follows, instead of a dangling `AND`.
//! assert_eq!(
//!     query!("SELECT * FROM t WHERE a = ${?missing} AND b IS NULL").sql(),
//!     "SELECT * FROM t WHERE b IS NULL"
//! );
//! ```
//!
//! ## Where it is allowed
//!
//! Removal is well defined only for a whole top-level predicate, so `${?...}`
//! must be the right-hand side of a comparison introduced by `WHERE`, `HAVING`,
//! `AND` or `OR`. Anywhere else is a compile error rather than silently wrong
//! SQL: this crate does not parse SQL and rejects the cases it cannot justify.
//!
//! Text *after* the marker is taken along when the predicate drops, up to the
//! end of the predicate: the next top-level `AND`/`OR`, a clause keyword, or the
//! `)`/`;` closing the construct around it. A group opened inside that trailing
//! text belongs to the operand and is taken whole, keywords inside it included:
//!
//! ```
//! # use sqlx_dyn::query;
//! let none: Option<i32> = None;
//! // `coalesce(tax, 0)` is part of the operand, not the SQL after it.
//! assert_eq!(
//!     query!("SELECT * FROM o WHERE total = ${?none} + coalesce(tax, 0)").sql(),
//!     "SELECT * FROM o"
//! );
//! // The `UNION` belongs to the subquery, not to the clause the marker sits in.
//! assert_eq!(
//!     query!("SELECT * FROM t WHERE a = ${?none} IN (SELECT 1 UNION SELECT 2)").sql(),
//!     "SELECT * FROM t"
//! );
//! // A `)` closing a group opened *before* the marker ends the predicate.
//! assert_eq!(
//!     query!("SELECT * FROM t WHERE EXISTS (SELECT 1 FROM u WHERE k = ${?none})").sql(),
//!     "SELECT * FROM t WHERE EXISTS (SELECT 1 FROM u)"
//! );
//! ```
//!
//! Another `${...}` inside that trailing text is a compile error: the predicate
//! straddles it and cannot be removed as one piece. That includes one nested in
//! a group the trailing text itself opened.
//!
//! ```compile_fail
//! # use sqlx_dyn::query;
//! let x: Option<i32> = None;
//! let y: i32 = 9;
//! // Dropping the predicate would leave `SELECT * FROM t $1`.
//! query!("SELECT * FROM t WHERE a = ${?x} || ${y}");
//! ```
//!
//! ```compile_fail
//! # use sqlx_dyn::query;
//! let x: Option<i32> = None;
//! let y: i32 = 9;
//! // And this would leave `SELECT * FROM t($1)`.
//! query!("SELECT * FROM t WHERE a = ${?x} + f(${y})");
//! ```
//!
//! A clause boundary between the two is what makes them separable, so the
//! ordinary shape is unaffected:
//!
//! ```
//! # use sqlx_dyn::query;
//! let x: Option<i32> = None;
//! let y: i32 = 9;
//! assert_eq!(
//!     query!("SELECT * FROM t WHERE a = ${?x} AND b = ${y}").sql(),
//!     "SELECT * FROM t WHERE b = $1"
//! );
//! ```
//!
//! A template using `${?...}` also cannot contain a SQL comment — neither
//! before nor after the marker: a comment can swallow the keyword joining the
//! surviving predicates, and the query would silently match the wrong rows.
//!
//! ```compile_fail
//! # use sqlx_dyn::query;
//! let lo: Option<i32> = None;
//! let hi: Option<i32> = None;
//! // `BETWEEN` needs both operands; removing one would leave invalid SQL.
//! query!("SELECT * FROM t WHERE age BETWEEN ${?lo} AND ${?hi}");
//! ```
//!
//! ```compile_fail
//! # use sqlx_dyn::query;
//! let extra: Option<i32> = None;
//! // Not a predicate at all.
//! query!("SELECT id, ${?extra} FROM t");
//! ```
//!
//! For conditions that do not fit this shape, use a plain bind `${...}` or
//! reach for `builder_mut()` and append by hand.
//!
//! ## Fragments and optional predicates
//!
//! A `#{...}` marker is opaque to the template scanner: it sees the marker, not
//! the SQL the fragment supplies, which may be chosen at runtime. Clause
//! bookkeeping for `${?...}` is therefore built from the template alone.
//!
//! [`sql_fragment!`] checks that a fragment's brackets balance within it — an
//! unmatched bracket reaches into the *template's* nesting, where a `)` can
//! close a construct the fragment never opened. Clause keywords are not
//! checked, because how deep a fragment lands is a property of the template:
//!
//! ```
//! # use sqlx_dyn::{query, sql_fragment, SqlFragment};
//! # let id = Some(1i32);
//! // Top-level within the fragment, nested once the template wraps it.
//! const TREE: SqlFragment = sql_fragment!(
//!     "SELECT id FROM t WHERE parent IS NULL \
//!      UNION ALL \
//!      SELECT c.id FROM t c JOIN tree ON c.parent = tree.id"
//! );
//! query!("WITH RECURSIVE tree AS (#{TREE}) SELECT * FROM tree WHERE id = ${?id}");
//! ```
//!
//! Hence a constraint this crate documents rather than enforces: **a fragment
//! used alongside `${?...}` must not introduce a top-level clause boundary** —
//! `UNION`, `INTERSECT`, `EXCEPT`, `HAVING`, `QUALIFY` or `;` — at the depth the
//! template inserts it.
//!
//! Break it and the boundary opens a predicate list the template never counted:
//!
//! ```
//! # use sqlx_dyn::{query, SqlFragment};
//! let p: Option<i32> = None;
//! let q = Some(2i32);
//! // The fragment carries a predicate *and* the query's shape — the mistake.
//! const F: SqlFragment =
//!     SqlFragment::new("deleted_at IS NULL UNION SELECT x FROM u");
//! assert_eq!(
//!     query!("SELECT x FROM t WHERE a = ${?p} AND #{F} AND b = ${?q}").sql(),
//!     // The `UNION` starts a second select whose `WHERE` was never written,
//!     // but `q` is bookkept against the first select's clause — already
//!     // opened — so it joins with the written `AND`. PostgreSQL rejects this.
//!     "SELECT x FROM t WHERE deleted_at IS NULL UNION SELECT x FROM u AND b = $1"
//! );
//! ```
//!
//! Symptoms: a clause keyword arriving from a fragment with an `AND`/`OR` after
//! it and no `WHERE` in between; a PostgreSQL syntax error at the boundary; and
//! only for *some* `Option` combinations, since all-`Some` and all-`None` often
//! stay valid. The statement never parses, so it cannot silently match other
//! rows.
//!
//! The same opacity carries a second, narrower obligation. A `${...}` reached in
//! a predicate's trailing text is rejected at compile time, because the
//! predicate would straddle it. A `#{...}` there cannot be judged: the fragment
//! may *be* the clause boundary that ends the predicate, which is the working
//! `WHERE a = ${?x} #{ORDER_BY_ID}`, or it may continue the predicate — and
//! which one it is may be decided at runtime. So **a fragment placed in a
//! predicate's trailing text must be a clause boundary, not a continuation of
//! the predicate**:
//!
//! ```
//! # use sqlx_dyn::{query, sql_fragment, SqlFragment};
//! let n: Option<i64> = None;
//! const ORDER: SqlFragment = sql_fragment!("ORDER BY id");
//! // A boundary: the predicate ends before it, and both parts stand alone.
//! assert_eq!(
//!     query!("SELECT * FROM t WHERE a = ${?n} #{ORDER}").sql(),
//!     "SELECT * FROM t ORDER BY id"
//! );
//! // A continuation is the mistake — `SqlFragment::new("|| 'x'")` here leaves
//! // `SELECT * FROM t || 'x'` once the predicate drops.
//! ```
//!
//! Only at the top level. Nested in a group the trailing text opened, a fragment
//! cannot be the boundary — it is positionally inside the predicate whatever SQL
//! it carries — so that case is rejected like a `${...}`:
//!
//! ```compile_fail
//! # use sqlx_dyn::{query, sql_fragment, SqlFragment};
//! const F: SqlFragment = sql_fragment!("1");
//! let x: Option<i32> = None;
//! // Would leave `SELECT * FROM t(1)`.
//! query!("SELECT * FROM t WHERE a = ${?x} + f(#{F})");
//! ```
//!
//! A fragment is for the part you reuse — a predicate, an ordering, a join. The
//! query's *shape*, `UNION` included, belongs in the template. Split that way
//! the fragment becomes more useful: the same predicate applies on both sides
//! of the boundary.
//!
//! ```
//! # use sqlx_dyn::{query, sql_fragment, SqlFragment};
//! let p: Option<i32> = None;
//! let q = Some(2i32);
//! const ACTIVE: SqlFragment = sql_fragment!("deleted_at IS NULL");
//! assert_eq!(
//!     query!("SELECT x FROM t WHERE a = ${?p} AND #{ACTIVE} \
//!             UNION SELECT x FROM u WHERE #{ACTIVE} AND b = ${?q}").sql(),
//!     "SELECT x FROM t WHERE deleted_at IS NULL \
//!      UNION SELECT x FROM u WHERE deleted_at IS NULL AND b = $1"
//! );
//! ```
//!
//! If a fragment genuinely must carry a boundary, drop `${?...}` from that
//! template: with plain binds `${...}` there is no bookkeeping to invalidate.
//!
//! [`sql_fragment!`] **strips** SQL comments from a fragment. A comment is a note
//! about the fragment, not SQL it contributes, and left in place a trailing `--`
//! would comment out the template text after the marker — PostgreSQL accepts
//! that, and the query silently matches different rows. Comments are blanked to
//! spaces rather than deleted, so `c = 1/* note */AND d = 2` does not collapse
//! into `1AND`. Nothing else is touched, including the fragment's own leading
//! and trailing whitespace: that is what separates it from the template around
//! it. `'--'` and `$tag$--$tag$` are data and pass through untouched, and so
//! does a quote inside a comment — `/* it's */` is a comment containing an
//! apostrophe, not a comment plus an open literal.
//!
//! Where a literal *ends* follows PostgreSQL's default
//! `standard_conforming_strings = on`: a backslash before the closing quote is
//! ordinary data in `'a\'` and `"a\"`, so the literal ends there, but an escape
//! in `E'a\'`, so it continues. `s = 'a\' -- c` is therefore a literal followed
//! by a real comment (stripped), while `s = E'a\' -- c'` is one string whose body
//! contains `--` (left alone). `standard_conforming_strings = off` is not
//! supported.
//!
//! The `E` counts only as a standalone prefix: `code'a\'` is a *type-prefixed*
//! literal, which does not escape, so it ends at the quote. `U&'...'` is an
//! ordinary literal here too — its backslash introduces a codepoint (`\0041`),
//! never a quote escape, and PostgreSQL rejects a trailing `U&'a\'` outright.
//!
//! An *unterminated* `/*` is rejected instead: there is no end to strip up to,
//! so it would swallow whatever follows the marker. So is an unclosed `'`, `"`
//! or `$tag$`, for the same reason one step out: the literal does not stop at
//! the fragment's edge, so it consumes the template after the marker. PostgreSQL
//! rejects that statement rather than silently matching other rows, but which
//! template the fragment lands in decides whether it does — so it fails where it
//! is written. So is a fragment that contributes nothing but a comment: it would
//! splice an empty string, leaving `WHERE #{F}` as a bare `WHERE` that
//! PostgreSQL blames on the template.
//!
//! For the same reason [`sql_fragment!`] rejects a fragment that *starts* with
//! `AND`/`OR`. The joiner describes how the fragment is combined, not what it
//! is, so it belongs in the template where the scanner can hand a dropped
//! `WHERE` over to it: write `WHERE a = ${?x} AND #{F}`, not
//! `WHERE a = ${?x} #{AND_F}`.
//!
//! Two cases that look similar and are fine: a fragment's own leading `WHERE`
//! (it opens the very clause the surrounding predicates already belong to), and
//! a boundary nested in brackets (a subquery or CTE body leaves the template's
//! top-level clause count unchanged).
//!
//! # Scalar type selection
//!
//! `query_scalar!` takes no type argument; the column type is pinned at the
//! `fetch_*` call site, as in `sqlx::query_scalar`:
//!
//! ```no_run
//! # use sqlx_dyn::query_scalar;
//! # async fn run(pool: &sqlx::PgPool, org: i64) -> sqlx::Result<()> {
//! let count: i64 = query_scalar!("SELECT COUNT(*) FROM users WHERE org = ${org}")
//!     .fetch_one(pool)
//!     .await?;
//! # Ok(()) }
//! ```
//!
//! # Evaluation semantics
//!
//! Every interpolation is evaluated **exactly once, in source order, at the
//! point where it appears**. Repeated expressions are *not* deduplicated —
//! `${next_id()}` twice calls the function twice and yields two bind parameters.
//!
//! # Escaping
//!
//! A literal `${` is written `$${`, a literal `#{` is written `##{`. A `$` or
//! `#` not followed by `{` is never special, so `$1` and `a # b` pass through
//! unchanged.
//!
//! ```
//! # use sqlx_dyn::query;
//! let q = query!("SELECT '$${not_a_bind}', '##{not_a_fragment}', $$1");
//! assert_eq!(q.sql(), "SELECT '${not_a_bind}', '#{not_a_fragment}', $$1");
//! ```
//!
//! Escaping is needed there because interpolation is a text layer *above* SQL:
//! markers are found by position in the template, before anything is read as
//! SQL. A `${...}` inside a string literal or a SQL comment still interpolates —
//! which is why the example above escapes both. The lexical rules described
//! under [optional predicates](#optional-predicates) govern where a *literal*
//! ends and what a comment covers; they never decide whether a marker is a
//! marker.
//!
//! # Scope
//!
//! PostgreSQL only. No compile-time SQL validation, no schema introspection, no
//! `DATABASE_URL` at build time. Execution is delegated to sqlx entirely.

mod fragment;
mod optional;
mod query;

pub use fragment::{SqlFragment, SqlFragmentLike};
pub use query::{DynQuery, DynQueryAs, DynQueryScalar};

pub use sqlx_dyn_macros::{query, query_as, query_scalar, sql_fragment};

/// Implementation detail: a re-export so generated code does not require the
/// caller to have `sqlx` in scope. Not a stable API.
#[doc(hidden)]
pub mod __private {
    pub use crate::optional::Predicates;
    pub use sqlx::{Postgres, QueryBuilder};
}
