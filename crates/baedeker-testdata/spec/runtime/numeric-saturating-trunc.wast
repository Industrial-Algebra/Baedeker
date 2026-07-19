;; Saturating float-to-int truncation: never traps; NaN -> 0,
;; out-of-range -> min/max of the target type.

(module
  (func (export "i32_trunc_sat_f32_s") (param f32) (result i32) local.get 0 i32.trunc_sat_f32_s)
  (func (export "i32_trunc_sat_f32_u") (param f32) (result i32) local.get 0 i32.trunc_sat_f32_u)
  (func (export "i32_trunc_sat_f64_s") (param f64) (result i32) local.get 0 i32.trunc_sat_f64_s)
  (func (export "i32_trunc_sat_f64_u") (param f64) (result i32) local.get 0 i32.trunc_sat_f64_u)
  (func (export "i64_trunc_sat_f32_s") (param f32) (result i64) local.get 0 i64.trunc_sat_f32_s)
  (func (export "i64_trunc_sat_f32_u") (param f32) (result i64) local.get 0 i64.trunc_sat_f32_u)
  (func (export "i64_trunc_sat_f64_s") (param f64) (result i64) local.get 0 i64.trunc_sat_f64_s)
  (func (export "i64_trunc_sat_f64_u") (param f64) (result i64) local.get 0 i64.trunc_sat_f64_u)
)

;; In-range: truncation toward zero.
(assert_return (invoke "i32_trunc_sat_f32_s" (f32.const 1.5)) (i32.const 1))
(assert_return (invoke "i32_trunc_sat_f32_s" (f32.const -1.5)) (i32.const -1))
(assert_return (invoke "i32_trunc_sat_f32_s" (f32.const 1.9)) (i32.const 1))
(assert_return (invoke "i32_trunc_sat_f32_s" (f32.const -0.9)) (i32.const 0))
(assert_return (invoke "i32_trunc_sat_f32_s" (f32.const 0.0)) (i32.const 0))
(assert_return (invoke "i32_trunc_sat_f32_s" (f32.const -0.0)) (i32.const 0))

;; Signed saturation.
(assert_return (invoke "i32_trunc_sat_f32_s" (f32.const inf)) (i32.const 2147483647))
(assert_return (invoke "i32_trunc_sat_f32_s" (f32.const -inf)) (i32.const -2147483648))
(assert_return (invoke "i32_trunc_sat_f32_s" (f32.const 2147483648.0)) (i32.const 2147483647))
(assert_return (invoke "i32_trunc_sat_f32_s" (f32.const -2147483904.0)) (i32.const -2147483648))
(assert_return (invoke "i32_trunc_sat_f32_s" (f32.const nan)) (i32.const 0))

;; Unsigned saturation: negatives clamp to zero.
(assert_return (invoke "i32_trunc_sat_f32_u" (f32.const 1.5)) (i32.const 1))
(assert_return (invoke "i32_trunc_sat_f32_u" (f32.const -0.9)) (i32.const 0))
(assert_return (invoke "i32_trunc_sat_f32_u" (f32.const -1.5)) (i32.const 0))
(assert_return (invoke "i32_trunc_sat_f32_u" (f32.const inf)) (i32.const 4294967295))
(assert_return (invoke "i32_trunc_sat_f32_u" (f32.const -inf)) (i32.const 0))
(assert_return (invoke "i32_trunc_sat_f32_u" (f32.const 4294967296.0)) (i32.const 4294967295))
(assert_return (invoke "i32_trunc_sat_f32_u" (f32.const nan)) (i32.const 0))

;; f64 sources.
(assert_return (invoke "i32_trunc_sat_f64_s" (f64.const -1.5)) (i32.const -1))
(assert_return (invoke "i32_trunc_sat_f64_s" (f64.const 2147483648.0)) (i32.const 2147483647))
(assert_return (invoke "i32_trunc_sat_f64_s" (f64.const -2147483649.0)) (i32.const -2147483648))
(assert_return (invoke "i32_trunc_sat_f64_s" (f64.const nan)) (i32.const 0))
(assert_return (invoke "i32_trunc_sat_f64_u" (f64.const 4294967296.0)) (i32.const 4294967295))
(assert_return (invoke "i32_trunc_sat_f64_u" (f64.const -1.5)) (i32.const 0))
(assert_return (invoke "i32_trunc_sat_f64_u" (f64.const inf)) (i32.const 4294967295))
(assert_return (invoke "i32_trunc_sat_f64_u" (f64.const nan)) (i32.const 0))

;; i64 targets.
(assert_return (invoke "i64_trunc_sat_f32_s" (f32.const -1.5)) (i64.const -1))
(assert_return (invoke "i64_trunc_sat_f32_s" (f32.const 9223372036854775808.0)) (i64.const 9223372036854775807))
(assert_return (invoke "i64_trunc_sat_f32_s" (f32.const -9223372036854775809.0)) (i64.const -9223372036854775808))
(assert_return (invoke "i64_trunc_sat_f32_s" (f32.const nan)) (i64.const 0))
(assert_return (invoke "i64_trunc_sat_f32_u" (f32.const 18446744073709551616.0)) (i64.const -1))
(assert_return (invoke "i64_trunc_sat_f32_u" (f32.const -1.5)) (i64.const 0))
(assert_return (invoke "i64_trunc_sat_f32_u" (f32.const nan)) (i64.const 0))
(assert_return (invoke "i64_trunc_sat_f64_s" (f64.const -1.5)) (i64.const -1))
(assert_return (invoke "i64_trunc_sat_f64_s" (f64.const 9223372036854775808.0)) (i64.const 9223372036854775807))
(assert_return (invoke "i64_trunc_sat_f64_s" (f64.const -9223372036854775809.0)) (i64.const -9223372036854775808))
(assert_return (invoke "i64_trunc_sat_f64_s" (f64.const nan)) (i64.const 0))
(assert_return (invoke "i64_trunc_sat_f64_u" (f64.const 18446744073709551616.0)) (i64.const -1))
(assert_return (invoke "i64_trunc_sat_f64_u" (f64.const -1.5)) (i64.const 0))
(assert_return (invoke "i64_trunc_sat_f64_u" (f64.const inf)) (i64.const -1))
(assert_return (invoke "i64_trunc_sat_f64_u" (f64.const nan)) (i64.const 0))
