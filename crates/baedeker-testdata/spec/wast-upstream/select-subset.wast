;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/select.wast

(module
  (func $dummy)

  (func (export "as-loop-mid") (param i32) (result i32)
    (loop (result i32)
      (call $dummy)
      (select (i32.const 2) (i32.const 3) (local.get 0))
      (call $dummy)))

  (func (export "as-if-then") (param i32) (result i32)
    (if (result i32)
      (i32.const 1)
      (then (select (i32.const 2) (i32.const 3) (local.get 0)))
      (else (i32.const 4))))

  (func (export "as-br_if-last") (param i32) (result i32)
    (block (result i32)
      (br_if 0 (i32.const 2) (select (i32.const 2) (i32.const 3) (local.get 0)))))

  (func (export "as-br_table-first") (param i32) (result i32)
    (block (result i32)
      (select (i32.const 2) (i32.const 3) (local.get 0))
      (i32.const 2)
      (br_table 0 0))))

(assert_invalid
  (module (func $arity-0-implicit (select (nop) (nop) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module (func $arity-0 (select (result) (nop) (nop) (i32.const 1))))
  "invalid result arity"
)

(assert_invalid
  (module
    (func $type-mismatch
      (drop (select (i32.const 1) (f32.const 2) (i32.const 0)))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-num-vs-num
    (select (i32.const 1) (i64.const 1) (i32.const 1)) (drop)))
  "type mismatch"
)
