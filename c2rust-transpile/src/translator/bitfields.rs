#![deny(missing_docs)]
//! This module provides translation for bitfield structs and operations on them. Generated code
//! requires the use of the bitfield crate.

// TODO: See if union bitfields are supportable or not
// FIXME: size_of won't call the bitfield struct with required generic params, same
// for variable bindings: `let foo: Foo` should be `let foo: Foo<[u8; X]>`
// TODO: We can generate static bitfields in a slightly less nice, but safe, way:
// static mut bitfield: BitField<[u8; X]> = BitField([A, B, C, D]); Either we figure
// out what bits to assign here, or else we reuse the local var version but ensure that
// static bitfields get sectioned off

use std::collections::HashSet;
use std::ops::Index;

use c_ast::{BinOp, CDeclId, CDeclKind, CExprId, CQualTypeId, CTypeId};
use c2rust_ast_builder::mk;
use syntax::ast::{AttrStyle, BinOpKind, Expr, MetaItemKind, NestedMetaItem, NestedMetaItemKind, Lit, LitIntType, LitKind, StrStyle, StructField, Ty, TyKind};
use syntax::ext::quote::rt::Span;
use syntax::ptr::P;
use syntax::source_map::symbol::Symbol;
use syntax_pos::DUMMY_SP;
use translator::{ExprContext, Translation, ConvertedDecl, simple_metaitem};
use with_stmts::WithStmts;

use itertools::Itertools;
use itertools::EitherOrBoth::{Both, Right};

/// (name, type, bitfield_width, platform_bit_offset, platform_type_bitwidth)
type FieldInfo = (String, CQualTypeId, Option<u64>, u64, u64);

#[derive(Debug)]
enum FieldType {
    BitfieldGroup { start_bit: u64, field_name: String, bytes: u64, attrs: Vec<(String, P<Ty>, String)> },
    Padding { bytes: u64 },
    Regular { name: String, ctype: CTypeId, field: StructField },
}

fn assigment_metaitem(lhs: &str, rhs: &str) -> NestedMetaItem {
    let meta_item = mk().meta_item(
        vec![lhs],
        MetaItemKind::NameValue(Lit {
            span: DUMMY_SP,
            node: LitKind::Str(Symbol::intern(rhs), StrStyle::Cooked)
        }),
    );

    mk().nested_meta_item(NestedMetaItemKind::MetaItem(meta_item))
}

impl<'a> Translation<'a> {
    /// TODO
    fn get_field_types(&self, field_info: Vec<FieldInfo>, platform_byte_size: u64) -> Result<Vec<FieldType>, String> {
        let mut reorganized_fields = Vec::new();
        let mut last_bitfield_group: Option<FieldType> = None;
        let mut next_byte_pos = 0;
        let mut encountered_bytes = HashSet::new();

        for (field_name, ty, bitfield_width, bit_index, platform_ty_bitwidth) in field_info {
            let ctype = ty.ctype;
            let ty = self.convert_type(ctype)?;
            let bitfield_width = match bitfield_width {
                // Bitfield widths of 0 should just be markers for clang,
                // we shouldn't need to explicitly handle it ourselves
                Some(0) => {
                    // Hit non bitfield group so existing one is all set
                    if let Some(field_group) = last_bitfield_group.take() {
                        reorganized_fields.push(field_group);
                    }

                    continue
                },
                None => {
                    // Hit non bitfield group so existing one is all set
                    if let Some(field_group) = last_bitfield_group.take() {
                        reorganized_fields.push(field_group);
                    }

                    let byte_diff = bit_index / 8 - next_byte_pos;

                    // Need to add padding first
                    if byte_diff > 1 {
                        reorganized_fields.push(FieldType::Padding { bytes: byte_diff });
                    }

                    let field = mk().pub_().struct_field(field_name.clone(), ty);

                    reorganized_fields.push(FieldType::Regular {
                        name: field_name,
                        ctype,
                        field,
                    });

                    next_byte_pos = (bit_index + platform_ty_bitwidth) / 8;

                    continue
                },
                Some(bw) => bw,
            };

            // Ensure we aren't looking at overlapping bits in the same byte
            if bit_index / 8 > next_byte_pos {
                let byte_diff = (bit_index / 8) - next_byte_pos;

                // if byte_diff > 0 {
                reorganized_fields.push(FieldType::Padding { bytes: byte_diff });
                // }
            }

            match last_bitfield_group {
                Some(FieldType::BitfieldGroup { start_bit, field_name: ref mut name, ref mut bytes, ref mut attrs }) => {
                    name.push('_');
                    name.push_str(&field_name);

                    let end_bit = bit_index + bitfield_width;

                    // Add to the total byte size of the bitfield group only if
                    // we have not already enountered this byte
                    for bit in bit_index..end_bit {
                        let byte = bit / 8;

                        if !encountered_bytes.contains(&byte) {
                            *bytes += 1;
                            encountered_bytes.insert(byte);
                        }
                    }

                    let bit_start = bit_index - start_bit;
                    let bit_end = bit_start + bitfield_width - 1;
                    let bit_range = format!("{}..={}", bit_start, bit_end);

                    attrs.push((field_name.clone(), ty, bit_range));
                },
                Some(_) => unreachable!("Found last bitfield group which is not a group"),
                None => {
                    let mut bytes = 0;
                    let end_bit = bit_index + bitfield_width;

                    // Add to the total byte size of the bitfield group only if
                    // we have not already enountered this byte
                    for bit in bit_index..end_bit {
                        let byte = bit / 8;

                        if !encountered_bytes.contains(&byte) {
                            bytes += 1;
                            encountered_bytes.insert(byte);
                        }
                    }

                    let bit_range = format!("0..={}", bitfield_width - 1);
                    let attrs = vec![
                        (field_name.clone(), ty, bit_range),
                    ];

                    last_bitfield_group = Some(FieldType::BitfieldGroup {
                        start_bit: bit_index,
                        field_name,
                        bytes,
                        attrs,
                    });
                },
            }

            next_byte_pos = (bit_index + bitfield_width) / 8 + 1;
        }

        // Find leftover bitfield group at end: it's all set
        if let Some(field_group) = last_bitfield_group.take() {
            reorganized_fields.push(field_group);
        }

        let byte_diff = platform_byte_size - next_byte_pos;

        // Need to add padding to end if we haven't hit the expected total byte size
        if byte_diff > 0 {
            reorganized_fields.push(FieldType::Padding { bytes: byte_diff });
        }

        Ok(reorganized_fields)
    }


