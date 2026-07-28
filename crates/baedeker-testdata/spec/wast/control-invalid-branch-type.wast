(assert_invalid
  (module
    (func (result i32)
      block (result i32)
        f32.const 0.0
        br 0
      end))
  "type mismatch")
