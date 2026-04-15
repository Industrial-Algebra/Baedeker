;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/imports.wast

(module
  (type (func (param i32)))
  (import "spectest" "print_i32" (func (type 0)))
)

(assert_malformed
  (module quote "(func) (import \"\" \"\" (func))")
  "import after function"
)

(assert_malformed
  (module quote "(global i64 (i64.const 0)) (import \"\" \"\" (func))")
  "import after global"
)
