;; Integer unary bit-count execution cohort.
;; These operations are non-trapping and return the same integer width as their input.

(module
  (func (export "i32_clz") (param i32) (result i32) local.get 0 i32.clz)
  (func (export "i32_ctz") (param i32) (result i32) local.get 0 i32.ctz)
  (func (export "i32_popcnt") (param i32) (result i32) local.get 0 i32.popcnt)

  (func (export "i64_clz") (param i64) (result i64) local.get 0 i64.clz)
  (func (export "i64_ctz") (param i64) (result i64) local.get 0 i64.ctz)
  (func (export "i64_popcnt") (param i64) (result i64) local.get 0 i64.popcnt))

(assert_return (invoke "i32_clz" (i32.const 0)) (i32.const 32))
(assert_return (invoke "i32_clz" (i32.const 1)) (i32.const 31))
(assert_return (invoke "i32_clz" (i32.const -2147483648)) (i32.const 0))
(assert_return (invoke "i32_ctz" (i32.const 0)) (i32.const 32))
(assert_return (invoke "i32_ctz" (i32.const 16)) (i32.const 4))
(assert_return (invoke "i32_ctz" (i32.const -2147483648)) (i32.const 31))
(assert_return (invoke "i32_popcnt" (i32.const 0)) (i32.const 0))
(assert_return (invoke "i32_popcnt" (i32.const -1)) (i32.const 32))
(assert_return (invoke "i32_popcnt" (i32.const 269488144)) (i32.const 4))

(assert_return (invoke "i64_clz" (i64.const 0)) (i64.const 64))
(assert_return (invoke "i64_clz" (i64.const 1)) (i64.const 63))
(assert_return (invoke "i64_clz" (i64.const -9223372036854775808)) (i64.const 0))
(assert_return (invoke "i64_ctz" (i64.const 0)) (i64.const 64))
(assert_return (invoke "i64_ctz" (i64.const 16)) (i64.const 4))
(assert_return (invoke "i64_ctz" (i64.const -9223372036854775808)) (i64.const 63))
(assert_return (invoke "i64_popcnt" (i64.const 0)) (i64.const 0))
(assert_return (invoke "i64_popcnt" (i64.const -1)) (i64.const 64))
(assert_return (invoke "i64_popcnt" (i64.const 72340172838076673)) (i64.const 8))
