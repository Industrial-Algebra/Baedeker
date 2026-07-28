(module
  (type (func (param i32) (result i32)))
  (table 1 funcref)
  (func (type 0) (param i32) (result i32)
    local.get 0)
  (elem (i32.const 0) func 0)
  (func (result i32)
    i32.const 7
    i32.const 0
    call_indirect (type 0)))

(assert_invalid
  (module
    (type (func))
    (table 1 externref)
    (func
      i32.const 0
      call_indirect (type 0)))
  "type mismatch")

(assert_invalid
  (module
    (type (func (param i32) (result i32)))
    (table 1 funcref)
    (func (result i32)
      i32.const 0
      call_indirect (type 0)))
  "type mismatch")
