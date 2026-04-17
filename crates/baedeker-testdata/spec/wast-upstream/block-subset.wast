;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/block.wast

(module
  (func (block))
)

(assert_invalid
  (module (func $type-empty-i32 (result i32) (block)))
  "type mismatch"
)

(assert_malformed
  (module quote
    "(type $sig (func (param i32) (result i32)))"
    "(func (i32.const 0) (block (type $sig) (result i32) (param i32)))"
  )
  "unexpected token"
)