    /// Here we output a struct derive to generate bitfield data that looks like this:
    ///
    /// ```no_run
    /// #[derive(BitfieldStruct, Clone, Copy)]
    /// #[repr(C, align(2))]
    /// struct Foo {
    ///     #[bitfield(name = "bf1", ty = "libc::c_char", bits = "0..=9")]
    ///     #[bitfield(name = "bf2", ty = "libc::c_uchar",bits = "10..=15")]
    ///     bf1_bf2: [u8; 2],
    ///     non_bf: u64,
    /// }
    /// ```
    pub fn convert_bitfield_struct_decl(
        &self,
        name: String,
        manual_alignment: Option<u64>,
        platform_alignment: u64,
        platform_byte_size: u64,
        span: Span,
        field_info: Vec<FieldInfo>,
    ) -> Result<ConvertedDecl, String> {
        self.extern_crates.borrow_mut().insert("c2rust_bitfields");

        let mut item_store = self.item_store.borrow_mut();

        item_store.uses
            .get_mut(vec!["c2rust_bitfields".into()])
            .insert("BitfieldStruct");

        let mut field_entries = Vec::with_capacity(field_info.len());
        // We need to clobber bitfields in consecutive bytes together (leaving
        // regular fields alone) and add in padding as necessary
        let reorganized_fields = self.get_field_types(field_info, platform_byte_size)?;

        let mut padding_count = 0;

        for field_type in reorganized_fields {
            match field_type {
                FieldType::BitfieldGroup { start_bit: _, field_name, bytes, attrs } => {
                    let ty = mk().array_ty(
                        mk().ident_ty("u8"),
                        mk().lit_expr(mk().int_lit(bytes.into(), LitIntType::Unsuffixed)),
                    );
                    let mut field = mk();
                    let field_attrs = attrs.iter().map(|attr| {
                        let ty_str = match &attr.1.node {
                            TyKind::Path(_, path) => format!("{}", path),
                            _ => unreachable!("Found type other than path"),
                        };
                        let field_attr_items = vec![
                            assigment_metaitem("name", &attr.0),
                            assigment_metaitem("ty", &ty_str),
                            assigment_metaitem("bits", &attr.2),
                        ];

                        mk().meta_item("bitfield", MetaItemKind::List(field_attr_items))
                    });

                    for field_attr in field_attrs {
                        field = field.meta_item_attr(AttrStyle::Outer, field_attr);
                    }

                    field_entries.push(field.pub_().struct_field(field_name, ty));
                },
                FieldType::Padding { bytes } => {
                    let field_name = if padding_count == 0 {
                        "_pad".into()
                    } else {
                        format!("_pad{}", padding_count + 1)
                    };
                    let ty = mk().array_ty(
                        mk().ident_ty("u8"),
                        mk().lit_expr(mk().int_lit(bytes.into(), LitIntType::Unsuffixed)),
                    );
                    let field = mk().pub_().struct_field(field_name, ty);

                    padding_count += 1;

                    field_entries.push(field);
                },
                FieldType::Regular { field, .. } => field_entries.push(field),
            }
        }

        let repr_items = vec![
            simple_metaitem("C"),
            simple_metaitem(&format!("align({})", manual_alignment.unwrap_or(platform_alignment))),
        ];
        let repr_attr = mk().meta_item("repr", MetaItemKind::List(repr_items));

        let item = mk()
            .span(span)
            .pub_()
            .call_attr("derive", vec!["BitfieldStruct", "Clone", "Copy"])
            .meta_item_attr(AttrStyle::Outer, repr_attr)
            .struct_item(name, field_entries);

        Ok(ConvertedDecl::Item(item))
    }

