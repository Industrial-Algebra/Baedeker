;; Numeric conversion, reinterpretation, and saturating truncation cohort.

(module
  ;; float -> int truncation (traps on overflow/NaN)
  (func (export "i32_trunc_f32_s") (param f32) (result i32) local.get 0 i32.trunc_f32_s)
  (func (export "i32_trunc_f32_u") (param f32) (result i32) local.get 0 i32.trunc_f32_u)
  (func (export "i32_trunc_f64_s") (param f64) (result i32) local.get 0 i32.trunc_f64_s)
  (func (export "i32_trunc_f64_u") (param f64) (result i32) local.get 0 i32.trunc_f64_u)
  (func (export "i64_trunc_f32_s") (param f32) (result i64) local.get 0 i64.trunc_f32_s)
  (func (export "i64_trunc_f32_u") (param f32) (result i64) local.get 0 i64.trunc_f32_u)
  (func (export "i64_trunc_f64_s") (param f64) (result i64) local.get 0 i64.trunc_f64_s)
  (func (export "i64_trunc_f64_u") (param f64) (result i64) local.get 0 i64.trunc_f64_u)
  ;; int -> float conversion
  (func (export "f32_convert_i32_s") (param i32) (result f32) local.get 0 f32.convert_i32_s)
  (func (export "f32_convert_i32_u") (param i32) (result f32) local.get 0 f32.convert_i32_u)
  (func (export "f32_convert_i64_s") (param i64) (result f32) local.get 0 f32.convert_i64_s)
  (func (export "f32_convert_i64_u") (param i64) (result f32) local.get 0 f32.convert_i64_u)
  (func (export "f64_convert_i32_s") (param i32) (result f64) local.get 0 f64.convert_i32_s)
  (func (export "f64_convert_i32_u") (param i32) (result f64) local.get 0 f64.convert_i32_u)
  (func (export "f64_convert_i64_s") (param i64) (result f64) local.get 0 f64.convert_i64_s)
  (func (export "f64_convert_i64_u") (param i64) (result f64) local.get 0 f64.convert_i64_u)
  ;; float width changes
  (func (export "f32_demote_f64") (param f64) (result f32) local.get 0 f32.demote_f64)
  (func (export "f64_promote_f32") (param f32) (result f64) local.get 0 f64.promote_f32)
  ;; reinterpretation
  (func (export "i32_reinterpret_f32") (param f32) (result i32) local.get 0 i32.reinterpret_f32)
  (func (export "i64_reinterpret_f64") (param f64) (result i64) local.get 0 i64.reinterpret_f64)
  (func (export "f32_reinterpret_i32") (param i32) (result f32) local.get 0 f32.reinterpret_i32)
  (func (export "f64_reinterpret_i64") (param i64) (result f64) local.get 0 f64.reinterpret_i64)
  ;; saturating truncation (does NOT trap)
  (func (export "i32_trunc_sat_f32_s") (param f32) (result i32) local.get 0 i32.trunc_sat_f32_s)
  (func (export "i32_trunc_sat_f32_u") (param f32) (result i32) local.get 0 i32.trunc_sat_f32_u)
  (func (export "i32_trunc_sat_f64_s") (param f64) (result i32) local.get 0 i32.trunc_sat_f64_s)
  (func (export "i32_trunc_sat_f64_u") (param f64) (result i32) local.get 0 i32.trunc_sat_f64_u)
  (func (export "i64_trunc_sat_f32_s") (param f32) (result i64) local.get 0 i64.trunc_sat_f32_s)
  (func (export "i64_trunc_sat_f32_u") (param f32) (result i64) local.get 0 i64.trunc_sat_f32_u)
  (func (export "i64_trunc_sat_f64_s") (param f64) (result i64) local.get 0 i64.trunc_sat_f64_s)
  (func (export "i64_trunc_sat_f64_u") (param f64) (result i64) local.get 0 i64.trunc_sat_f64_u))

;; float -> int truncation
(assert_return (invoke "i32_trunc_f32_s" (f32.const 42.0)) (i32.const 42))
(assert_return (invoke "i32_trunc_f32_s" (f32.const -3.7)) (i32.const -3))
(assert_return (invoke "i32_trunc_f32_u" (f32.const 42.0)) (i32.const 42))
(assert_return (invoke "i32_trunc_f64_s" (f64.const -3.7)) (i32.const -3))
(assert_return (invoke "i32_trunc_f64_u" (f64.const 42.0)) (i32.const 42))
(assert_return (invoke "i64_trunc_f32_s" (f32.const 42.0)) (i64.const 42))
(assert_return (invoke "i64_trunc_f32_u" (f32.const 42.0)) (i64.const 42))
(assert_return (invoke "i64_trunc_f64_s" (f64.const -3.7)) (i64.const -3))
(assert_return (invoke "i64_trunc_f64_u" (f64.const 42.0)) (i64.const 42))

;; int -> float conversion
(assert_return (invoke "f32_convert_i32_s" (i32.const -1)) (f32.const -1.0))
(assert_return (invoke "f32_convert_i32_u" (i32.const 7)) (f32.const 7.0))
(assert_return (invoke "f32_convert_i64_s" (i64.const -1)) (f32.const -1.0))
(assert_return (invoke "f32_convert_i64_u" (i64.const 7)) (f32.const 7.0))
(assert_return (invoke "f64_convert_i32_s" (i32.const -1)) (f64.const -1.0))
(assert_return (invoke "f64_convert_i32_u" (i32.const 7)) (f64.const 7.0))
(assert_return (invoke "f64_convert_i64_s" (i64.const -1)) (f64.const -1.0))
(assert_return (invoke "f64_convert_i64_u" (i64.const 7)) (f64.const 7.0))

;; float width changes
(assert_return (invoke "f32_demote_f64" (f64.const 3.14159)) (f32.const 3.14159))
(assert_return (invoke "f64_promote_f32" (f32.const 3.14159)) (f64.const 3.141590118408203))

;; reinterpretation — NaN bit pattern round-trip
(assert_return (invoke "i32_reinterpret_f32" (f32.const 0)) (i32.const 0))
(assert_return (invoke "i64_reinterpret_f64" (f64.const 0)) (i64.const 0))
(assert_return (invoke "f32_reinterpret_i32" (i32.const 1065353216)) (f32.const 1.0))
(assert_return (invoke "f64_reinterpret_i64" (i64.const 4607182418800017408)) (f64.const 1.0))

;; saturating truncation
(assert_return (invoke "i32_trunc_sat_f32_s" (f32.const 42.0)) (i32.const 42))
(assert_return (invoke "i32_trunc_sat_f32_u" (f32.const 42.0)) (i32.const 42))
(assert_return (invoke "i32_trunc_sat_f64_s" (f64.const 42.0)) (i32.const 42))
(assert_return (invoke "i32_trunc_sat_f64_u" (f64.const 42.0)) (i32.const 42))
(assert_return (invoke "i64_trunc_sat_f32_s" (f32.const 42.0)) (i64.const 42))
(assert_return (invoke "i64_trunc_sat_f32_u" (f32.const 42.0)) (i64.const 42))
(assert_return (invoke "i64_trunc_sat_f64_s" (f64.const 42.0)) (i64.const 42))
(assert_return (invoke "i64_trunc_sat_f64_u" (f64.const 42.0)) (i64.const 42))
