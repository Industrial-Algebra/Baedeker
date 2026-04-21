;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/block.wast

(module
  (func $dummy)

  (func (export "singular") (result i32)
    (block (nop))
    (block (result i32) (i32.const 7)))

  (func (export "nested") (result i32)
    (block (result i32)
      (block (call $dummy) (block) (nop))
      (block (result i32) (call $dummy) (i32.const 9))))

  (func (export "as-if-then") (result i32)
    (if (result i32)
      (i32.const 1)
      (then (block (result i32) (i32.const 1)))
      (else (i32.const 2)))))

(assert_invalid
  (module
    (type $sig (func))
    (func (block (type $sig) (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-empty-i32 (result i32) (block)))
  "type mismatch"
)

(assert_invalid
  (module (func $type-value-i32-vs-void
    (block (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-value-empty-vs-i32 (result i32)
    (block (result i32))))
  "type mismatch"
)

(assert_malformed
  (module quote
    "(type $sig (func (param i32) (result i32)))"
    "(func (i32.const 0) (block (type $sig) (result i32) (param i32)))")
  "unexpected token"
)
