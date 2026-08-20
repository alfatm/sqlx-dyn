// `:literal` accepted any literal, so `sql_fragment!(42)` parsed and failed
// later as a type error inside the expansion. `LitStr` rejects it here, at the
// argument, which is where the mistake is.
use sqlx_dyn::sql_fragment;

fn main() {
    let _f = sql_fragment!(42);
}
