;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/call_ref.wast

(module
  (type $ii (func (param i32) (result i32)))

  (func $f (type $ii)
    (local.get 0))
  (export "f" (func $f))

  (global (mut (ref null $ii))
    (ref.null $ii))

  (global (ref null $ii)
    (ref.func $f))

  (func (export "id") (param (ref $ii)) (result (ref null $ii))
    (local.get 0)))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (global (ref null $t1)
    (ref.func $f)))

(assert_invalid
  (module
    (type $ii (func (param i32) (result i32)))
    (func $f (type $ii) (local.get 0))
    (global (ref null $ii)
      (ref.null extern)))
  "type mismatch"
)
