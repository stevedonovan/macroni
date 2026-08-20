use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    AngleBracketedGenericArguments, Attribute, FnArg, GenericArgument, Ident, ImplItem, Item,
    ItemImpl, ItemTrait, LitStr, MetaNameValue, Pat, PathArguments, PathSegment, ReturnType,
    Signature, Token, TraitItem, Type, Visibility, parse::Parser, parse_macro_input,
    punctuated::Punctuated, spanned::Spanned,
};

#[proc_macro_attribute]
pub fn api(arguments: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as Item);
    match item {
        Item::Impl(mut impl_item) => {
            let config = Config::default();
            // impl_item.
            let struct_name = match type_at_end_of_path(&impl_item.self_ty) {
                Ok(seq) => seq.ident.clone(),
                Err(err) => return err.into_compile_error().into(),
            };
            let visibility = Visibility::Public(syn::token::Pub::default());
            let methods = match get_impl_methods(&mut impl_item) {
                Ok(methods) => methods,
                Err(error) => return error.into_compile_error().into(),
            };
            let item = quote! { #impl_item };
            match expand(item, &config, struct_name, visibility, methods, true) {
                Ok(output) => output.into(),
                Err(error) => error.into_compile_error().into(),
            }
        }
        Item::Trait(mut trait_item) => {
            let config = match Config::parse(arguments) {
                Ok(config) => config,
                Err(error) => return error.into_compile_error().into(),
            };

            if !trait_item.generics.params.is_empty() {
                return syn::Error::new(
                    trait_item.generics.span(),
                    "generic API traits are not supported",
                )
                .into_compile_error()
                .into();
            }
            let trait_name = trait_item.ident.clone();
            let visibility = trait_item.vis.clone();
            let methods = match get_trait_methods(&mut trait_item) {
                Ok(methods) => methods,
                Err(error) => return error.into_compile_error().into(),
            };
            let item = quote! { #trait_item };
            match expand(item, &config, trait_name, visibility, methods, false) {
                Ok(output) => output.into(),
                Err(error) => error.into_compile_error().into(),
            }
        }
        _ => syn::Error::new(
            item.span(),
            "api macro only defined on traits and impl blocks",
        )
        .into_compile_error()
        .into(),
    }
}

#[derive(Default)]
struct Config {
    client_feature: Option<LitStr>,
    server_feature: Option<LitStr>,
}

impl Config {
    fn parse(arguments: TokenStream) -> syn::Result<Self> {
        if arguments.is_empty() {
            return Ok(Self::default());
        }
        let arguments =
            Punctuated::<MetaNameValue, Token![,]>::parse_terminated.parse(arguments)?;
        let mut config = Self::default();
        for argument in arguments {
            let value = match argument.value {
                syn::Expr::Lit(expression) => match expression.lit {
                    syn::Lit::Str(value) => value,
                    _ => {
                        return Err(syn::Error::new(
                            expression.span(),
                            "feature names must be string literals",
                        ));
                    }
                },
                expression => {
                    return Err(syn::Error::new(
                        expression.span(),
                        "feature names must be string literals",
                    ));
                }
            };
            if argument.path.is_ident("client_feature") && config.client_feature.is_none() {
                config.client_feature = Some(value);
            } else if argument.path.is_ident("server_feature") && config.server_feature.is_none() {
                config.server_feature = Some(value);
            } else {
                return Err(syn::Error::new(
                    argument.path.span(),
                    "expected `client_feature` or `server_feature` exactly once",
                ));
            }
        }
        if config.client_feature.is_none() || config.server_feature.is_none() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "specify both `client_feature` and `server_feature`, or neither",
            ));
        }
        Ok(config)
    }
}

