;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/ref_func.wast

(module
  (func $f)
  (global funcref (ref.func $f))
)

(module
  (func $f)
  (func (drop (ref.func $f)))
  (export "f" (func $f))
)

(module
  (func $f (import "M" "f"))
  (func (drop (ref.func $f)))
  (export "f" (func $f))
)

(module
  (func $f)
  (elem declare func $f)
  (func (drop (ref.func $f)))
)

(assert_invalid
  (module
    (func $f (import "M" "f") (param i32) (result i32))
    (func $g (import "M" "g") (param i32) (result i32))
    (global funcref (ref.func 7))
  )
  "unknown function 7"
)
