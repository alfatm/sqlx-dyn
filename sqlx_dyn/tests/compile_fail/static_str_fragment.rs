//! Even `&'static str` must not implicitly become a fragment: SqlFragment has to
//! be named explicitly, so raw SQL is always visible at the construction
//! site.
use sqlx_dyn::query;

static FILTER: &str = "1=1";

fn main() {
    let _q = query!("SELECT * FROM users WHERE #{FILTER}");
}
