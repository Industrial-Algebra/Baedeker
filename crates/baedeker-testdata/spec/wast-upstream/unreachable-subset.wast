;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/unreachable.wast

(module
  (func $dummy)
  (func $dummy3 (param i32 i32 i32))
  (type $sig (func (param i32 i32 i32)))
  (table funcref (elem $dummy3))
  (memory 1)
  (global $a (mut f32) (f32.const 0))

  (func (export "as-block-broke") (result i32)
    (block (result i32) (call $dummy) (br 0 (i32.const 1)) (unreachable)))

  (func (export "as-loop-broke") (result i32)
    (block (result i32)
      (loop (result i32) (call $dummy) (br 1 (i32.const 1)) (unreachable))))

  (func (export "as-br_if-value-cond") (result i32)
    (block (result i32)
      (drop (br_if 0 (i32.const 6) (unreachable)))
      (i32.const 7)))

  (func (export "as-br_table-value-and-index") (result i32)
    (block (result i32) (br_table 0 0 (unreachable)) (i32.const 8)))

  (func (export "as-if-cond") (result i32)
    (if (result i32) (unreachable) (then (i32.const 0)) (else (i32.const 1))))

  (func (export "as-call_indirect-last")
    (call_indirect (type $sig)
      (i32.const 0) (i32.const 1) (i32.const 2) (unreachable)))

  (func (export "as-global.set-value")
    (global.set $a (unreachable)))

  (func (export "as-memory.grow-size") (result i32)
    (memory.grow (unreachable))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (result (ref null $t1))
    (block (result (ref null $t1))
      (unreachable)
      (ref.func $f))))

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (result (ref null $t1))
      (block (result (ref null $t1))
        (unreachable)
        (ref.func $f))))
  "type mismatch"
)
