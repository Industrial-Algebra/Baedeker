(assert_invalid
  (module
    (func
      block (result i32)
        i32.const 1
        block (result i64)
          i32.const 0
          br_table 0 1
        end
      end))
  "type mismatch")
