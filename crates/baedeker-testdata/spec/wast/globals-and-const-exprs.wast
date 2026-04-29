(module
  (import "env" "g" (global i32))
  (global i32 (global.get 0))
  (memory 1)
  (data (global.get 0) "A"))

(module
  (global i32 (i32.const 0))
  (global i32 (global.get 0))
  (memory 1)
  (data (global.get 1) "A"))

(module
  (import "env" "g" (global i32))
  (global i32 (i32.add (global.get 0) (i32.const 42))))

(assert_invalid
  (module
    (import "env" "g" (global (mut i32)))
    (global i32 (global.get 0)))
  "constant expression required")
