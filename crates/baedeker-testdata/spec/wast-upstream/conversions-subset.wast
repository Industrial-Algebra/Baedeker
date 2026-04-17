;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/conversions.wast

(module
  (func (result i32)
    (i32.wrap_i64 (i64.const 0)))
)

(assert_invalid
  (module (func (result i32) (i32.wrap_i64 (f32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module (func (result i64) (i64.extend_i32_s (f32.const 0))))
  "type mismatch"
)
