(module
  (memory 1)
  (data "abc")
  (data "xyz")
  (func
    i32.const 0
    i32.const 0
    i32.const 1
    memory.init 0
    data.drop 1))

(assert_invalid
  (module
    (memory 1)
    (data "abc")
    (func
      i32.const 0
      i32.const 0
      i32.const 1
      memory.init 1))
  "unknown data")
