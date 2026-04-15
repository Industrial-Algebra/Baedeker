;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/table_set.wast

(module
  (table $t 10 externref)
  (func (export "write-null")
    (table.set $t (i32.const 0) (ref.null extern)))
)

(assert_invalid
  (module
    (table $t 10 externref)
    (func $type-index-value-empty-vs-i32-externref
      (table.set $t)
    )
  )
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 10 externref)
    (func $type-index-empty-vs-i32
      (table.set $t (ref.null extern))
    )
  )
  "type mismatch"
)