    /// Here we output a block to generate a struct literal initializer in.
    /// It looks like this in locals and (sectioned) statics:
    ///
    /// ```no_run
    /// {
    ///     let mut init = Foo {
    ///         bf1_bf2: [0; 2],
    ///         non_bf: 32,
    ///     };
    ///     init.set_bf1(-12);
    ///     init.set_bf2(34);
    ///     init
    /// }
    /// ```
    pub fn convert_bitfield_struct_literal(
        &self,
        name: String,
        platform_byte_size: u64,
        field_ids: &[CExprId],
        field_info: Vec<FieldInfo>,
        ctx: ExprContext,
    ) -> Result<WithStmts<P<Expr>>, String> {
        // REVIEW: statics? Could section them off
        let mut fields = Vec::with_capacity(field_ids.len());
        let reorganized_fields = self.get_field_types(field_info.clone(), platform_byte_size)?;
        let local_pat = mk().mutbl().ident_pat("init");
        let mut padding_count = 0;
        let mut stmts = Vec::new();

        // Add in zero inits for both padding as well as bitfield groups
        for field_type in reorganized_fields {
            match field_type {
                FieldType::BitfieldGroup { field_name, bytes, .. } => {
                    let array_expr = mk().repeat_expr(
                        mk().lit_expr(mk().int_lit(0, LitIntType::Unsuffixed)),
                        mk().lit_expr(mk().int_lit(bytes.into(), LitIntType::Unsuffixed)),
                    );
                    let field = mk().field(field_name, array_expr);

                    fields.push(field);
                },
                FieldType::Padding { bytes } => {
                    let field_name = if padding_count == 0 {
                        "_pad".into()
                    } else {
                        format!("_pad{}", padding_count + 1)
                    };
                    let array_expr = mk().repeat_expr(
                        mk().lit_expr(mk().int_lit(0, LitIntType::Unsuffixed)),
                        mk().lit_expr(mk().int_lit(bytes.into(), LitIntType::Unsuffixed)),
                    );
                    let field = mk().field(field_name, array_expr);

                    padding_count += 1;

                    fields.push(field);
                },
                _ => {},
            }
        }

        // Bitfield widths of 0 should just be markers for clang,
        // we shouldn't need to explicitly handle it ourselves
        let field_info_iter = field_info.iter().filter(|info| info.2 != Some(0));
        let zipped_iter = field_ids.iter().zip_longest(field_info_iter);
        let mut bitfield_inits = Vec::new();

        // Specified record fields which are not bitfields need to be added
        for item in zipped_iter {
            match item {
                Right((field_name, ty, bitfield_width, _, _)) => {
                    if bitfield_width.is_some() {
                        continue;
                    }

                    fields.push(mk().field(field_name, self.implicit_default_expr(ty.ctype, ctx.is_static)?));
                },
                Both(field_id, (field_name, _, bitfield_width, _, _)) => {
                    let expr = self.convert_expr(ctx.used(), *field_id)?;

                    if bitfield_width.is_some() {
                        bitfield_inits.push((field_name, expr.val));

                        continue;
                    }

                    fields.push(mk().field(field_name, expr.val));
                },
                _ => unreachable!(),
            }
        }

        let struct_expr = mk().struct_expr(name.as_str(), fields);
        let local_variable = P(mk().local(local_pat, None as Option<P<Ty>>, Some(struct_expr)));

        stmts.push(mk().local_stmt(local_variable));

        // Now we must use the bitfield methods to initialize bitfields
        for (field_name, val) in bitfield_inits {
            let field_name_setter = format!("set_{}", field_name);
            let struct_ident = mk().ident_expr("init");
            let expr = mk().method_call_expr(struct_ident, field_name_setter, vec![val]);

            stmts.push(mk().expr_stmt(expr));
        }

        let struct_ident = mk().ident_expr("init");

        stmts.push(mk().expr_stmt(struct_ident));

        let val = mk().block_expr(mk().block(stmts));

        Ok(WithStmts {
            stmts: Vec::new(),
            val,
        })
    }

