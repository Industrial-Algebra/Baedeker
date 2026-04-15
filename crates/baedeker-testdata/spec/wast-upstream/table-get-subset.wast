;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/table_get.wast

(module
  (table $t 10 externref)
  (func (export "get-zero") (result externref)
    (table.get $t (i32.const 0)))
)

(assert_invalid
  (module
    (table $t 10 externref)
    (func $type-index-empty-vs-i32 (result externref)
      (table.get $t)
    )
  )
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 10 externref)
    (func $type-index-f32-vs-i32 (result externref)
      (table.get $t (f32.const 1))
    )
  )
  "type mismatch"
)
