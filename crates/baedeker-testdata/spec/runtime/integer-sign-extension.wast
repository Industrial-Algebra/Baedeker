;; Integer sign-extension execution cohort.
;; These operations reinterpret the low 8/16/32 bits as signed values and extend
;; them back to the original integer width.

(module
  (func (export "i32_extend8_s") (param i32) (result i32) local.get 0 i32.extend8_s)
  (func (export "i32_extend16_s") (param i32) (result i32) local.get 0 i32.extend16_s)

  (func (export "i64_extend8_s") (param i64) (result i64) local.get 0 i64.extend8_s)
  (func (export "i64_extend16_s") (param i64) (result i64) local.get 0 i64.extend16_s)
  (func (export "i64_extend32_s") (param i64) (result i64) local.get 0 i64.extend32_s))

(assert_return (invoke "i32_extend8_s" (i32.const 127)) (i32.const 127))
(assert_return (invoke "i32_extend8_s" (i32.const 128)) (i32.const -128))
(assert_return (invoke "i32_extend8_s" (i32.const 4660)) (i32.const 52))
(assert_return (invoke "i32_extend16_s" (i32.const 32767)) (i32.const 32767))
(assert_return (invoke "i32_extend16_s" (i32.const 32768)) (i32.const -32768))
(assert_return (invoke "i32_extend16_s" (i32.const 305419896)) (i32.const 22136))

(assert_return (invoke "i64_extend8_s" (i64.const 127)) (i64.const 127))
(assert_return (invoke "i64_extend8_s" (i64.const 128)) (i64.const -128))
(assert_return (invoke "i64_extend8_s" (i64.const 4660)) (i64.const 52))
(assert_return (invoke "i64_extend16_s" (i64.const 32767)) (i64.const 32767))
(assert_return (invoke "i64_extend16_s" (i64.const 32768)) (i64.const -32768))
(assert_return (invoke "i64_extend16_s" (i64.const 305419896)) (i64.const 22136))
(assert_return (invoke "i64_extend32_s" (i64.const 2147483647)) (i64.const 2147483647))
(assert_return (invoke "i64_extend32_s" (i64.const 2147483648)) (i64.const -2147483648))
(assert_return (invoke "i64_extend32_s" (i64.const 1311768467139281697)) (i64.const -2023406815))
