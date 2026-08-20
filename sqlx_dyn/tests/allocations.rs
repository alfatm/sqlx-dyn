//! Allocation parity with a hand-written `QueryBuilder`.
//!
//! The crate's claim is *parity*, not frugality: the macro must not allocate more
//! than the builder calls it expands into. Absolute numbers depend on sqlx
//! internals, so this asserts the relation and prints the numbers rather than
//! pinning them down.
//!
//! The counter is a process-wide `GlobalAlloc`, so it cannot tell this test's
//! allocations from those of another running at the same time. Every measurement
//! therefore happens inside a single `#[test]` under a mutex, and each is taken
//! as the best (smallest) of several rounds — interference can only inflate the
//! count, so the minimum is a clean reading.
//!
//! Run `cargo test --test allocations -- --nocapture` to see the numbers.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use sqlx_dyn::{query, sql_fragment, SqlFragment};

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static COUNTING: AtomicUsize = AtomicUsize::new(0);

struct Counter;

unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::SeqCst) == 1 {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::SeqCst) == 1 {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counter = Counter;

/// Allocations performed while `f` runs, taken as the smallest of several
/// rounds.
///
/// Another thread allocating inside the window can only add to the count, never
/// subtract, so the minimum across rounds is an interference-free reading.
fn count(mut f: impl FnMut()) -> usize {
    const ROUNDS: usize = 8;
    let mut best = usize::MAX;
    for _ in 0..ROUNDS {
        ALLOCS.store(0, Ordering::SeqCst);
        COUNTING.store(1, Ordering::SeqCst);
        f();
        COUNTING.store(0, Ordering::SeqCst);
        best = best.min(ALLOCS.load(Ordering::SeqCst));
    }
    best
}

const FILTER: SqlFragment = sql_fragment!("deleted_at IS NULL");

/// One test, because the allocation counter is global: separate `#[test]`
/// functions would run concurrently and pollute each other's windows.
#[test]
fn allocation_parity() {
    let id: i64 = 7;

    let macro_allocs = count(|| {
        let q = query!("SELECT * FROM users WHERE id = ${id} AND #{FILTER}");
        std::hint::black_box(q.sql());
    });

    let manual_allocs = count(|| {
        let mut b = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT * FROM users WHERE id = ");
        b.push_bind(id);
        b.push(" AND ");
        b.push("deleted_at IS NULL");
        std::hint::black_box(b.sql().as_str().to_owned());
    });

    println!("macro={macro_allocs} manual={manual_allocs}");
    assert_eq!(
        macro_allocs, manual_allocs,
        "the macro must allocate exactly what the equivalent builder calls do"
    );

    // `Predicates` is a borrow plus two scalars; it must add no allocation of
    // its own.
    let a: Option<i64> = Some(1);

    let with_optional = count(|| {
        let q = query!("SELECT * FROM t WHERE a = ${?a}");
        std::hint::black_box(q.sql());
    });

    let equivalent = count(|| {
        let mut b = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT * FROM t");
        b.push(" WHERE");
        b.push(" a = ");
        b.push_bind(1i64);
        b.push("");
        std::hint::black_box(b.sql().as_str().to_owned());
    });

    println!("optional={with_optional} equivalent={equivalent}");
    assert_eq!(with_optional, equivalent);

    // A check that the counter sees real work: a dropped predicate skips a bind
    // and the growth of its buffer.
    let some: Option<i64> = Some(1);
    let none: Option<i64> = None;

    let survives = count(|| {
        let q = query!("SELECT * FROM t WHERE a = ${?some}");
        std::hint::black_box(q.sql());
    });
    let drops = count(|| {
        let q = query!("SELECT * FROM t WHERE a = ${?none}");
        std::hint::black_box(q.sql());
    });

    println!("survives={survives} drops={drops}");
    assert!(
        drops < survives,
        "removing a predicate must not cost more: {drops} vs {survives}"
    );
}
