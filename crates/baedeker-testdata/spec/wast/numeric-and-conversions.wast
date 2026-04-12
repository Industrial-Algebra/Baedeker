(module
  (func (result i32)
    i64.const 7
    i32.wrap_i64)
  (func (result i32)
    f32.const 1.0
    i32.trunc_sat_f32_s)
  (func (result f64)
    f32.const 1.0
    f64.promote_f32))

(assert_invalid
  (module
    (func (result i32)
      f32.const 1.0))
  "type mismatch")

(assert_invalid
  (module
    (func (result i32)
      i32.const 0
      i32.trunc_f32_s))
  "type mismatch")
