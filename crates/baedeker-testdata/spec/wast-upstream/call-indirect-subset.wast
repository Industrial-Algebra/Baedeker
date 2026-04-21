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

(module
  (type $ii-i (func (param i32 i32) (result i32)))

  (table $t1 funcref (elem $f $g))
  (table $t2 funcref (elem $h $i $j))
  (table $t3 4 funcref)
  (elem (table $t3) (i32.const 0) func $g $h)
  (elem (table $t3) (i32.const 3) func $z)

  (func $f (type $ii-i) (i32.add (local.get 0) (local.get 1)))
  (func $g (type $ii-i) (i32.sub (local.get 0) (local.get 1)))
  (func $h (type $ii-i) (i32.mul (local.get 0) (local.get 1)))
  (func $i (type $ii-i) (i32.div_u (local.get 0) (local.get 1)))
  (func $j (type $ii-i) (i32.rem_u (local.get 0) (local.get 1)))
  (func $z)

  (func (export "call-1") (param i32 i32 i32) (result i32)
    (call_indirect $t1 (type $ii-i) (local.get 0) (local.get 1) (local.get 2)))
  (func (export "call-2") (param i32 i32 i32) (result i32)
    (call_indirect $t2 (type $ii-i) (local.get 0) (local.get 1) (local.get 2)))
  (func (export "call-3") (param i32 i32 i32) (result i32)
    (call_indirect $t3 (type $ii-i) (local.get 0) (local.get 1) (local.get 2))))

(assert_invalid
  (module
    (type (func))
    (func $no-table (call_indirect (type 0) (i32.const 0))))
  "unknown table"
)

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
    (type (func))
    (table 0 funcref)
    (func $type-void-vs-num (i32.eqz (call_indirect (type 0) (i32.const 0)))))
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
