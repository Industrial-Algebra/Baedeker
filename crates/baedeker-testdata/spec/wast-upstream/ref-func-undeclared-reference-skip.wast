;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/ref_func.wast

(assert_invalid
  (module (func $f (drop (ref.func $f))))
  "undeclared function reference"
)

(assert_invalid
  (module (start $f) (func $f (drop (ref.func $f))))
  "undeclared function reference"
)
