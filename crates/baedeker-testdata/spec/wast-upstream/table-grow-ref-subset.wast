;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/table_grow.wast

(module
  (table $t 10 funcref)
  (func $f)
  (elem declare func $f)

  (func (export "grow-null") (param i32) (result i32)
    (table.grow $t (ref.null func) (local.get 0)))

  (func (export "grow-ref-func") (result i32)
    (table.grow $t (ref.func $f) (i32.const 1)))

  (func (export "check-table-null") (param i32 i32) (result funcref)
    (local funcref)
    (local.set 2 (ref.func $f))
    (block
      (loop
        (local.set 2 (table.get $t (local.get 0)))
        (br_if 1 (i32.eqz (ref.is_null (local.get 2))))
        (br_if 1 (i32.ge_u (local.get 0) (local.get 1)))
        (local.set 0 (i32.add (local.get 0) (i32.const 1)))
        (br_if 0 (i32.le_u (local.get 0) (local.get 1)))))
    (local.get 2)))

(assert_invalid
  (module
    (table $t 0 funcref)
    (func (result i32)
      (table.grow $t (ref.null extern) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 0 funcref)
    (func $f)
    (elem declare func $f)
    (func (result i32)
      (table.grow $t (ref.func $f) (f32.const 1))))
  "type mismatch"
)
