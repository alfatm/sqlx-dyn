// A closed comment is blanked out of the fragment, but an unclosed `/*` has no
// end to blank up to: it would comment out the template text following the
// marker, and the query would silently match different rows.
use sqlx_dyn::sql_fragment;

fn main() {
    let _f = sql_fragment!("c = 1 /* x");
}
