//! Bind parameter assertions: values, types, order and count.
//!
//! Every other test in the crate asserts on the *text* of the generated SQL.
//! That leaves a blind spot: a codegen bug that put the binds in the wrong
//! order, dropped one, or bound the wrong expression would leave `.sql()`
//! byte-identical and pass the whole suite. These tests read the encoded
//! `PgArguments` back instead.
//!
//! Values are checked through the encoded wire buffer — what actually reaches
//! the server. The Postgres binary format prefixes each value with its 4-byte
//! length, so an `i64` looks like `00 00 00 08` plus eight big-endian bytes, and
//! a string is its length plus UTF-8.

use sqlx::{Arguments, Execute};
use sqlx_dyn::{query, query_as, query_scalar, sql_fragment, SqlFragment};

/// Everything sqlx will send: the parameter count, the types in order, and the
/// encoded wire buffer.
///
/// `QueryBuilder::build()` moves the arguments out of the builder and panics on a
/// second call, so all three are read in a single pass.
struct Binds {
    count: usize,
    types: Vec<String>,
    buffer: Vec<u8>,
}

fn binds<T>(q: &mut T) -> Binds
where
    T: BuilderAccess,
{
    let mut built = q.builder().build();
    let args = built.take_arguments().unwrap().expect("query has arguments");
    let count = args.len();
    // `PgArguments` keeps its fields private; `Debug` is the supported way to
    // read the type list and the encoded buffer back out.
    let dbg = format!("{args:?}");
    let types = between(&dbg, "types: [", "]")
        .split(", ")
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.trim_start_matches("PgTypeInfo(")
                .trim_end_matches(')')
                .to_string()
        })
        .collect();
    let buffer = between(&dbg, "buffer: [", "]")
        .split(", ")
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u8>().expect("byte"))
        .collect();
    Binds {
        count,
        types,
        buffer,
    }
}

fn between<'a>(haystack: &'a str, open: &str, close: &str) -> &'a str {
    haystack
        .split(open)
        .nth(1)
        .and_then(|s| s.split(close).next())
        .unwrap_or("")
}

/// Lets `binds` work with all three wrappers.
trait BuilderAccess {
    fn builder(&mut self) -> &mut sqlx::QueryBuilder<sqlx::Postgres>;
}

impl BuilderAccess for sqlx_dyn::DynQuery {
    fn builder(&mut self) -> &mut sqlx::QueryBuilder<sqlx::Postgres> {
        self.builder_mut()
    }
}

impl<T> BuilderAccess for sqlx_dyn::DynQueryAs<T> {
    fn builder(&mut self) -> &mut sqlx::QueryBuilder<sqlx::Postgres> {
        self.builder_mut()
    }
}

impl BuilderAccess for sqlx_dyn::DynQueryScalar {
    fn builder(&mut self) -> &mut sqlx::QueryBuilder<sqlx::Postgres> {
        self.builder_mut()
    }
}

/// The encoded form of an `i64` parameter: a 4-byte length prefix, then the
/// big-endian value.
fn i64_bytes(v: i64) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 8];
    out.extend_from_slice(&v.to_be_bytes());
    out
}

