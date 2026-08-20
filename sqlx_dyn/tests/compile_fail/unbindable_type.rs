//! Binds are still type-checked by sqlx: a type without `Encode`/`Type` fails.

use sqlx_dyn::query;

struct NotEncodable;

fn main() {
    let x = NotEncodable;
    let _q = query!("SELECT * FROM users WHERE id = ${x}");
}
