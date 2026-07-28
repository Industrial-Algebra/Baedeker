(module
  (func (result i32)
    i32.const 1))

(assert_invalid
  (module
    (func (result i32)
      f32.const 0.0))
  "type mismatch")