    /// TODO
    pub fn bitfield_zero_initializer(
        &self,
        name: String,
        field_ids: &[CDeclId],
        platform_byte_size: u64,
        is_static: bool,
    ) -> Result<P<Expr>, String> {
        let field_info: Vec<FieldInfo> = field_ids.iter()
            .map(|field_id| match self.ast_context.index(*field_id).kind {
                CDeclKind::Field { ref name, typ, bitfield_width, platform_bit_offset, platform_type_bitwidth, .. } =>
                    (name.clone(), typ, bitfield_width, platform_bit_offset, platform_type_bitwidth),
                _ => unreachable!("Found non-field in record field list"),
            }).collect();
        let reorganized_fields = self.get_field_types(field_info, platform_byte_size)?;
        let mut fields = Vec::with_capacity(reorganized_fields.len());
        let mut padding_count = 0;

        for field_type in reorganized_fields {
            match field_type {
                FieldType::BitfieldGroup { field_name, bytes, .. } => {
                    let array_expr = mk().repeat_expr(
                        mk().lit_expr(mk().int_lit(0, LitIntType::Unsuffixed)),
                        mk().lit_expr(mk().int_lit(bytes.into(), LitIntType::Unsuffixed)),
                    );
                    let field = mk().field(field_name, array_expr);

                    fields.push(field);
                },
                FieldType::Padding { bytes } => {
                    let field_name = if padding_count == 0 {
                        "_pad".into()
                    } else {
                        format!("_pad{}", padding_count + 1)
                    };
                    let array_expr = mk().repeat_expr(
                        mk().lit_expr(mk().int_lit(0, LitIntType::Unsuffixed)),
                        mk().lit_expr(mk().int_lit(bytes.into(), LitIntType::Unsuffixed)),
                    );
                    let field = mk().field(field_name, array_expr);

                    padding_count += 1;

                    fields.push(field);
                },
                FieldType::Regular { ctype, name, .. } => {
                    let field_init = self.implicit_default_expr(ctype, is_static)?;

                    fields.push(mk().field(name, field_init));
                },
            }
        }

        Ok(mk().struct_expr(name.as_str(), fields))
    }

    /// TODO
    pub fn convert_bitfield_assignment_op_with_rhs(
        &self,
        ctx: ExprContext,
        op: BinOp,
        field_name: &str,
        lhs: CExprId,
        rhs: WithStmts<P<Expr>>,
    ) -> Result<WithStmts<P<Expr>>, String> {
        let lhs_expr = self.convert_expr(ctx, lhs)?.to_expr();
        let setter_name = format!("set_{}", field_name);

        let expr = match op {
            BinOp::AssignAdd => {
                let param = mk().binary_expr(BinOpKind::Add, lhs_expr.clone(), rhs.to_expr());

                mk().method_call_expr(lhs_expr, setter_name, vec![param])
            },
            BinOp::AssignSubtract => {
                let param = mk().binary_expr(BinOpKind::Sub, lhs_expr.clone(), rhs.to_expr());

                mk().method_call_expr(lhs_expr, setter_name, vec![param])
            },
            BinOp::AssignMultiply => {
                let param = mk().binary_expr(BinOpKind::Mul, lhs_expr.clone(), rhs.to_expr());

                mk().method_call_expr(lhs_expr, setter_name, vec![param])
            },
            BinOp::AssignDivide => {
                let param = mk().binary_expr(BinOpKind::Div, lhs_expr.clone(), rhs.to_expr());

                mk().method_call_expr(lhs_expr, setter_name, vec![param])
            },
            BinOp::Assign => mk().method_call_expr(lhs_expr, setter_name, vec![rhs.to_expr()]),
            e => unimplemented!("{:?}", e),
        };

        let stmt = mk().expr_stmt(expr);
        let val = self.panic("Empty statement expression is not supposed to be used");

        return Ok(WithStmts { stmts: vec![stmt], val });
    }
}
