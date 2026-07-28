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

(module
  (memory 1)
  (memory 2)
  (export "m0" (memory 0))
  (export "m1" (memory 1)))

(module
  (table 1 funcref)
  (table 2 funcref)
  (export "t0" (table 0))
  (export "t1" (table 1)))
