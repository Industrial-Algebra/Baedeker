;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/return_call_ref.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i64) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func (param $x i32) (result i32)
    (return_call_ref $t0 (local.get $x) (ref.func $f)))

  (func (param $x i32) (result i32)
    (return_call_ref $t0 (local.get $x) (ref.null $t0))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func (param $x i32) (result i32)
    (return_call_ref $t1 (local.get $x) (ref.func $f))))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (func $f (type $t0) (local.get 0))
    (export "f" (func $f))
    (func (param $x i64) (result i32)
      (return_call_ref $t1 (local.get $x) (ref.func $f))))
  "type mismatch"
)
