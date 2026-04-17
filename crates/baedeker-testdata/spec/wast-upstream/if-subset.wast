;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/if.wast

(module
  (func (i32.const 0) (if (then)))
)

(assert_invalid
  (module (func $type-empty-i32 (result i32) (if (i32.const 0) (then))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-empty-i32 (result i32) (if (i32.const 0) (then) (else))))
  "type mismatch"
)

(assert_malformed
  (module quote
    "(type $sig (func (param i32) (result i32)))"
    "(func (i32.const 0)"
    "  (if (type $sig) (result i32) (param i32) (i32.const 1) (then))"
    ")"
  )
  "unexpected token"
)
