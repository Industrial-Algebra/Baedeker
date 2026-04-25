;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/br_table.wast

(module
  (func (export "type-i32-value") (result i32)
    (block (result i32) (i32.ctz (br_table 0 0 (i32.const 1) (i32.const 0)))))

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