#[derive(Clone, Copy)]
enum Verb {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Verb {
    fn uses_query(self) -> bool {
        matches!(self, Self::Get | Self::Delete)
    }
}

struct Parameter {
    name: Ident,
    ty: Type,
    in_path: bool,
    in_extension: bool,
}

struct Method {
    name: Ident,
    verb: Verb,
    path: LitStr,
    parameters: Vec<Parameter>,
    result_type: Type,
}

fn expand(
    item: proc_macro2::TokenStream,
    config: &Config,
    trait_name: Ident,
    visibility: Visibility,
    methods: Vec<Method>,
    was_impl: bool,
) -> syn::Result<proc_macro2::TokenStream> {
    let client_name = format_ident!("{}Client", trait_name);
    let client_builder_name = format_ident!("{}ClientBuilder", trait_name);
    let server_name = format_ident!("{}Server", trait_name);
    let module_name = format_ident!("__macroni_{}", trait_name.to_string().to_snake_case());
    let client_cfg = feature_cfg(config.client_feature.as_ref());
    let server_cfg = feature_cfg(config.server_feature.as_ref());
    // the custom serializable structs containing the parameters are needed for both client and server
    let transport_cfg = either_feature_cfg(
        config.client_feature.as_ref(),
        config.server_feature.as_ref(),
    );

    let client_methods = methods.iter().map(generate_client_method);
    let client_convenience_methods = methods
        .iter()
        .filter_map(|method| generate_client_convenience_method(method, &visibility));
    let handler_items = methods
        .iter()
        .map(|method| generate_handler(&trait_name, method, &server_cfg, &transport_cfg, was_impl));
    let routes = methods.iter().map(|m| generate_route(m, was_impl));

    let client_impl = if !was_impl {
        quote! {
                #client_cfg
                #[derive(Clone, Debug)]
                #visibility struct #client_name {
                    base_url: ::macroni::__private::reqwest::Url,
                    http: ::macroni::__private::reqwest::Client,
                    max_response_bytes: usize,
                }

                #client_cfg
                impl #client_name {
                    #visibility fn new(base_url: impl ::core::convert::AsRef<str>)
                        -> ::macroni::Result<Self>
                    {
                        Self::builder(base_url)?.build()
                    }

                    #visibility fn builder(base_url: impl ::core::convert::AsRef<str>)
                        -> ::macroni::Result<#client_builder_name>
                    {
                        #client_builder_name::new(base_url)
                    }

                    #visibility fn with_http_client(
                        base_url: impl ::core::convert::AsRef<str>,
                        http: ::macroni::__private::reqwest::Client,
                    ) -> ::macroni::Result<Self> {
                        let base_url = ::macroni::__private::reqwest::Url::parse(base_url.as_ref())
                            .map_err(|error| ::macroni::Error::protocol(
                                None,
                                ::std::format!("invalid API base URL: {error}"),
                                None,
                            ))?;
                        Ok(Self {
                            base_url,
                            http,
                            max_response_bytes: 8 * 1024 * 1024,
                        })
                    }

                    #(#client_convenience_methods)*
                }

