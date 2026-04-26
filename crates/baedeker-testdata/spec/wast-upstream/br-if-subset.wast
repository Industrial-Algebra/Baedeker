;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/br_if.wast

(module
  (func $dummy)

  (func (export "type-i32-value") (result i32)
    (block (result i32) (i32.ctz (br_if 0 (i32.const 1) (i32.const 1)))))

  (func (export "as-br_if-value") (result i32)
    (block (result i32)
      (drop (br_if 0 (br_if 0 (i32.const 1) (i32.const 2)) (i32.const 3)))
      (i32.const 4)))

  (func (export "as-br_table-value-index") (result i32)
    (block (result i32)
      (br_table 0 0 (i32.const 2) (br_if 0 (i32.const 1) (i32.const 3)))
      (i32.const 4)))

  (func (export "as-if-cond") (param i32) (result i32)
    (block (result i32)
      (if (result i32)
        (br_if 0 (i32.const 1) (local.get 0))
        (then (i32.const 2))
        (else (i32.const 3))))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (param i32) (result (ref null $t1))
    (block (result (ref null $t1))
      (br_if 0 (ref.func $f) (local.get 0)))))

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param i32) (result (ref null $t1))
      (block (result (ref null $t1))
        (br_if 0 (ref.func $f) (local.get 0)))))
  "type mismatch"
)

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t1)
    (local.get 0))
  (export "f" (func $f))
  (func (result (ref null $t0))
    (block (result (ref null $t0))
      (ref.null $t0)
      (loop (param (ref null $t0)) (result (ref null $t0))
        (drop)
        (br_if 0 (ref.func $f) (i32.const 1))))))

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f (type $t1)
      (local.get 0))
    (export "f" (func $f))
    (func (result (ref null $t0))
      (block (result (ref null $t0))
        (ref.null $t0)
        (loop (param (ref null $t0)) (result (ref null $t0))
          (drop)
          (br_if 0 (ref.func $f) (i32.const 1))))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-false-i32 (block (i32.ctz (br_if 0 (i32.const 0))))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-true-i64 (block (i64.ctz (br_if 0 (i64.const 1))))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-false-arg-void-vs-num (result i32)
    (block (result i32) (br_if 0 (i32.const 0)) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-true-arg-void-vs-num (result i32)
    (block (result i32) (br_if 0 (i32.const 1)) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-false-arg-num-vs-void
    (block (br_if 0 (i32.const 0) (i32.const 0)))))
  "type mismatch"
)
