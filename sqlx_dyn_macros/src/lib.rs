//! Proc-macro implementation of `sqlx_dyn`. See the `sqlx_dyn` crate for docs.

mod parse;

use parse::{parse_template, FragmentLex, Part};
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, LitStr, Token, Type};

/// `query!("SELECT ...")`
#[proc_macro]
pub fn query(input: TokenStream) -> TokenStream {
    let template = match syn::parse::<LitStr>(input) {
        Ok(lit) => lit,
        Err(err) => return err.to_compile_error().into(),
    };
    expand(
        &template,
        quote! { ::sqlx_dyn::DynQuery::new(__sqlx_dyn_builder) },
    )
    .into()
}

/// `query_as!(Type, "SELECT ...")`
#[proc_macro]
pub fn query_as(input: TokenStream) -> TokenStream {
    let TypedInput { ty, template } = match syn::parse::<TypedInput>(input) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };
    expand(
        &template,
        quote! { ::sqlx_dyn::DynQueryAs::<#ty>::new(__sqlx_dyn_builder) },
    )
    .into()
}

/// `query_scalar!("SELECT count(*) ...")`
#[proc_macro]
pub fn query_scalar(input: TokenStream) -> TokenStream {
    let template = match syn::parse::<LitStr>(input) {
        Ok(lit) => lit,
        Err(err) => return err.to_compile_error().into(),
    };
    expand(
        &template,
        quote! { ::sqlx_dyn::DynQueryScalar::new(__sqlx_dyn_builder) },
    )
    .into()
}