                #client_cfg
                #visibility struct #client_builder_name {
                    base_url: ::macroni::__private::reqwest::Url,
                    http: ::macroni::__private::reqwest::ClientBuilder,
                    default_headers: ::macroni::__private::reqwest::header::HeaderMap,
                    max_response_bytes: usize,
                }

                #client_cfg
                impl #client_builder_name {
                    fn new(base_url: impl ::core::convert::AsRef<str>) -> ::macroni::Result<Self> {
                        let base_url = ::macroni::__private::reqwest::Url::parse(base_url.as_ref())
                            .map_err(|error| ::macroni::Error::protocol(
                                None,
                                ::std::format!("invalid API base URL: {error}"),
                                None,
                            ))?;
                        if !matches!(base_url.scheme(), "http" | "https") || base_url.cannot_be_a_base() {
                            return Err(::macroni::Error::protocol(
                                None,
                                "API base URL must be an absolute HTTP or HTTPS URL",
                                None,
                            ));
                        }
                        Ok(Self {
                            base_url,
                            http: ::macroni::__private::reqwest::Client::builder()
                                .timeout(::std::time::Duration::from_secs(10))
                                .connect_timeout(::std::time::Duration::from_secs(2)),
                            default_headers: ::macroni::__private::reqwest::header::HeaderMap::new(),
                            max_response_bytes: 8 * 1024 * 1024,
                        })
                    }

                    #visibility fn timeout(mut self, timeout: ::std::time::Duration) -> Self {
                        self.http = self.http.timeout(timeout);
                        self
                    }

                    #visibility fn connect_timeout(mut self, timeout: ::std::time::Duration) -> Self {
                        self.http = self.http.connect_timeout(timeout);
                        self
                    }

                    #visibility fn default_headers(
                        mut self,
                        headers: ::macroni::__private::reqwest::header::HeaderMap,
                    ) -> Self {
                        self.default_headers.extend(headers);
                        self
                    }

                    #visibility fn authorization(
                        mut self,
                        mut value: ::macroni::__private::reqwest::header::HeaderValue,
                    ) -> Self {
                        value.set_sensitive(true);
                        self.default_headers.insert(
                            ::macroni::__private::reqwest::header::AUTHORIZATION,
                            value,
                        );
                        self
                    }

                    #visibility fn bearer_token(
                        self,
                        token: impl ::core::convert::AsRef<str>,
                    ) -> ::macroni::Result<Self> {
                        let value = ::macroni::__private::reqwest::header::HeaderValue::from_str(
                            &::std::format!("Bearer {}", token.as_ref()),
                        ).map_err(|error| ::macroni::Error::protocol(
                            None,
                            ::std::format!("invalid bearer token: {error}"),
                            None,
                        ))?;
                        Ok(self.authorization(value))
                    }

                    #visibility fn response_body_limit(mut self, max_bytes: usize) -> Self {
                        self.max_response_bytes = max_bytes;
                        self
                    }

                    #visibility fn build(self) -> ::macroni::Result<#client_name> {
                        let http = self.http
                            .default_headers(self.default_headers)
                            .build()
                            .map_err(::macroni::Error::from)?;
                        Ok(#client_name {
                            base_url: self.base_url,
                            http,
                            max_response_bytes: self.max_response_bytes,
                        })
                    }
                }

                #client_cfg
                impl super::#trait_name for #client_name {
                    #(#client_methods)*
                }


        }
    } else {
        quote! {}
    };

    let router = if was_impl {
        // this is a pure implementation on a plain impl block (server)
        let struct_name = trait_name.clone();
        quote! {
            pub fn router(implementation: ::std::sync::Arc<#struct_name>)
              -> ::macroni::__private::axum::Router
            {
                ::macroni::__private::axum::Router::new()
                #(#routes)*
                .with_state(implementation)
            }
        }
    } else {
        // this is a trait-based implementation (client and/or server)
        quote! {
            #visibility fn router<T>(implementation: ::std::sync::Arc<T>)
              -> ::macroni::__private::axum::Router
            where
                T: super::#trait_name + Send + Sync + 'static,
            {
                ::macroni::__private::axum::Router::new()
                #(#routes)*
                .with_state(implementation)
            }
        }
    };

    let client_builder_export = if was_impl {
        quote! {}
    } else {
        quote! {
        #client_cfg
        #visibility use #module_name::{#client_builder_name, #client_name};
        }
    };

    Ok(quote! {
        #item

        #[doc(hidden)]
        mod #module_name {
            use super::*;

            #client_impl

            #server_cfg
            #visibility struct #server_name;

            #server_cfg
            impl #server_name {
                #router
            }

            #(#handler_items)*
        }

        #client_builder_export

        #server_cfg
        #visibility use #module_name::#server_name;
    })
}

fn get_trait_methods(trait_item: &mut ItemTrait) -> syn::Result<Vec<Method>> {
    let mut methods = Vec::new();

    for item in &mut trait_item.items {
        let TraitItem::Fn(function) = item else {
            return Err(syn::Error::new(
                item.span(),
                "API traits may contain methods only",
            ));
        };
        if function.default.is_some() {
            return Err(syn::Error::new(
                function.span(),
                "default API method implementations are not supported",
            ));
        }
        methods.push(parse_method(&mut function.sig, &mut function.attrs, true)?);
    }
    Ok(methods)
}

fn get_impl_methods(impl_item: &mut ItemImpl) -> syn::Result<Vec<Method>> {
    let mut methods = Vec::new();
    for item in &mut impl_item.items {
        match item {
            ImplItem::Fn(function) => {
                methods.push(parse_method(&mut function.sig, &mut function.attrs, false)?);
            }
            _ => {
                return Err(syn::Error::new(
                    impl_item.span(),
                    "only impl block methods are supported",
                ));
            }
        }
    }
    Ok(methods)
}

