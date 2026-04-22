;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/data.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/multi-memory/data0.wast

(module
  (memory $mem0 1)
  (memory $mem1 1)
  (memory $mem2 1)

  (data (memory $mem0) (i32.const 0) "")
  (data (memory $mem1) (i32.const 1) "a" "" "bcd")
  (data (memory $mem2) (offset (i32.const 0)) "" "a" "bc" ""))

(module
  (global $g (import "spectest" "global_i32") i32)
  (import "spectest" "memory0" (memory 1))
  (import "spectest" "memory1" (memory 1))
  (data (memory 0) (global.get $g) "a")
  (data (memory 1) (global.get $g) "b"))

(assert_invalid
  (module
    (memory 1)
    (data (memory 1) (i32.const 0) "a"))
  "unknown memory"
)

(assert_invalid
  (module
    (memory 1)
    (memory 1)
    (data (memory 1) (i64.const 0) "a"))
  "type mismatch"
)

(assert_invalid
  (module
    (global $g (import "test" "g") (mut i32))
    (memory 1)
    (memory 1)
    (data (memory 1) (global.get $g) "a"))
  "constant expression required"
)
