// The predicate must be removable as one piece, while the `$${` escape is
// unwrapped separately. The tail reaches the escape without passing a clause
// boundary, so the escape sits inside this predicate and the two overlap: the
// predicate cannot be emitted or removed as one unit.
use sqlx_dyn::query;

fn main() {
    let x: Option<&str> = None;
    query!("SELECT * FROM t WHERE a = ${?x} || '$${z}' AND b = 1");
}
