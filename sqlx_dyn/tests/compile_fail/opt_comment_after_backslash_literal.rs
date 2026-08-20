// `'a\'` is a complete literal under the default `standard_conforming_strings =
// on`, so the `--` after it is a real comment. Treating `\'` as an escape made
// the scanner read the rest of the line as literal data, bypassing the ban on
// SQL comments in a template using `${?...}` — and `AND b = 1` would have
// reached the database commented out.
use sqlx_dyn::query;

fn main() {
    let v: Option<i32> = None;
    let _q = query!(r"SELECT * FROM t WHERE a = ${?v} AND s = 'a\' -- c AND b = 1");
}
