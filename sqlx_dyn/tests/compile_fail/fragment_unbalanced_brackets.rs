// Bracket counts balance, but the first `)` closes a bracket the fragment never
// opened — one belonging to the template — and the `(` leaves a construct open
// past the fragment's end. Only the balance is checked; clause keywords are the
// author's business, since how deep a fragment lands is up to the template.
use sqlx_dyn::sql_fragment;

fn main() {
    let _f = sql_fragment!("a = 1) AND (b = 2");
}
