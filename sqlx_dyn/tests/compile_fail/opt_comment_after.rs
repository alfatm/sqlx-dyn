// A comment after the optional marker is just as dangerous: the predicate tail
// stops at the keyword inside the comment, and the newline closing the line
// comment is dropped, so `AND b = 1` ends up commented out when the predicate
// survives. The query would silently match the wrong rows.
use sqlx_dyn::query;

fn main() {
    let x: Option<i32> = None;
    query!("SELECT * FROM t WHERE a = ${?x} -- note\nAND b = 1");
}
