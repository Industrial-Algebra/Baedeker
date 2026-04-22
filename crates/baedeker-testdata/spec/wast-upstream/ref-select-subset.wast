;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/select.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_func.wast

(module
  (func $tf)
  (elem declare func $tf)

  (func (param funcref funcref i32) (result funcref)
    (select (result funcref) (local.get 0) (local.get 1) (local.get 2)))

  (func (param externref externref i32) (result externref)
    (select (result externref) (local.get 0) (local.get 1) (local.get 2)))

  (func (param i32) (result funcref)
    (select (result funcref)
      (ref.func $tf)
      (ref.null func)
      (local.get 0)))

  (func (param i32) (result i32)
    (ref.is_null
      (select (result funcref)
        (ref.func $tf)
        (ref.null func)
        (local.get 0)))))

(assert_invalid
  (module
    (func (param externref funcref i32) (result externref)
      (select (result externref) (local.get 0) (local.get 1) (local.get 2))))
  "type mismatch"
)

(assert_invalid
  (module
    (func (param externref externref externref) (result externref)
      (select (result externref) (local.get 0) (local.get 1) (local.get 2))))
  "type mismatch"
)
