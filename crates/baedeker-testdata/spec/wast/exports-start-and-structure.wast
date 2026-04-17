(module
  (func)
  (memory 1)
  (export "f" (func 0))
  (export "m" (memory 0))
  (start 0))

(assert_invalid
  (module
    (func (result i32)
      i32.const 0)
    (start 0))
  "start function")

(assert_invalid
  (module
    (func)
    (memory 1)
    (export "dup" (func 0))
    (export "dup" (memory 0)))
  "duplicate export")

(assert_invalid
  (module
    (memory 1)
    (memory 1))
  "multiple memories")

(assert_invalid
  (module
    (table 1 funcref)
    (table 1 funcref))
  "multiple tables")
