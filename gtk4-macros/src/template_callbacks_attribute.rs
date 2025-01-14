// Take a look at the license at the top of the repository in the LICENSE file.

use crate::util::*;
use proc_macro2::{Ident, Span, TokenStream};
use proc_macro_error::{abort, abort_call_site};
use quote::{quote, ToTokens, TokenStreamExt};
use syn::{parse::Parse, Token};

pub const WRONG_PLACE_MSG: &str =
    "This macro should be used on the `impl` block for a CompositeTemplate widget";

mod keywords {
    syn::custom_keyword!(functions);
    syn::custom_keyword!(function);
    syn::custom_keyword!(name);
}

pub struct Args {
    functions: bool,
}

impl Parse for Args {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut args = Self { functions: false };
        while !input.is_empty() {
            let lookahead = input.lookahead1();
            if lookahead.peek(keywords::functions) {
                input.parse::<keywords::functions>()?;
                args.functions = true;
            } else {
                return Err(lookahead.error());
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}

pub struct CallbackArgs {
    name: Option<String>,
    function: Option<bool>,
}

impl CallbackArgs {
    fn is_function(&self, args: &Args) -> bool {
        self.function.unwrap_or(args.functions)
    }
    fn start(&self, args: &Args) -> usize {
        match self.is_function(args) {
            true => 1,
            false => 0,
        }
    }
}

impl Parse for CallbackArgs {
    fn parse(stream: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut args = Self {
            name: None,
            function: None,
        };
        if stream.is_empty() {
            return Ok(args);
        }
        let input;
        syn::parenthesized!(input in stream);
        while !input.is_empty() {
            let lookahead = input.lookahead1();
            if lookahead.peek(keywords::name) {
                let kw = input.parse::<keywords::name>()?;
                if args.name.is_some() {
                    return Err(syn::Error::new_spanned(kw, "Duplicate `name` attribute"));
                }
                input.parse::<Token![=]>()?;
                let name = input.parse::<syn::LitStr>()?;
                args.name.replace(name.value());
            } else if lookahead.peek(keywords::function) {
                let kw = input.parse::<keywords::function>()?;
                if args.function.is_some() {
                    return Err(syn::Error::new_spanned(
                        kw,
                        "Only one of `function` is allowed",
                    ));
                }
                let function = if input.peek(Token![=]) {
                    input.parse::<Token![=]>()?;
                    input.parse::<syn::LitBool>()?.value
                } else {
                    true
                };
                args.function.replace(function);
            } else {
                return Err(lookahead.error());
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}

pub fn impl_template_callbacks(mut input: syn::ItemImpl, args: Args) -> TokenStream {
    let syn::ItemImpl {
        attrs,
        generics,
        trait_,
        self_ty,
        items,
        ..
    } = &mut input;
    if trait_.is_some() {
        abort_call_site!(WRONG_PLACE_MSG);
    }
    let crate_ident = crate_ident_new();

    let mut callbacks = vec![];
    for item in items.iter_mut() {
        if let syn::ImplItem::Method(method) = item {
            let mut i = 0;
            let mut attr = None;
            while i < method.attrs.len() {
                if method.attrs[i].path.is_ident("template_callback") {
                    if attr.is_some() {
                        abort!(method.attrs[i], "Duplicate `template_callback` attribute");
                    }
                    attr.replace(method.attrs.remove(i));
                } else {
                    i += 1;
                }
            }

            let attr = match attr {
                Some(attr) => attr,
                None => continue,
            };

            let ident = &method.sig.ident;
            let callback_args =
                syn::parse2::<CallbackArgs>(attr.tokens).unwrap_or_else(|e| abort!(e));
            let name = callback_args
                .name
                .as_ref()
                .cloned()
                .unwrap_or_else(|| ident.to_string());
            let start = callback_args.start(&args);

            let call_site = Span::call_site();
            let mut arg_names = vec![];
            let mut has_rest = false;
            let value_unpacks = method.sig.inputs.iter_mut().enumerate().map(|(index, arg)| {
                if has_rest {
                    abort!(arg, "Arguments past argument with `rest` attribute");
                }
                let index = index + start;
                let name = Ident::new(&format!("value{}", index), call_site);
                arg_names.push(name.clone());
                let unwrap_value = |ty, err_msg| {
                    let index_err_msg = format!(
                        "Failed to get argument `{}` at index {}: Closure invoked with only {{}} arguments",
                        ident,
                        index
                    );
                    quote! {
                        let #name = <[#crate_ident::glib::Value]>::get(&values, #index)
                            .unwrap_or_else(|| panic!(#index_err_msg, values.len()));
                        let #name = #crate_ident::glib::Value::get::<#ty>(#name)
                            .unwrap_or_else(|e| panic!(#err_msg, e));
                    }
                };
                match arg {
                    syn::FnArg::Receiver(receiver) => {
                        if receiver.reference.is_none() || receiver.mutability.is_some() {
                            abort!(receiver, "Receiver must be &self");
                        }
                        let err_msg = format!(
                            "Wrong type for `self` in template callback `{}`: {{:?}}",
                            ident
                        );
                        let self_value_ty = quote! {
                            &<#self_ty as #crate_ident::glib::subclass::types::FromObject>::FromObjectType
                        };
                        let mut unwrap = unwrap_value(self_value_ty, err_msg);
                        unwrap.append_all(quote! {
                            let #name = <#self_ty as #crate_ident::glib::subclass::types::FromObject>::from_object(#name);
                        });
                        unwrap
                    },
                    syn::FnArg::Typed(typed) => {
                        let mut i = 0;
                        while i < typed.attrs.len() {
                            if typed.attrs[i].path.is_ident("rest") {
                                let rest = typed.attrs.remove(i);
                                if has_rest {
                                    abort!(rest, "Duplicate `rest` attribute");
                                }
                                if !rest.tokens.is_empty() {
                                    abort!(rest, "Tokens after `rest` attribute");
                                }
                                has_rest = true;
                            } else {
                                i += 1;
                            }
                        }
                        if has_rest {
                            let end = if callback_args.is_function(&args) {
                                quote! { (values.len() - #start) }
                            } else {
                                quote! { values.len() }
                            };
                            quote! {
                                let #name = &values[#index..#end];
                            }
                        } else {
                            let ty = typed.ty.as_ref();
                            let err_msg = format!(
                                "Wrong type for argument {} in template callback `{}`: {{:?}}",
                                index,
                                ident
                            );
                            unwrap_value(ty.to_token_stream(), err_msg)
                        }
                    }
                }
            }).collect::<Vec<_>>();

            let call = quote! { #self_ty::#ident(#(#arg_names),*) };
            let call = match method.sig.output {
                syn::ReturnType::Default => quote! {
                    #call;
                    None
                },
                syn::ReturnType::Type(_, _) => quote! {
                    let ret = #call;
                    Some(#crate_ident::glib::value::ToValue::to_value(&ret))
                },
            };

            callbacks.push(quote! {
                (#name, |values| {
                    #(#value_unpacks)*
                    #call
                })
            });
        }
    }

    quote! {
        #(#attrs)*
        impl #generics #self_ty {
            #(#items)*
        }

        impl #crate_ident::subclass::widget::CompositeTemplateCallbacks for #self_ty {
            const CALLBACKS: &'static [#crate_ident::subclass::widget::TemplateCallback] = &[
                #(#callbacks),*
            ];
        }
    }
}
