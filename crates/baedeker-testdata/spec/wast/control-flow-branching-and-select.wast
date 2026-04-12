(module
  (func (result i32)
    block (result i32)
      i32.const 7
      br 0
      i32.const 9
    end)

  (func (result i32)
    block (result i32)
      i32.const 1
      i32.const 2
      i32.const 0
      select
    end)

  (func (result i64)
    i64.const 1
    i64.const 2
    i32.const 1
    select (result i64))

  (func (result i32)
    block (result i32)
      i32.const 3
      i32.const 0
      br_table 0 0
      i32.const 4
    end))
