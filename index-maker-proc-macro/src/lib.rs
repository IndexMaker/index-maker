use proc_macro::TokenStream;
use quote::quote;
use syn::{BinOp, Expr, parse_macro_input};

#[proc_macro]
pub fn checked_arithmetic(input: TokenStream) -> TokenStream {
    // Parse the input as a binary expression.
    let input_clone = input.clone();
    let expr = parse_macro_input!(input as Expr);

    // Match on the binary expression.
    match expr {
        Expr::Paren(paren_expr) => {
            // Handle parenthesized expressions recursively
            let inner_expr = paren_expr.expr;
            let inner_result = checked_arithmetic(quote!(#inner_expr).into()); // Call macro recursively
            inner_result
        }
        Expr::Binary(bin_expr) => {
            let left = bin_expr.left;
            let op = bin_expr.op;
            let right = bin_expr.right;

            // Generate the appropriate checked operation based on the operator.
            let checked_op = match op {
                BinOp::Add(_) => quote! {
                    {
                        let a_val = #left;
                        let b_val = #right;
                        DecimalExt::checked_math(&a_val, |x| x.checked_add(b_val))
                    }
                },
                BinOp::Sub(_) => quote! {
                    {
                        let a_val = #left;
                        let b_val = #right;
                        DecimalExt::checked_math(&a_val, |x| x.checked_sub(b_val))
                    }
                },
                BinOp::Mul(_) => quote! {
                    {
                        let a_val = #left;
                        let b_val = #right;
                        DecimalExt::checked_math(&a_val, |x| x.checked_mul(b_val))
                    }
                },
                BinOp::Div(_) => quote! {
                    {
                        let a_val = #left;
                        let b_val = #right;
                        DecimalExt::checked_math(&a_val, |x| x.checked_div(b_val))
                    }
                },
                BinOp::AddAssign(_) => quote! {
                    {
                        // Option<Amount> += Amount -> Amount
                        let a_val = &mut #left;
                        let b_val = #right;
                        a_val.update_with(|x| x?.checked_add(b_val))
                    }
                },
                BinOp::SubAssign(_) => quote! {
                    {
                        // Option<Amount> += Amount -> Amount
                        let a_val = &mut #left;
                        let b_val = #right;
                        a_val.update_with(|x| x?.checked_sub(b_val))
                    }
                },
                _ => {
                    // Return the original expression if it's not a supported operator.
                    return input_clone;
                }
            };
            // Return the modified expression.
            checked_op.into()
        }
        _ => {
            //if it is not a binary expression
            input_clone
        }
    }
}
