//! `sql_fragment!` takes only a literal, so runtime strings cannot become SQL.

use sqlx_dyn::sql_fragment;

fn main() {
    let runtime_string = String::from("deleted_at IS NULL");
    let _f = sql_fragment!(runtime_string);
}
