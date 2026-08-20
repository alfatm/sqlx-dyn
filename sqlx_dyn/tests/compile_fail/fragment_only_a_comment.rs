// A fragment holding nothing but a comment blanks away to nothing. Splicing it
// leaves `WHERE #{F}` as a bare `WHERE`, which PostgreSQL rejects at runtime
// while naming the template rather than the fragment behind it.
use sqlx_dyn::sql_fragment;

fn main() {
    let _f = sql_fragment!("-- only a note");
}
