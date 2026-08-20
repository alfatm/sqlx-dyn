// A fragment marker is opaque, so codegen cannot lift a joiner out of it the way
// it does for literal text. If the optional predicate before such a fragment
// drops, its `WHERE` goes with it and the fragment's `AND` is left dangling:
// `WHERE a = ${?x} #{F}` emitted `SELECT * FROM t AND b = 1`.
use sqlx_dyn::sql_fragment;

fn main() {
    let _f = sql_fragment!("AND b = 1");
}
