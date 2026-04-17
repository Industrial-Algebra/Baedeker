(assert_invalid
  (module
    (func (result i32)
      i32.const 1
      i32.const 2
      i32.const 0
      select (result i32 i32)))
  "type mismatch")
