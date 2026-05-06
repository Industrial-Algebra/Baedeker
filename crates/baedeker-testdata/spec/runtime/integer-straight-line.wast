;; Runtime-focused WAST fixtures for Phase 2 register-IR execution.
;; These cases assume decode/validate has already accepted the module, then
;; assert observable execution through exported function entry.

(module
  (func (export "add") (param i32 i32) (result i32)
    local.get 0
    local.get 1
    i32.add)

  (func (export "sub_mul") (result i32)
    i32.const 50
    i32.const 8
    i32.sub
    i32.const 3
    i32.mul)

  (func (export "i64_add") (result i64)
    i64.const 20
    i64.const 22
    i64.add)

  (func (export "eqz") (param i32) (result i32)
    local.get 0
    i32.eqz))

(assert_return (invoke "add" (i32.const 20) (i32.const 22)) (i32.const 42))
(assert_return (invoke "sub_mul") (i32.const 126))
(assert_return (invoke "i64_add") (i64.const 42))
(assert_return (invoke "eqz" (i32.const 0)) (i32.const 1))
(assert_return (invoke "eqz" (i32.const 7)) (i32.const 0))
