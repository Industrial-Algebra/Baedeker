;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/type.wast

(module
  (type (func))
)

(module
  (type (func (param i32) (result i32)))
)

(assert_malformed
  (module quote
    "(type)")
  "unexpected token")

(assert_malformed
  (module quote
    "(type (func) (func))")
  "unexpected token")
