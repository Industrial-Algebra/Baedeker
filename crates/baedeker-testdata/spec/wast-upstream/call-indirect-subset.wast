;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/call_indirect.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast

(module
  (type $out-i32 (func (result i32)))
  (type $out-i64 (func (result i64)))
  (type $out-f32 (func (result f32)))
  (type $out-f64 (func (result f64)))
  (type $out-f64-i32 (func (result f64 i32)))
  (type $over-i32 (func (param i32) (result i32)))
  (type $over-i64 (func (param i64) (result i64)))
  (type $over-f32 (func (param f32) (result f32)))
  (type $over-f64 (func (param f64) (result f64)))
  (type $swap-i32-i64 (func (param i32 i64) (result i64 i32)))
  (type $f32-i32 (func (param f32 i32) (result i32)))
  (type $i32-i64 (func (param i32 i64) (result i64)))
  (type $f64-f32 (func (param f64 f32) (result f32)))
  (type $i64-f64 (func (param i64 f64) (result f64)))

  (func $const-i32 (type $out-i32) (i32.const 0x132))
  (func $const-i64 (type $out-i64) (i64.const 0x164))
  (func $const-f32 (type $out-f32) (f32.const 0xf32))
  (func $const-f64 (type $out-f64) (f64.const 0xf64))
  (func $const-f64-i32 (type $out-f64-i32) (f64.const 0xf64) (i32.const 32))

  (func $id-i32 (type $over-i32) (local.get 0))
  (func $id-i64 (type $over-i64) (local.get 0))
  (func $id-f32 (type $over-f32) (local.get 0))
  (func $id-f64 (type $over-f64) (local.get 0))
  (func $swap-i32-i64 (type $swap-i32-i64) (local.get 1) (local.get 0))
  (func $f32-i32 (type $f32-i32) (local.get 1))
  (func $i32-i64 (type $i32-i64) (local.get 1))
  (func $f64-f32 (type $f64-f32) (local.get 1))
  (func $i64-f64 (type $i64-f64) (local.get 1))

  (table funcref
    (elem
      $const-i32 $const-i64 $const-f32 $const-f64 $const-f64-i32
      $id-i32 $id-i64 $id-f32 $id-f64
      $f32-i32 $i32-i64 $f64-f32 $i64-f64
      $swap-i32-i64))

  (func (export "type-i32") (result i32)
    (call_indirect (type $out-i32) (i32.const 0)))
  (func (export "type-i64") (result i64)
    (call_indirect (type $out-i64) (i32.const 1)))
  (func (export "type-f32") (result f32)
    (call_indirect (type $out-f32) (i32.const 2)))
  (func (export "type-f64") (result f64)
    (call_indirect (type $out-f64) (i32.const 3)))
  (func (export "type-f64-i32") (result f64 i32)
    (call_indirect (type $out-f64-i32) (i32.const 4)))
  (func (export "type-first-i32") (result i32)
    (call_indirect (type $over-i32) (i32.const 32) (i32.const 5)))
  (func (export "type-second-i64") (result i64)
    (call_indirect (type $i32-i64) (i32.const 32) (i64.const 64) (i32.const 10)))
  (func (export "type-all-i32-i64") (result i64 i32)
    (call_indirect (type $swap-i32-i64) (i32.const 1) (i64.const 2) (i32.const 13)))

  (memory 1)

  (func (export "as-select-first") (result i32)
    (select (call_indirect (type $out-i32) (i32.const 0)) (i32.const 2) (i32.const 3)))
  (func (export "as-select-mid") (result i32)
    (select (i32.const 2) (call_indirect (type $out-i32) (i32.const 0)) (i32.const 3)))
  (func (export "as-select-last") (result i32)
    (select (i32.const 2) (i32.const 3) (call_indirect (type $out-i32) (i32.const 0))))

  (func (export "as-if-condition") (result i32)
    (if (result i32) (call_indirect (type $out-i32) (i32.const 0))
      (then (i32.const 1))
      (else (i32.const 2))))

  (func (export "as-br_if-first") (result i64)
    (block (result i64)
      (br_if 0 (call_indirect (type $out-i64) (i32.const 1)) (i32.const 2))))
  (func (export "as-br_if-last") (result i32)
    (block (result i32)
      (br_if 0 (i32.const 2) (call_indirect (type $out-i32) (i32.const 0)))))

  (func (export "as-br_table-first") (result f32)
    (block (result f32)
      (call_indirect (type $out-f32) (i32.const 2))
      (i32.const 2)
      (br_table 0 0)))
  (func (export "as-br_table-last") (result i32)
    (block (result i32)
      (i32.const 2)
      (call_indirect (type $out-i32) (i32.const 0))
      (br_table 0 0)))

  (func (export "as-store-first")
    (call_indirect (type $out-i32) (i32.const 0)) (i32.const 1) (i32.store))
  (func (export "as-store-last")
    (i32.const 10) (call_indirect (type $out-f64) (i32.const 3)) (f64.store))

  (func (export "as-memory.grow-value") (result i32)
    (memory.grow (call_indirect (type $out-i32) (i32.const 0))))
  (func (export "as-return-value") (result i32)
    (call_indirect (type $over-i32) (i32.const 1) (i32.const 5)) (return))
  (func (export "as-drop-operand")
    (call_indirect (type $over-i64) (i64.const 1) (i32.const 6)) (drop))
  (func (export "as-br-value") (result f32)
    (block (result f32)
      (br 0 (call_indirect (type $over-f32) (f32.const 1) (i32.const 7)))))
  (func (export "as-local.set-value") (result f64)
    (local f64)
    (local.set 0 (call_indirect (type $over-f64) (f64.const 1) (i32.const 8)))
    (local.get 0))
  (func (export "as-local.tee-value") (result f64)
    (local f64)
    (local.tee 0 (call_indirect (type $over-f64) (f64.const 1) (i32.const 8))))
  (global $a (mut f64) (f64.const 10.0))
  (func (export "as-global.set-value") (result f64)
    (global.set $a (call_indirect (type $over-f64) (f64.const 1.0) (i32.const 8)))
    (global.get $a))
  (func (export "as-load-operand") (result i32)
    (i32.load (call_indirect (type $out-i32) (i32.const 0))))

  (func (export "as-unary-operand") (result f32)
    (block (result f32)
      (f32.sqrt (call_indirect (type $over-f32) (f32.const 0x0p+0) (i32.const 7)))))

  (func (export "as-binary-left") (result i32)
    (block (result i32)
      (i32.add
        (call_indirect (type $over-i32) (i32.const 1) (i32.const 5))
        (i32.const 10))))
  (func (export "as-binary-right") (result i32)
    (block (result i32)
      (i32.sub
        (i32.const 10)
        (call_indirect (type $over-i32) (i32.const 1) (i32.const 5)))))

  (func (export "as-test-operand") (result i32)
    (block (result i32)
      (i32.eqz (call_indirect (type $over-i32) (i32.const 1) (i32.const 5)))))

  (func (export "as-compare-left") (result i32)
    (block (result i32)
      (i32.le_u
        (call_indirect (type $over-i32) (i32.const 1) (i32.const 5))
        (i32.const 10))))
  (func (export "as-compare-right") (result i32)
    (block (result i32)
      (i32.ne
        (i32.const 10)
        (call_indirect (type $over-i32) (i32.const 1) (i32.const 5)))))

  (func (export "as-convert-operand") (result i64)
    (block (result i64)
      (i64.extend_i32_s
        (call_indirect (type $over-i32) (i32.const 1) (i32.const 5))))))

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

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (type $callee (func (param (ref null $t1)) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func $use (type $callee)
    (local.get 0)
    drop
    (i32.const 1))

  (table funcref (elem $use))

  (func (result i32)
    (call_indirect (type $callee)
      (block (result (ref null $t0))
        (ref.func $f))
      (i32.const 0))))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (type $callee (func (param (ref null $t1)) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func $use (type $callee)
      (local.get 0)
      drop
      (i32.const 1))
    (table funcref (elem $use))
    (func (result i32)
      (call_indirect (type $callee)
        (block (result (ref null $t0))
          (ref.func $f))
        (i32.const 0))))
  "type mismatch"
)

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
    (type (func (result i64)))
    (table 0 funcref)
    (func $type-num-vs-num (i32.eqz (call_indirect (type 0) (i32.const 0)))))
  "type mismatch"
)

