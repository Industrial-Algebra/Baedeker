;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/ref_is_null.wast

(module
  (func (param $r externref) (result i32)
    (ref.is_null (local.get $r)))
)

(assert_invalid
  (module (func $ref-vs-num (param i32) (ref.is_null (local.get 0))))
  "type mismatch"
)

(assert_invalid
  (module (func $ref-vs-empty (ref.is_null)))
  "type mismatch"
)
