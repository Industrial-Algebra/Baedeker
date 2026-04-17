(module
  (table 1 externref)
  (elem (i32.const 0) externref (ref.null extern)))

(module
  (func (result i32)
    ref.null extern
    ref.is_null))

(assert_invalid
  (module
    (table 1 externref)
    (func)
    (elem (i32.const 0) func 0))
  "type mismatch")

(assert_invalid
  (module
    (func (result i32)
      i32.const 0
      ref.is_null))
  "type mismatch")
