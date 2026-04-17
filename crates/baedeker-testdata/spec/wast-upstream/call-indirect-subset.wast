;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/call_indirect.wast

(assert_invalid
  (module
    (type $sig (func (param i32) (result i32)))
    (table 0 externref)
    (func (result i32)
      i32.const 0
      i32.const 0
      call_indirect (type $sig)))
  "type mismatch"
)

(assert_malformed
  (module quote
    "(type $sig (func (param i32) (result i32)))"
    "(table 0 funcref)"
    "(func (result i32)"
    "  (call_indirect (type $sig) (result i32) (param i32)"
    "    (i32.const 0) (i32.const 0)"
    "  )"
    ")"
  )
  "unexpected token"
)
