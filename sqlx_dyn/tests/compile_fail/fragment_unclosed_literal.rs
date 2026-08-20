// A fragment is spliced verbatim, so an unclosed quote does not end at the
// fragment's edge: it runs into the template and swallows the SQL after the
// marker. PostgreSQL rejects the statement, but whether it does depends on the
// template rather than on the fragment — so this fails where it is written.
use sqlx_dyn::sql_fragment;

fn main() {
    let _quote = sql_fragment!("s = 'a");
    let _dollar = sql_fragment!("s = $q$a");
}
