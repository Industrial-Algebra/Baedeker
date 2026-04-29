;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/global.wast

(module
  (import "test" "r" (global externref))
  (import "test" "f" (global funcref))
  (global externref (global.get 0))
  (global funcref (global.get 1))
  (global (mut externref) (ref.null extern))
  (global (mut funcref) (ref.null func)))

(assert_invalid
  (module
    (import "test" "r" (global (mut externref)))
    (global externref (global.get 0)))
  "constant expression required"
)

(assert_invalid
  (module
    (import "test" "r" (global externref))
    (global funcref (global.get 0)))
  "type mismatch"
)

(assert_invalid
  (module
    (import "test" "f" (global funcref))
    (global externref (global.get 0)))
  "type mismatch"
)
