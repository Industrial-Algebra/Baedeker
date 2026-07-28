;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/table_get.wast

(module
  (table $t2 2 externref)
  (table $t3 3 funcref)
  (elem (table $t3) (i32.const 1) func $dummy)
  (func $dummy)

  (func (export "get-externref") (param $i i32) (result externref)
    (table.get $t2 (local.get $i)))
  (func (export "get-funcref") (param $i i32) (result funcref)
    (table.get $t3 (local.get $i)))
  (func (export "is-null-funcref") (param $i i32) (result i32)
    (ref.is_null (table.get $t3 (local.get $i)))))

(assert_invalid
  (module
    (table $t 10 externref)
    (func $type-index-empty-vs-i32 (result externref)
      (table.get $t)))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 10 externref)
    (func $type-index-f32-vs-i32 (result externref)
      (table.get $t (f32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 10 externref)
    (func $type-result-externref-vs-empty
      (table.get $t (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 10 externref)
    (func $type-result-externref-vs-funcref (result funcref)
      (table.get $t (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t1 1 funcref)
    (table $t2 1 externref)
    (func $type-result-externref-vs-funcref-multi (result funcref)
      (table.get $t2 (i32.const 0))))
  "type mismatch"
)
