;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/exports.wast

(assert_invalid
  (module (export "a" (func 0)))
  "unknown function"
)

(assert_invalid
  (module (func) (export "a" (func 1)))
  "unknown function"
)

(assert_invalid
  (module (func) (export "a" (func 0)) (export "a" (func 0)))
  "duplicate export name"
)

(assert_invalid
  (module (memory 1) (export "m" (memory 1)))
  "unknown memory"
)
