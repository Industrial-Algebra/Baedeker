;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/start.wast

(module
  (func $main)
  (start $main)
)

(assert_invalid
  (module (func) (start 1))
  "unknown function"
)

(assert_invalid
  (module
    (func $main (result i32) (return (i32.const 0)))
    (start $main)
  )
  "start function"
)

(assert_invalid
  (module
    (func $main (param $a i32))
    (start $main)
  )
  "start function"
)
