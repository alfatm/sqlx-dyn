// A `--` comment can hide the `AND` introducing a predicate, or swallow the
// predicate itself, when the template is flattened onto one line. Either way the
// query would silently match the wrong rows, so the template is rejected.
use sqlx_dyn::query;

fn main() {
    let x: Option<i32> = None;
    query!("SELECT * FROM t WHERE a = 1 -- and stuff\n AND col = ${?x}");
}
