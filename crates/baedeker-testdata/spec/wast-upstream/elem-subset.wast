;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/elem.wast

(module
  (table 1 externref)
  (elem (i32.const 0) externref (ref.null extern))
)

(assert_invalid
  (module
    (func)
    (table 1 externref)
    (elem (i32.const 0) funcref (ref.func 0))
  )
  "type mismatch"
)

(assert_invalid
  (module
    (func $f)
    (elem (i32.const 0) $f)
  )
  "unknown table"
)

(assert_invalid
  (module
    (table 1 funcref)
    (elem (i64.const 0))
  )
  "type mismatch"
)
