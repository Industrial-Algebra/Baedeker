//! Host functions: native callables registered with a [`Store`] that WASM
//! modules can import and call.
//!
//! Registration is by `(module, name)` against the module's function import
//! declarations, with the host function's signature checked at registration.
//! Calls resolve lazily: an unregistered import fails only when called.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::runtime::{RuntimeError, Value};
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
