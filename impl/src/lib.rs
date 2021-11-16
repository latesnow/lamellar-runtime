extern crate proc_macro;

mod parse;
mod replace;

use crate::parse::ReductionArgs;
use crate::replace::LamellarDSLReplace;

use proc_macro::TokenStream;
use proc_macro2::Span;
use proc_macro_error::proc_macro_error;
use quote::{quote, quote_spanned, ToTokens};
use syn::parse_macro_input;
use syn::spanned::Spanned;
use syn::visit_mut::VisitMut;

fn type_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(syn::TypePath { qself: None, path }) => {
            Some(path.segments.last().unwrap().ident.to_string())
        }
        _ => None,
    }
}

#[allow(dead_code)]
fn get_impl_associated_type(name: String, tys: &Vec<syn::ImplItem>) -> Option<syn::Type> {
    for ty in tys {
        match ty {
            syn::ImplItem::Type(ref item) => {
                if item.ident.to_string() == name {
                    return Some(item.ty.clone());
                }
            }
            _ => (),
        }
    }
    None
}

fn get_return_of_method(name: String, tys: &Vec<syn::ImplItem>) -> Option<syn::Type> {
    for ty in tys {
        match ty {
            syn::ImplItem::Method(ref item) => {
                if item.sig.ident.to_string() == name {
                    match item.sig.output.clone() {
                        syn::ReturnType::Default => {
                            return None;
                        }
                        syn::ReturnType::Type(_, item) => {
                            return Some(*item);
                        }
                    }
                }
            }
            _ => (),
        }
    }
    None
}

fn get_impl_method(name: String, tys: &Vec<syn::ImplItem>) -> Option<syn::Block> {
    for ty in tys {
        match ty {
            syn::ImplItem::Method(ref item) => {
                if item.sig.ident.to_string() == name {
                    return Some(item.block.clone());
                }
            }
            _ => (),
        }
    }
    None
}

fn get_expr(stmt: &syn::Stmt) -> Option<syn::Expr> {
    let expr = match stmt {
        syn::Stmt::Semi(expr, _semi) => match expr.clone() {
            syn::Expr::Return(expr) => Some(*(expr.expr.unwrap())),
            _ => None,
        },
        syn::Stmt::Expr(expr) => Some(expr.clone()),
        _ => {
            println!("something else!");
            None
        }
    };
    expr
}

fn replace_lamellar_dsl(mut stmt: syn::Stmt) -> syn::Stmt {
    LamellarDSLReplace.visit_stmt_mut(&mut stmt);
    stmt
}

enum AmType {
    NoReturn,
    ReturnData(syn::Type),
    ReturnAm(proc_macro2::TokenStream),
}

