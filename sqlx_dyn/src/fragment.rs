//! Raw SQL fragments.
//!
//! `#{expr}` in [`query!`](crate::query) interpolates SQL *syntax*, not a bind
//! parameter. To keep that from becoming a SQL injection primitive, the macro
//! requires the expression to implement [`SqlFragmentLike`], which is sealed and
//! deliberately *not* implemented for `String`, `&str` or any `Display` type.
//!
//! Fragments hold a `Cow<'static, str>`: static ones borrow and never allocate;
//! this crate leaks nothing.

use std::borrow::Cow;

/// A SQL fragment: text interpolated as SQL *syntax*.
///
/// ```
/// use sqlx_dyn::{sql_fragment, SqlFragment};
/// const ACTIVE: SqlFragment = sql_fragment!("deleted_at IS NULL");
/// assert_eq!(ACTIVE.as_str(), "deleted_at IS NULL");
/// ```
///
/// Build one via [`sql_fragment!`](crate::sql_fragment) or
/// [`SqlFragment::new`]; both require a `&'static str`. Neither constructor
/// accepts a runtime `String`, so a fragment cannot accidentally carry user
/// input.
///
/// It can still carry it deliberately: `str::leak` turns a `String` into a
/// `&'static str` in safe code. That is an intentional act, not a typo, and it
/// is a boundary this type marks rather than enforces.
///
/// Static fragments borrow and do not allocate; [`SqlFragment::new`] is
/// `const`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlFragment(Cow<'static, str>);

impl SqlFragment {
    /// Wraps a static SQL string. Callable in `const`, never allocates.
    ///
    /// Unlike [`sql_fragment!`](crate::sql_fragment), this does **not** check
    /// that the brackets balance: being `const fn`, it cannot. An unmatched
    /// bracket reaches into the `query!` template's own nesting, where a `)`
    /// can close a construct the fragment never opened. Prefer
    /// `sql_fragment!`, which rejects that at compile time; this is the same
    /// kind of marked-not-enforced boundary as `str::leak` described above.
    ///
    /// Neither constructor checks clause keywords — see the crate-level
    /// "Fragments and optional predicates" section for the constraint that
    /// applies to both.
    pub const fn new(sql: &'static str) -> Self {
        Self(Cow::Borrowed(sql))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Types usable in a `#{...}` interpolation.
///
/// Sealed: implemented for [`SqlFragment`] and references to it, and impossible
/// to implement downstream. That is what makes `#{user_input}` a compile error
/// rather than a convention — a newtype around `String` cannot opt itself in.
///
/// Note that sealing closes the *trait*, but closes neither `str::leak` nor
/// [`DynQuery::builder_mut`](crate::DynQuery::builder_mut). See the crate-level
/// "SQL injection protection model" section for where the guarantee actually
/// stands.
pub trait SqlFragmentLike: sealed::Sealed {
    fn as_sql(&self) -> &str;
}

/// Private supertrait: downstream crates cannot name it, so they cannot
/// implement [`SqlFragmentLike`] either.
mod sealed {
    pub trait Sealed {}
    impl Sealed for super::SqlFragment {}
    impl<T: Sealed + ?Sized> Sealed for &T {}
}

impl SqlFragmentLike for SqlFragment {
    fn as_sql(&self) -> &str {
        &self.0
    }
}

impl<T: SqlFragmentLike + ?Sized> SqlFragmentLike for &T {
    fn as_sql(&self) -> &str {
        (**self).as_sql()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_fragment_borrows_and_does_not_allocate() {
        const F: SqlFragment = SqlFragment::new("a = 1");
        assert!(matches!(F.0, Cow::Borrowed(_)));
        assert_eq!(F.as_str(), "a = 1");
    }

    #[test]
    fn const_fragment_usable_in_match_without_copy() {
        // `SqlFragment` is not `Copy`; constants are inlined into each arm, so
        // the dynamic ORDER BY pattern from the SQL injection model keeps
        // working.
        const A: SqlFragment = SqlFragment::new("created_at DESC");
        const B: SqlFragment = SqlFragment::new("name ASC");
        let pick = |newest: bool| if newest { A } else { B };
        assert_eq!(pick(true).as_str(), "created_at DESC");
        assert_eq!(pick(false).as_str(), "name ASC");
    }

    #[test]
    fn fragment_like_reaches_through_reference() {
        const F: SqlFragment = SqlFragment::new("x IS NULL");
        assert_eq!(SqlFragmentLike::as_sql(&F), "x IS NULL");
        assert_eq!(SqlFragmentLike::as_sql(&&F), "x IS NULL");
    }
}
