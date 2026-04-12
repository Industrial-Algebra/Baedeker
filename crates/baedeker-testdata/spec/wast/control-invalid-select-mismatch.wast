(assert_invalid
  (module
    (func (result i32)
      i32.const 1
      f32.const 0.0
      i32.const 0
      select))
  "type mismatch")
