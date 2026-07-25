//! Host functions: native callables registered with a [`Store`] that WASM
//! modules can import and call.
//!
//! Registration is by `(module, name)` against the module's function import
//! declarations, with the host function's signature checked at registration.
//! Calls resolve lazily: an unregistered import fails only when called.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::runtime::{RuntimeError, RuntimeErrorKind, Value, execute_func_in};
use crate::types::FuncType;

/// The native closure behind a [`HostFunction`].
type HostClosure = Box<dyn FnMut(&[Value]) -> Result<Vec<Value>, RuntimeError>>;

/// A host function callable from WASM, wrapping a native closure with its
/// declared WASM signature.
pub struct HostFunction {
    ty: FuncType,
    func: HostClosure,
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
        }
    }

    /// The declared WASM signature.
    pub fn ty(&self) -> &FuncType {
        &self.ty
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
/// Cross-module call chains are bounded by [`RuntimeErrorKind::ReentrantStore`]:
/// a call that re-enters a store already executing fails rather than
/// deadlocking (mutual recursion across modules is not yet supported).
pub fn link_func(
    module: alloc::rc::Rc<crate::lower::RegModule>,
    store: alloc::rc::Rc<core::cell::RefCell<crate::runtime::Store>>,
    func_idx: crate::types::FuncIdx,
    ty: FuncType,
) -> HostFunction {
    HostFunction::new(ty, move |args| {
        let Some(func) = module.funcs.iter().find(|func| func.idx == func_idx) else {
            return Err(RuntimeError {
                kind: RuntimeErrorKind::UnknownFunction { func: func_idx.0 },
            });
        };
        let mut store = store.try_borrow_mut().map_err(|_| RuntimeError {
            kind: RuntimeErrorKind::ReentrantStore,
        })?;
        execute_func_in(Some(&module), Some(&mut *store), func, args, 0)
    })
}
