;; Integer division/remainder execution cohort.
;; This is the first runtime-trap cohort: div/rem can trap on divide-by-zero,
;; and signed division can trap on MIN / -1 overflow.

(module
  (func (export "i32_div_s") (param i32 i32) (result i32) local.get 0 local.get 1 i32.div_s)
  (func (export "i32_div_u") (param i32 i32) (result i32) local.get 0 local.get 1 i32.div_u)
  (func (export "i32_rem_s") (param i32 i32) (result i32) local.get 0 local.get 1 i32.rem_s)
  (func (export "i32_rem_u") (param i32 i32) (result i32) local.get 0 local.get 1 i32.rem_u)

  (func (export "i64_div_s") (param i64 i64) (result i64) local.get 0 local.get 1 i64.div_s)
  (func (export "i64_div_u") (param i64 i64) (result i64) local.get 0 local.get 1 i64.div_u)
  (func (export "i64_rem_s") (param i64 i64) (result i64) local.get 0 local.get 1 i64.rem_s)
  (func (export "i64_rem_u") (param i64 i64) (result i64) local.get 0 local.get 1 i64.rem_u))

(assert_return (invoke "i32_div_s" (i32.const 7) (i32.const 2)) (i32.const 3))
(assert_return (invoke "i32_div_s" (i32.const -7) (i32.const 2)) (i32.const -3))
(assert_return (invoke "i32_div_u" (i32.const -1) (i32.const 2)) (i32.const 2147483647))
(assert_return (invoke "i32_rem_s" (i32.const -7) (i32.const 2)) (i32.const -1))
(assert_return (invoke "i32_rem_s" (i32.const -2147483648) (i32.const -1)) (i32.const 0))
(assert_return (invoke "i32_rem_u" (i32.const -1) (i32.const 2)) (i32.const 1))

(assert_return (invoke "i64_div_s" (i64.const 7) (i64.const 2)) (i64.const 3))
(assert_return (invoke "i64_div_s" (i64.const -7) (i64.const 2)) (i64.const -3))
(assert_return (invoke "i64_div_u" (i64.const -1) (i64.const 2)) (i64.const 9223372036854775807))
(assert_return (invoke "i64_rem_s" (i64.const -7) (i64.const 2)) (i64.const -1))
(assert_return (invoke "i64_rem_s" (i64.const -9223372036854775808) (i64.const -1)) (i64.const 0))
(assert_return (invoke "i64_rem_u" (i64.const -1) (i64.const 2)) (i64.const 1))

(assert_trap (invoke "i32_div_s" (i32.const 1) (i32.const 0)) "integer divide by zero")
(assert_trap (invoke "i32_div_u" (i32.const 1) (i32.const 0)) "integer divide by zero")
(assert_trap (invoke "i32_rem_s" (i32.const 1) (i32.const 0)) "integer divide by zero")
(assert_trap (invoke "i32_rem_u" (i32.const 1) (i32.const 0)) "integer divide by zero")
(assert_trap (invoke "i32_div_s" (i32.const -2147483648) (i32.const -1)) "integer overflow")

(assert_trap (invoke "i64_div_s" (i64.const 1) (i64.const 0)) "integer divide by zero")
(assert_trap (invoke "i64_div_u" (i64.const 1) (i64.const 0)) "integer divide by zero")
(assert_trap (invoke "i64_rem_s" (i64.const 1) (i64.const 0)) "integer divide by zero")
(assert_trap (invoke "i64_rem_u" (i64.const 1) (i64.const 0)) "integer divide by zero")
(assert_trap (invoke "i64_div_s" (i64.const -9223372036854775808) (i64.const -1)) "integer overflow")
