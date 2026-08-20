// Same overlap as `opt_escape_in_predicate`, but the literal that stays open
// across the escape is a quoted identifier rather than a string. Deciding this
// by counting `'` parity saw no open literal here and emitted the predicate,
// scattering the escape: `SELECT * FROM t${z}"`.
use sqlx_dyn::query;

fn main() {
    let x: Option<&str> = None;
    query!("SELECT * FROM t WHERE a = ${?x} || \"$${z}\"");
}
