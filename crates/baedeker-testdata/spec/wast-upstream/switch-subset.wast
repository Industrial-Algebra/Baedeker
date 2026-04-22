;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/switch.wast

(module
  (func (export "corner") (result i32)
    (block
      (br_table 0 (i32.const 0))
    )
    (i32.const 1)
  )
)

(assert_invalid
  (module (func (br_table 3 (i32.const 0))))
  "unknown label"
)
