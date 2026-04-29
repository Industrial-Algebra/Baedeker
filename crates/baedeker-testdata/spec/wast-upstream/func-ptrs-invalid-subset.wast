;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/func_ptrs.wast

(assert_invalid (module (elem (i32.const 0))) "unknown table")
(assert_invalid (module (elem (i32.const 0) 0) (func)) "unknown table")

(assert_invalid
  (module (table 1 funcref) (elem (i64.const 0)))
  "type mismatch"
)

(assert_invalid
  (module (table 1 funcref) (elem (i32.ctz (i32.const 0))))
  "constant expression required"
)

(assert_invalid
  (module (table 1 funcref) (elem (nop)))
  "constant expression required"
)

(assert_invalid (module (func (type 42))) "unknown type")
(assert_invalid (module (import "spectest" "print_i32" (func (type 43)))) "unknown type")
