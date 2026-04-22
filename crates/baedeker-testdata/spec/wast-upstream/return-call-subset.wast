;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/return_call.wast

(module
  (func $const-i32 (result i32) (i32.const 0))
  (func $id-i32 (param i32) (result i32) (local.get 0))

  (func (result i32)
    (return_call $const-i32))

  (func (result i32)
    (return_call $id-i32 (i32.const 32))))

(assert_invalid
  (module
    (func (result i32) (return_call 1) (i32.const 0))
    (func))
  "type mismatch"
)

(assert_invalid
  (module
    (func (return_call 1))
    (func (param i32)))
  "type mismatch"
)

(assert_invalid
  (module
    (func $f (result i32 i32) unreachable)
    (func (result i32)
      (return_call $f)))
  "type mismatch"
)
