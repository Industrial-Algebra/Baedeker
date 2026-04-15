;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/table.wast

(module
  (table 1 funcref)
)

(assert_invalid
  (module (elem (i32.const 0)))
  "unknown table"
)

(assert_invalid
  (module (elem (i32.const 0) $f) (func $f))
  "unknown table"
)

(assert_malformed
  (module quote
    "(table $foo 1 funcref)"
    "(table $foo 1 funcref)"
  )
  "duplicate table"
)