/// The encoded form of a text parameter: a 4-byte length prefix, then
/// UTF-8.
fn text_bytes(s: &str) -> Vec<u8> {
    let mut out = (s.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(s.as_bytes());
    out
}

#[test]
fn values_reach_the_driver_in_template_order() {
    let a: i64 = 111;
    let b: i64 = 222;
    let mut q = query!("SELECT * FROM t WHERE x = ${a} AND y = ${b}");
    let mut want = i64_bytes(111);
    want.extend(i64_bytes(222));
    assert_eq!(binds(&mut q).buffer, want);
}

#[test]
fn swapping_two_binds_changes_the_buffer_but_not_the_sql() {
    // This is the bug class invisible to SQL-only tests: both templates yield
    // identical text, so only the buffer tells them apart.
    let a: i64 = 111;
    let b: i64 = 222;
    let mut first = query!("SELECT * FROM t WHERE x = ${a} AND y = ${b}");
    let mut second = query!("SELECT * FROM t WHERE x = ${b} AND y = ${a}");
    assert_eq!(first.sql(), second.sql());
    assert_ne!(binds(&mut first).buffer, binds(&mut second).buffer);
}

#[test]
fn bind_types_follow_the_expression_types() {
    let n: i64 = 1;
    let s: &str = "x";
    let mut q = query!("SELECT * FROM t WHERE a = ${n} AND b = ${s}");
    assert_eq!(binds(&mut q).types, vec!["Int8", "Text"]);
}

#[test]
fn a_fragment_adds_no_parameter() {
    const ACTIVE: SqlFragment = sql_fragment!("deleted_at IS NULL");
    let id: i64 = 7;
    let mut q = query!("SELECT * FROM t WHERE id = ${id} AND #{ACTIVE}");
    let b = binds(&mut q);
    assert_eq!(b.count, 1);
    assert_eq!(b.buffer, i64_bytes(7));
}

#[test]
fn a_dropped_optional_predicate_removes_its_parameter() {
    let x: Option<i64> = None;
    let y: i64 = 9;
    let mut q = query!("SELECT * FROM t WHERE a = ${?x} AND b = ${y}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE b = $1");
    let b = binds(&mut q);
    assert_eq!(b.count, 1);
    // `y`, not `x` — the surviving bind is the one left over.
    assert_eq!(b.buffer, i64_bytes(9));
}

#[test]
fn a_surviving_optional_predicate_binds_its_inner_value() {
    // The macro binds the unwrapped value, not the `Option`.
    let x: Option<i64> = Some(5);
    let mut q = query!("SELECT * FROM t WHERE a = ${?x}");
    let b = binds(&mut q);
    assert_eq!(b.count, 1);
    assert_eq!(b.buffer, i64_bytes(5));
    assert_eq!(b.types, vec!["Int8"]);
}

#[test]
fn optionals_bind_in_template_order_when_only_some_survive() {
    let a: Option<i64> = Some(1);
    let b: Option<i64> = None;
    let c: Option<i64> = Some(3);
    let mut q = query!("SELECT * FROM t WHERE a = ${?a} AND b = ${?b} AND c = ${?c}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a = $1 AND c = $2");
    let mut want = i64_bytes(1);
    want.extend(i64_bytes(3));
    assert_eq!(binds(&mut q).buffer, want);
}

#[test]
fn a_cast_tail_does_not_add_or_move_parameters() {
    let x: Option<i64> = Some(4);
    let y: i64 = 8;
    let mut q = query!("SELECT * FROM t WHERE a = ${?x}::int8 AND b = ${y}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a = $1::int8 AND b = $2");
    let mut want = i64_bytes(4);
    want.extend(i64_bytes(8));
    assert_eq!(binds(&mut q).buffer, want);
}

#[test]
fn repeated_expression_binds_twice_with_both_values_present() {
    let v: i64 = 42;
    let mut q = query!("SELECT * FROM t WHERE a = ${v} OR b = ${v}");
    let b = binds(&mut q);
    assert_eq!(b.count, 2);
    let mut want = i64_bytes(42);
    want.extend(i64_bytes(42));
    assert_eq!(b.buffer, want);
}

#[test]
fn text_values_are_encoded_verbatim_not_escaped_into_sql() {
    // The injection tests assert the payload is absent from the SQL; this one
    // asserts it is present, unmodified, in the parameter buffer, where it
    // belongs.
    let evil = "'; DROP TABLE users; --";
    let mut q = query!("SELECT * FROM t WHERE name = ${evil}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE name = $1");
    assert_eq!(binds(&mut q).buffer, text_bytes(evil));
}

#[test]
fn manual_push_bind_continues_the_same_parameter_sequence() {
    let a: i64 = 1;
    let mut q = query!("SELECT * FROM t WHERE a = ${a} AND b = ");
    q.builder_mut().push_bind(2i64);
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a = $1 AND b = $2");
    let mut want = i64_bytes(1);
    want.extend(i64_bytes(2));
    assert_eq!(binds(&mut q).buffer, want);
}

#[test]
fn query_as_binds_the_same_values_as_query() {
    #[derive(sqlx::FromRow)]
    struct Row {
        #[allow(dead_code)]
        id: i64,
    }
    let a: i64 = 7;
    let mut typed = query_as!(Row, "SELECT id FROM t WHERE a = ${a}");
    let b = binds(&mut typed);
    assert_eq!(b.count, 1);
    assert_eq!(b.types, vec!["Int8"]);
    assert_eq!(b.buffer, i64_bytes(7));
}

#[test]
fn query_scalar_binds_the_same_values_as_query() {
    let a: i64 = 7;
    let mut scalar = query_scalar!("SELECT count(*) FROM t WHERE a = ${a}");
    let b = binds(&mut scalar);
    assert_eq!(b.count, 1);
    assert_eq!(b.types, vec!["Int8"]);
    assert_eq!(b.buffer, i64_bytes(7));
}