fn feature_cfg(feature: Option<&LitStr>) -> proc_macro2::TokenStream {
    match feature {
        Some(feature) => quote!(#[cfg(feature = #feature)]),
        None => quote!(),
    }
}

fn either_feature_cfg(
    client_feature: Option<&LitStr>,
    server_feature: Option<&LitStr>,
) -> proc_macro2::TokenStream {
    match (client_feature, server_feature) {
        (Some(client), Some(server)) => quote!(#[cfg(any(feature = #client, feature = #server))]),
        _ => quote!(),
    }
}

fn parse_method(
    sig: &mut Signature,
    attrs: &mut Vec<Attribute>,
    was_trait: bool,
) -> syn::Result<Method> {
    if !sig.generics.params.is_empty() {
        return Err(syn::Error::new(
            sig.generics.span(),
            "generic API methods are not supported",
        ));
    }
    if sig.asyncness.is_none() {
        return Err(syn::Error::new(
            sig.fn_token.span(),
            "API methods must be async",
        ));
    }

    let (verb, path) = take_route_attribute(attrs)?;
    let extension_names = take_extension_attributes(attrs)?;
    let path_names = placeholders(&path)?;
    let mut inputs = sig.inputs.iter();
    match inputs.next() {
        Some(FnArg::Receiver(receiver))
            if receiver.reference.is_some() && receiver.mutability.is_none() => {}
        _ => {
            return Err(syn::Error::new(
                sig.inputs.span(),
                "API methods must begin with an `&self` receiver",
            ));
        }
    }

    let mut parameters = Vec::new();
    for input in inputs {
        let FnArg::Typed(argument) = input else {
            return Err(syn::Error::new(input.span(), "unexpected receiver"));
        };
        let Pat::Ident(pattern) = argument.pat.as_ref() else {
            return Err(syn::Error::new(
                argument.pat.span(),
                "API parameters must use simple identifier patterns",
            ));
        };
        if pattern.by_ref.is_some() || pattern.mutability.is_some() || pattern.subpat.is_some() {
            return Err(syn::Error::new(
                pattern.span(),
                "API parameters must use simple identifier patterns",
            ));
        }
        parameters.push(Parameter {
            name: pattern.ident.clone(),
            ty: (*argument.ty).clone(),
            in_path: path_names
                .iter()
                .any(|name| name == &pattern.ident.to_string()),
            in_extension: extension_names
                .iter()
                .any(|name| name == &pattern.ident.to_string()),
        });
    }

    for placeholder in &path_names {
        if !parameters
            .iter()
            .any(|parameter| parameter.name == placeholder)
        {
            return Err(syn::Error::new(
                path.span(),
                format!("path parameter `{placeholder}` has no matching function parameter"),
            ));
        }
    }

    for extension in &extension_names {
        let Some(parameter) = parameters
            .iter()
            .find(|parameter| parameter.name == extension)
        else {
            return Err(syn::Error::new(
                sig.ident.span(),
                format!("extension parameter `{extension}` has no matching function parameter"),
            ));
        };
        if parameter.in_path {
            return Err(syn::Error::new(
                parameter.name.span(),
                "a parameter cannot be both a path parameter and an extension",
            ));
        }
    }

    let result_type = result_type(&sig.output)?;
    let ReturnType::Type(_, declared_output) = &sig.output else {
        unreachable!("result_type validated the return type")
    };
    if was_trait {
        let declared_output = declared_output.clone();
        sig.asyncness = None;
        sig.output = syn::parse_quote! {
            -> impl ::core::future::Future<Output = #declared_output> + Send
        };
    }

    Ok(Method {
        name: sig.ident.clone(),
        verb,
        path,
        parameters,
        result_type,
    })
}

fn take_extension_attributes(attributes: &mut Vec<Attribute>) -> syn::Result<Vec<String>> {
    let mut extensions = Vec::new();
    let mut error = None;
    attributes.retain(|attribute| {
        if !attribute.path().is_ident("extension") {
            return true;
        }
        match attribute.parse_args::<Ident>() {
            Ok(name) if extensions.contains(&name.to_string()) => {
                error = Some(syn::Error::new(
                    name.span(),
                    format!("duplicate extension parameter `{name}`"),
                ));
            }
            Ok(name) => extensions.push(name.to_string()),
            Err(parse_error) => error = Some(parse_error),
        }
        false
    });
    match error {
        Some(error) => Err(error),
        None => Ok(extensions),
    }
}

fn take_route_attribute(attributes: &mut Vec<Attribute>) -> syn::Result<(Verb, LitStr)> {
    let mut route = None;
    attributes.retain(|attribute| {
        let verb = if attribute.path().is_ident("get") {
            Some(Verb::Get)
        } else if attribute.path().is_ident("post") {
            Some(Verb::Post)
        } else if attribute.path().is_ident("put") {
            Some(Verb::Put)
        } else if attribute.path().is_ident("patch") {
            Some(Verb::Patch)
        } else if attribute.path().is_ident("delete") {
            Some(Verb::Delete)
        } else {
            None
        };
        let Some(verb) = verb else {
            return true;
        };
        if route.is_some() {
            route = Some(Err(syn::Error::new(
                attribute.span(),
                "API methods must have exactly one route attribute",
            )));
        } else {
            route = Some(attribute.parse_args::<LitStr>().map(|path| (verb, path)));
        }
        false
    });
    route.unwrap_or_else(|| {
        Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "API methods require a `#[get]`, `#[post]`, `#[put]`, `#[patch]`, or `#[delete]` route attribute",
        ))
    })
}

