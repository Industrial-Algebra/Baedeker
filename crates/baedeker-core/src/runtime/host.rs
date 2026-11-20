// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Host functions: native callables registered with a [`Store`](crate::runtime::Store) that WASM
//! modules can import and call.
//!
//! Registration is by `(module, name)` against the module's function import
//! declarations, with the host function's signature checked at registration.
//! Calls resolve lazily: an unregistered import fails only when called.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::runtime::{RuntimeError, RuntimeErrorKind, Value, execute_func_in};
use crate::types::{ClosedDefinedType, FuncType};

/// The native closure behind a [`HostFunction`].
type HostClosure = Box<dyn FnMut(&[Value]) -> Result<Vec<Value>, RuntimeError>>;

/// A host function callable from WASM, wrapping a native closure with its
/// declared WASM signature.
pub struct HostFunction {
    ty: FuncType,
    func: HostClosure,
    /// The DEFINED TYPE of the wasm function this binding forwards to, when
    /// the binding is a [`link_func`] bridge. A funcref's runtime type is its
    /// function instance's type (`runtime/instance.ml:48`: `| FuncRef f ->
    /// UseHT (Def (Func.type_of f))`), NOT the type the importing module
    /// declared the import with — the declaration only constrains linking.
    /// `None` for a native host function, whose own type is its `FuncType`.
    origin_type: Option<ClosedDefinedType>,
}

impl HostFunction {
    /// Wrap a closure with its WASM signature. The signature is checked
    /// against the import declaration at registration; argument values are
    /// passed positionally and results must match the declared result types.
    pub fn new(
        ty: FuncType,
        func: impl FnMut(&[Value]) -> Result<Vec<Value>, RuntimeError> + 'static,
    ) -> Self {
        Self {
            ty,
            func: Box::new(func),
            origin_type: None,
        }
    }

    /// The declared WASM signature.
    pub fn ty(&self) -> &FuncType {
        &self.ty
    }

    /// The defining wasm function's type entry, when this binding forwards to
    /// one ([`link_func`]).
    pub(crate) fn origin_type(&self) -> Option<&ClosedDefinedType> {
        self.origin_type.as_ref()
    }

    /// Invoke the host function.
    pub fn call(&mut self, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
        (self.func)(args)
    }
}

impl core::fmt::Debug for HostFunction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HostFunction")
            .field("ty", &self.ty)
            .finish()
    }
}

/// Link a function exported by another module instance: the resulting
/// [`HostFunction`] executes `func_idx` against that instance's store when
/// called, so cross-module calls work with each instance's own state.
///
/// Register it as an import on the calling module's store, e.g.
/// `store_b.register_host_func("a", "add", link_func(module_a, store_a, idx, ty))`.
///
/// The `ty` argument is the IMPORT DECLARATION's signature (what calls through
/// this import are checked against). The reference type a funcref to this
/// import reports is the PROVIDER's defining type, captured here as
/// `func_origin_type` (`runtime/instance.ml:48`), so `ref.test` sees a
/// provider subtype rather than the declaration.
///
/// Cross-module call chains are bounded by [`RuntimeErrorKind::ReentrantStore`]:
/// a call that re-enters a store already executing fails rather than
/// deadlocking (mutual recursion across modules is not yet supported).
pub fn link_func(
    module: alloc::rc::Rc<crate::lower::RegModule>,
    store: alloc::rc::Rc<core::cell::RefCell<crate::runtime::Store>>,
    func_idx: crate::types::FuncIdx,
    ty: FuncType,
) -> HostFunction {
    // Preserve the PROVIDER's identity through the binding: a funcref to this
    // import must report the defining function's type, not the importing
    // module's declaration. `func_origin_type` resolves the provider's own
    // import chain (a re-exported function keeps its original definition) and
    // is computed once here, while the provider store is quiescent.
    let origin_type = store
        .try_borrow()
        .ok()
        .and_then(|store| store.func_origin_type(func_idx.0));
    let mut host = HostFunction::new(ty, move |args| {
        let store = store.try_borrow().map_err(|_| RuntimeError {
            kind: RuntimeErrorKind::ReentrantStore,
        })?;
        if let Some(func) = module.funcs.iter().find(|func| func.idx == func_idx) {
            return execute_func_in(Some(&module), Some(&*store), func, args, 0);
        }
        // The provider re-exports one of its own imports: the function
        // instance is whatever that import is bound to (`exec/eval.ml:632-634`
        // resolves `ref.func`/calls through `func c.frame.inst x`), so forward
        // to the binding instead of failing.
        if (func_idx.0 as usize) < module.imported_funcs.len() {
            return store.call_host(func_idx.0, args);
        }
        Err(RuntimeError {
            kind: RuntimeErrorKind::UnknownFunction { func: func_idx.0 },
        })
    });
    host.origin_type = origin_type;
    host
}