/// `sql_fragment!("a = 1")`
///
/// Validates the literal and expands to `SqlFragment::new`, which is `const`,
/// so the result stays usable in a `const` item.
///
/// Only string literals are accepted, so runtime data cannot reach a fragment
/// through this macro. SQL comments are stripped from the text, and the fragment
/// is rejected if it leaves a block comment unterminated, leaves a quoted
/// literal unclosed, contributes no SQL at all, starts with `AND`/`OR`, or
/// leaves brackets unbalanced. Clause keywords are deliberately allowed; the
/// `FragmentLex` method behind each check documents why.
///
/// Every one of those checks reads a single lexical pass over the literal,
/// because literals and comments hide each other's delimiters and cannot be
/// scanned in layers. The scan and the checks are private; run
/// `cargo doc --document-private-items` to read them.
#[proc_macro]
pub fn sql_fragment(input: TokenStream) -> TokenStream {
    let lit = match syn::parse::<LitStr>(input) {
        Ok(lit) => lit,
        Err(err) => return err.to_compile_error().into(),
    };
    // An unclosed `/*` has no end to blank up to, and would comment out the
    // template text after the marker. Nothing to salvage — reject it.
    let lex = FragmentLex::scan(&lit.value());
    if lex.comment_unterminated() {
        return syn::Error::new(
            lit.span(),
            "this SQL fragment leaves a block comment unterminated.\n\
             A `#{...}` marker is opaque to the template scanner, so an unclosed \
             `/*` comments out the template text that follows the marker and the \
             query silently matches different rows. A closed comment is fine — \
             it is stripped from the fragment.\n\
             Close the comment, or remove it.",
        )
        .to_compile_error()
        .into();
    }
    // An unclosed `'`, `"` or `$tag$` runs past the marker and swallows template
    // SQL. PostgreSQL will reject the result, but whether it does depends on the
    // template rather than on the fragment — so it fails here, where it is
    // written.
    if lex.literal_unterminated() {
        return syn::Error::new(
            lit.span(),
            "this SQL fragment leaves a quoted literal unclosed.\n\
             A fragment is spliced verbatim, so the literal does not end at the \
             fragment's edge: it runs on into the template and swallows the SQL \
             after the marker. PostgreSQL then rejects the whole statement, and \
             which template the fragment is used in decides whether it does.\n\
             Close the quote, or double it (`''`, `\"\"`) if it is data.",
        )
        .to_compile_error()
        .into();
    }
    // A fragment that contributes nothing splices an empty string into the
    // template, so `WHERE #{F}` becomes a bare `WHERE`: a runtime syntax error
    // pointing at the template rather than at the fragment behind it.
    if lex.is_empty() {
        return syn::Error::new(
            lit.span(),
            "this SQL fragment contributes no SQL.\n\
             Spliced into a template it leaves nothing behind, so \
             `WHERE #{F}` becomes a bare `WHERE` and PostgreSQL rejects the \
             query at runtime, naming the template instead of this fragment.\n\
             Write the predicate, or drop the marker from the template.",
        )
        .to_compile_error()
        .into();
    }
    // A closed comment describes the fragment; it is not SQL the fragment
    // contributes. Blank it here so it cannot escape the whole-template comment
    // rule, and let the fragment keep working. Whitespace is left as written:
    // a fragment is spliced verbatim, so its edges are the separators between it
    // and the template around it.
    let lit = match lex.comments_blanked() {
        Some(sql) => LitStr::new(sql, lit.span()),
        None => lit,
    };

    if let Some(kw) = lex.starts_with_joiner() {
        return syn::Error::new(
            lit.span(),
            format!(
                "a SQL fragment must not start with `{kw}`: the joiner belongs to \
                 the template.\n\
                 A `#{{...}}` marker is opaque, so the macro cannot hand a \
                 dropped `WHERE` over to a joiner hidden inside the fragment. If \
                 the optional predicate before it is `None`, the `{kw}` is left \
                 dangling: `SELECT * FROM t WHERE a = ${{?x}} #{{F}}` becomes \
                 `SELECT * FROM t {kw} b = 1`.\n\
                 Write the joiner in the template instead: \
                 `WHERE a = ${{?x}} {kw} #{{F}}`."
            ),
        )
        .to_compile_error()
        .into();
    }
    if lex.brackets_unbalanced() {
        return syn::Error::new(
            lit.span(),
            "this SQL fragment's brackets do not close within it.\n\
             A fragment is spliced verbatim into the `query!` template, so an \
             unmatched bracket reaches into the template's own nesting: a `)` \
             can close a construct the fragment never opened.\n\
             Balance the brackets inside the fragment, or move the surrounding \
             bracket into the template.",
        )
        .to_compile_error()
        .into();
    }
    quote! { ::sqlx_dyn::SqlFragment::new(#lit) }.into()
}

struct TypedInput {
    ty: Type,
    template: LitStr,
}

impl Parse for TypedInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ty = input.parse::<Type>()?;
        input.parse::<Token![,]>()?;
        let template = input.parse::<LitStr>()?;
        // Allow a trailing comma, like sqlx's own macros.
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        Ok(Self { ty, template })
    }
}

