;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/br_table.wast

(assert_invalid
  (module (func $type-arg-void-vs-num (result i32)
    (block (br_table 0 (i32.const 1)) (i32.const 1))
  ))
  "type mismatch"
)

(assert_invalid
  (module (func $type-arg-empty-vs-num (result i32)
    (block (br_table 0) (i32.const 1))
  ))
  "type mismatch"
)

(assert_invalid
  (module (func $unknown-label
    (br_table 0 1 (i32.const 0))
  ))
  "unknown label"
)
