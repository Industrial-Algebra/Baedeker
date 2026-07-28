;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/if.wast

(module
  (memory 1)
  (func $dummy)
  (func $func (param i32) (result i32) (local.get 0))
  (type $check (func (param i32) (result i32)))
  (table funcref (elem $func))

  (func (export "singular") (param i32) (result i32)
    (if (local.get 0) (then (nop)))
    (if (result i32)
      (local.get 0)
      (then (i32.const 7))
      (else (i32.const 8))))

  (func (export "nested") (param i32 i32) (result i32)
    (if (result i32)
      (local.get 0)
      (then
        (if (result i32)
          (local.get 1)
          (then (call $dummy) (i32.const 9))
          (else (call $dummy) (i32.const 10))))
      (else
        (if (result i32)
          (local.get 1)
          (then (call $dummy) (i32.const 10))
          (else (call $dummy) (i32.const 11))))))

  (func (export "as-select-first") (param i32) (result i32)
    (select
      (if (result i32)
        (local.get 0)
        (then (call $dummy) (i32.const 1))
        (else (call $dummy) (i32.const 0)))
      (i32.const 2)
      (i32.const 3)))

  (func (export "as-call_indirect-last") (param i32) (result i32)
    (call_indirect (type $check)
      (i32.const 1)
      (if (result i32)
        (local.get 0)
        (then (i32.const 0))
        (else (i32.const 0)))))

  (func (export "as-memory.grow-size") (param i32) (result i32)
    (memory.grow
      (if (result i32)
        (local.get 0)
        (then (i32.const 1))
        (else (i32.const 0))))))

(module
  (type $t0 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (param i32) (result (ref null $t0))
    (if (result (ref null $t0))
      (local.get 0)
      (then
        (ref.func $f))
      (else
        (ref.null $t0)))))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param i32) (result (ref $t0))
      (if (result (ref $t0))
        (local.get 0)
        (then
          (ref.func $f))
        (else
          (ref.null $t0)))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $sig (func))
    (func (i32.const 1) (if (type $sig) (i32.const 0) (then))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-empty-i32 (result i32) (if (i32.const 0) (then))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-empty-i32 (result i32) (if (i32.const 0) (then) (else))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-then-value-num-vs-void
    (if (i32.const 1) (then (i32.const 1)))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-else-value-num-vs-void
    (if (i32.const 1) (then) (else (i32.const 1)))))
  "type mismatch"
)

(assert_malformed
  (module quote
    "(type $sig (func (param i32) (result i32)))"
    "(func (i32.const 0)"
    "  (if (type $sig) (result i32) (param i32) (i32.const 1) (then))"
    ")")
  "unexpected token"
)
