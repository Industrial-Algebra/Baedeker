;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/table_size.wast

(module
  (table $t0 0 externref)
  (table $t1 1 externref)
  (table $t2 0 2 externref)
  (table $t3 3 8 externref)

  (func (export "size-t0") (result i32) table.size)
  (func (export "size-t1") (result i32) (table.size $t1))
  (func (export "size-t2") (result i32) (table.size $t2))
  (func (export "size-t3") (result i32) (table.size $t3))

  (func (export "grow-t0") (param $sz i32)
    (drop (table.grow $t0 (ref.null extern) (local.get $sz))))
  (func (export "grow-t1") (param $sz i32)
    (drop (table.grow $t1 (ref.null extern) (local.get $sz)))))

(assert_invalid
  (module
    (table $t 1 externref)
    (func $type-result-i32-vs-empty
      (table.size $t)))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 1 externref)
    (func $type-result-i32-vs-f32 (result f32)
      (table.size $t)))
  "type mismatch"
)
