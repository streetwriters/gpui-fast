//! For crates on GPUI Kit, which reach GPUI through it: with the `gpui-pre-facade` feature (which
//! the `compat/gpui-pre-macros` shim turns on), the `gpui::` and `gpui_platform::` paths a macro
//! emits name `gpui-kit` instead when the crate being compiled depends on it, as the gpui-pre
//! snapshots' macros do. A crate that doesn't depend on GPUI Kit gets the paths unchanged.
//! Taken from gpui-pre-macros 0.3.12 (`src/gpui_pre_facade_paths.rs`).

#[cfg(not(feature = "gpui-pre-facade"))]
pub(crate) fn rewrite(stream: proc_macro::TokenStream) -> proc_macro::TokenStream {
    stream
}

#[cfg(feature = "gpui-pre-facade")]
pub(crate) use facade::rewrite;

#[cfg(feature = "gpui-pre-facade")]
mod facade {
use proc_macro::{Group, Ident, Literal, Punct, Spacing, TokenStream, TokenTree};
use proc_macro_crate::{crate_name, FoundCrate};


enum FacadePath {
    Itself,
    Name(String),
}

pub(crate) fn rewrite(stream: TokenStream) -> TokenStream {
    let facade = match crate_name("gpui-kit") {
        Ok(FoundCrate::Name(name)) => Some(FacadePath::Name(name)),
        Ok(FoundCrate::Itself) => Some(FacadePath::Itself),
        Err(_) => None,
    };
    rewrite_stream(stream, facade.as_ref())
}

fn rewrite_stream(stream: TokenStream, facade: Option<&FacadePath>) -> TokenStream {
    let tokens: Vec<_> = stream.into_iter().collect();
    let mut output = TokenStream::new();
    for (index, token) in tokens.iter().enumerate() {
        match token {
            TokenTree::Group(group) => {
                let mut rewritten = Group::new(group.delimiter(), rewrite_stream(group.stream(), facade));
                rewritten.set_span(group.span());
                output.extend([TokenTree::Group(rewritten)]);
            }
            TokenTree::Ident(ident)
                if is_path_head(&tokens, index)
                    && (ident.to_string() == "gpui" || ident.to_string() == "gpui_platform") =>
            {
                append_path_head(&mut output, ident, facade);
            }
            TokenTree::Literal(literal) => {
                output.extend([TokenTree::Literal(rewrite_literal(literal, facade))]);
            }
            token => output.extend([token.clone()]),
        }
    }
    output
}

fn is_path_head(tokens: &[TokenTree], index: usize) -> bool {
    matches!(tokens.get(index + 1), Some(TokenTree::Punct(first)) if first.as_char() == ':')
        && matches!(tokens.get(index + 2), Some(TokenTree::Punct(second)) if second.as_char() == ':')
}

fn append_path_head(output: &mut TokenStream, original: &Ident, facade: Option<&FacadePath>) {
    let name = match facade {
        Some(FacadePath::Itself) => "crate",
        Some(FacadePath::Name(name)) => name,
        None => {
            output.extend([TokenTree::Ident(original.clone())]);
            return;
        }
    };
    output.extend([TokenTree::Ident(Ident::new(name, original.span()))]);
    if original.to_string() == "gpui_platform" {
        output.extend([
            TokenTree::Punct(Punct::new(':', Spacing::Joint)),
            TokenTree::Punct(Punct::new(':', Spacing::Alone)),
            TokenTree::Ident(Ident::new("platform", original.span())),
        ]);
    }
}

fn rewrite_literal(literal: &Literal, facade: Option<&FacadePath>) -> Literal {
    let (gpui, platform) = match facade {
        Some(FacadePath::Itself) => ("crate::".into(), "crate::platform::".into()),
        Some(FacadePath::Name(name)) =>
            (format!("::{name}::"), format!("::{name}::platform::")),
        None => return literal.clone(),
    };
    let text = literal.to_string();
    let rewritten = text
        .replace("::gpui_platform::", &platform)
        .replace("::gpui::", &gpui);
    if rewritten == text { return literal.clone() }
    let Ok(parsed) = rewritten.parse::<TokenStream>() else { return literal.clone() };
    let mut tokens = parsed.into_iter();
    match (tokens.next(), tokens.next()) {
        (Some(TokenTree::Literal(literal)), None) => literal,
        _ => literal.clone(),
    }
}

}
