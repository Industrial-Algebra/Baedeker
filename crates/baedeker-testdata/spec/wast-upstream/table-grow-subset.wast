;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/table_grow.wast

(module
  (table $t 0 externref)
  (func (export "grow-zero") (result i32)
    (table.grow $t (ref.null extern) (i32.const 0)))
)

(assert_invalid
  (module
    (table $t 0 externref)
    (func $type-init-size-empty-vs-i32-externref (result i32)
      (table.grow $t)
    )
  )
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 0 externref)
    (func $type-size-empty-vs-i32 (result i32)
      (table.grow $t (ref.null extern))
    )
  )
  "type mismatch"
)
