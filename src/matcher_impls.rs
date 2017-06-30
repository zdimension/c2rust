use std::rc::Rc;
use syntax::ast::*;
use syntax::abi::Abi;
use syntax::ast::*;
use syntax::codemap::{Span, Spanned};
use syntax::ext::hygiene::SyntaxContext;
use syntax::ptr::P;
use syntax::tokenstream::{TokenStream, ThinTokenStream};

use matcher::{self, TryMatch, MatchCtxt};


impl TryMatch for Ident {
    fn try_match(&self, target: &Self, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        if mcx.maybe_capture_ident(self, target)? {
            return Ok(());
        }

        if self == target {
            Ok(())
        } else {
            Err(matcher::Error::SymbolMismatch)
        }
    }
}

impl TryMatch for Expr {
    fn try_match(&self, target: &Self, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        if mcx.maybe_capture_expr(self, target)? {
            return Ok(());
        }

        default_try_match_expr(self, target, mcx)
    }
}

impl TryMatch for Pat {
    fn try_match(&self, target: &Self, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        if mcx.maybe_capture_pat(self, target)? {
            return Ok(());
        }

        default_try_match_pat(self, target, mcx)
    }
}

impl TryMatch for Stmt {
    fn try_match(&self, target: &Self, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        if mcx.maybe_capture_stmt(self, target)? {
            return Ok(());
        }

        default_try_match_stmt(self, target, mcx)
    }
}


impl<T: TryMatch> TryMatch for [T] {
    fn try_match(&self, target: &Self, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        if self.len() != target.len() {
            return Err(matcher::Error::LengthMismatch);
        }
        for i in 0 .. self.len() {
            mcx.try_match(&self[i], &target[i])?;
        }
        Ok(())
    }
}

impl<T: TryMatch> TryMatch for Vec<T> {
    fn try_match(&self, target: &Self, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        <[T] as TryMatch>::try_match(self, target, mcx)
    }
}

impl<T: TryMatch> TryMatch for ThinVec<T> {
    fn try_match(&self, target: &Self, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        <[T] as TryMatch>::try_match(self, target, mcx)
    }
}

impl<T: TryMatch> TryMatch for P<T> {
    fn try_match(&self, target: &Self, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        mcx.try_match(&**self, &**target)
    }
}

impl<T: TryMatch> TryMatch for Rc<T> {
    fn try_match(&self, target: &Self, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        mcx.try_match(&**self, &**target)
    }
}

impl<T: TryMatch> TryMatch for Spanned<T> {
    fn try_match(&self, target: &Self, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        mcx.try_match(&self.node, &target.node)
    }
}

impl<T: TryMatch> TryMatch for Option<T> {
    fn try_match(&self, target: &Option<T>, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        match (self, target) {
            (&Some(ref x), &Some(ref y)) => mcx.try_match(x, y),
            (&None, &None) => Ok(()),
            (_, _) => Err(matcher::Error::VariantMismatch),
        }
    }
}

impl<A: TryMatch, B: TryMatch> TryMatch for (A, B) {
    fn try_match(&self, target: &Self, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        mcx.try_match(&self.0, &target.0)?;
        mcx.try_match(&self.1, &target.1)?;
        Ok(())
    }
}

impl<A: TryMatch, B: TryMatch, C: TryMatch> TryMatch for (A, B, C) {
    fn try_match(&self, target: &Self, mcx: &mut MatchCtxt) -> matcher::Result<()> {
        mcx.try_match(&self.0, &target.0)?;
        mcx.try_match(&self.1, &target.1)?;
        mcx.try_match(&self.2, &target.2)?;
        Ok(())
    }
}


include!("matcher_impls_gen.inc.rs");
