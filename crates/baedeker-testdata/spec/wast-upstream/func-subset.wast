;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/func.wast

(module
  (func (param i32) (drop (local.get 0)))
)

(assert_invalid
  (module (func $g (type 4)))
  "unknown type"
)

(assert_invalid
  (module (func $type-local-num-vs-num (result i64) (local i32) (local.get 0)))
  "type mismatch"
)

(assert_malformed
  (module quote
    "(type $sig (func (param i32) (result i32)))"
    "(func (type $sig) (result i32) (param i32) (i32.const 0))"
  )
  "unexpected token"
)
