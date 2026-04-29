;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/data.wast

(module
  (memory 1)
  (data (i32.const 0) ""))

(module
  (global (import "spectest" "global_i32") i32)
  (memory 1)
  (data (global.get 0) "a"))

(module
  (global $g (import "spectest" "global_i32") i32)
  (import "spectest" "memory" (memory 1))
  (data (global.get $g) "a"))

(module
  (memory 1)
  (global i32 (i32.const 0))
  (data (global.get 0) "a"))

(module
  (memory 1)
  (global $g i32 (i32.const 0))
  (data (global.get $g) "a"))

(module
  (global (import "spectest" "global_i32") i32)
  (memory 1)
  (data (i32.mul
          (i32.const 2)
          (i32.add
            (i32.sub (global.get 0) (i32.const 1))
            (i32.const 2)))
        "a"))

(assert_invalid
  (module
    (data (i32.const 0) ""))
  "unknown memory"
)

(assert_invalid
  (module
    (memory 1)
    (data (i64.const 0) ""))
  "type mismatch"
)

(assert_invalid
  (module
    (memory 1)
    (data (i32.ctz (i32.const 0))))
  "constant expression required"
)

(assert_invalid
  (module
    (global $g (import "test" "g") (mut i32))
    (memory 1)
    (data (global.get $g)))
  "constant expression required"
)

(assert_invalid
   (module
     (memory 1)
     (data (global.get 0)))
   "unknown global 0"
)

(assert_invalid
   (module
     (global (import "test" "global-i32") i32)
     (memory 1)
     (data (global.get 1)))
   "unknown global 1"
)

(assert_invalid
  (module
    (global (import "test" "global-i32") i32)
    (memory 1)
    (data (offset (global.get 0) (i32.const 0))))
  "type mismatch"
)
