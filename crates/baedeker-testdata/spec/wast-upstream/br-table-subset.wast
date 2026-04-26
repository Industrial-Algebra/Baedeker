;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/br_table.wast

(module
  (func (export "type-i32-value") (result i32)
    (block (result i32) (i32.ctz (br_table 0 0 (i32.const 1) (i32.const 0)))))

  (func (export "type-i64-value") (result i64)
    (block (result i64) (i64.ctz (br_table 0 0 (i64.const 2) (i32.const 0)))))

  (func (export "type-f32-value") (result f32)
    (block (result f32) (f32.neg (br_table 0 0 (f32.const 3) (i32.const 0)))))

  (func (export "type-f64-value") (result f64)
    (block (result f64) (f64.neg (br_table 0 0 (f64.const 4) (i32.const 0)))))

  (func (export "empty-value") (param i32) (result i32)
    (block (result i32)
      (br_table 0 (i32.const 33) (local.get 0))
      (i32.const 31)))

  (func (export "singleton-value") (param i32) (result i32)
    (block (result i32)
      (drop
        (block (result i32)
          (br_table 0 1 (i32.const 33) (local.get 0))
          (return (i32.const 31))))
      (i32.const 32))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (result (ref null $t1))
    (block (result (ref null $t1))
      (ref.func $f)
      (i32.const 0)
      (br_table 0 0))))

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (result (ref null $t1))
      (block (result (ref null $t1))
        (ref.func $f)
        (i32.const 0)
        (br_table 0 0))))
  "type mismatch"
)

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (param i32) (result (ref null $t1))
    (block (result (ref null $t1))
      (block (result (ref null $t0))
        (ref.func $f)
        (local.get 0)
        (br_table 0 1 1)))))

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param i32) (result (ref null $t1))
      (block (result (ref null $t1))
        (block (result (ref null $t0))
          (ref.func $f)
          (local.get 0)
          (br_table 0 1 1)))))
  "type mismatch"
)

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (param i32) (result (ref null $t1))
    (block (result (ref null $t1))
      (ref.null $t0)
      (loop (param (ref null $t0)) (result (ref null $t0))
        (drop)
        (ref.func $f)
        (local.get 0)
        (br_table 0 1 1)
        (ref.null $t0)))))

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param i32) (result (ref null $t1))
      (block (result (ref null $t1))
        (ref.null $t0)
        (loop (param (ref null $t0)) (result (ref null $t0))
          (drop)
          (ref.func $f)
          (local.get 0)
          (br_table 0 1 1)
          (ref.null $t0)))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-arg-void-vs-num (result i32)
    (block (br_table 0 (i32.const 1)) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-arg-empty-vs-num (result i32)
    (block (br_table 0) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-arg-num-vs-num (result i32)
    (block (result i32)
      (br_table 0 0 0 (i64.const 1) (i32.const 1))
      (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-index-num-vs-i32
    (block (br_table 0 (i64.const 0)))))
  "type mismatch"
)

(assert_invalid
  (module (func $unknown-label
    (br_table 0 1 (i32.const 0))))
  "unknown label"
)