fn get_return_am_return_type(args: String) -> proc_macro2::TokenStream {
    let return_am = args
        .split("return_am")
        .collect::<Vec<&str>>()
        .last()
        .expect("error in lamellar::am argument")
        .trim_matches(&['=', ' ', '"'][..])
        .to_string();
    let mut return_type = "".to_string();
    if return_am.find("->") != None {
        let temp = return_am.split("->").collect::<Vec<&str>>();
        return_type = temp
            .last()
            .expect("error in lamellar::am argument")
            .trim()
            .to_string();
    }
    if return_type.len() > 0 {
        // let ident = syn::Ident::new(&return_type, Span::call_site());
        let ret_type: syn::Type = syn::parse_str(&return_type).expect("invalid type");
        quote! {#ret_type}
    } else {
        quote! {()}
    }
}

fn generate_am(input: syn::ItemImpl, local: bool, rt: bool, am_type: AmType) -> TokenStream {
    let name = type_name(&input.self_ty).expect("unable to find name");
    let lamellar = if rt {
        quote::format_ident!("crate")
    } else {
        quote::format_ident!("__lamellar")
    };

    let generics = input.generics.clone();
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let mut exec_fn =
        get_impl_method("exec".to_string(), &input.items).expect("unable to extract exec body");
    let func_span = exec_fn.span();
    let last_expr = if let Some(stmt) = exec_fn.stmts.pop() {
        let last_stmt = replace_lamellar_dsl(stmt.clone());
        match am_type {
            AmType::NoReturn => quote_spanned! {last_stmt.span()=>
                #last_stmt
                #lamellar::LamellarReturn::Unit
            },
            AmType::ReturnData(_) | AmType::ReturnAm(_) => {
                let temp = get_expr(&last_stmt)
                    .expect("failed to get exec return value (try removing the last \";\")");
                quote_spanned! {last_stmt.span()=> #temp}
            }
        }
    } else {
        quote_spanned! {func_span=> #lamellar::LamellarReturn::Unit }
    };

    let mut temp = quote_spanned! {func_span=>};
    let stmts = exec_fn.stmts;

    for stmt in stmts {
        let new_stmt = replace_lamellar_dsl(stmt.clone());
        temp.extend(quote_spanned! {stmt.span()=>
            #new_stmt
        });
    }

    let orig_name = syn::Ident::new(&name, Span::call_site());
    let orig_name_unpack = quote::format_ident!("{}_unpack", orig_name.clone());

    let ret_type = {
        match am_type {
            AmType::NoReturn => quote! {()},
            AmType::ReturnData(ref output) => quote! {#output},
            AmType::ReturnAm(ref output) => {
                quote! {#output}
            }
        }
    };
    let ret_struct = quote::format_ident!("{}Result", orig_name);
    let mut am = quote! {
        impl #impl_generics #lamellar::LocalAM for #orig_name #ty_generics #where_clause{
            type Output = #ret_type;
        }
    };
    if !local {
        am.extend(quote!{
            impl #impl_generics #lamellar::LamellarAM for #orig_name #ty_generics #where_clause {
                type Output = #ret_type;
            }

            impl  #impl_generics #lamellar::Serde for #orig_name #ty_generics #where_clause {}

            impl #impl_generics #lamellar::LamellarSerde for #orig_name #ty_generics #where_clause {
                fn serialized_size(&self)->usize{
                    // println!("serialized size: {:?}", #lamellar::serialized_size(self));
                    #lamellar::serialized_size(self)
                }
                fn serialize_into(&self,buf: &mut [u8]){
                    // println!("buf_len: {:?} serialized size: {:?}",buf.len(), #lamellar::serialized_size(self));
                    #lamellar::serialize_into(buf,self).unwrap();
                }
            }

            impl #impl_generics #lamellar::LamellarResultSerde for #orig_name #ty_generics #where_clause {
                fn serialized_result_size(&self,result: &Box<dyn std::any::Any + Send + Sync>)->usize{
                    let result  = result.downcast_ref::<#ret_type>().unwrap();
                    #lamellar::serialized_size(result)
                }
                fn serialize_result_into(&self,buf: &mut [u8],result: &Box<dyn std::any::Any + Send + Sync>){
                    let result  = result.downcast_ref::<#ret_type>().unwrap();
                    #lamellar::serialize_into(buf,result).unwrap();
                }
            }
        });
    }

    let ret_statement = match am_type {
        AmType::NoReturn => quote! {
            #last_expr
        },
        AmType::ReturnData(_) => {
            quote! {
                let ret = match __local{ //should probably just separate these into exec_local exec_remote to get rid of a conditional...
                    true => #lamellar::LamellarReturn::LocalData(Box::new(#last_expr)),
                    false => #lamellar::LamellarReturn::RemoteData(std::sync::Arc::new (#ret_struct{
                        val: #last_expr
                    })),
                };
                ret
            }
        }
        AmType::ReturnAm(_) => {
            quote! {
                let ret = match __local{
                    true => #lamellar::LamellarReturn::LocalAm(
                        std::sync::Arc::new (
                            #last_expr
                        )
                    ),
                    false => #lamellar::LamellarReturn::RemoteAm(
                        std::sync::Arc::new (
                            #last_expr
                        )
                    ),
                };
                ret
            }
        }
    };

    let mut expanded = quote_spanned! {temp.span()=>
        impl #impl_generics #lamellar::LamellarActiveMessage for #orig_name #ty_generics #where_clause {
            fn exec(self: std::sync::Arc<Self>,__lamellar_current_pe: usize,__lamellar_num_pes: usize, __local: bool, __lamellar_world: std::sync::Arc<#lamellar::LamellarTeam>, __lamellar_team: std::sync::Arc<#lamellar::LamellarTeam>) -> std::pin::Pin<Box<dyn std::future::Future<Output=#lamellar::LamellarReturn> + Send>>{
                Box::pin( async move {
                #temp
                #ret_statement
                })

            }
            fn get_id(&self) -> String{
                stringify!(#orig_name).to_string()
            }
        }

        #am
    };

    if let AmType::ReturnData(_) = am_type {
        expanded.extend(quote_spanned! {temp.span()=>
            struct #ret_struct{
                val: #ret_type
            }

            impl #generics #lamellar::LamellarSerde for #ret_struct {
                fn serialized_size(&self)->usize{
                    #lamellar::serialized_size(&self.val)
                }
                fn serialize_into(&self,buf: &mut [u8]){
                    #lamellar::serialize_into(buf,&self.val).unwrap();
                }
            }
        });
    }

    if !local {
        expanded.extend( quote_spanned! {temp.span()=>
            impl #impl_generics #lamellar::RemoteActiveMessage for #orig_name #ty_generics #where_clause {}

            fn #orig_name_unpack(bytes: &[u8], cur_pe: Result<usize,#lamellar::IdError>) -> std::sync::Arc<dyn #lamellar::RemoteActiveMessage + Send + Sync>  {
                let __lamellar_data: std::sync::Arc<#orig_name> = std::sync::Arc::new(#lamellar::deserialize(&bytes).unwrap());
                <#orig_name as #lamellar::DarcSerde>::des(&__lamellar_data,cur_pe);
                __lamellar_data
            }

            #lamellar::inventory::submit! {
                #![crate = #lamellar]
                #lamellar::RegisteredAm{
                    exec: #orig_name_unpack,
                    name: stringify!(#orig_name).to_string()
                }
            }
        });
    }

    let user_expanded = quote_spanned! {expanded.span()=>
        const _: () = {
            extern crate lamellar as __lamellar;
            #expanded
        };
    };

    if lamellar == "crate" {
        TokenStream::from(expanded)
    } else {
        TokenStream::from(user_expanded)
    }
}