fn placeholders(path: &LitStr) -> syn::Result<Vec<String>> {
    let value = path.value();
    if !value.starts_with('/') {
        return Err(syn::Error::new(
            path.span(),
            "API paths must begin with `/`",
        ));
    }
    let mut names = Vec::new();
    for segment in value.split('/') {
        if segment.starts_with('{') || segment.ends_with('}') {
            if !(segment.starts_with('{') && segment.ends_with('}') && segment.len() > 2) {
                return Err(syn::Error::new(path.span(), "invalid path placeholder"));
            }
            let name = segment[1..segment.len() - 1].to_owned();
            if names.contains(&name) {
                return Err(syn::Error::new(
                    path.span(),
                    format!("duplicate path parameter `{name}`"),
                ));
            }
            names.push(name);
        }
    }
    Ok(names)
}

fn type_at_end_of_path(ty: &Box<Type>) -> syn::Result<&PathSegment> {
    let Type::Path(type_path) = ty.as_ref() else {
        return Err(syn::Error::new(ty.span(), "not a type path"));
    };
    let Some(segment) = type_path.path.segments.last() else {
        return Err(syn::Error::new(ty.span(), "no last path segment found"));
    };
    Ok(segment)
}

fn result_type(output: &ReturnType) -> syn::Result<Type> {
    let ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new(
            output.span(),
            "API methods must return a type",
        ));
    };
    let Ok(segment) = type_at_end_of_path(ty) else {
        return Err(syn::Error::new(
            ty.span(),
            "API methods must return `Result<T>`",
        ));
    };
    if segment.ident != "Result" {
        return Err(syn::Error::new(
            ty.span(),
            "API methods must return `Result<T>`",
        ));
    }
    let PathArguments::AngleBracketed(AngleBracketedGenericArguments { args, .. }) =
        &segment.arguments
    else {
        return Err(syn::Error::new(
            ty.span(),
            "API methods must return `Result<T>`",
        ));
    };
    match args.first() {
        Some(GenericArgument::Type(ty)) if args.len() == 1 => Ok(ty.clone()),
        _ => Err(syn::Error::new(
            args.span(),
            "API methods must return the `macroni::Result<T>` alias",
        )),
    }
}

fn generated_type_name(method: &Method, suffix: &str) -> Ident {
    format_ident!(
        "__{}{}",
        method.name.to_string().to_upper_camel_case(),
        suffix
    )
}

