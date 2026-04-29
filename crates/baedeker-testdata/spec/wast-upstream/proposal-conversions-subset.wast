;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/conversions.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/i32.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/i64.wast

(module
  (func (result i32)
    (i32.wrap_i64 (i64.const 0)))
  (func (result i32)
    (i32.trunc_sat_f32_s (f32.const 1)))
  (func (result i32)
    (i32.trunc_sat_f64_u (f64.const 1)))
  (func (result i64)
    (i64.extend_i32_s (i32.const -1)))
  (func (result i64)
    (i64.trunc_sat_f32_u (f32.const 1)))
  (func (result f32)
    (f32.convert_i64_u (i64.const 1)))
  (func (result f32)
    (f32.demote_f64 (f64.const 1)))
  (func (result f64)
    (f64.promote_f32 (f32.const 1)))
  (func (result i32)
    (i32.reinterpret_f32 (f32.const 0)))
  (func (result i64)
    (i64.reinterpret_f64 (f64.const 0)))
  (func (result f32)
    (f32.reinterpret_i32 (i32.const 0)))
  (func (result f64)
    (f64.reinterpret_i64 (i64.const 0)))
  (func (result i32)
    (i32.extend8_s (i32.const 255)))
  (func (result i64)
    (i64.extend32_s (i64.const 4294967295))))

(assert_invalid
  (module
    (func (result i32)
      (i32.wrap_i64 (f32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (func (result i64)
      (i64.extend_i32_s (f32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (func (result f32)
      (f32.demote_f64 (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (func (result i32)
      (i32.trunc_sat_f32_s (i32.const 0))))
  "type mismatch"
)
