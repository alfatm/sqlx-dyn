// Same overlap as `opt_escape_in_predicate`, with a quoted identifier holding
// the escape rather than a string. Both are caught by where the tail stopped, so
// neither needs the literal to be recognised — an earlier fix decided this by
// counting `'` parity, which saw no open literal here and emitted the predicate,
// scattering the escape: `SELECT * FROM t${z}"`.
use sqlx_dyn::query;

fn main() {
    let x: Option<&str> = None;
    query!("SELECT * FROM t WHERE a = ${?x} || \"$${z}\"");
}
