;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/call.wast

(module
  (func $const-i32 (result i32) (i32.const 0x132))
  (func $swap-i32-i32 (param i32 i32) (result i32 i32)
    (local.get 1) (local.get 0))
  (func $i32-i64 (param i32 i64) (result i64)
    (local.get 1))

  (func (export "type-i32") (result i32)
    (call $const-i32))
  (func (export "type-second-i64") (result i64)
    (call $i32-i64 (i32.const 32) (i64.const 64)))
  (func (export "as-binary-all-operands") (result i32)
    (i32.add (call $swap-i32-i32 (i32.const 3) (i32.const 4)))))

(assert_invalid
  (module
    (func $type-void-vs-num (i32.eqz (call 1)))
    (func))
  "type mismatch"
)

(assert_invalid
  (module
    (func $arity-0-vs-1 (call 1))
    (func (param i32)))
  "type mismatch"
)

(assert_invalid
  (module
    (func $type-first-num-vs-num (call 1 (f64.const 1) (i32.const 1)))
    (func (param i32 f64)))
  "type mismatch"
)

(assert_invalid
  (module (func $unbound-func (call 1)))
  "unknown function"
)
