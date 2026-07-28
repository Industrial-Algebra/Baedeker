;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/table_grow.wast

(module
  (table $t0 0 externref)
  (table $t1 10 funcref)
  (func (export "grow-externref") (param $sz i32) (param $init externref) (result i32)
    (table.grow $t0 (local.get $init) (local.get $sz)))
  (func (export "grow-funcref") (param $sz i32) (result i32)
    (table.grow $t1 (ref.null func) (local.get $sz)))
  (func (export "size-t1") (result i32)
    (table.size $t1)))

(assert_invalid
  (module
    (table $t 0 externref)
    (func $type-init-size-empty-vs-i32-externref (result i32)
      (table.grow $t)))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 0 externref)
    (func $type-size-empty-vs-i32 (result i32)
      (table.grow $t (ref.null extern))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 0 externref)
    (func $type-init-empty-vs-externref (result i32)
      (table.grow $t (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 0 externref)
    (func $type-size-f32-vs-i32 (result i32)
      (table.grow $t (ref.null extern) (f32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 0 funcref)
    (func $type-init-externref-vs-funcref (param $r externref) (result i32)
      (table.grow $t (local.get $r) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 1 externref)
    (func $type-result-i32-vs-empty
      (table.grow $t (ref.null extern) (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 1 externref)
    (func $type-result-i32-vs-f32 (result f32)
      (table.grow $t (ref.null extern) (i32.const 0))))
  "type mismatch"
)
