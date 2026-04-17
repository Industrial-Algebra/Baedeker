;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/data.wast

(module
  (memory 1)
  (data (i32.const 0) "")
)

(assert_invalid
  (module
    (data (i32.const 0) "")
  )
  "unknown memory"
)

(assert_invalid
  (module
    (memory 1)
    (data (i64.const 0) "")
  )
  "type mismatch"
)
