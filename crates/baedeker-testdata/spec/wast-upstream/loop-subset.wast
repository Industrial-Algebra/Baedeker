;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/loop.wast

(module
  (memory 1)
  (func $dummy)
  (func $func (param i32 i32) (result i32) (local.get 0))
  (type $check (func (param i32 i32) (result i32)))
  (table funcref (elem $func))
  (global $a (mut i32) (i32.const 0))

  (func (export "singular") (result i32)
    (loop (nop))
    (loop (result i32) (i32.const 7)))

  (func (export "nested") (result i32)
    (loop (result i32)
      (loop (call $dummy) (block) (nop))
      (loop (result i32) (call $dummy) (i32.const 9))))

  (func (export "as-br_if-last") (result i32)
    (block (result i32) (br_if 0 (i32.const 2) (loop (result i32) (i32.const 1)))))

  (func (export "as-br_table-last") (result i32)
    (block (result i32) (i32.const 2) (loop (result i32) (i32.const 1)) (br_table 0 0)))

  (func (export "as-call_indirect-mid") (result i32)
    (block (result i32)
      (call_indirect (type $check)
        (i32.const 2) (loop (result i32) (i32.const 1)) (i32.const 0))))

  (func (export "as-br-value") (result i32)
    (block (result i32) (br 0 (loop (result i32) (i32.const 1)))))

  (func (export "as-global.set-value") (result i32)
    (global.set $a (loop (result i32) (i32.const 1)))
    (global.get $a))

  (func (export "as-local.tee-value") (result i32) (local i32)
    (local.tee 0 (loop (result i32) (i32.const 1))))

  (func (export "as-memory.grow-size") (result i32)
    (memory.grow (loop (result i32) (i32.const 1)))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (result (ref null $t1))
    (loop (result (ref null $t1))
      (ref.func $f))))

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (result (ref null $t1))
      (loop (result (ref null $t1))
        (ref.func $f))))
  "type mismatch"
)

(module
  (type $t0 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (result (ref null $t0))
    (loop (result (ref null $t0))
      (ref.func $f))))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func (result (ref $t0))
      (loop (result (ref $t0))
        (ref.null $t0))))
  "type mismatch"
)

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (result (ref null $t1))
    (block (result (ref null $t0))
      (ref.func $f))
    (loop (param (ref null $t1)) (result (ref null $t1))))
)

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (result (ref null $t1))
      (block (result (ref null $t0))
        (ref.func $f))
      (loop (param (ref null $t1)) (result (ref null $t1)))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-value-empty-vs-num (result i32)
    (loop (result i32))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-value-num-vs-num (result i32)
    (loop (result i32) (f32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-param-void-vs-num
    (loop (param i32) (drop))))
  "type mismatch"
)

(assert_invalid
  (module
    (func $type-value-empty-in-then
      (i32.const 0) (i32.const 0)
      (if (then (loop (result i32)) (drop)))))
  "type mismatch"
)

(assert_malformed
  (module quote "(func (param i32) (result i32) (loop (param $x i32)))")
  "unexpected token"
)
