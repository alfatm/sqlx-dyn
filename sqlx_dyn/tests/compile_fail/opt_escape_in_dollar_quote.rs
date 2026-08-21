// Same overlap again, with a dollar-quoted body holding the escape.
use sqlx_dyn::query;

fn main() {
    let x: Option<&str> = None;
    query!("SELECT * FROM t WHERE a = ${?x} || $q$$${z}");
}
