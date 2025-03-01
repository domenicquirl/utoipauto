use std::vec;

use crate::file_utils::{extract_module_name_from_path, parse_files};
use crate::token_utils::Parameters;
use quote::ToTokens;
use syn::token::Comma;
use syn::Ident;
use syn::{punctuated::Punctuated, Attribute, GenericParam, Item, ItemFn, ItemImpl, Meta, Token};

/// Discover everything from a file, will explore folder recursively
pub fn discover_from_file(
    src_path: String,
    crate_name: String,
    params: &Parameters,
) -> (Vec<syn::Path>, Vec<syn::Path>, Vec<syn::Path>) {
    let files = parse_files(&src_path).unwrap_or_else(|_| panic!("Failed to parse file {}", src_path));

    files
        .into_iter()
        .map(|e| parse_module_items(extract_module_name_from_path(&e.0, &crate_name), e.1.items, params))
        .fold(Vec::<DiscoverType>::new(), |mut acc, mut v| {
            acc.append(&mut v);
            acc
        })
        .into_iter()
        .fold(
            (
                Vec::<syn::Path>::new(),
                Vec::<syn::Path>::new(),
                Vec::<syn::Path>::new(),
            ),
            |mut acc, v| {
                match v {
                    DiscoverType::Fn(n) => acc.0.push(n),
                    DiscoverType::Model(n) => acc.1.push(n),
                    DiscoverType::Response(n) => acc.2.push(n),
                    DiscoverType::CustomModelImpl(n) => acc.1.push(n),
                    DiscoverType::CustomResponseImpl(n) => acc.2.push(n),
                };

                acc
            },
        )
}

#[allow(unused)]
enum DiscoverType {
    Fn(syn::Path),
    Model(syn::Path),
    Response(syn::Path),
    CustomModelImpl(syn::Path),
    CustomResponseImpl(syn::Path),
}

fn parse_module_items(module_path: syn::Path, items: Vec<Item>, params: &Parameters) -> Vec<DiscoverType> {
    items
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                Item::Mod(_) | Item::Fn(_) | Item::Struct(_) | Item::Enum(_) | Item::Impl(_)
            )
        })
        .map(|v| match v {
            Item::Mod(m) => m.content.map_or(Vec::<DiscoverType>::new(), |cs| {
                parse_module_items(build_path(&module_path, &m.ident), cs.1, params)
            }),
            Item::Fn(f) => parse_function(&f, &params.fn_attribute_name)
                .into_iter()
                .map(|item| DiscoverType::Fn(build_path(&module_path, &item)))
                .collect(),
            Item::Struct(s) => parse_from_attr(&s.attrs, build_path(&module_path, &s.ident), s.generics.params, params),
            Item::Enum(e) => parse_from_attr(&e.attrs, build_path(&module_path, &e.ident), e.generics.params, params),
            Item::Impl(im) => parse_from_impl(&im, &module_path, params),
            _ => vec![],
        })
        .fold(Vec::<DiscoverType>::new(), |mut acc, mut v| {
            acc.append(&mut v);
            acc
        })
}

/// Search for ToSchema and ToResponse implementations in attr
fn parse_from_attr(
    a: &Vec<Attribute>,
    name: syn::Path,
    generic_params: Punctuated<GenericParam, Comma>,
    params: &Parameters,
) -> Vec<DiscoverType> {
    let mut out: Vec<DiscoverType> = vec![];
    if !generic_params.is_empty() {
        return out;
    }

    for attr in a {
        let meta = &attr.meta;
        if meta.path().is_ident("utoipa_ignore") {
            return vec![];
        }
        if meta.path().is_ident("derive") {
            let nested = attr
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .expect("Failed to parse derive attribute");
            for nested_meta in nested {
                if nested_meta.path().segments.len() == 2 && nested_meta.path().segments[0].ident == "utoipa" {
                    match nested_meta.path().segments[1].ident.to_string().as_str() {
                        "ToSchema" => out.push(DiscoverType::Model(name.clone())),
                        "ToResponse" => out.push(DiscoverType::Response(name.clone())),
                        _ => {}
                    }
                } else {
                    if nested_meta.path().is_ident(&params.schema_attribute_name) {
                        out.push(DiscoverType::Model(name.clone()));
                    }
                    if nested_meta.path().is_ident(&params.response_attribute_name) {
                        out.push(DiscoverType::Response(name.clone()));
                    }
                }
            }
        }
    }

    out
}

fn parse_from_impl(im: &ItemImpl, module_base_path: &syn::Path, params: &Parameters) -> Vec<DiscoverType> {
    im.trait_
        .as_ref()
        .and_then(|trt| trt.1.segments.last().map(|p| p.ident.to_string()))
        .and_then(|impl_name| {
            if impl_name.eq(params.schema_attribute_name.as_str()) {
                Some(vec![DiscoverType::CustomModelImpl(build_path(
                    module_base_path,
                    &im.self_ty,
                ))])
            } else if impl_name.eq(params.response_attribute_name.as_str()) {
                Some(vec![DiscoverType::CustomResponseImpl(build_path(
                    module_base_path,
                    &im.self_ty,
                ))])
            } else {
                None
            }
        })
        .unwrap_or_default()
}

fn parse_function(f: &ItemFn, fn_attributes_name: &str) -> Vec<Ident> {
    let mut fns_name: Vec<Ident> = vec![];
    if should_parse_fn(f) {
        for i in 0..f.attrs.len() {
            if f.attrs[i]
                .meta
                .path()
                .segments
                .iter()
                .any(|item| item.ident.eq(fn_attributes_name))
            {
                fns_name.push(f.sig.ident.clone());
            }
        }
    }
    fns_name
}

fn should_parse_fn(f: &ItemFn) -> bool {
    !f.attrs.is_empty() && !is_ignored(f)
}

fn is_ignored(f: &ItemFn) -> bool {
    f.attrs.iter().any(|attr| {
        if let Some(name) = attr.path().get_ident() {
            name.eq("utoipa_ignore")
        } else {
            false
        }
    })
}

fn build_path(file_path: &syn::Path, fn_name: impl ToTokens) -> syn::Path {
    syn::parse_quote!(#file_path::#fn_name)
}

#[cfg(test)]
mod test {
    use quote::quote;
    use syn::ItemFn;

    #[test]
    fn test_parse_function() {
        let quoted = quote! {
            #[utoipa]
            pub fn route_custom() {}
        };

        let item_fn: ItemFn = syn::parse2(quoted).unwrap();
        let fn_name = super::parse_function(&item_fn, "utoipa");
        assert_eq!(fn_name, vec!["route_custom"]);

        let quoted = quote! {
            #[handler]
            pub fn route_custom() {}
        };

        let item_fn: ItemFn = syn::parse2(quoted).unwrap();
        let fn_name = super::parse_function(&item_fn, "handler");
        assert_eq!(fn_name, vec!["route_custom"]);
    }
}