(assert_invalid
  (module
    (type (func (param i32)))
    (table 0 funcref)
    (func $arity-0-vs-1 (call_indirect (type 0) (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type (func (param f64 i32)))
    (table 0 funcref)
    (func $arity-0-vs-2 (call_indirect (type 0) (i32.const 0))))
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

(assert_invalid
  (module
    (type (func (param i32 i32)))
    (table 0 funcref)
    (func $type-first-void-vs-num
      (call_indirect (type 0) (nop) (i32.const 1) (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type (func (param i32 i32)))
    (table 0 funcref)
    (func $type-second-void-vs-num
      (call_indirect (type 0) (i32.const 1) (nop) (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type (func (param f64 i32)))
    (table 0 funcref)
    (func $type-second-num-vs-num
      (call_indirect (type 0) (i32.const 1) (f64.const 1) (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type (func (param i32)))
    (table 0 funcref)
    (func $type-first-empty-in-block
      (block (call_indirect (type 0) (i32.const 0)))))
  "type mismatch"
)

(assert_invalid
  (module
    (type (func (param i32 i32)))
    (table 0 funcref)
    (func $type-second-empty-in-loop
      (loop (call_indirect (type 0) (i32.const 0) (i32.const 0)))))
  "type mismatch"
)

(assert_invalid
  (module
    (type (func (param i32)))
    (table 0 funcref)
    (func $type-first-empty-in-then
      (if (i32.const 0) (then (call_indirect (type 0) (i32.const 0))))))
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