fn derive_am_data(
    input: TokenStream,
    args: TokenStream,
    crate_header: String,
    local: bool,
) -> TokenStream {
    let lamellar = quote::format_ident!("{}", crate_header.clone());
    let input: syn::Item = parse_macro_input!(input);
    let mut output = quote! {};

    if let syn::Item::Struct(data) = input {
        let name = &data.ident;
        let generics = data.generics.clone();
        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        let mut fields = quote! {};
        let mut ser = quote! {};
        let mut des = quote! {};

        for field in &data.fields {
            if let syn::Type::Path(ref ty) = field.ty {
                if let Some(_seg) = ty.path.segments.first() {
                    if !local {
                        let field_name = field.ident.clone();
                        fields.extend(quote_spanned! {field.span()=>
                            #field,
                        });
                        ser.extend(quote_spanned! {field.span()=>
                            (&self.#field_name).ser(num_pes,cur_pe);
                        });
                        des.extend(quote_spanned! {field.span()=>
                            (&self.#field_name).des(cur_pe);
                        });
                    } else {
                        fields.extend(quote_spanned! {field.span()=>
                            #field,
                        });
                    }
                }
            } else if let syn::Type::Tuple(ref ty) = field.ty {
                let field_name = field.ident.clone();
                let mut ind = 0;
                for elem in &ty.elems {
                    if let syn::Type::Path(ref ty) = elem {
                        if let Some(_seg) = ty.path.segments.first() {
                            if !local {
                                let temp_ind = syn::Index {
                                    index: ind,
                                    span: field.span(),
                                };
                                ind += 1;
                                ser.extend(quote_spanned! {field.span()=>

                                    (self.#field_name).#temp_ind.ser(num_pes,cur_pe);
                                });
                                des.extend(quote_spanned! {field.span()=>
                                    (self.#field_name).#temp_ind.des(cur_pe);
                                });
                            }
                        }
                    }
                }
                fields.extend(quote_spanned! {field.span()=>
                    #field,
                });
            }
        }
        let my_ser: syn::Path = syn::parse(
            format!("{}::serde::Serialize", crate_header)
                .parse()
                .unwrap(),
        )
        .unwrap();
        let my_de: syn::Path = syn::parse(
            format!("{}::serde::Deserialize", crate_header)
                .parse()
                .unwrap(),
        )
        .unwrap();
        let mut impls = quote! {};
        if !local {
            impls.extend(quote! { #my_ser, #my_de, });
        } else {
            ser.extend(quote! {panic!{"should not serialize data in LocalAM"}});
        }
        for t in args.to_string().split(",") {
            if !t.contains("serde::Serialize")
                && !t.contains("serde::Deserialize")
                && t.trim().len() > 0
            {
                let temp = quote::format_ident!("{}", t.trim());
                impls.extend(quote! {#temp, });
            }
        }
        let traits = quote! { #[derive( #impls)] };
        let serde_temp_2 = if crate_header != "crate" && !local {
            quote! {#[serde(crate = "lamellar::serde")]}
        } else {
            quote! {}
        };
        let vis = data.vis.to_token_stream();
        output.extend(quote! {
            #traits
            #serde_temp_2
            #vis struct #name#ty_generics #where_clause{
                #fields
            }
            impl #impl_generics#lamellar::DarcSerde for #name #ty_generics #where_clause{
                fn ser (&self,  num_pes: usize, cur_pe: Result<usize, #lamellar::IdError>) {
                    // println!("in outer ser");
                    #ser
                }
                fn des (&self,cur_pe: Result<usize, #lamellar::IdError>){
                    // println!("in outer des");
                    #des
                }
            }
        });
    }
    TokenStream::from(output)
}

fn derive_darcserde(input: TokenStream, crate_header: String) -> TokenStream {
    let lamellar = quote::format_ident!("{}", crate_header.clone());
    println!("input: {:?}", input);
    let input: syn::Item = parse_macro_input!(input);
    let mut output = quote! {};

    if let syn::Item::Struct(data) = input {
        let name = &data.ident;
        let generics = data.generics.clone();
        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        let mut fields = quote! {};
        let mut ser = quote! {};
        let mut des = quote! {};

        for field in &data.fields {
            if let syn::Type::Path(ref ty) = field.ty {
                if let Some(seg) = ty.path.segments.first() {
                    let field_name = field.ident.clone();
                    if seg.ident.to_string().contains("Darc") {
                        let (serialize, deserialize) =
                            if seg.ident.to_string().contains("LocalRwDarc") {
                                let serialize = format!("{}::localrw_serialize", crate_header);
                                let deserialize = format!("{}::localrw_from_ndarc", crate_header);
                                (serialize, deserialize)
                            } else if seg.ident.to_string().contains("GlobalRwDarc") {
                                let serialize = format!("{}::globalrw_serialize", crate_header);
                                let deserialize = format!("{}::globalrw_from_ndarc", crate_header);
                                (serialize, deserialize)
                            } else {
                                let serialize = format!("{}::darc_serialize", crate_header);
                                let deserialize = format!("{}::darc_from_ndarc", crate_header);
                                (serialize, deserialize)
                            };

                        fields.extend(quote_spanned! {field.span()=>
                            #[serde(serialize_with = #serialize, deserialize_with = #deserialize)]
                            #field,
                        });

                        ser.extend(quote_spanned!{field.span()=>
                            match cur_pe{
                                Ok(cur_pe) => {self.#field_name.serialize_update_cnts(num_pes,cur_pe);},
                                Err(err) =>  {panic!("can only access darcs within team members ({:?})",err);}
                            }
                        });
                        des.extend(quote_spanned!{field.span()=>
                            match cur_pe{
                                Ok(cur_pe) => {self.#field_name.deserialize_update_cnts(cur_pe);},
                                Err(err) => {panic!("can only access darcs within team members ({:?})",err);}
                            }
                        });
                    } else {
                        fields.extend(quote_spanned! {field.span()=>
                            #field,
                        });
                    }
                }
            }
        }

        output.extend(quote! {
            impl #impl_generics#lamellar::DarcSerde for #name #ty_generics #where_clause{
                fn ser (&self,  num_pes: usize, cur_pe: Result<usize, #lamellar::IdError>) {
                    #ser
                }
                fn des (&self,cur_pe: Result<usize, #lamellar::IdError>){
                    #des
                }
            }
        });
    }
    TokenStream::from(output)
}

#[proc_macro_derive(DarcSerdeRT)]
pub fn darc_serde_rt_derive(input: TokenStream) -> TokenStream {
    derive_darcserde(input, "crate".to_string())
}

#[allow(non_snake_case)]
#[proc_macro_error]
#[proc_macro_attribute]
pub fn AmData(args: TokenStream, input: TokenStream) -> TokenStream {
    derive_am_data(input, args, "lamellar".to_string(), false)
}

#[allow(non_snake_case)]
#[proc_macro_error]
#[proc_macro_attribute]
pub fn AmLocalData(args: TokenStream, input: TokenStream) -> TokenStream {
    derive_am_data(input, args, "lamellar".to_string(), true)
}

#[allow(non_snake_case)]
#[proc_macro_error]
#[proc_macro_attribute]
pub fn AmDataRT(args: TokenStream, input: TokenStream) -> TokenStream {
    derive_am_data(input, args, "crate".to_string(), false)
}

#[allow(non_snake_case)]
#[proc_macro_error]
#[proc_macro_attribute]
pub fn AmLocalDataRT(args: TokenStream, input: TokenStream) -> TokenStream {
    derive_am_data(input, args, "crate".to_string(), true)
}

fn parse_am(args: TokenStream, input: TokenStream, local: bool, rt: bool) -> TokenStream {
    let args = args.to_string();
    if args.len() > 0 {
        assert!(args.starts_with("return_am"), "#[lamellar::am] only accepts an (optional) argument of the form:
                                               #[lamellar::am(return_am = \"<am to exec upon return>\")]");
    }
    // println!("args: {:?}", args);
    let input: syn::Item = parse_macro_input!(input);

    let output = match input.clone() {
        syn::Item::Impl(input) => {
            let output = get_return_of_method("exec".to_string(), &input.items);
            match output {
                Some(output) => {
                    if args.len() > 0 {
                        let output = get_return_am_return_type(args);
                        generate_am(input, local, rt, AmType::ReturnAm(output))
                    } else {
                        generate_am(input, local, rt, AmType::ReturnData(output))
                    }
                }

                None => {
                    if args.len() > 0 {
                        panic!("no return type detected (try removing the last \";\"), but parameters passed into [#lamellar::am]
                        #[lamellar::am] only accepts an (optional) argument of the form:
                        #[lamellar::am(return_am = \"<am to exec upon return>\")]");
                    }
                    generate_am(input, local, rt, AmType::NoReturn)
                }
            }
        }
        _ => {
            println!("lamellar am attribute only valid for impl blocks");
            let output = quote! { #input };
            output.into()
        }
    };
    output
}

#[proc_macro_error]
#[proc_macro_attribute]
pub fn am(args: TokenStream, input: TokenStream) -> TokenStream {
    parse_am(args, input, false, false)
}

#[proc_macro_error]
#[proc_macro_attribute]
pub fn local_am(args: TokenStream, input: TokenStream) -> TokenStream {
    parse_am(args, input, true, false)
}

#[proc_macro_error]
#[proc_macro_attribute]
pub fn rt_am(args: TokenStream, input: TokenStream) -> TokenStream {
    parse_am(args, input, false, true)
}

#[proc_macro_error]
#[proc_macro_attribute]
pub fn rt_am_local(args: TokenStream, input: TokenStream) -> TokenStream {
    parse_am(args, input, true, true)
}

fn create_reduction(
    typeident: syn::Ident,
    reduction: String,
    op: proc_macro2::TokenStream,
    array_types: &Vec<syn::Ident>,
    rt: bool,
) -> proc_macro2::TokenStream {
    let lamellar = if rt {
        quote::format_ident!("crate")
    } else {
        quote::format_ident!("__lamellar")
    };

    let (am_data, am): (syn::Path, syn::Path) = if rt {
        (
            syn::parse("lamellar_impl::AmDataRT".parse().unwrap()).unwrap(),
            syn::parse("lamellar_impl::rt_am".parse().unwrap()).unwrap(),
        )
    } else {
        (
            syn::parse("lamellar::AmData".parse().unwrap()).unwrap(),
            syn::parse("lamellar::am".parse().unwrap()).unwrap(),
        )
    };
    let reduction = quote::format_ident!("{:}", reduction);
    let reduction_gen = quote::format_ident!("{:}_{:}_reduction_gen", typeident, reduction);

    let mut gen_match_stmts = quote! {};
    let mut array_impls = quote! {};

    for array_type in array_types {
        let reduction_name =
            quote::format_ident!("{:}_{:}_{:}_reduction", array_type, typeident, reduction);

        gen_match_stmts.extend(quote!{
            #lamellar::array::LamellarReadArray::#array_type(inner) => std::sync::Arc::new(#reduction_name{
                data: inner.clone().to_base_inner::<#typeident>() , start_pe: 0, end_pe: num_pes-1}),
        });

        array_impls.extend(quote!{
            #[allow(non_camel_case_types)]
            #[#am_data(Clone)]
            struct #reduction_name{
                data: #lamellar::array::#array_type<#typeident>,
                start_pe: usize,
                end_pe: usize,
            }

            #[#am]
            impl LamellarAM for #reduction_name{
                fn exec(&self) -> #typeident{
                    if self.start_pe == self.end_pe{
                        // println!("[{:?}] root {:?} {:?}",__lamellar_current_pe,self.start_pe, self.end_pe);
                        let timer = std::time::Instant::now();
                        let data_slice = self.data.local_as_slice();
                        // println!("data: {:?}",data_slice);
                        let first = data_slice.first().unwrap().clone();
                        let res = data_slice[1..].iter().fold(first, #op );
                        // println!("[{:?}] {:?} {:?}",__lamellar_current_pe,res,timer.elapsed().as_secs_f64());
                        res
                    }
                    else{
                        // println!("[{:?}] recurse {:?} {:?}",__lamellar_current_pe,self.start_pe, self.end_pe);
                        let mid_pe = (self.start_pe + self.end_pe)/2;
                        let op = #op;
                        let timer = std::time::Instant::now();
                        let left = __lamellar_team.exec_am_pe( self.start_pe,  #reduction_name { data: self.data.clone(), start_pe: self.start_pe, end_pe: mid_pe}).into_future();
                        let right = __lamellar_team.exec_am_pe( mid_pe+1, #reduction_name { data: self.data.clone(), start_pe: mid_pe+1, end_pe: self.end_pe}).into_future();
                        let res = op(left.await.unwrap(),&right.await.unwrap());

                        // println!("[{:?}] {:?} {:?}",__lamellar_current_pe,res,timer.elapsed().as_secs_f64());
                        res
                    }
                }
            }
        });
    }

    let expanded = quote! {
        fn  #reduction_gen<T: #lamellar::Arraydist > (data: #lamellar::array::LamellarReadArray<T>, num_pes: usize)
        -> std::sync::Arc<dyn #lamellar::RemoteActiveMessage + Send + Sync >{
            match data{
                #gen_match_stmts
            }

        }

        #lamellar::inventory::submit! {
            #![crate = #lamellar]
            #lamellar::ReduceKey{
                id: std::any::TypeId::of::<#typeident>(),
                name: stringify!(#reduction).to_string(),
                gen: #reduction_gen
            }
        }

        #array_impls
    };

    let user_expanded = quote_spanned! {expanded.span()=>
        const _: () = {
            extern crate lamellar as __lamellar;
            use __lamellar::array::LamellarArrayRead;
            #expanded
        };
    };
    if lamellar == "crate" {
        expanded
    } else {
        user_expanded
    }
}

fn create_add(
    typeident: syn::Ident,
    array_types: &Vec<syn::Ident>,
    rt: bool,
) -> proc_macro2::TokenStream {
    let lamellar = if rt {
        quote::format_ident!("crate")
    } else {
        quote::format_ident!("__lamellar")
    };

    let (am_data, am): (syn::Path, syn::Path) = if rt {
        (
            syn::parse("lamellar_impl::AmDataRT".parse().unwrap()).unwrap(),
            syn::parse("lamellar_impl::rt_am".parse().unwrap()).unwrap(),
        )
    } else {
        (
            syn::parse("lamellar::AmData".parse().unwrap()).unwrap(),
            syn::parse("lamellar::am".parse().unwrap()).unwrap(),
        )
    };

    let mut match_stmts = quote! {};
    let mut array_impls = quote! {};
    for array_type in array_types {
        let add_name_am = quote::format_ident!("{:}_{:}_add_am", array_type, typeident);
        match_stmts.extend(quote! {
            #lamellar::array::LamellarWriteArray::#array_type(inner) => inner.add(index,val),
        });
        array_impls.extend(quote!{
            #[allow(non_camel_case_types)]
            impl #lamellar::array::ArrayOps<#typeident> for #lamellar::array::#array_type<#typeident>{
                fn add(&self,index: usize, val: #typeident)->Option<Box<dyn #lamellar::LamellarRequest<Output = ()> + Send + Sync>>{
                    let pe = self.pe_for_dist_index(index);
                    let local_index = self.pe_offset_for_dist_index(pe,index);
                    if pe == self.my_pe{
                        self.local_add(local_index,val);
                        None
                    }
                    else{
                        Some(self.dist_add(
                            index,
                            Arc::new (#add_name_am{
                                data: self.clone(),
                                local_index: local_index,
                                val: val,
                            })
                        ))
                    }
                }
            }
            #[#am_data]
            struct #add_name_am{
                data: #lamellar::array::#array_type<#typeident>,
                local_index: usize,
                val: #typeident,
            }

            #[#am]
            impl LamellarAM for #add_name_am{
                fn exec(&self) {
                    self.data.local_add(self.local_index,self.val);
                }
            }
        });
    }

    let expanded = quote! {
        #[allow(non_camel_case_types)]
        impl #lamellar::array::ArrayOps<#typeident> for #lamellar::array::LamellarWriteArray<#typeident>{
            fn add(&self,index: usize, val: #typeident)->Option<Box<dyn #lamellar::LamellarRequest<Output = ()> + Send + Sync>>{
                match self{
                    #match_stmts
                }
            }
        }

        #array_impls
    };

    let user_expanded = quote_spanned! {expanded.span()=>
        const _: () = {
            extern crate lamellar as __lamellar;
            use __lamellar::array::LamellarArrayRead;
            #expanded
        };
    };
    if lamellar == "crate" {
        expanded
    } else {
        user_expanded
    }
}

#[proc_macro_error]
#[proc_macro]
pub fn register_reduction(item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(item as ReductionArgs);
    let mut output = quote! {};
    let array_types: Vec<syn::Ident> = vec![
        quote::format_ident!("UnsafeArray"),
        quote::format_ident!("ReadOnlyArray"),
    ];

    for ty in args.tys {
        let mut closure = args.closure.clone();
        let tyc = ty.clone();
        if let syn::Pat::Ident(a) = &closure.inputs[0] {
            let pat: syn::PatType = syn::PatType {
                attrs: vec![],
                pat: Box::new(syn::Pat::Ident(a.clone())),
                colon_token: syn::Token![:](Span::call_site()),
                ty: Box::new(syn::Type::Path(tyc.clone())),
            };
            closure.inputs[0] = syn::Pat::Type(pat);
        }
        if let syn::Pat::Ident(b) = &closure.inputs[1] {
            let tyr: syn::TypeReference = syn::parse_quote! {&#tyc};
            let pat: syn::PatType = syn::PatType {
                attrs: vec![],
                pat: Box::new(syn::Pat::Ident(b.clone())),
                colon_token: syn::Token![:](Span::call_site()),
                ty: Box::new(syn::Type::Reference(tyr)),
            };
            closure.inputs[1] = syn::Pat::Type(pat);
        }

        output.extend(create_reduction(
            ty.path.segments[0].ident.clone(),
            args.name.to_string(),
            quote! {#closure},
            &array_types,
            false,
            // "lamellar".to_string(),
        ));
    }
    TokenStream::from(output)
}

#[proc_macro_error]
#[proc_macro]
pub fn generate_reductions_for_type(item: TokenStream) -> TokenStream {
    let mut output = quote! {};
    let read_array_types: Vec<syn::Ident> = vec![
        quote::format_ident!("UnsafeArray"),
        quote::format_ident!("ReadOnlyArray"),
    ];
    let write_array_types: Vec<syn::Ident> = vec![quote::format_ident!("UnsafeArray")];

    for t in item.to_string().split(",").collect::<Vec<&str>>() {
        let typeident = quote::format_ident!("{:}", t.trim());
        output.extend(create_reduction(
            typeident.clone(),
            "sum".to_string(),
            quote! {
                let data_slice = <lamellar::LamellarMemoryRegion<#typeident> as lamellar::RegisteredMemoryRegion>::as_slice(&self.data).unwrap();
                let first = data_slice.first().unwrap().clone();
                data_slice[1..].iter().fold(first,|acc,val|{ acc+val } )
            },
            &read_array_types,
            false,
        ));
        output.extend(create_reduction(
            typeident.clone(),
            "prod".to_string(),
            quote! {
                let data_slice = <lamellar::LamellarMemoryRegion<#typeident> as lamellar::RegisteredMemoryRegion>::as_slice(&self.data).unwrap();
                let first = data_slice.first().unwrap().clone();
                data_slice[1..].iter().fold(first,|acc,val|{ acc*val } )
            },
            &read_array_types,
            false,
        ));
        output.extend(create_reduction(
            typeident.clone(),
            "max".to_string(),
            quote! {
                *<lamellar::LamellarMemoryRegion<#typeident> as lamellar::RegisteredMemoryRegion>::as_slice(&self.data).unwrap().iter().max().unwrap()
            },
            &read_array_types,
            false,
        ));
        output.extend(create_add(typeident.clone(), &write_array_types, false));
    }

    TokenStream::from(output)
}
#[proc_macro_error]
#[proc_macro]
pub fn generate_reductions_for_type_rt(item: TokenStream) -> TokenStream {
    let mut output = quote! {};

    let read_array_types: Vec<syn::Ident> = vec![
        quote::format_ident!("UnsafeArray"),
        quote::format_ident!("ReadOnlyArray"),
    ];
    let write_array_types: Vec<syn::Ident> = vec![quote::format_ident!("UnsafeArray")];
    for t in item.to_string().split(",").collect::<Vec<&str>>() {
        let t = t.trim().to_string();
        let typeident = quote::format_ident!("{:}", t.clone());
        output.extend(create_reduction(
            typeident.clone(),
            "sum".to_string(),
            quote! {
                |acc: #typeident, val: &#typeident|{ acc+*val }
            },
            &read_array_types,
            true,
        ));
        output.extend(create_reduction(
            typeident.clone(),
            "prod".to_string(),
            quote! {
                |acc: #typeident, val: &#typeident| { acc* *val }
            },
            &read_array_types,
            true,
        ));
        output.extend(create_reduction(
            typeident.clone(),
            "max".to_string(),
            quote! {
                |val1: #typeident, val2: &#typeident| { if val1 > *val2 {val1} else {*val2} }
            },
            &read_array_types,
            true,
        ));
        output.extend(create_add(typeident.clone(), &write_array_types, true));
    }
    TokenStream::from(output)
}
