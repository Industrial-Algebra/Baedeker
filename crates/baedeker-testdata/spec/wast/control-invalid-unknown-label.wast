(assert_invalid
  (module
    (func (result i32)
      block (result i32)
        i32.const 1
        i32.const 0
        br_table 0 2
      end))
  "unknown label")
