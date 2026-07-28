(module
  (table 1 funcref)
  (func (result funcref)
    i32.const 0
    table.get 0)
  (func
    i32.const 0
    ref.null func
    table.set 0)
  (func (result i32)
    table.size 0)
  (func (result i32)
    ref.null func
    i32.const 1
    table.grow 0))

(assert_invalid
  (module
    (table 1 externref)
    (func
      i32.const 0
      ref.null func
      table.set 0))
  "type mismatch")

(assert_invalid
  (module
    (table 1 funcref)
    (func (result i32)
      i32.const 1
      ref.null func
      table.grow 0))
  "type mismatch")
