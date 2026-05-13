;; Integer width conversion execution cohort.
;; These operations are non-trapping and either discard high bits or extend a
;; 32-bit value into 64 bits with signed/unsigned interpretation.

(module
  (func (export "i32_wrap_i64") (param i64) (result i32) local.get 0 i32.wrap_i64)
  (func (export "i64_extend_i32_s") (param i32) (result i64) local.get 0 i64.extend_i32_s)
  (func (export "i64_extend_i32_u") (param i32) (result i64) local.get 0 i64.extend_i32_u))

(assert_return (invoke "i32_wrap_i64" (i64.const 0)) (i32.const 0))
(assert_return (invoke "i32_wrap_i64" (i64.const 2147483647)) (i32.const 2147483647))
(assert_return (invoke "i32_wrap_i64" (i64.const 2147483648)) (i32.const -2147483648))
(assert_return (invoke "i32_wrap_i64" (i64.const 4294967295)) (i32.const -1))
(assert_return (invoke "i32_wrap_i64" (i64.const 1311768467139281697)) (i32.const -2023406815))

(assert_return (invoke "i64_extend_i32_s" (i32.const 0)) (i64.const 0))
(assert_return (invoke "i64_extend_i32_s" (i32.const 2147483647)) (i64.const 2147483647))
(assert_return (invoke "i64_extend_i32_s" (i32.const -2147483648)) (i64.const -2147483648))
(assert_return (invoke "i64_extend_i32_s" (i32.const -1)) (i64.const -1))

(assert_return (invoke "i64_extend_i32_u" (i32.const 0)) (i64.const 0))
(assert_return (invoke "i64_extend_i32_u" (i32.const 2147483647)) (i64.const 2147483647))
(assert_return (invoke "i64_extend_i32_u" (i32.const -2147483648)) (i64.const 2147483648))
(assert_return (invoke "i64_extend_i32_u" (i32.const -1)) (i64.const 4294967295))
