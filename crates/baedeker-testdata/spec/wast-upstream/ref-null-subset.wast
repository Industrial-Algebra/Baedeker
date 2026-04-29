;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast

(module
  (func (result funcref)
    (ref.null func))
  (func (result externref)
    (ref.null extern))
  (global funcref (ref.null func))
  (global externref (ref.null extern))
  (func (result i32)
    (ref.is_null (global.get 0)))
  (func (result i32)
    (ref.is_null (global.get 1))))

(assert_invalid
  (module
    (func (result externref)
      (ref.null func)))
  "type mismatch"
)

(assert_invalid
  (module
    (global funcref (ref.null extern)))
  "type mismatch"
)

(assert_invalid
  (module
    (func (result i32)
      (ref.null extern)))
  "type mismatch"
)