fn generate_client_method(method: &Method) -> proc_macro2::TokenStream {
    let name = &method.name;
    let result_type = &method.result_type;
    let arguments = method.parameters.iter().map(|parameter| {
        let name = &parameter.name;
        let ty = &parameter.ty;
        quote!(#name: #ty)
    });
    let ignored_extensions = method
        .parameters
        .iter()
        .filter(|p| p.in_extension)
        .map(|p| {
            let name = &p.name;
            quote!(let _ = #name;)
        });
    let request_body = generate_client_request(method);

    quote! {
        async fn #name(&self, #(#arguments),*) -> ::macroni::Result<#result_type> {
            #(#ignored_extensions)*
            #request_body
        }
    }
}

fn generate_client_convenience_method(
    method: &Method,
    visibility: &Visibility,
) -> Option<proc_macro2::TokenStream> {
    method.parameters.iter().any(|p| p.in_extension).then(|| {
        let name = &method.name;
        let result_type = &method.result_type;
        let arguments = method
            .parameters
            .iter()
            .filter(|p| !p.in_extension)
            .map(|parameter| {
                let name = &parameter.name;
                let ty = &parameter.ty;
                quote!(#name: #ty)
            });
        let request_body = generate_client_request(method);
        quote! {
            #visibility async fn #name(&self, #(#arguments),*) -> ::macroni::Result<#result_type> {
                #request_body
            }
        }
    })
}

fn generate_client_request(method: &Method) -> proc_macro2::TokenStream {
    let result_type = &method.result_type;
    // any path parameters are passed by replacing the placeholders (/path/{a}/{b})
    // with the percent-encoded values
    let path_replacements = method
        .parameters
        .iter()
        .filter(|p| p.in_path)
        .map(|parameter| {
            let name = &parameter.name;
            let placeholder = format!("{{{name}}}");
            quote! {
                path = path.replace(
                    #placeholder,
                    &::macroni::__private::encode_path_segment(&#name.to_string()),
                );
            }
        });
    let non_path: Vec<_> = method
        .parameters
        .iter()
        .filter(|p| !p.in_path && !p.in_extension)
        .collect();
    let payload_type = generated_type_name(
        method,
        if method.verb.uses_query() {
            "Query"
        } else {
            "Body"
        },
    );
    let payload_fields = non_path.iter().map(|parameter| {
        let name = &parameter.name;
        quote!(#name)
    });
    // Note the conventions here: GET & DELETE get via query, the rest via body.
    // We are using the generated structs to marshall the payload fields.
    let request = match (method.verb, non_path.is_empty()) {
        (Verb::Get, true) => quote!(self.http.get(url)),
        (Verb::Get, false) => quote!({
            let payload = #payload_type { #(#payload_fields),* };
            self.http.get(url).query(&payload)
        }),
        (Verb::Post, true) => quote!(self.http.post(url)),
        (Verb::Post, false) => quote!({
            let payload = #payload_type { #(#payload_fields),* };
            self.http.post(url).json(&payload)
        }),
        (Verb::Put, true) => quote!(self.http.put(url)),
        (Verb::Put, false) => quote!({
            let payload = #payload_type { #(#payload_fields),* };
            self.http.put(url).json(&payload)
        }),
        (Verb::Patch, true) => quote!(self.http.patch(url)),
        (Verb::Patch, false) => quote!({
            let payload = #payload_type { #(#payload_fields),* };
            self.http.patch(url).json(&payload)
        }),
        (Verb::Delete, true) => quote!(self.http.delete(url)),
        (Verb::Delete, false) => quote!({
            let payload = #payload_type { #(#payload_fields),* };
            self.http.delete(url).query(&payload)
        }),
    };
    let path = &method.path;

    quote! {
            let mut path = #path.to_owned();
            #(#path_replacements)*
            let url = self.base_url.join(&path).map_err(|error| {
                ::macroni::Error::protocol(
                    None,
                    ::std::format!("invalid generated request URL: {error}"),
                    None,
                )
            })?;
            let request = #request.header(
                ::macroni::__private::reqwest::header::ACCEPT,
                "application/json",
            );
            let response = request.send().await.map_err(::macroni::Error::from)?;
            ::macroni::__private::decode_response::<#result_type>(
                response,
                self.max_response_bytes,
            ).await
    }
}

// For each trait method, we are going to create an Axum handler which will call the method.
// The handler will be passed:
//  - the implementation as State
//  - any Extract parameters
//  - maybe a Path parameter
//  - either Query or Json parameter depending on whether we are GET or POST, etc
// The original formal parameters are grouped together in synthesized ser/de structs,
// so e.g. we will have handler parameters like `Query(Ty{name,id}): Query<Ty>`
// where `Ty` is the synthesized struct name.
fn generate_handler(
    trait_name: &Ident,
    method: &Method,
    server_cfg: &proc_macro2::TokenStream,
    transport_cfg: &proc_macro2::TokenStream,
    was_impl: bool,
) -> proc_macro2::TokenStream {
    let handler_name = format_ident!("__handle_{}", method.name);
    let result_type = &method.result_type;
    let path_parameters: Vec<_> = method.parameters.iter().filter(|p| p.in_path).collect();
    let non_path: Vec<_> = method
        .parameters
        .iter()
        .filter(|p| !p.in_path && !p.in_extension)
        .collect();
    let extension_parameters: Vec<_> = method
        .parameters
        .iter()
        .filter(|p| p.in_extension)
        .collect();
    let path_type = generated_type_name(method, "Path");
    let payload_type = generated_type_name(
        method,
        if method.verb.uses_query() {
            "Query"
        } else {
            "Body"
        },
    );
    let derive_path = struct_definition(&path_type, &path_parameters, false, server_cfg);
    let derive_payload = struct_definition(&payload_type, &non_path, true, transport_cfg);
    let path_extractor = if path_parameters.is_empty() {
        quote!()
    } else {
        let names = path_parameters.iter().map(|parameter| &parameter.name);
        quote!(::macroni::__private::Path(#path_type { #(#names),* }): ::macroni::__private::Path<#path_type>,)
    };
    let payload_extractor = if non_path.is_empty() {
        quote!()
    } else {
        let names = non_path.iter().map(|parameter| &parameter.name);
        if method.verb.uses_query() {
            quote!(::macroni::__private::Query(#payload_type { #(#names),* }): ::macroni::__private::Query<#payload_type>,)
        } else {
            quote!(::macroni::__private::Json(#payload_type { #(#names),* }): ::macroni::__private::Json<#payload_type>,)
        }
    };
    let extension_extractors = extension_parameters.iter().map(|parameter| {
        let name = &parameter.name;
        let ty = &parameter.ty;
        quote!(::macroni::__private::axum::Extension(#name): ::macroni::__private::axum::Extension<#ty>,)
    });
    let call_arguments = method.parameters.iter().map(|parameter| &parameter.name);
    let method_name = &method.name;

    let handler = if was_impl {
        // server-only handler implemented directly on the impl block
        let struct_name = trait_name;
        quote! {
        async fn #handler_name(
            ::macroni::__private::axum::extract::State(implementation):
                ::macroni::__private::axum::extract::State<::std::sync::Arc<#struct_name>>,
            #path_extractor
            #(#extension_extractors)*
            #payload_extractor
        ) -> ::macroni::Result<::macroni::__private::axum::Json<#result_type>>
        {
            let result = implementation.#method_name(#(#call_arguments),*).await?;
            Ok(::macroni::__private::axum::Json(result))
        }

        }
    } else {
        // and this handler is generic for any implementation of the trait (client and/or server)
        quote! {
        async fn #handler_name <T> (
            ::macroni::__private::axum::extract::State(implementation):
                ::macroni::__private::axum::extract::State<::std::sync::Arc<T>>,
            #path_extractor
            #(#extension_extractors)*
            #payload_extractor
        ) -> ::macroni::Result<::macroni::__private::axum::Json<#result_type>>
            where
                T: super::#trait_name + Send + Sync + 'static,
        {
            use super::#trait_name as _;
            let result = implementation.#method_name(#(#call_arguments),*).await?;
            Ok(::macroni::__private::axum::Json(result))
        }

        }
    };

    quote! {
        #derive_path
        #derive_payload

        #server_cfg
        #handler
    }
}
// we wrap method parameters into custom structs
fn struct_definition(
    name: &Ident,
    parameters: &[&Parameter],
    serialize: bool,
    item_cfg: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    if parameters.is_empty() {
        return quote!();
    }
    let fields = parameters.iter().map(|parameter| {
        let name = &parameter.name;
        let ty = &parameter.ty;
        quote!(#name: #ty)
    });
    let derive = if serialize {
        quote!(
            ::macroni::__private::serde::Serialize,
            ::macroni::__private::serde::Deserialize
        )
    } else {
        quote!(::macroni::__private::serde::Deserialize)
    };
    quote! {
        #item_cfg
        #[derive(#derive)]
        #[serde(crate = "::macroni::__private::serde")]
        struct #name { #(#fields),* }
    }
}

fn generate_route(method: &Method, use_impl: bool) -> proc_macro2::TokenStream {
    let path = &method.path;
    let handler = format_ident!("__handle_{}", method.name);
    let generic = if use_impl {
        quote! {}
    } else {
        quote!( ::<T> )
    };
    match method.verb {
        Verb::Get => {
            quote!(.route(#path, ::macroni::__private::axum::routing::get(#handler #generic)))
        }
        Verb::Post => {
            quote!(.route(#path, ::macroni::__private::axum::routing::post(#handler #generic)))
        }
        Verb::Put => {
            quote!(.route(#path, ::macroni::__private::axum::routing::put(#handler #generic)))
        }
        Verb::Patch => {
            quote!(.route(#path, ::macroni::__private::axum::routing::patch(#handler #generic)))
        }
        Verb::Delete => {
            quote!(.route(#path, ::macroni::__private::axum::routing::delete(#handler #generic)))
        }
    }
}
