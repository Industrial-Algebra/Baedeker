(module
  (import "env" "g" (global i32))
  (global i32 (global.get 0))
  (memory 1)
  (data (global.get 0) "A"))

(assert_invalid
  (module
    (import "env" "g" (global (mut i32)))
    (global i32 (global.get 0)))
  "constant expression required")

(assert_invalid
  (module
    (global i32 (i32.const 0))
    (memory 1)
    (data (global.get 0) "A"))
  "constant expression required")
