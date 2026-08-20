// The predicate must be removable as one piece, while the `$${` escape is
// unwrapped separately. When the escape sits in the predicate's own trailing
// text — here a string literal opened before it closes after it — the two
// overlap, and the predicate cannot be emitted or removed as one unit.
use sqlx_dyn::query;

fn main() {
    let x: Option<&str> = None;
    query!("SELECT * FROM t WHERE a = ${?x} || '$${z}' AND b = 1");
}
