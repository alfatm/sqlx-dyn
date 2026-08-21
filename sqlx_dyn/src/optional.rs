//! Runtime support for `${?expr}` optional predicates.
//!
//! Which predicates survive is known only at runtime, so the joiner
//! (`AND`/`OR`/`WHERE`) cannot be baked into the static SQL chunks: if the first
//! optional predicate is `None`, the second must be introduced by `WHERE`
//! rather than joined by `AND`. Generated code therefore routes every optional
//! predicate through [`Predicates`], which decides the joiner at runtime.
//!
//! Bookkeeping is *per predicate list*. A template may hold several — a `WHERE`
//! and a `HAVING`, or the two halves of a `UNION` — and they are independent: a
//! predicate that survived in one must not make a later clause believe its
//! introducing keyword is already written.

use sqlx::{Postgres, QueryBuilder};

/// Emits the right joiner for each surviving optional predicate.
///
/// Driven by generated code; not a public building block.
#[doc(hidden)]
pub struct Predicates<'b> {
    builder: &'b mut QueryBuilder<Postgres>,
    /// Clause index -> the `WHERE`/`HAVING` introducing it, for clauses whose
    /// first optional predicate is the one that would introduce them. A borrowed
    /// slice of a `const`-promoted array, so holding it costs nothing.
    introducers: &'b [(u32, &'static str)],
    /// One bit per clause: set as soon as something stands there, so the next
    /// one joins rather than introduces. A single mask suffices because every
    /// `open` call sets it — including the one that consumed the clause's
    /// introducer — so a consumed introducer is always "emitted" too. The bitmask
    /// keeps `Predicates` allocation-free, which `tests/allocations.rs` pins
    /// down. Clause indices above 63 degrade to "already emitted", which degrades
    /// to the written joiner.
    emitted: u64,
}

impl<'b> Predicates<'b> {
    /// `introducers` maps a clause index to the `WHERE`/`HAVING` belonging to
    /// that clause's first optional predicate. A clause is absent when mandatory
    /// SQL already opened it; its predicates then always join.
    pub fn new(
        builder: &'b mut QueryBuilder<Postgres>,
        introducers: &'b [(u32, &'static str)],
    ) -> Self {
        Self {
            builder,
            introducers,
            emitted: 0,
        }
    }

    /// Writes the keyword appropriate for this position in `clause` and returns
    /// the builder so the caller can append the predicate body and the bind.
    ///
    /// `joiner` is the keyword from the template (`AND`/`OR`). It is used only
    /// when a predicate already stands in this clause; the clause's first
    /// surviving predicate gets the introducer.
    pub fn open(&mut self, clause: u32, joiner: &str) -> &mut QueryBuilder<Postgres> {
        // A single leading space keeps the SQL readable whichever predicates
        // were dropped; the template's own indentation was already consumed at
        // parse time.
        let mask = Self::mask(clause);
        let introducer = self
            .introducers
            .iter()
            .find(|(at, _)| *at == clause)
            .map(|(_, kw)| *kw);

        self.builder.push(" ");
        match introducer {
            // This clause is still waiting for its introducing keyword, and
            // this is the first predicate to survive in it.
            Some(kw) if self.emitted & mask == 0 => {
                self.builder.push(kw);
            }
            // Either a predicate already stands here, or mandatory SQL opened
            // the clause; in both cases this one joins.
            _ => {
                self.builder.push(joiner);
            }
        }

        self.emitted |= mask;
        self.builder
    }

    /// The bit for `clause`.
    ///
    /// Indices past the bitmask width saturate and share the top bit, so clauses
    /// that far into a template stop being tracked independently: if such a
    /// clause's first optional predicate is `None` and a later one survives, the
    /// bit is already set from the earlier clause and the introducing `WHERE`
    /// will not fire — a dangling `AND` remains. One index is consumed per clause
    /// boundary keyword, nested ones included, so the ceiling is 63 boundaries in
    /// one template — far beyond what this crate is for. Extending it would mean a
    /// heap-allocated set, which costs the very allocation parity that
    /// `tests/allocations.rs` pins down.
    fn mask(clause: u32) -> u64 {
        1u64 << clause.min(63)
    }

    /// Access for the mandatory chunks that follow predicates.
    pub fn builder(&mut self) -> &mut QueryBuilder<Postgres> {
        self.builder
    }
}
