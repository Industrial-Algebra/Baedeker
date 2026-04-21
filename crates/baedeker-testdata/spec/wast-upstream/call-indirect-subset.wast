;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/call_indirect.wast

(module
  (type $over-i32 (func (param i32) (result i32)))
  (type $swap-i32-i64 (func (param i32 i64) (result i64 i32)))

  (func $id-i32 (type $over-i32) (local.get 0))
  (func $swap-i32-i64 (type $swap-i32-i64) (local.get 1) (local.get 0))

  (table funcref (elem $id-i32 $swap-i32-i64))

  (func (export "type-first-i32") (result i32)
    (call_indirect (type $over-i32) (i32.const 32) (i32.const 0)))
  (func (export "type-all-i32-i64") (result i64 i32)
    (call_indirect (type $swap-i32-i64) (i32.const 1) (i64.const 2) (i32.const 1))))

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

(assert_invalid
  (module
    (type $sig (func (param i32) (result i32)))
    (func $id (type $sig) (local.get 0))
    (table funcref (elem $id))
    (func (result i32)
      (call_indirect (type $sig) (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $sig (func (param i32) (result i32)))
    (func $id (type $sig) (local.get 0))
    (table funcref (elem $id))
    (func (result i32)
      (call_indirect (type $sig) (f64.const 0) (i32.const 0))))
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