/// Builds the query from the template and wraps the finished builder in
/// `construct` — the caller supplies that wrapper expression, because only the
/// caller knows which of the three macros is being expanded.
fn expand(template: &LitStr, construct: TokenStream2) -> TokenStream2 {
    let span = template.span();
    let parts = match parse_template(&template.value(), span) {
        Ok(parts) => parts,
        Err(err) => return syn::Error::new(span, err.message).to_compile_error(),
    };

    // The first literal chunk seeds QueryBuilder::new, so it costs no push.
    let mut iter = parts.iter().peekable();
    let init = match iter.peek() {
        Some(Part::Text(text)) => {
            let text = text.clone();
            iter.next();
            text
        }
        _ => String::new(),
    };

    let parts: Vec<&Part> = iter.collect();
    let has_optional = parts.iter().any(|p| matches!(p, Part::OptBind(_)));

    // Each predicate list has its own introducer: the `WHERE`/`HAVING` carried
    // by the *first* optional predicate of that clause. When an unconditional
    // predicate already opened the clause, the joiner is `AND`/`OR` instead and
    // there is nothing to introduce. Clauses are tracked separately so that a
    // `WHERE` left unclaimed by a dropped predicate cannot leak into a later
    // clause.
    let mut introducers: Vec<(u32, &str)> = Vec::new();
    for part in &parts {
        if let Part::OptBind(pred) = part {
            if introducers.iter().any(|(clause, _)| *clause == pred.clause) {
                continue;
            }
            let kw = pred.joiner.as_str();
            if kw.eq_ignore_ascii_case("WHERE") || kw.eq_ignore_ascii_case("HAVING") {
                introducers.push((pred.clause, kw));
            }
        }
    }

    let mut statements = Vec::new();
    for part in parts {
        match part {
            Part::Text(text) => statements.push(if has_optional {
                quote! { __sqlx_dyn_preds.builder().push(#text); }
            } else {
                quote! { __sqlx_dyn_builder.push(#text); }
            }),
            Part::Bind(src, span) => match parse_expr(src, *span, "${...}") {
                Ok(expr) => statements.push(if has_optional {
                    quote! { __sqlx_dyn_preds.builder().push_bind(#expr); }
                } else {
                    quote! { __sqlx_dyn_builder.push_bind(#expr); }
                }),
                Err(err) => return err,
            },
            Part::Fragment(src, span) => match parse_expr(src, *span, "#{...}") {
                Ok(expr) => {
                    let sql = quote! { ::sqlx_dyn::SqlFragmentLike::as_sql(&#expr) };
                    statements.push(if has_optional {
                        quote! { __sqlx_dyn_preds.builder().push(#sql); }
                    } else {
                        quote! { __sqlx_dyn_builder.push(#sql); }
                    })
                }
                Err(err) => return err,
            },
            Part::Joined {
                joiner,
                text,
                clause,
            } => statements.push(quote! {
                {
                    let __sqlx_dyn_b = __sqlx_dyn_preds.open(#clause, #joiner);
                    __sqlx_dyn_b.push(#text);
                }
            }),
            Part::OptBind(pred) => match parse_expr(&pred.expr, pred.span, "${?...}") {
                Ok(expr) => {
                    let joiner = pred.joiner.as_str();
                    let before = &pred.before;
                    let after = &pred.after;
                    let clause = pred.clause;
                    statements.push(quote! {
                        if let ::core::option::Option::Some(__sqlx_dyn_value) = #expr {
                            let __sqlx_dyn_b = __sqlx_dyn_preds.open(#clause, #joiner);
                            __sqlx_dyn_b.push(#before);
                            __sqlx_dyn_b.push_bind(__sqlx_dyn_value);
                            __sqlx_dyn_b.push(#after);
                        }
                    });
                }
                Err(err) => return err,
            },
        }
    }

    if has_optional {
        let entries = introducers
            .iter()
            .map(|(clause, kw)| quote! { (#clause, #kw) });
        quote! {{
            let mut __sqlx_dyn_builder =
                ::sqlx_dyn::__private::QueryBuilder::<::sqlx_dyn::__private::Postgres>::new(#init);
            {
                let mut __sqlx_dyn_preds = ::sqlx_dyn::__private::Predicates::new(
                    &mut __sqlx_dyn_builder,
                    &[#(#entries),*],
                );
                #(#statements)*
            }
            #construct
        }}
    } else {
        quote! {{
            let mut __sqlx_dyn_builder =
                ::sqlx_dyn::__private::QueryBuilder::<::sqlx_dyn::__private::Postgres>::new(#init);
            #(#statements)*
            #construct
        }}
    }
}

fn parse_expr(src: &str, span: Span, marker: &str) -> Result<Expr, TokenStream2> {
    match syn::parse_str::<Expr>(src) {
        Ok(mut expr) => {
            // Point diagnostics at the macro call site, not at a synthetic span.
            set_span(&mut expr, span);
            Ok(expr)
        }
        Err(err) => Err(syn::Error::new(
            span,
            format!(
                "invalid Rust expression inside `{marker}`: `{src}`\n  {}",
                err
            ),
        )
        .to_compile_error()),
    }
}

/// Moves the string literal's span onto the parsed expression so type errors
/// are reported at the `query!` call site.
fn set_span(expr: &mut Expr, span: Span) {
    let tokens: TokenStream2 = quote::ToTokens::to_token_stream(expr)
        .into_iter()
        .map(|mut tt| {
            tt.set_span(span);
            tt
        })
        .collect();
    if let Ok(respanned) = syn::parse2::<Expr>(tokens) {
        *expr = respanned;
    }
}
