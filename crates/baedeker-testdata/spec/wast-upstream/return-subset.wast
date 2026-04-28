;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/return.wast

(module
  (func $dummy)
  (func $id (param i32) (result i32) (local.get 0))

  (func (export "type-i32-value") (result i32)
    (block (result i32) (i32.ctz (return (i32.const 1)))))
  (func (export "as-block-value") (result i32)
    (block (result i32) (nop) (call $dummy) (return (i32.const 2))))
  (func (export "as-if-then") (param i32 i32) (result i32)
    (if (result i32)
      (local.get 0)
      (then (return (i32.const 3)))
      (else (local.get 1))))
  (func (export "as-if-else") (param i32 i32) (result i32)
    (if (result i32)
      (local.get 0)
      (then (local.get 1))
      (else (return (i32.const 4)))))

  (func (export "as-call-value") (result i32)
    (call $id (return (i32.const 5))))

  (func (export "as-br-value") (result i32)
    (block (result i32) (br 0 (return (i32.const 6))))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (result (ref null $t1))
    (return (ref.func $f))
    (ref.null $t1)))

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (result (ref null $t1))
      (return (ref.func $f))
      (ref.null $t1)))
  "type mismatch"
)

(module
  (type $t0 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (param $c i32) (result (ref null $t0))
    (return
      (if (result (ref null $t0))
        (local.get $c)
        (then
          (ref.func $f))
        (else
          (ref.null $t0))))
    (ref.null $t0)))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param $c i32) (result (ref $t0))
      (return
        (if (result (ref null $t0))
          (local.get $c)
          (then
            (ref.func $f))
          (else
            (ref.null $t0))))
      (ref.func $f)))
  "type mismatch"
)

(assert_invalid
  (module (func $type-value-empty-vs-num (result i32) (return)))
  "type mismatch"
)

(assert_invalid
  (module
    (func $type-value-empty-vs-num-in-block (result i32)
      (i32.const 0)
      (block (return))))
  "type mismatch"
)

(assert_invalid
  (module
    (func $type-value-empty-vs-num-in-loop (result i32)
      (i32.const 0)
      (loop (return))))
  "type mismatch"
)

(assert_invalid
  (module
    (func $type-value-empty-vs-num-in-then (result i32)
      (i32.const 0) (i32.const 0)
      (if (then (return)))))
  "type mismatch"
)

(assert_invalid
  (module
    (func $type-value-empty-vs-num-in-return (result i32)
      (return (return))))
  "type mismatch"
)

(assert_invalid
  (module
    (func $type-value-empty-vs-num-in-call (result i32)
      (call 1 (return)))
    (func (param i32) (result i32) (local.get 0)))
  "type mismatch"
)
