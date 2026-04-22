;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/return_call_ref.wast

(module
  (type $ii (func (param i32) (result i32)))

  (func $f (type $ii)
    (local.get 0))
  (export "f" (func $f))

  (func (param $x i32) (result i32)
    (return_call_ref $ii (local.get $x) (ref.func $f))))

(assert_invalid
  (module
    (type $ii (func (param i32) (result i32)))
    (func $f (type $ii) (local.get 0))
    (export "f" (func $f))
    (func (param $x i32) (result f64)
      (return_call_ref $ii (local.get $x) (ref.func $f))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $ii (func (param i32) (result i32)))
    (func $f (type $ii) (local.get 0))
    (export "f" (func $f))
    (func (param $x i32) (result i32)
      (return_call_ref $ii (local.get $x) (ref.null extern))))
  "type mismatch"
)
