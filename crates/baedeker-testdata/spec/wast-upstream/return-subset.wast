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
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param $c i32) (result (ref $t0))
      (local $r (ref null $t0))
      (local.set $r
        (if (result (ref null $t0))
          (local.get $c)
          (then
            (ref.func $f))
          (else
            (ref.null $t0))))
      (local.get $r)))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (global $g (mut (ref null $t0)) (ref.null $t0))
    (func (param $c i32) (result (ref $t0))
      (global.set $g
        (if (result (ref null $t0))
          (local.get $c)
          (then
            (ref.func $f))
          (else
            (ref.null $t0))))
      (global.get $g)))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (table $t 1 (ref null $t0))
    (func (param $c i32) (result (ref $t0))
      (table.set $t
        (i32.const 0)
        (if (result (ref null $t0))
          (local.get $c)
          (then
            (ref.func $f))
          (else
            (ref.null $t0))))
      (table.get $t (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (global $g0 (ref null $t0)
      (ref.func $f))
    (table $t 1 (ref null $t0))
    (elem (ref null $t0)
      (global.get $g0))
    (func (result (ref $t0))
      (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
      (table.get $t (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (import "env" "g" (global (ref null $t0)))
    (table $t 1 (ref null $t0))
    (elem (ref null $t0)
      (global.get 0))
    (func (result (ref $t0))
      (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
      (table.get $t (i32.const 0))))
  "type mismatch"
)

(module
  (type $t0 (func (param i32) (result i32)))
  (type $callee (func (result (ref null $t0))))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func $g (type $callee)
    (ref.null $t0))
  (global $gref (ref null $callee)
    (ref.func $g))
  (table $t 1 (ref null $callee))
  (elem (ref null $callee)
    (global.get $gref))
  (func (result (ref null $callee))
    (local $r (ref null $callee))
    (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
    (local.set $r
      (table.get $t (i32.const 0)))
    (return
      (local.get $r))))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $callee (func (result (ref null $t0))))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func $g (type $callee)
      (ref.null $t0))
    (global $gref (ref null $callee)
      (ref.func $g))
    (table $t 1 (ref null $callee))
    (elem (ref null $callee)
      (global.get $gref))
    (func (result (ref $callee))
      (local $r (ref null $callee))
      (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
      (local.set $r
        (table.get $t (i32.const 0)))
      (return
        (local.get $r))))
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
